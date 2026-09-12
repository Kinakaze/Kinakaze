//! Per-description host gateways, served by the existing ordinary loader helper.
//! The host stream terminates here; only newly generated IP packets can enter
//! guest TCP/BPF. Portable metadata, not cross-image Rust objects, crosses this
//! boundary. Closing the last guest token wakes the AFD wait and retires the link.
use super::*;
use crate::epoll::packet_wait::PacketWait;
use crate::usernet::stack::{IpCidr, SocketHandle, Stack, TcpState, shared::now_ms};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::windows::io::{AsRawSocket, FromRawSocket};
use std::thread::JoinHandle;
use windows_sys::Win32::Networking::WinSock as wsa;

pub(super) fn trace(id: u64, args: std::fmt::Arguments<'_>) {
    let Some(path) = std::env::var_os("KINAKAZE_GATEWAY_TRACE_DIR") else {
        return;
    };
    let path = std::path::Path::new(&path).join(format!("gateway-{}.log", std::process::id()));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{} {id} {args}", now_ms());
    }
}

fn error(error: std::io::Error) -> i32 {
    error
        .raw_os_error()
        .map(crate::socket::errno_from_wsa)
        .unwrap_or(EIO)
}
fn native(a: Address) -> Result<SocketAddr, i32> {
    let ip = match a.family {
        2 => std::net::IpAddr::V4(std::net::Ipv4Addr::new(a.ip[0], a.ip[1], a.ip[2], a.ip[3])),
        10 => std::net::IpAddr::V6(std::net::Ipv6Addr::from(a.ip)),
        _ => return Err(crate::EAFNOSUPPORT),
    };
    Ok(SocketAddr::new(ip, a.port))
}
fn guest(a: SocketAddr) -> Address {
    let mut result = Address {
        port: a.port(),
        ..Address::default()
    };
    match a.ip() {
        std::net::IpAddr::V4(ip) => {
            result.family = 2;
            result.ip[..4].copy_from_slice(&ip.octets());
        }
        std::net::IpAddr::V6(ip) => {
            result.family = 10;
            result.ip = ip.octets();
        }
    }
    result
}
fn link(family: u16) -> Result<UdpSocket, i32> {
    let address = Address {
        family,
        ..Address::default()
    }
    .local_transport();
    let socket = UdpSocket::bind(native(address)?).map_err(error)?;
    stack::transport::configure_link(socket.as_raw_socket() as usize)?;
    socket.set_nonblocking(true).map_err(error)?;
    Ok(socket)
}
fn snapshot(store: &Store) -> Result<State, i32> {
    State::decode(&store.read()?.1)
}
fn update_relay(store: &Store, remote: Address, transport: Address) -> Result<(), i32> {
    store.update(|bytes| {
        let mut state = State::decode(bytes)?;
        if let Some(row) = state.relays.iter_mut().find(|(peer, _)| *peer == remote) {
            row.1 = transport;
        } else {
            state.relays.push((remote, transport));
        }
        Ok((state.encode(), ()))
    })
}
pub(super) fn relay(state: &State, peer: Address) -> Option<Address> {
    state
        .relays
        .iter()
        .find(|(remote, _)| *remote == peer)
        .map(|(_, host)| *host)
}
pub(super) fn relay_for(state: &State, peer: Address) -> Result<Option<Address>, i32> {
    // Accepted descriptors drive the entire shared listener arena. A sibling
    // accepted later is absent from their initial portable snapshot, but its
    // gateway is published on the live listener description before its SYN.
    if let Some(reference) = state.packet.filter(|r| r.slot != u32::MAX) {
        match Store::user_object(reference.arena, false) {
            Ok(store) => {
                if let Some(host) = relay(&snapshot(&store)?, peer) {
                    return Ok(Some(host));
                }
            }
            Err(crate::ENOENT) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(relay(state, peer))
}
fn gate_key() -> Address {
    Address {
        port: 1,
        ..Address::default()
    }
}
pub(super) fn blocked(state: &State) -> bool {
    state.relays.iter().any(|(key, _)| *key == gate_key())
}
pub(super) fn mirror_filter(state: &mut State, filter: &crate::socket::filter::State) {
    state.relays.retain(|(key, _)| *key != gate_key());
    // A verified constant RET 0 is an unconditional drop. Native TCP admission
    // can defer this rule without inventing the host's original TCP headers.
    if filter
        .program
        .as_ref()
        .is_some_and(|p| p.encode() == [6, 0, 0, 0, 0, 0, 0, 0])
    {
        state.relays.push((gate_key(), Address::default()));
    }
}
pub(super) fn allowed(state: &State, target: Address) -> Result<(), i32> {
    if !target.loopback() && !target.any() {
        for ns in namespace_ids()?.into_iter().filter(|ns| *ns != 1) {
            if address_owned(ns, target)? {
                return Err(if reachable(state.ns, ns)? {
                    crate::ECONNREFUSED
                } else {
                    ENETUNREACH
                });
            }
        }
    }
    if state.ns == 1 {
        return Ok(());
    }
    if target.loopback() || address_owned(state.ns, target)? {
        return Err(crate::ECONNREFUSED);
    }
    if !reachable(state.ns, 1)?
        || !crate::route_state::snapshot_for(state.ns)?
            .routes
            .iter()
            .any(|r| r.family as u16 == target.family && r.dst_len == 0 && r.kind == 1)
    {
        return Err(ENETUNREACH);
    }
    Ok(())
}
pub(super) fn egress(owner: &Endpoint, target: Address) -> Result<Address, i32> {
    let state = read(owner)?;
    if let Some(host) = relay(&state, target) {
        return Ok(host);
    }
    allowed(&state, target)?;
    let host = crate::usernet_broker::request(
        if state.kind == 1 { 4 } else { 3 },
        owner.store.id(),
        target,
        0,
    )?;
    edit(owner, |state| {
        state.relays.push((target, host));
        Ok(())
    })?;
    Ok(host)
}
pub(super) fn route(id: u64, target: Address) -> Result<Address, i32> {
    let store = Store::user_object(id, false)
        .inspect_err(|e| trace(id, format_args!("route store {e}")))?;
    let state = snapshot(&store).inspect_err(|e| trace(id, format_args!("route snapshot {e}")))?;
    if let Some(host) = relay_for(&state, target)? {
        return Ok(host);
    }
    trace(
        id,
        format_args!(
            "route miss {:?} listening={} relays={}",
            native(target),
            state.listening,
            state.relays.len()
        ),
    );
    allowed(&state, target)?;
    let host = crate::usernet_broker::request(if state.kind == 1 { 4 } else { 3 }, id, target, 0)?;
    update_relay(&store, target, host)?;
    Ok(host)
}
pub(super) fn reserve(owner: &Endpoint, address: Address) -> Result<(), i32> {
    if read(owner)?.ns != 1 {
        return Ok(());
    }
    let kind = if read(owner)?.kind == 1 { 5 } else { 6 };
    let host = crate::usernet_broker::request(kind, owner.store.id(), address, 0)?;
    edit(owner, |state| {
        state.relays.push((Address::default(), host));
        Ok(())
    })
}
pub(super) fn kick(fd: i32, owner: &Endpoint) -> Result<(), i32> {
    if let Some(host) = relay(&read(owner)?, Address::default()) {
        io::clone_link(fd, owner)?
            .send_to(&[0], native(host)?)
            .map_err(error)?;
    }
    Ok(())
}
pub(super) fn activate(owner: &Endpoint) -> Result<(), i32> {
    let Some(host) = relay(&read(owner)?, Address::default()) else {
        return Ok(());
    };
    let control = link(host.family)?;
    control.connect(native(host)?).map_err(error)?;
    let mut request = b"CYGW".to_vec();
    request.extend_from_slice(&owner.store.id().to_le_bytes());
    control.send(&request).map_err(error)?;
    let waiter = PacketWait::new()?;
    let changed = crate::fs::object::Object::owned(unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(
            std::ptr::null(),
            1,
            0,
            std::ptr::null(),
        )
    })?;
    let deadline = now_ms() + 5000;
    let mut answer = [0u8; 4];
    loop {
        match control.recv(&mut answer) {
            Ok(4) => {
                let code = i32::from_le_bytes(answer);
                return if code == 0 { Ok(()) } else { Err(code) };
            }
            Ok(_) => return Err(EIO),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(error(e)),
        }
        let remaining = deadline.saturating_sub(now_ms());
        if remaining <= 0 {
            return Err(crate::ETIMEDOUT);
        }
        unsafe {
            waiter.publication(
                &[(control.as_raw_socket() as usize, 1)],
                changed.raw(),
                remaining as u32,
            )?;
        }
    }
}

struct Lease {
    store: Arc<Store>,
    arena: u64,
    watch: crate::fifo::lifecycle::DirectoryWatch,
    _namespace: Arc<Store>,
    _storage: crate::fs::object::Object,
    _directory: crate::fs::object::Object,
}
impl Lease {
    fn new(id: u64) -> Result<Self, i32> {
        let store = Arc::new(Store::user_object(id, false)?);
        let state = snapshot(&store)?;
        let arena = state.packet.ok_or(EINVAL)?.arena;
        Ok(Self {
            watch: lifetime::watch(arena)?,
            _namespace: crate::namespaces::pin_network(state.ns)?,
            _storage: SharedEndpoint::pin_reference(
                kinakaze_runtime::authority::domain_id(),
                arena,
            )?,
            _directory: SharedEndpoint::pin_allocation_directory()?,
            store,
            arena,
        })
    }
    fn live(&mut self) -> Result<bool, i32> {
        self.watch.rearm()?;
        Ok(lifetime::present(self.arena)?.contains(&self.store.id()))
    }
}

/// The helper only maps portable State and pins protocol storage opaquely. Its
/// loader executable and the guest's libc DLL have different Rust-image hashes.
pub(crate) fn start(kind: i32, id: u64, target: Address) -> Result<(Address, JoinHandle<()>), i32> {
    trace(id, format_args!("start {kind} {:?}", native(target)));
    let lease = Lease::new(id)?;
    let packet = link(target.family)?;
    let host = guest(packet.local_addr().map_err(error)?);
    let worker = match kind {
        3 => {
            let socket = bind_udp_native(
                Address {
                    family: target.family,
                    ..Address::default()
                },
                &snapshot(&lease.store)?,
            )?;
            socket.set_nonblocking(true).map_err(error)?;
            socket.connect(native(target)?).map_err(error)?;
            std::thread::spawn(move || {
                let result = udp_flow(lease, packet, socket, target);
                trace(id, format_args!("udp exit {result:?}"));
            })
        }
        4 => {
            let remote = connect_native(target)
                .inspect_err(|error| trace(id, format_args!("native connect {error}")))?;
            std::thread::spawn(move || {
                let result = tcp_flow(lease, packet, remote, target, None);
                trace(id, format_args!("tcp exit {result:?}"));
            })
        }
        5 => {
            let listener = bind_native(target, &snapshot(&lease.store)?)?;
            std::thread::spawn(move || {
                let _ = listen(lease, packet, listener);
            })
        }
        6 => {
            let socket = bind_udp_native(target, &snapshot(&lease.store)?)?;
            socket.set_nonblocking(true).map_err(error)?;
            std::thread::spawn(move || {
                let _ = udp_listener(lease, packet, socket);
            })
        }
        _ => return Err(EINVAL),
    };
    Ok((host, worker))
}
fn connect_native(target: Address) -> Result<TcpStream, i32> {
    crate::socket::ensure_winsock()?;
    let raw = unsafe {
        wsa::WSASocketW(
            if target.family == 10 { 23 } else { 2 },
            1,
            6,
            std::ptr::null(),
            0,
            wsa::WSA_FLAG_OVERLAPPED,
        )
    };
    if raw == wsa::INVALID_SOCKET {
        return Err(crate::socket::last_wsa_errno());
    }
    let stream = unsafe { TcpStream::from_raw_socket(raw as _) };
    stream.set_nonblocking(true).map_err(error)?;
    stream.set_nodelay(true).map_err(error)?;
    let (bytes, len) = target.bytes(true);
    crate::socket::connect_policy::configure(raw, &bytes[..len as usize])?;
    if unsafe { wsa::connect(raw, bytes.as_ptr().cast(), len) } != 0 {
        let result = crate::socket::last_wsa_errno();
        if !matches!(result, crate::EAGAIN | crate::EINPROGRESS) {
            return Err(result);
        }
    }
    Ok(stream)
}
fn bind_native(target: Address, state: &State) -> Result<TcpListener, i32> {
    crate::socket::ensure_winsock()?;
    let raw = unsafe {
        wsa::WSASocketW(
            if target.family == 10 { 23 } else { 2 },
            1,
            6,
            std::ptr::null(),
            0,
            wsa::WSA_FLAG_OVERLAPPED,
        )
    };
    if raw == wsa::INVALID_SOCKET {
        return Err(crate::socket::last_wsa_errno());
    }
    let listener = unsafe { TcpListener::from_raw_socket(raw as _) };
    listener.set_nonblocking(true).map_err(error)?;
    let one = 1i32;
    let option = if state.reuse {
        wsa::SO_REUSEADDR
    } else {
        wsa::SO_EXCLUSIVEADDRUSE
    };
    if unsafe { wsa::setsockopt(raw, wsa::SOL_SOCKET, option, (&one as *const i32).cast(), 4) } != 0
    {
        return Err(crate::socket::last_wsa_errno());
    }
    if unsafe { wsa::setsockopt(raw, wsa::SOL_SOCKET, 0x3002, (&one as *const i32).cast(), 4) } != 0
    {
        return Err(crate::socket::last_wsa_errno());
    }
    if target.family == 10 {
        let only = i32::from(state.v6only);
        if unsafe { wsa::setsockopt(raw, 41, 27, (&only as *const i32).cast(), 4) } != 0 {
            return Err(crate::socket::last_wsa_errno());
        }
    }
    let (bytes, len) = target.bytes(true);
    if unsafe { wsa::bind(raw, bytes.as_ptr().cast(), len) } != 0 {
        return Err(crate::socket::last_wsa_errno());
    }
    Ok(listener) // listen is deferred until the guest actually invokes it
}
fn bind_udp_native(target: Address, state: &State) -> Result<UdpSocket, i32> {
    crate::socket::ensure_winsock()?;
    let raw = unsafe {
        wsa::WSASocketW(
            if target.family == 10 { 23 } else { 2 },
            2,
            17,
            std::ptr::null(),
            0,
            wsa::WSA_FLAG_OVERLAPPED,
        )
    };
    if raw == wsa::INVALID_SOCKET {
        return Err(crate::socket::last_wsa_errno());
    }
    let socket = unsafe { UdpSocket::from_raw_socket(raw as _) };
    if target.family == 10 {
        let only = i32::from(state.v6only);
        if unsafe { wsa::setsockopt(raw, 41, 27, (&only as *const i32).cast(), 4) } != 0 {
            return Err(crate::socket::last_wsa_errno());
        }
    }
    socket.set_broadcast(state.broadcast).map_err(error)?;
    let (bytes, n) = target.bytes(true);
    if unsafe { wsa::bind(raw, bytes.as_ptr().cast(), n) } != 0 {
        return Err(crate::socket::last_wsa_errno());
    }
    Ok(socket)
}
fn engine(local: Address, seed: u64) -> Result<Stack, i32> {
    Stack::new(
        &[IpCidr::new(route::endpoint(local)?.addr, 0)],
        1,
        seed,
        now_ms(),
    )
}
fn flush(
    stack: &mut Stack,
    packet: &UdpSocket,
    host: Address,
    pending: &mut std::collections::VecDeque<Vec<u8>>,
) -> Result<bool, i32> {
    stack.poll(now_ms());
    while let Some(frame) = stack.take_packet() {
        pending.push_back(frame);
    }
    while let Some(frame) = pending.front() {
        match packet.send_to(frame, native(host.local_transport())?) {
            Ok(n) if n == frame.len() => {
                pending.pop_front();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(true),
            Err(e) => return Err(error(e)),
            _ => return Err(EIO),
        }
    }
    Ok(false)
}
fn timeout(stack: &mut Stack, deadline: Option<i64>) -> Option<u32> {
    stack
        .poll_delay(now_ms())
        .map(|n| now_ms().saturating_add(n.min(i64::MAX as u64) as i64))
        .into_iter()
        .chain(deadline)
        .min()
        .map(|n| n.saturating_sub(now_ms()).clamp(0, u32::MAX as i64 - 1) as u32)
}
fn abort_flow(
    stack: &mut Stack,
    handle: SocketHandle,
    packet: &UdpSocket,
    host: Address,
    output: &mut std::collections::VecDeque<Vec<u8>>,
    wait: &PacketWait,
    changed: windows_sys::Win32::Foundation::HANDLE,
) -> Result<(), i32> {
    stack.abort_tcp(handle)?;
    let deadline = now_ms() + 5000;
    while flush(stack, packet, host, output)? {
        let remaining = deadline.saturating_sub(now_ms());
        if remaining <= 0 {
            return Err(crate::ETIMEDOUT);
        }
        unsafe {
            wait.publication(
                &[(packet.as_raw_socket() as usize, 4)],
                changed,
                remaining as u32,
            )?;
        }
    }
    Ok(())
}
fn udp_flow(
    mut lease: Lease,
    packet: UdpSocket,
    remote: UdpSocket,
    target: Address,
) -> Result<(), i32> {
    let mut stack = engine(target, lease.store.id())?;
    let handle = stack.bind_udp(route::endpoint(target)?)?;
    let wait = PacketWait::new()?;
    let mut output = std::collections::VecDeque::new();
    let mut bytes = [0u8; 65536];
    let mut outbound: Option<Vec<u8>> = None;
    loop {
        if !lease.live()? {
            return Ok(());
        }
        let state = snapshot(&lease.store)?;
        for _ in 0..64 {
            match packet.recv_from(&mut bytes) {
                Ok((n, source)) if source == native(state.host.local_transport())? => {
                    stack.receive_packet(bytes[..n].to_vec(), now_ms());
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(error(e)),
            }
        }
        for _ in 0..64 {
            if outbound.is_none() {
                match stack.recv_udp(handle, &mut bytes, false) {
                    Ok((n, _, _)) => outbound = Some(bytes[..n].to_vec()),
                    Err(crate::EAGAIN) => break,
                    Err(e) => return Err(e),
                }
            }
            match remote.send(outbound.as_ref().unwrap()) {
                Ok(_) => outbound = None,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(error(e)),
            }
        }
        for _ in 0..64 {
            match remote.recv(&mut bytes) {
                Ok(n) => {
                    let _ = stack.send_udp(handle, &bytes[..n], route::endpoint(state.local)?);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(error(e)),
            }
        }
        let blocked = flush(&mut stack, &packet, state.host, &mut output)?;
        unsafe {
            wait.sockets(
                &[
                    (
                        packet.as_raw_socket() as usize,
                        1 | if blocked { 4 } else { 0 },
                    ),
                    (
                        remote.as_raw_socket() as usize,
                        1 | if outbound.is_some() { 4 } else { 0 },
                    ),
                ],
                lease.watch.event(),
                timeout(&mut stack, None),
            )?;
        }
    }
}

fn tcp_flow(
    mut lease: Lease,
    packet: UdpSocket,
    mut remote: TcpStream,
    target: Address,
    incoming: Option<Address>,
) -> Result<(), i32> {
    remote.set_nonblocking(true).map_err(error)?;
    if incoming.is_some() {
        remote.set_nodelay(true).map_err(error)?;
    }
    let mut stack = engine(target, lease.store.id())?;
    let handle = if let Some(local) = incoming {
        stack.connect(route::endpoint(target)?, route::endpoint(local)?)?
    } else {
        stack.listen(route::endpoint(target)?)?
    };
    let wait = PacketWait::new()?;
    let mut output = std::collections::VecDeque::new();
    let mut bytes = [0u8; 65536];
    let mut incoming_bytes = Vec::new();
    let mut incoming_at = 0;
    let mut host_eof = false;
    let mut guest_eof = false;
    let mut established = incoming.is_some();
    let connect_deadline = now_ms() + 30000;
    let mut failed = 0;
    let mut accepted_owner = None;
    let mut host_ready = false;
    let mut pending_syn = None;
    let mut orphan_deadline = None;
    loop {
        lease.watch.rearm()?;
        let owners = lifetime::present(lease.arena)?;
        if incoming.is_some() && accepted_owner.is_none() {
            accepted_owner = records()?.into_iter().find_map(|(id, state)| {
                (state.packet.is_some_and(|p| p.arena == lease.arena)
                    && state.peer == target
                    && !state.listening)
                    .then_some(id)
            });
        }
        let orphan = !owners.contains(&accepted_owner.unwrap_or(lease.store.id()));
        if orphan && orphan_deadline.is_none() {
            orphan_deadline = Some(now_ms() + 30000);
        }
        if orphan_deadline.is_some_and(|deadline| now_ms() >= deadline) {
            return Ok(());
        }
        let state = snapshot(&lease.store)?;
        if !established && failed == 0 {
            if let Some(e) = remote.take_error().map_err(error)? {
                failed = error(e);
            } else if host_ready {
                established = true;
            } else if now_ms() >= connect_deadline {
                failed = crate::ETIMEDOUT;
            }
            if failed != 0 {
                trace(lease.store.id(), format_args!("connect failed {failed}"));
                crate::usernet::connection_result(lease.store.id(), failed)?;
            }
        }
        if established {
            if let Some(frame) = pending_syn.take() {
                stack.receive_packet(frame, now_ms());
            }
        }
        for _ in 0..64 {
            match packet.recv_from(&mut bytes) {
                Ok((n, source)) if source == native(state.host.local_transport())? => {
                    if established || failed != 0 {
                        stack.receive_packet(bytes[..n].to_vec(), now_ms());
                        if failed != 0 {
                            while stack.take_packet().is_some() {}
                            stack.abort_tcp(handle)?;
                        }
                    } else if pending_syn.is_none() {
                        pending_syn = Some(bytes[..n].to_vec());
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(error(e)),
            }
        }
        let tcp = stack.tcp_state(handle)?;
        let connected = matches!(
            tcp,
            TcpState::Established
                | TcpState::CloseWait
                | TcpState::FinWait1
                | TcpState::FinWait2
                | TcpState::Closing
                | TcpState::LastAck
                | TcpState::TimeWait
        );
        if orphan && !connected && tcp != TcpState::SynReceived {
            return Ok(());
        }
        let mut output_blocked = false;
        if established && connected {
            for _ in 0..64 {
                match stack.recv_tcp(handle, &mut bytes, true) {
                    Ok(0) => {
                        if !guest_eof {
                            let _ = remote.shutdown(std::net::Shutdown::Write);
                            guest_eof = true;
                        }
                        break;
                    }
                    Ok(n) => match remote.write(&bytes[..n]) {
                        Ok(0) => return Err(crate::EPIPE),
                        Ok(written) => {
                            stack.recv_tcp(handle, &mut bytes[..written], false)?;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            output_blocked = true;
                            break;
                        }
                        Err(e) => {
                            let code = error(e);
                            abort_flow(
                                &mut stack,
                                handle,
                                &packet,
                                state.host,
                                &mut output,
                                &wait,
                                lease.watch.event(),
                            )?;
                            return Err(code);
                        }
                    },
                    Err(crate::EAGAIN) => break,
                    Err(e) => return Err(e),
                }
            }
            if incoming_at == incoming_bytes.len() && !host_eof {
                match remote.read(&mut bytes) {
                    Ok(0) => host_eof = true,
                    Ok(n) => {
                        incoming_bytes.clear();
                        incoming_bytes.extend_from_slice(&bytes[..n]);
                        incoming_at = 0;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        let code = error(e);
                        abort_flow(
                            &mut stack,
                            handle,
                            &packet,
                            state.host,
                            &mut output,
                            &wait,
                            lease.watch.event(),
                        )?;
                        return Err(code);
                    }
                }
            }
            if incoming_at < incoming_bytes.len() {
                match stack.send_tcp(handle, &incoming_bytes[incoming_at..]) {
                    Ok(n) => incoming_at += n,
                    Err(crate::EAGAIN) => {}
                    Err(e) => return Err(e),
                }
            }
            if host_eof && incoming_at == incoming_bytes.len() {
                stack.close_tcp(handle)?;
            }
            if orphan && !output_blocked {
                // Drain already received guest bytes before releasing the host
                // stream, then finish the guest FIN exchange. Its last fd can
                // disappear while the listener still drives the shared arena.
                let _ = remote.shutdown(std::net::Shutdown::Write);
                incoming_bytes.clear();
                incoming_at = 0;
                host_eof = true;
                stack.close_tcp(handle)?;
            }
        }
        let blocked = flush(&mut stack, &packet, state.host, &mut output)?;
        if failed != 0 && stack.tcp_state(handle)? == TcpState::Closed {
            return Ok(());
        }
        if (orphan || guest_eof && host_eof) && stack.tcp_state(handle)? == TcpState::Closed {
            return Ok(());
        }
        let mut sockets = vec![(
            packet.as_raw_socket() as usize,
            1 | if blocked { 4 } else { 0 },
        )];
        let interest = if !established {
            4
        } else {
            (if connected && incoming_at == incoming_bytes.len() && !host_eof {
                1 | 8
            } else {
                0
            }) | if output_blocked { 4 } else { 0 }
        };
        if interest != 0 {
            sockets.push((remote.as_raw_socket() as usize, interest));
        }
        let deadline = (!established && failed == 0)
            .then_some(connect_deadline)
            .into_iter()
            .chain(orphan_deadline)
            .min();
        let events = unsafe {
            wait.sockets(&sockets, lease.watch.event(), timeout(&mut stack, deadline))
                .inspect_err(|error| {
                    trace(
                        lease.store.id(),
                        format_args!("AFD established={established} error={error}"),
                    )
                })?
        };
        if events.get(1).is_some_and(|events| events & 4 != 0) {
            host_ready = true;
        }
    }
}

fn listen(mut lease: Lease, packet: UdpSocket, listener: TcpListener) -> Result<(), i32> {
    let wait = PacketWait::new()?;
    let mut listening = false;
    let mut workers = Vec::new();
    let mut bytes = [0u8; 65536];
    while lease.live()? {
        let mut replies = Vec::new();
        while let Ok((n, source)) = packet.recv_from(&mut bytes) {
            if n == 12 && bytes[..4] == *b"CYGW" && bytes[4..12] == lease.store.id().to_le_bytes() {
                replies.push(source);
            }
        }
        let state = snapshot(&lease.store)?;
        let mut activation_error = 0;
        if state.listening && !listening {
            if unsafe { wsa::listen(listener.as_raw_socket() as usize, 64) } != 0 {
                activation_error = crate::socket::last_wsa_errno();
            } else {
                listening = true;
            }
        }
        for peer in replies {
            packet
                .send_to(&activation_error.to_le_bytes(), peer)
                .map_err(error)?;
        }
        if activation_error != 0 {
            return Err(activation_error);
        }
        if listening && !blocked(&state) {
            for _ in 0..64 {
                if blocked(&snapshot(&lease.store)?) {
                    break;
                }
                workers.retain(|worker: &JoinHandle<()>| !worker.is_finished());
                match listener.accept() {
                    Ok((stream, peer)) => {
                        let peer = guest(peer);
                        let local = guest(stream.local_addr().map_err(error)?);
                        let flow = link(peer.family)?;
                        update_relay(&lease.store, peer, guest(flow.local_addr().map_err(error)?))?;
                        let child = Lease::new(lease.store.id())?;
                        workers.push(std::thread::spawn(move || {
                            let _ = tcp_flow(child, flow, stream, peer, Some(local));
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(error(e)),
                }
            }
        }
        let mut sockets = vec![(packet.as_raw_socket() as usize, 1)];
        if listening && !blocked(&state) {
            sockets.push((listener.as_raw_socket() as usize, 0x80));
        }
        unsafe {
            wait.sockets(&sockets, lease.watch.event(), None)?;
        }
    }
    drop(listener);
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

fn udp_listener(mut lease: Lease, packet: UdpSocket, socket: UdpSocket) -> Result<(), i32> {
    struct Flow {
        stack: Stack,
        handle: SocketHandle,
        used: i64,
        output: std::collections::VecDeque<Vec<u8>>,
    }
    let mut flows: Vec<(Address, Flow)> = Vec::new();
    let wait = PacketWait::new()?;
    let mut bytes = [0u8; 65536];
    loop {
        if !lease.live()? {
            return Ok(());
        }
        let state = snapshot(&lease.store)?;
        for _ in 0..64 {
            match socket.recv_from(&mut bytes) {
                Ok((n, peer)) => {
                    if blocked(&state) {
                        continue;
                    }
                    let peer = guest(peer);
                    let index = if let Some(index) = flows.iter().position(|(p, _)| *p == peer) {
                        index
                    } else {
                        if flows.len() >= 64 {
                            continue;
                        }
                        let mut stack = engine(peer, lease.store.id())?;
                        let handle = stack.bind_udp(route::endpoint(peer)?)?;
                        update_relay(
                            &lease.store,
                            peer,
                            guest(packet.local_addr().map_err(error)?),
                        )?;
                        flows.push((
                            peer,
                            Flow {
                                stack,
                                handle,
                                used: now_ms(),
                                output: Default::default(),
                            },
                        ));
                        flows.len() - 1
                    };
                    let flow = &mut flows[index].1;
                    flow.used = now_ms();
                    let mut local = state.local;
                    if local.any() {
                        local.ip = guest(socket.local_addr().map_err(error)?).ip;
                        if local.any() {
                            local = local.local_transport();
                        }
                    }
                    let _ = flow
                        .stack
                        .send_udp(flow.handle, &bytes[..n], route::endpoint(local)?);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(error(e)),
            }
        }
        for _ in 0..64 {
            match packet.recv_from(&mut bytes) {
                Ok((n, source)) if source == native(state.host.local_transport())? => {
                    if let Ok(Some((_, target, _))) = stack::packet_endpoints(&bytes[..n]) {
                        if let Some((_, flow)) = flows
                            .iter_mut()
                            .find(|(peer, _)| *peer == route::address(target))
                        {
                            flow.used = now_ms();
                            flow.stack.receive_packet(bytes[..n].to_vec(), now_ms());
                        }
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(error(e)),
            }
        }
        let mut blocked = false;
        for (peer, flow) in &mut flows {
            for _ in 0..64 {
                match flow.stack.recv_udp(flow.handle, &mut bytes, false) {
                    Ok((n, _, _)) => {
                        socket.send_to(&bytes[..n], native(*peer)?).map_err(error)?;
                    }
                    Err(crate::EAGAIN) => break,
                    Err(e) => return Err(e),
                }
            }
            blocked |= flush(&mut flow.stack, &packet, state.host, &mut flow.output)?;
        }
        flows.retain(|(_, flow)| now_ms() - flow.used < 300000);
        let expiry = flows
            .iter()
            .map(|(_, f)| f.used + 300000)
            .min()
            .map(|n| n.saturating_sub(now_ms()) as u32);
        unsafe {
            wait.sockets(
                &[
                    (
                        packet.as_raw_socket() as usize,
                        1 | if blocked { 4 } else { 0 },
                    ),
                    (socket.as_raw_socket() as usize, 1),
                ],
                lease.watch.event(),
                expiry,
            )?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket;
    fn packet(family: i32, kind: i32) -> i32 {
        let fd = socket::socket(family, 2 | socket::SOCK_NONBLOCK, 0).unwrap();
        let owner = crate::usernet::endpoint(fd).unwrap().unwrap();
        edit(&owner, |state| {
            state.kind = kind;
            Ok(())
        })
        .unwrap();
        super::super::initialize(fd).unwrap();
        fd
    }
    fn wait(fd: i32, interest: u32) {
        use crate::epoll::*;
        let ep = epoll_create1(0).unwrap();
        epoll_ctl(
            ep,
            EPOLL_CTL_ADD,
            fd,
            Some(EpollEvent {
                events: interest,
                data: 1,
            }),
        )
        .unwrap();
        let mut events = [EpollEvent::default(); 1];
        let result = epoll_wait(ep, &mut events, 3000);
        crate::close(ep).unwrap();
        assert_eq!(result, Ok(1));
    }
    #[test]
    fn outbound_tcp_and_udp_reach_native_peers_in_both_families() {
        for family in [2, 10] {
            let host = Address {
                family,
                ..Address::default()
            }
            .local_transport();
            let listener = TcpListener::bind(native(host).unwrap()).unwrap();
            let target = guest(listener.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = [0; 4];
                stream.read_exact(&mut bytes).unwrap();
                assert_eq!(&bytes, b"ping");
                stream.write_all(b"pong").unwrap();
                stream.shutdown(std::net::Shutdown::Write).unwrap();
            });
            let fd = packet(family as i32, 1);
            let (bytes, n) = target.bytes(false);
            assert_eq!(
                unsafe { socket::connect(fd, bytes.as_ptr(), n) },
                Err(crate::EINPROGRESS)
            );
            wait(fd, 4);
            let mut code = -1;
            let mut len = 4;
            unsafe {
                socket::getsockopt(fd, 1, 4, (&mut code as *mut i32).cast(), &mut len).unwrap();
            }
            assert_eq!(code, 0);
            assert_eq!(unsafe { socket::send(fd, b"ping".as_ptr(), 4, 0) }, Ok(4));
            wait(fd, 1);
            let mut data = [0; 32];
            assert_eq!(
                unsafe { socket::recv(fd, data.as_mut_ptr(), data.len(), 0) },
                Ok(4)
            );
            assert_eq!(&data[..4], b"pong");
            crate::close(fd).unwrap();
            worker.join().unwrap();
            let remote = UdpSocket::bind(native(host).unwrap()).unwrap();
            remote
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let target = guest(remote.local_addr().unwrap());
            let worker = std::thread::spawn(move || {
                let mut bytes = [0; 32];
                let (n, peer) = remote.recv_from(&mut bytes).unwrap();
                assert_eq!(&bytes[..n], b"udp");
                remote.send_to(b"reply", peer).unwrap();
            });
            let fd = packet(family as i32, 2);
            let (bytes, n) = target.bytes(false);
            assert_eq!(
                unsafe { socket::sendto(fd, b"udp".as_ptr(), 3, 0, bytes.as_ptr(), n) },
                Ok(3)
            );
            wait(fd, 1);
            assert_eq!(
                unsafe { socket::recv(fd, data.as_mut_ptr(), data.len(), 0) },
                Ok(5)
            );
            assert_eq!(&data[..5], b"reply");
            crate::close(fd).unwrap();
            worker.join().unwrap();
        }
    }
    #[test]
    fn constant_drop_filter_defers_the_native_tcp_handshake_until_detach() {
        for family in [2, 10] {
            let fd = packet(family, 1);
            let code = [6u8, 0, 0, 0, 0, 0, 0, 0];
            let mut program = [0u8; 16];
            program[..2].copy_from_slice(&1u16.to_le_bytes());
            program[8..].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
            unsafe {
                socket::setsockopt(fd, 1, 26, program.as_ptr(), 16).unwrap();
            }
            let local = Address {
                family: family as u16,
                ..Address::default()
            }
            .local_transport();
            let (bytes, n) = local.bytes(false);
            unsafe {
                socket::bind(fd, bytes.as_ptr(), n).unwrap();
            }
            socket::listen(fd, 8).unwrap();
            let target = read(&crate::usernet::endpoint(fd).unwrap().unwrap())
                .unwrap()
                .local;
            let native = connect_native(target).unwrap();
            let changed = crate::fs::object::Object::owned(unsafe {
                windows_sys::Win32::System::Threading::CreateEventW(
                    std::ptr::null(),
                    1,
                    0,
                    std::ptr::null(),
                )
            })
            .unwrap();
            let waiter = PacketWait::new().unwrap();
            let ready = unsafe {
                waiter.sockets(
                    &[(native.as_raw_socket() as usize, 4)],
                    changed.raw(),
                    Some(40),
                )
            }
            .unwrap();
            assert_eq!(ready[0], 0, "RET 0 must keep the host handshake pending");
            unsafe {
                socket::setsockopt(fd, 1, 27, [0u8; 4].as_ptr(), 4).unwrap();
            }
            wait(fd, 1);
            let accepted = unsafe {
                socket::accept(
                    fd,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    socket::SOCK_NONBLOCK,
                )
            }
            .unwrap();
            assert!(native.peer_addr().is_ok());
            crate::close(accepted).unwrap();
            crate::close(fd).unwrap();
        }
    }
    #[test]
    fn native_clients_reach_published_tcp_and_udp_in_both_families() {
        for family in [2, 10] {
            let host = Address {
                family,
                ..Address::default()
            }
            .local_transport();
            let listener = packet(family as i32, 1);
            let (bytes, n) = host.bytes(false);
            unsafe {
                socket::bind(listener, bytes.as_ptr(), n).unwrap();
            }
            socket::listen(listener, 8).unwrap();
            let target = read(&crate::usernet::endpoint(listener).unwrap().unwrap())
                .unwrap()
                .local;
            let worker = std::thread::spawn(move || {
                let mut stream = TcpStream::connect(native(target).unwrap()).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                stream.write_all(b"host").unwrap();
                let mut bytes = [0; 4];
                stream.read_exact(&mut bytes).unwrap();
                assert_eq!(&bytes, b"back");
            });
            wait(listener, 1);
            let fd = unsafe {
                socket::accept(
                    listener,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    socket::SOCK_NONBLOCK,
                )
            }
            .unwrap();
            wait(fd, 1);
            let mut bytes = [0; 32];
            assert_eq!(
                unsafe { socket::recv(fd, bytes.as_mut_ptr(), bytes.len(), 0) },
                Ok(4)
            );
            assert_eq!(&bytes[..4], b"host");
            assert_eq!(unsafe { socket::send(fd, b"back".as_ptr(), 4, 0) }, Ok(4));
            worker.join().unwrap();
            crate::close(fd).unwrap();
            crate::close(listener).unwrap();
            let fd = packet(family as i32, 2);
            let (bytes, n) = host.bytes(false);
            unsafe {
                socket::bind(fd, bytes.as_ptr(), n).unwrap();
            }
            let target = read(&crate::usernet::endpoint(fd).unwrap().unwrap())
                .unwrap()
                .local;
            let worker = std::thread::spawn(move || {
                let remote = UdpSocket::bind(native(host).unwrap()).unwrap();
                remote
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                remote.send_to(b"host", native(target).unwrap()).unwrap();
                let mut bytes = [0; 32];
                let (n, _) = remote.recv_from(&mut bytes).unwrap();
                assert_eq!(&bytes[..n], b"back");
            });
            wait(fd, 1);
            let mut data = [0; 32];
            let mut peer = [0; 128];
            let mut plen = 128;
            assert_eq!(
                unsafe {
                    socket::recvfrom(
                        fd,
                        data.as_mut_ptr(),
                        data.len(),
                        0,
                        peer.as_mut_ptr(),
                        &mut plen,
                    )
                },
                Ok(4)
            );
            assert_eq!(&data[..4], b"host");
            assert_eq!(
                unsafe { socket::sendto(fd, b"back".as_ptr(), 4, 0, peer.as_ptr(), plen) },
                Ok(4)
            );
            worker.join().unwrap();
            crate::close(fd).unwrap();
        }
    }
    #[test]
    fn bridge_forwards_tcp_udp_and_a_down_veth_is_isolated() {
        use crate::route_state::{self, Attribute, Link, Network, Route};
        struct Restore(Network);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = route_state::transaction_for(1, |n| {
                    *n = self.0.clone();
                    Ok::<_, i32>(())
                });
            }
        }
        let _restore = Restore(route_state::snapshot_for(1).unwrap());
        // Use the namespace allocator: route-state fixtures can retain links
        // from other tests, and veth peer indices must remain globally unique.
        let bridge = route_state::transaction_for(1, |n| {
            let first = n.allocate_index()?;
            for offset in 1..5 {
                assert_eq!(n.allocate_index()?, first + offset);
            }
            Ok::<_, i32>(first)
        })
        .unwrap()
        .unwrap();
        let link = |index, peer, master, kind: &str| Link {
            index,
            peer,
            master,
            kind: kind.into(),
            name: format!("pkt{index}"),
            flags: 1,
            mtu: 1500,
            address: vec![2, 0, 0, 0, 0, (index % 255) as u8],
            broadcast: vec![255; 6],
            attributes: vec![],
        };
        let mut held = Vec::new();
        let mut namespaces = Vec::new();
        route_state::transaction_for(1, |n| {
            n.links.push(link(bridge, 0, 0, "bridge"));
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
        for i in 0..2 {
            let previous = namespace_ids().unwrap();
            held.push(
                crate::namespaces::prepare(crate::namespaces::FLAGS[crate::namespaces::NET])
                    .unwrap(),
            );
            let ns = namespace_ids()
                .unwrap()
                .into_iter()
                .find(|id| !previous.contains(id))
                .unwrap();
            namespaces.push(ns);
            let host = bridge + 1 + i * 2;
            let child = host + 1;
            route_state::transaction_for(1, |n| {
                n.links.push(link(host, child, bridge, "veth"));
                Ok::<_, i32>(())
            })
            .unwrap()
            .unwrap();
            route_state::transaction_for(ns, |n| {
                n.loopback_flags = 0x49;
                n.links.push(link(child, host, 0, "veth"));
                n.addresses.push(route_state::Address {
                    index: child,
                    family: 2,
                    prefix_len: 24,
                    flags: 0,
                    scope: 0,
                    attributes: vec![Attribute {
                        kind: 1,
                        value: vec![172, 31, 231, 2 + i as u8],
                    }],
                });
                n.routes.push(Route {
                    family: 2,
                    dst_len: 0,
                    src_len: 0,
                    tos: 0,
                    table: 254,
                    protocol: 3,
                    scope: 0,
                    kind: 1,
                    flags: 0,
                    attributes: vec![],
                });
                Ok::<_, i32>(())
            })
            .unwrap()
            .unwrap();
        }
        for (source_ns, kind) in [(namespaces[0], 1), (namespaces[0], 2), (1, 1), (1, 2)] {
            let listener = {
                let _scope = crate::usernet::scope(namespaces[1]);
                packet(2, kind)
            };
            let local = Address {
                family: 2,
                ..Address::default()
            };
            let (bytes, n) = local.bytes(false);
            unsafe {
                socket::bind(listener, bytes.as_ptr(), n).unwrap();
            }
            if kind == 1 {
                socket::listen(listener, 8).unwrap();
            }
            let mut target = read(&crate::usernet::endpoint(listener).unwrap().unwrap())
                .unwrap()
                .local;
            target.ip[..4].copy_from_slice(&[172, 31, 231, 3]);
            let client = {
                let _scope = crate::usernet::scope(source_ns);
                packet(2, kind)
            };
            let (bytes, n) = target.bytes(false);
            if kind == 1 {
                assert_eq!(
                    unsafe { socket::connect(client, bytes.as_ptr(), n) },
                    Err(crate::EINPROGRESS)
                );
                wait(client, 4);
            }
            let fd = if kind == 1 {
                unsafe {
                    socket::accept(
                        listener,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        socket::SOCK_NONBLOCK,
                    )
                }
                .unwrap()
            } else {
                listener
            };
            let sent = if kind == 1 {
                unsafe { socket::send(client, b"bridge".as_ptr(), 6, 0) }
            } else {
                unsafe { socket::sendto(client, b"bridge".as_ptr(), 6, 0, bytes.as_ptr(), n) }
            };
            assert_eq!(sent, Ok(6));
            wait(fd, 1);
            let mut data = [0; 16];
            assert_eq!(unsafe { socket::recv(fd, data.as_mut_ptr(), 16, 0) }, Ok(6));
            assert_eq!(&data[..6], b"bridge");
            let peer = read(&crate::usernet::endpoint(client).unwrap().unwrap())
                .unwrap()
                .local;
            let (peer_bytes, peer_len) = peer.bytes(false);
            let replied = if kind == 1 {
                unsafe { socket::send(fd, b"return".as_ptr(), 6, 0) }
            } else {
                unsafe {
                    socket::sendto(fd, b"return".as_ptr(), 6, 0, peer_bytes.as_ptr(), peer_len)
                }
            };
            assert_eq!(replied, Ok(6));
            wait(client, 1);
            assert_eq!(
                unsafe { socket::recv(client, data.as_mut_ptr(), 16, 0) },
                Ok(6)
            );
            assert_eq!(&data[..6], b"return");
            crate::close(client).unwrap();
            if kind == 1 {
                // A queued reply may outlive its client descriptor. It must
                // not open a new host connection to that client's old port.
                use crate::usernet::stack::transport::PacketRouter;
                // The original protocol peer may still be pinned for its FIN
                // exchange. Use a tuple with no remaining descriptor instead.
                let mut departed_peer = peer;
                let rows = records().unwrap();
                departed_peer.port = (61000..65535)
                    .find(|port| rows.iter().all(|(_, state)| state.local.port != *port))
                    .unwrap();
                let mut departed = Stack::new(
                    &[IpCidr::new(route::endpoint(departed_peer).unwrap().addr, 0)],
                    1,
                    11,
                    0,
                )
                .unwrap();
                departed
                    .connect(
                        route::endpoint(departed_peer).unwrap(),
                        route::endpoint(target).unwrap(),
                    )
                    .unwrap();
                departed.poll(0);
                let mut responder = Stack::new(
                    &[IpCidr::new(route::endpoint(target).unwrap().addr, 0)],
                    1,
                    12,
                    0,
                )
                .unwrap();
                responder.listen(route::endpoint(target).unwrap()).unwrap();
                responder.receive_packet(departed.take_packet().unwrap(), 0);
                responder.poll(0);
                let reply = responder.take_packet().unwrap();
                let owner = crate::usernet::endpoint(listener).unwrap().unwrap();
                let router = route::Router::new(namespaces[1], Some(owner.store.id()));
                assert_eq!(
                    router.route(&reply),
                    Ok(None),
                    "reply to a departed TCP client"
                );
                assert!(read(&owner).unwrap().relays.is_empty());
            }
            crate::close(fd).unwrap();
            if fd != listener {
                assert_eq!(
                    unsafe {
                        socket::accept(
                            listener,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            socket::SOCK_NONBLOCK,
                        )
                    },
                    Err(crate::EAGAIN)
                );
                crate::close(listener).unwrap();
            }
        }
        route_state::transaction_for(1, |n| {
            n.links
                .iter_mut()
                .find(|l| l.index == bridge + 3)
                .unwrap()
                .flags = 0;
            Ok::<_, i32>(())
        })
        .unwrap()
        .unwrap();
        assert!(!reachable(namespaces[0], namespaces[1]).unwrap());
        let fd = {
            let _scope = crate::usernet::scope(namespaces[0]);
            packet(2, 1)
        };
        let owner = crate::usernet::endpoint(fd).unwrap().unwrap();
        let mut target = Address {
            family: 2,
            port: 80,
            ..Address::default()
        };
        target.ip[..4].copy_from_slice(&[172, 31, 231, 3]);
        assert_eq!(allowed(&read(&owner).unwrap(), target), Err(ENETUNREACH));
        crate::close(fd).unwrap();
        drop(held);
    }
}
