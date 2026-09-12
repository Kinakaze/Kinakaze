use super::*;
use crate::usernet::stack::{TcpState, shared::Listener};
use std::os::windows::io::IntoRawSocket;

pub(super) fn refresh(owner: &Endpoint) -> Result<(), i32> {
    let state = read(owner)?;
    if state.kind != 1 || !state.connecting {
        return Ok(());
    }
    let tcp = owner.packet.as_ref().ok_or(EIO)?.endpoint.tcp_state()?;
    if matches!(tcp, TcpState::Established | TcpState::Closed) {
        edit(owner, |state| {
            if state.connecting {
                state.connecting = false;
                if tcp == TcpState::Closed && state.error == 0 {
                    state.error = crate::ECONNREFUSED;
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}
pub(crate) fn listen(fd: i32, owner: &Endpoint, backlog: i32) -> Result<(), i32> {
    let mut state = read(owner)?;
    if state.kind != 1 {
        return Err(crate::EOPNOTSUPP);
    }
    if state.peer.port != 0 {
        return Err(EINVAL);
    }
    if !state.bound {
        socket::bind(
            fd,
            owner,
            Address {
                family: state.local.family,
                ..Address::default()
            },
        )?;
        state = read(owner)?;
    }
    let binding = owner.packet.as_ref().ok_or(EIO)?;
    if state.listening {
        return Listener::reopen(binding.root.clone())?
            .ok_or(EIO)?
            .set_backlog(backlog.max(1) as usize);
    }
    Listener::new(
        binding.root.clone(),
        route::endpoint(state.local)?,
        backlog.max(1) as usize,
    )?;
    *binding.driver.lock().map_err(|_| EIO)? = None;
    edit(owner, |state| {
        state.listening = true;
        Ok(())
    })?;
    gateway::activate(owner)
}
pub(super) fn connect(fd: i32, owner: &Endpoint, target: Address) -> Result<(), i32> {
    let state = read(owner)?;
    if state.local.family != target.family || target.port == 0 {
        return Err(EINVAL);
    }
    if state.listening {
        return Err(crate::EISCONN);
    }
    if state.connecting {
        return Err(crate::EALREADY);
    }
    let binding = owner.packet.as_ref().ok_or(EIO)?;
    if binding.endpoint.tcp_state()? != TcpState::Closed {
        return Err(crate::EISCONN);
    }
    if target.loopback() && !loopback_up(state.ns)? {
        return Err(ENETUNREACH);
    }
    let mut destination_present = false;
    for (id, candidate) in records()? {
        if candidate.kind != 1
            || !candidate.listening
            || candidate.packet.is_none()
            || candidate.local.family != target.family
            || candidate.local.port != target.port
            || (!candidate.local.any() && candidate.local.ip != target.ip)
        {
            continue;
        }
        if target.loopback() && candidate.ns != state.ns {
            continue;
        }
        if !target.loopback() && !address_owned(candidate.ns, target)? {
            continue;
        }
        if !lifetime::present(candidate.packet.ok_or(EIO)?.arena)?.contains(&id) {
            continue;
        }
        if reachable(state.ns, candidate.ns)? {
            destination_present = true;
            break;
        }
    }
    if !destination_present {
        gateway::allowed(&state, target)?;
    }
    socket::autobind(fd, owner, target)?;
    if !destination_present {
        gateway::egress(owner, target)?;
    }
    let mut local = read(owner)?.local;
    if local.any() {
        let mut source = if target.loopback() {
            target.local_transport()
        } else {
            primary(state.ns, target.family)?
        };
        source.port = local.port;
        local = source;
    }
    edit(owner, |state| {
        state.peer = target;
        state.connecting = true;
        state.error = 0;
        Ok(())
    })?;
    if let Err(error) = binding
        .endpoint
        .connect(route::endpoint(local)?, route::endpoint(target)?)
    {
        edit(owner, |state| {
            state.peer = Address::default();
            state.connecting = false;
            Ok(())
        })?;
        return Err(error);
    }
    binding.drive(fd, owner)?;
    if crate::get(fd)?.flags.contains(crate::FdFlags::NONBLOCK) {
        return Err(crate::EINPROGRESS);
    }
    io::retry(fd, owner, 0, |endpoint| match endpoint.tcp_state()? {
        TcpState::Established | TcpState::CloseWait => Ok(()),
        TcpState::Closed => Err(crate::ECONNREFUSED),
        _ => Err(crate::EAGAIN),
    })
}
pub(crate) fn accept(fd: i32, owner: &Endpoint, flags: i32) -> Result<(i32, Address), i32> {
    use crate::socket::{SOCK_CLOEXEC, SOCK_NONBLOCK};
    if flags & !(SOCK_CLOEXEC | SOCK_NONBLOCK) != 0 {
        return Err(EINVAL);
    }
    let state = read(owner)?;
    if state.kind != 1 || !state.listening {
        return Err(EINVAL);
    }
    let binding = owner.packet.as_ref().ok_or(EIO)?;
    let listener = Listener::reopen(binding.root.clone())?.ok_or(EINVAL)?;
    let accepted = io::retry(fd, owner, 0, |_| listener.accept())?;
    let result = (|| {
        let (local, peer) = accepted.endpoint.tcp_endpoints()?;
        let peer = route::address(peer.ok_or(EIO)?);
        let mut reference = state.packet.ok_or(EIO)?;
        reference.slot = accepted.slot as u32;
        reference.generation = accepted.generation;
        let store = Arc::new(shared::new_object()?);
        let child_state = State {
            local: route::address(local.ok_or(EIO)?),
            peer,
            packet: Some(reference),
            listening: false,
            connecting: false,
            ..state.clone()
        };
        store.update(|_| Ok((child_state.encode(), ())))?;
        let endpoint = Endpoint::open(store.clone(), owner._namespace.clone())?;
        *endpoint.token.lock().map_err(|_| EIO)? =
            Some(lifetime::create(reference.arena, store.id())?);
        listener.publish_owner(&accepted, store.id())?;
        let link = io::clone_link(fd, owner)?;
        let child_flags = crate::FdFlags::NONE
            .union(if flags & SOCK_CLOEXEC != 0 {
                crate::FdFlags::CLOSE_ON_EXEC
            } else {
                crate::FdFlags::NONE
            })
            .union(if flags & SOCK_NONBLOCK != 0 {
                crate::FdFlags::NONBLOCK
            } else {
                crate::FdFlags::NONE
            });
        let raw = link.into_raw_socket() as usize;
        let child = crate::install(raw, crate::FdKind::Socket, child_flags).inspect_err(|_| {
            let _ = crate::socket::close_socket(raw);
        })?;
        let published = publish(child, crate::get(child)?.description_id, endpoint)
            .and_then(|_| register(2, store.id()));
        if let Err(error) = published {
            let _ = crate::close(child);
            return Err(error);
        }
        Ok((child, peer))
    })();
    if result.is_err() {
        let _ = listener.abort(&accepted);
    }
    crate::epoll::notify_read(fd);
    result
}
pub(crate) fn shutdown(fd: i32, owner: &Endpoint, how: i32) -> Result<(), i32> {
    if !(0..=2).contains(&how) {
        return Err(EINVAL);
    }
    let state = read(owner)?;
    if state.peer.port == 0 {
        return Err(crate::ENOTCONN);
    }
    edit(owner, |state| {
        state.read_shut |= how != 1;
        state.write_shut |= how != 0;
        Ok(())
    })?;
    let binding = owner.packet.as_ref().ok_or(EIO)?;
    if state.kind == 1 && how != 0 {
        binding.endpoint.close_tcp()?;
    }
    binding.drive(fd, owner)?;
    Ok(())
}
