use super::*;
use crate::usernet::stack::{IpAddress, IpCidr, shared::Kind};

/// Staged activation while the host gateway and the TCP descriptor lifecycle
/// are being integrated. Ordinary sockets keep their existing backend.
pub(crate) fn selected(family: i32, kind: i32, protocol: i32) -> bool {
    matches!(family, 2 | 10)
        && ((kind == 2 && matches!(protocol, 0 | 17)) || (kind == 1 && matches!(protocol, 0 | 6)))
        && std::env::var_os("KINAKAZE_USERNET_PACKET")
            .is_some_and(|value| value == "all" || (kind == 2 && value == "udp"))
}
pub(crate) fn initialize(fd: i32) -> Result<(), i32> {
    let owner = super::super::endpoint(fd)?.ok_or(crate::EBADF)?;
    let state = read(&owner)?;
    if state.packet.is_some() || !matches!(state.kind, 1 | 2) {
        return Err(EINVAL);
    }
    let address = if state.local.family == 2 {
        IpAddress::v4(127, 0, 0, 1)
    } else {
        IpAddress::Ipv6(std::net::Ipv6Addr::LOCALHOST)
    };
    let root = SharedEndpoint::create_managed(
        kinakaze_runtime::authority::domain_id(),
        owner.store.id(),
        &[IpCidr::new(address, 0)],
        1,
        if state.kind == 1 {
            Kind::Tcp
        } else {
            Kind::Udp
        },
    )?;
    edit(&owner, |state| {
        state.packet = Some(Reference {
            arena: owner.store.id(),
            slot: u32::MAX,
            generation: 0,
            managed: true,
        });
        Ok(())
    })?;
    let replacement = Endpoint::open(owner.store.clone(), owner._namespace.clone())?;
    *replacement.token.lock().map_err(|_| EIO)? =
        Some(lifetime::create(owner.store.id(), owner.store.id())?);
    publish(fd, crate::get(fd)?.description_id, replacement)?;
    drop(root);
    Ok(())
}
pub(crate) fn bind(fd: i32, owner: &Endpoint, wanted: Address) -> Result<(), i32> {
    bind_internal(fd, owner, wanted, true)
}
fn bind_internal(fd: i32, owner: &Endpoint, mut wanted: Address, expose: bool) -> Result<(), i32> {
    let _topology = shared::topology_guard()?;
    let state = read(owner)?;
    if state.bound || wanted.family != state.local.family {
        return Err(EINVAL);
    }
    if !address_owned(state.ns, wanted)? {
        return Err(EADDRNOTAVAIL);
    }
    let mut existing = Vec::new();
    for (id, other) in records()? {
        if let Some(reference) = other.packet.filter(|p| p.managed) {
            if !lifetime::present(reference.arena)?.contains(&id) {
                continue;
            }
        }
        existing.push((id, other));
    }
    let conflicts = |port| {
        existing.iter().any(|(_, other)| {
            other.ns == state.ns
                && other.kind == state.kind
                && other.bound
                && other.local.family == wanted.family
                && other.local.port == port
                && (other.local.any() || wanted.any() || other.local.ip == wanted.ip)
                && !(state.kind == 1 && state.reuse && other.reuse && !other.listening)
        })
    };
    if wanted.port == 0 {
        wanted.port = (0..28232)
            .map(|offset| 32768 + ((owner.store.id() + offset) % 28232) as u16)
            .find(|port| !conflicts(*port))
            .ok_or(EADDRINUSE)?;
    } else if conflicts(wanted.port) {
        return Err(EADDRINUSE);
    }
    let binding = owner.packet.as_ref().ok_or(EIO)?;
    let mut concrete = wanted;
    if concrete.any() {
        concrete = if state.ns == 1 {
            wanted.local_transport()
        } else {
            primary(state.ns, wanted.family)?
        };
    }
    binding
        .root
        .set_addresses(&[IpCidr::new(route::endpoint(concrete)?.addr, 0)])?;
    let host = {
        let table = crate::table().read().map_err(|_| EIO)?;
        let entry = table
            .slots
            .get(fd as usize)
            .and_then(|entry| *entry)
            .ok_or(crate::EBADF)?;
        if !ENDPOINTS
            .lock()
            .map_err(|_| EIO)?
            .get(&entry.description_id)
            .is_some_and(|current| std::ptr::eq(Arc::as_ptr(current), owner))
        {
            return Err(crate::EBADF);
        }
        let mut link = wanted.local_transport();
        link.port = 0;
        transport_bind(entry.raw, link)?
    };
    if state.kind == 2 {
        binding.endpoint.bind_udp(route::endpoint(wanted)?)?;
    }
    edit(owner, |state| {
        state.local = wanted;
        state.host = host;
        state.bound = true;
        Ok(())
    })?;
    if expose {
        gateway::reserve(owner, wanted)?;
    }
    Ok(())
}
pub(super) fn autobind(fd: i32, owner: &Endpoint, target: Address) -> Result<(), i32> {
    let state = read(owner)?;
    if !state.bound {
        let mut local = if target.loopback() {
            target.local_transport()
        } else {
            primary(state.ns, target.family)?
        };
        local.port = 0;
        bind_internal(fd, owner, local, false)?;
    }
    Ok(())
}
pub(crate) fn connect(fd: i32, owner: &Endpoint, target: Option<Address>) -> Result<(), i32> {
    let state = read(owner)?;
    if state.kind == 1 {
        return super::tcp::connect(fd, owner, target.ok_or(crate::EAFNOSUPPORT)?);
    }
    if let Some(target) = target {
        if target.family != state.local.family || target.port == 0 {
            return Err(EINVAL);
        }
        if target.loopback() && !loopback_up(state.ns)? {
            return Err(ENETUNREACH);
        }
        autobind(fd, owner, target)?;
    }
    owner
        .packet
        .as_ref()
        .ok_or(EIO)?
        .endpoint
        .connect_udp(target.map(route::endpoint).transpose()?)?;
    edit(owner, |state| {
        state.peer = target.unwrap_or_default();
        Ok(())
    })
}
