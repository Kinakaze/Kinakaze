//! Namespace-scoped socket addresses backed by unprivileged Winsock transports.
//! Socket descriptions pin their creating namespace across setns, dup and fork.
pub mod ioctl;
pub(crate) mod packet;
pub mod stack;
use crate::mount::shared::{self, Store};
use crate::{EADDRINUSE, EADDRNOTAVAIL, EINVAL, EIO, ENETUNREACH};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use windows_sys::Win32::Networking::WinSock::{SOCKET, SOCKET_ERROR, bind, getsockname};

thread_local! { static SCOPE: Cell<u64> = const { Cell::new(0) }; }
pub(crate) fn current() -> Result<u64, i32> {
    match SCOPE.get() {
        0 => crate::namespaces::current_id(crate::namespaces::NET),
        id => Ok(id),
    }
}
pub(crate) struct Scope(u64);
impl Drop for Scope {
    fn drop(&mut self) {
        SCOPE.set(self.0);
    }
}
pub(crate) fn scope(id: u64) -> Scope {
    Scope(SCOPE.replace(id))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Address {
    pub family: u16,
    pub port: u16,
    pub ip: [u8; 16],
    pub scope: u32,
}
impl Address {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() < 4 {
            return Err(EINVAL);
        }
        let family = u16::from_ne_bytes(bytes[..2].try_into().unwrap());
        let mut value = Self {
            family,
            port: u16::from_be_bytes(bytes[2..4].try_into().unwrap()),
            ..Self::default()
        };
        match family {
            2 if bytes.len() >= 16 => value.ip[..4].copy_from_slice(&bytes[4..8]),
            10 | 23 if bytes.len() >= 28 => {
                value.family = 10;
                value.ip.copy_from_slice(&bytes[8..24]);
                value.scope = u32::from_ne_bytes(bytes[24..28].try_into().unwrap());
            }
            _ => return Err(EINVAL),
        }
        Ok(value)
    }
    pub(crate) fn bytes(self, windows: bool) -> ([u8; 128], i32) {
        let mut bytes = [0; 128];
        bytes[..2].copy_from_slice(
            &(if windows && self.family == 10 {
                23u16
            } else {
                self.family
            })
            .to_ne_bytes(),
        );
        bytes[2..4].copy_from_slice(&self.port.to_be_bytes());
        if self.family == 2 {
            bytes[4..8].copy_from_slice(&self.ip[..4]);
        } else {
            bytes[8..24].copy_from_slice(&self.ip);
            bytes[24..28].copy_from_slice(&self.scope.to_ne_bytes());
        }
        (bytes, if self.family == 2 { 16 } else { 28 })
    }
    fn loopback(self) -> bool {
        if self.family == 2 {
            self.ip[0] == 127
        } else {
            self.ip[..15] == [0; 15] && self.ip[15] == 1
        }
    }
    fn any(self) -> bool {
        self.ip == [0; 16]
    }
    fn local_transport(self) -> Self {
        let mut value = Self {
            family: self.family,
            port: self.port,
            ..Self::default()
        };
        if value.family == 2 {
            value.ip[..4].copy_from_slice(&[127, 0, 0, 1]);
        } else {
            value.ip[15] = 1;
        }
        value
    }
    fn encode(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.family.to_le_bytes());
        out.extend_from_slice(&self.port.to_le_bytes());
        out.extend_from_slice(&self.ip);
        out.extend_from_slice(&self.scope.to_le_bytes());
    }
    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() != 24 {
            return Err(EIO);
        }
        Ok(Self {
            family: u16::from_le_bytes(bytes[..2].try_into().unwrap()),
            port: u16::from_le_bytes(bytes[2..4].try_into().unwrap()),
            ip: bytes[4..20].try_into().unwrap(),
            scope: u32::from_le_bytes(bytes[20..].try_into().unwrap()),
        })
    }
}
#[derive(Clone)]
struct State {
    ns: u64,
    kind: i32,
    local: Address,
    peer: Address,
    host: Address,
    bound: bool,
    listening: bool,
    reuse: bool,
    connecting: bool,
    error: i32,
    read_shut: bool,
    write_shut: bool,
    broadcast: bool,
    v6only: bool,
    packet: Option<packet::Reference>,
    relays: Vec<(Address, Address)>,
}
impl State {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = b"CYNETSK2".to_vec();
        bytes.extend_from_slice(&self.ns.to_le_bytes());
        bytes.extend_from_slice(&self.kind.to_le_bytes());
        bytes.extend_from_slice(
            &(u32::from(self.bound)
                | u32::from(self.listening) << 1
                | u32::from(self.reuse) << 2
                | u32::from(self.connecting) << 3
                | u32::from(self.read_shut) << 4
                | u32::from(self.write_shut) << 5
                | u32::from(self.broadcast) << 6
                | u32::from(self.v6only) << 7
                | (self.error as u32) << 8)
                .to_le_bytes(),
        );
        for a in [self.local, self.peer, self.host] {
            a.encode(&mut bytes);
        }
        packet::encode(self.packet, &mut bytes);
        for (remote, relay) in &self.relays {
            remote.encode(&mut bytes);
            relay.encode(&mut bytes);
        }
        bytes
    }
    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() < 120 || (bytes.len() - 120) % 48 != 0 || &bytes[..8] != b"CYNETSK2" {
            return Err(EIO);
        }
        Ok(Self {
            ns: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            kind: i32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            bound: bytes[20] & 1 != 0,
            listening: bytes[20] & 2 != 0,
            reuse: bytes[20] & 4 != 0,
            connecting: bytes[20] & 8 != 0,
            read_shut: bytes[20] & 16 != 0,
            write_shut: bytes[20] & 32 != 0,
            broadcast: bytes[20] & 64 != 0,
            v6only: bytes[20] & 128 != 0,
            error: (u32::from_le_bytes(bytes[20..24].try_into().unwrap()) >> 8) as i32,
            packet: packet::decode(&bytes[96..120])?,
            relays: bytes[120..]
                .chunks_exact(48)
                .map(|v| Ok((Address::decode(&v[..24])?, Address::decode(&v[24..])?)))
                .collect::<Result<_, i32>>()?,
            local: Address::decode(&bytes[24..48])?,
            peer: Address::decode(&bytes[48..72])?,
            host: Address::decode(&bytes[72..96])?,
        })
    }
}
pub(crate) struct Endpoint {
    store: Arc<Store>,
    _namespace: Arc<Store>,
    pins: Mutex<Option<Vec<Arc<crate::fs::object::Object>>>>,
    token: Mutex<Option<Arc<crate::fs::object::Object>>>,
    packet: Option<packet::Binding>,
}
impl Endpoint {
    fn open(store: Arc<Store>, namespace: Arc<Store>) -> Result<Arc<Self>, i32> {
        let state = State::decode(&store.read()?.1)?;
        let packet = state.packet.map(packet::Binding::open).transpose()?;
        if let Some(packet) = &packet {
            packet.endpoint.save_description(&state.encode())?;
        }
        Ok(Arc::new(Self {
            store,
            _namespace: namespace,
            pins: Mutex::new(None),
            token: Mutex::new(None),
            packet,
        }))
    }
    fn handoff_pins(&self) -> Result<Vec<Arc<crate::fs::object::Object>>, i32> {
        let mut saved = self.pins.lock().map_err(|_| EIO)?;
        if saved.is_none() {
            // Ordinary socket creation and I/O need no extra native handle.
            // Only fork/exec serialization requires inheritable handoff pins.
            let mut pins = vec![
                Arc::new(self.store.pin()?),
                Arc::new(self._namespace.pin()?),
            ];
            if let Some(packet) = &self.packet {
                pins.push(Arc::new(packet.endpoint.pin()?));
                if read(self)?.packet.is_some_and(|packet| packet.managed) {
                    pins.push(Arc::new(
                        stack::shared::SharedEndpoint::pin_allocation_directory()?,
                    ));
                    pins.push(self.token.lock().map_err(|_| EIO)?.clone().ok_or(EIO)?);
                }
            }
            for pin in &pins {
                crate::platform::try_set_inheritable(pin.raw() as usize, true)?;
            }
            *saved = Some(pins);
        }
        Ok(saved.as_ref().unwrap().clone())
    }
    fn exclude_inheritance(&self) {
        if let Ok(token) = self.token.lock() {
            if let Some(token) = &*token {
                crate::platform::set_inheritable(token.raw() as usize, false);
            }
        }
        if let Ok(pins) = self.pins.lock() {
            if let Some(pins) = &*pins {
                for pin in pins {
                    crate::platform::set_inheritable(pin.raw() as usize, false);
                }
            }
        }
    }
}
static ENDPOINTS: Mutex<BTreeMap<u64, Arc<Endpoint>>> = Mutex::new(BTreeMap::new());
static DIRECTORY: Mutex<Option<Arc<Store>>> = Mutex::new(None);
fn directory() -> Result<Arc<Store>, i32> {
    let mut slot = DIRECTORY.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        *slot = Some(Arc::new(Store::user_object(u64::MAX - 32, true)?));
    }
    Ok(slot.as_ref().unwrap().clone())
}
fn ids(bytes: &[u8]) -> Result<Vec<(u64, u64)>, i32> {
    if bytes.len() % 16 != 0 {
        return Err(EIO);
    }
    Ok(bytes
        .chunks_exact(16)
        .map(|b| {
            (
                u64::from_le_bytes(b[..8].try_into().unwrap()),
                u64::from_le_bytes(b[8..].try_into().unwrap()),
            )
        })
        .collect())
}
fn encode_ids(ids: &[(u64, u64)]) -> Vec<u8> {
    ids.iter()
        .flat_map(|(kind, id)| kind.to_le_bytes().into_iter().chain(id.to_le_bytes()))
        .collect()
}
fn register(kind: u64, id: u64) -> Result<(), i32> {
    directory()?.update(|bytes| {
        let mut list = ids(bytes)?;
        list.retain(|&(kind, id)| {
            kind == 1 && crate::namespaces::pin_network(id).is_ok()
                || kind == 2 && Store::user_object(id, false).is_ok()
        });
        if !list.contains(&(kind, id)) {
            list.push((kind, id));
        }
        Ok((encode_ids(&list), ()))
    })
}
pub(crate) fn register_namespace(id: u64) -> Result<(), i32> {
    register(1, id)
}
pub(crate) fn namespace_ids() -> Result<Vec<u64>, i32> {
    let mut values = vec![1];
    for (kind, id) in ids(&directory()?.read()?.1)? {
        if kind == 1 && crate::namespaces::pin_network(id).is_ok() {
            values.push(id);
        }
    }
    Ok(values)
}
fn endpoint(fd: i32) -> Result<Option<Arc<Endpoint>>, i32> {
    let entry = crate::get(fd)?;
    Ok(ENDPOINTS
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned())
}
fn publish(fd: i32, expected: u64, endpoint: Arc<Endpoint>) -> Result<(), i32> {
    // close holds TABLE before detaching ENDPOINTS. Keep that order, and keep
    // the descriptor pinned until publication so close/reuse cannot leave a
    // newly published endpoint behind for a dead description.
    let mut table = crate::table().write().map_err(|_| EIO)?;
    let entry = usize::try_from(fd)
        .ok()
        .and_then(|fd| table.slots.get(fd))
        .and_then(|entry| *entry)
        .ok_or(crate::EBADF)?;
    if entry.description_id != expected || entry.kind != crate::FdKind::Socket {
        return Err(crate::EBADF);
    }
    let mut endpoints = ENDPOINTS.lock().map_err(|_| EIO)?;
    if endpoint.packet.is_some() {
        for alias in table
            .slots
            .iter_mut()
            .flatten()
            .filter(|alias| alias.description_id == expected)
        {
            alias.flags = alias.flags.union(crate::FdFlags::PACKET_SOCKET);
        }
    } else if entry.flags.contains(crate::FdFlags::PACKET_SOCKET) {
        return Err(EIO);
    }
    if let Some(previous) = endpoints.insert(expected, endpoint.clone()) {
        if !Arc::ptr_eq(&previous, &endpoint) {
            previous.exclude_inheritance();
        }
    }
    Ok(())
}
pub(crate) fn attach(fd: i32, family: i32, kind: i32) -> Result<(), i32> {
    if family != 2 && family != 10 {
        return Ok(());
    }
    let description = crate::get(fd)?.description_id;
    let ns = current()?;
    if ns != 1 && kind != 1 && kind != 2 {
        return Err(crate::EPROTONOSUPPORT);
    }
    let namespace = crate::namespaces::pin_network(ns)?;
    let store = Arc::new(shared::new_object()?);
    let state = State {
        ns,
        kind,
        local: Address {
            family: family as u16,
            ..Address::default()
        },
        peer: Address::default(),
        host: Address::default(),
        bound: false,
        listening: false,
        reuse: false,
        connecting: false,
        error: 0,
        read_shut: false,
        write_shut: false,
        broadcast: false,
        v6only: false,
        packet: None,
        relays: Vec::new(),
    };
    store.update(|_| Ok((state.encode(), ())))?;
    register(2, store.id())?;
    publish(fd, description, Endpoint::open(store, namespace)?)?;
    Ok(())
}
pub(crate) fn closed(description: u64) {
    if let Ok(mut endpoints) = ENDPOINTS.lock() {
        if let Some(endpoint) = endpoints.remove(&description) {
            // A pending operation/rights export can keep this local reference
            // alive, but it must not leak its pins into an unrelated fork.
            endpoint.exclude_inheritance();
        }
    }
}
pub(crate) fn auxiliary_handles() -> Result<Vec<(u64, Arc<crate::fs::object::Object>)>, i32> {
    let mut result = Vec::new();
    for (&id, endpoint) in ENDPOINTS.lock().map_err(|_| EIO)?.iter() {
        if let Some(pins) = &*endpoint.pins.lock().map_err(|_| EIO)? {
            result.extend(pins.iter().map(|pin| (id, pin.clone())));
        }
        if let Some(token) = &*endpoint.token.lock().map_err(|_| EIO)? {
            result.push((id, token.clone()));
        }
    }
    Ok(result)
}
fn read(item: &Endpoint) -> Result<State, i32> {
    State::decode(&item.store.read()?.1)
}
fn edit<T>(item: &Endpoint, f: impl FnOnce(&mut State) -> Result<T, i32>) -> Result<T, i32> {
    let (value, snapshot) = item.store.update(|bytes| {
        let mut state = State::decode(bytes)?;
        let value = f(&mut state)?;
        let snapshot = state.encode();
        Ok((snapshot.clone(), (value, snapshot)))
    })?;
    if let Some(packet) = &item.packet {
        packet.endpoint.save_description(&snapshot)?;
    }
    Ok(value)
}
fn host_address(raw: usize) -> Result<Address, i32> {
    let mut bytes = [0; 128];
    let mut len = 128;
    if unsafe { getsockname(raw as SOCKET, bytes.as_mut_ptr().cast(), &mut len) } == SOCKET_ERROR {
        return Err(crate::socket::last_wsa_errno());
    }
    Address::parse(&bytes[..len as usize])
}
fn records() -> Result<Vec<(u64, State)>, i32> {
    let mut result = Vec::new();
    for (kind, id) in ids(&directory()?.read()?.1)? {
        if kind != 2 {
            continue;
        }
        if let Ok(store) = Store::user_object(id, false) {
            result.push((id, State::decode(&store.read()?.1)?));
        }
    }
    Ok(result)
}
fn address_owned(ns: u64, addr: Address) -> Result<bool, i32> {
    if addr.any() || addr.loopback() {
        return Ok(true);
    }
    let mut addresses = crate::route_state::snapshot_for(ns)?.addresses;
    if ns == 1 {
        addresses.extend(crate::hostnet::addresses(
            ns,
            &crate::hostnet::interfaces(ns)?,
        )?);
    }
    Ok(addresses.iter().any(|a| {
        a.family as u16 == addr.family
            && a.attributes.iter().any(|v| {
                matches!(v.kind, 1 | 2)
                    && v.value == addr.ip[..if addr.family == 2 { 4 } else { 16 }]
            })
    }))
}
fn loopback_up(ns: u64) -> Result<bool, i32> {
    Ok(ns == 1 || crate::route_state::snapshot_for(ns)?.loopback_flags & 1 != 0)
}
fn transport_bind(raw: usize, address: Address) -> Result<Address, i32> {
    let (bytes, len) = address.bytes(true);
    if unsafe { bind(raw as SOCKET, bytes.as_ptr().cast(), len) } == SOCKET_ERROR {
        return Err(crate::socket::last_wsa_errno());
    }
    host_address(raw)
}
pub(crate) fn bind_address(fd: i32, raw: usize, bytes: &[u8]) -> Result<bool, i32> {
    let Some(item) = endpoint(fd)? else {
        return Ok(false);
    };
    let mut wanted = Address::parse(bytes)?;
    let state = read(&item)?;
    if state.ns == 1 {
        return Ok(false);
    }
    if state.bound {
        return Err(EINVAL);
    }
    if wanted.family != state.local.family {
        return Err(EINVAL);
    }
    if !address_owned(state.ns, wanted)? {
        return Err(EADDRNOTAVAIL);
    }
    let _guard = shared::topology_guard()?;
    let existing = records()?;
    if wanted.port == 0 {
        wanted.port = (0..28232)
            .map(|offset| 32768 + ((item.store.id() + offset) % 28232) as u16)
            .find(|port| {
                !existing.iter().any(|(_, other)| {
                    other.ns == state.ns
                        && other.kind == state.kind
                        && other.bound
                        && other.local.family == wanted.family
                        && other.local.port == *port
                        && (other.local.any() || wanted.any() || other.local.ip == wanted.ip)
                })
            })
            .ok_or(EADDRINUSE)?;
    }
    for (_, other) in existing {
        if other.ns == state.ns
            && other.kind == state.kind
            && other.bound
            && other.local.family == wanted.family
            && other.local.port == wanted.port
            && wanted.port != 0
            && (other.local.any() || wanted.any() || other.local.ip == wanted.ip)
            && !(state.reuse && other.reuse && state.kind == 2)
        {
            return Err(EADDRINUSE);
        }
    }
    let mut transport = wanted.local_transport();
    transport.port = 0;
    let host = transport_bind(raw, transport)?;
    edit(&item, |s| {
        s.local = wanted;
        if s.local.port == 0 {
            s.local.port = host.port;
        }
        s.host = host;
        s.bound = true;
        Ok(())
    })?;
    Ok(true)
}
pub(crate) fn bound_native(fd: i32, raw: usize) -> Result<(), i32> {
    if let Some(item) = endpoint(fd)? {
        let host = host_address(raw)?;
        edit(&item, |s| {
            s.local = host;
            s.host = host;
            s.bound = true;
            Ok(())
        })?;
    }
    Ok(())
}
pub(crate) fn listening(fd: i32, raw: usize) -> Result<(), i32> {
    if let Some(item) = endpoint(fd)? {
        let state = read(&item)?;
        if !state.bound {
            let (bytes, len) = state.local.bytes(true);
            if !bind_address(fd, raw, &bytes[..len as usize])? {
                transport_bind(raw, state.local)?;
                bound_native(fd, raw)?;
            }
        }
        edit(&item, |s| {
            s.listening = true;
            Ok(())
        })?;
    }
    Ok(())
}
fn primary(ns: u64, family: u16) -> Result<Address, i32> {
    let addresses = if ns == 1 {
        let interfaces = crate::hostnet::interfaces(ns)?;
        crate::hostnet::addresses(ns, &interfaces)?
    } else {
        crate::route_state::snapshot_for(ns)?.addresses
    };
    for addr in &addresses {
        if addr.family as u16 != family
            || addr.index == 1
            || (ns == 1 && (addr.scope >= 253 || addr.flags & (0x08 | 0x20 | 0x40) != 0))
        {
            continue;
        }
        if let Some(value) = addr.attributes.iter().find(|a| matches!(a.kind, 1 | 2)) {
            let mut result = Address {
                family,
                ..Address::default()
            };
            let n = if family == 2 { 4 } else { 16 };
            if value.value.len() == n {
                result.ip[..n].copy_from_slice(&value.value);
                return Ok(result);
            }
        }
    }
    Err(ENETUNREACH)
}
fn reachable(source: u64, target: u64) -> Result<bool, i32> {
    if source == target {
        return Ok(true);
    }
    let snapshots = namespace_ids()?
        .into_iter()
        .map(|id| crate::route_state::snapshot_for(id).map(|n| (id, n)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut pending = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    if let Some((_, network)) = snapshots.iter().find(|(id, _)| *id == source) {
        for link in &network.links {
            if link.flags & 1 != 0 {
                pending.push((source, link.index));
            }
        }
    }
    while let Some((ns, index)) = pending.pop() {
        if !seen.insert((ns, index)) {
            continue;
        }
        if ns == target {
            return Ok(true);
        }
        let Some((_, network)) = snapshots.iter().find(|(id, _)| *id == ns) else {
            continue;
        };
        let Some(link) = network
            .links
            .iter()
            .find(|l| l.index == index && l.flags & 1 != 0)
        else {
            continue;
        };
        if link.master != 0 {
            pending.push((ns, link.master));
        }
        if link.kind == "bridge" {
            for port in &network.links {
                if port.master == index && port.flags & 1 != 0 {
                    pending.push((ns, port.index));
                }
            }
        }
        if link.peer != 0 {
            for (peer_ns, net) in &snapshots {
                if net
                    .links
                    .iter()
                    .any(|p| p.index == link.peer && p.flags & 1 != 0)
                {
                    pending.push((*peer_ns, link.peer));
                }
            }
        }
    }
    Ok(false)
}
pub(crate) fn destination(
    fd: i32,
    raw: usize,
    target: &[u8],
    connecting: bool,
) -> Result<Option<([u8; 128], i32)>, i32> {
    let Some(item) = endpoint(fd)? else {
        return Ok(None);
    };
    let state = read(&item)?;
    let wanted = Address::parse(target)?;
    let mut chosen = None;
    for (_, other) in records()? {
        if !other.bound
            || other.kind != state.kind
            || other.local.family != wanted.family
            || other.local.port != wanted.port
        {
            continue;
        }
        if state.kind == 1 && !other.listening {
            continue;
        }
        let matches = if wanted.loopback() {
            other.ns == state.ns && (other.local.any() || other.local.ip == wanted.ip)
        } else {
            (other.local.ip == wanted.ip || (other.local.any() && address_owned(other.ns, wanted)?))
                && reachable(state.ns, other.ns)?
        };
        if matches {
            chosen = Some(other);
            break;
        }
    }
    if wanted.loopback() && !loopback_up(state.ns)? {
        return Err(ENETUNREACH);
    }
    if let Some(other) = chosen {
        if !state.bound {
            let mut local = if wanted.loopback() {
                wanted.local_transport()
            } else {
                primary(state.ns, wanted.family)?
            };
            local.port = 0;
            let (bytes, len) = local.bytes(true);
            if !bind_address(fd, raw, &bytes[..len as usize])? {
                let host = transport_bind(raw, local.local_transport())?;
                edit(&item, |s| {
                    s.local = host;
                    s.host = host;
                    s.bound = true;
                    Ok(())
                })?;
            }
        }
        if connecting {
            edit(&item, |s| {
                s.peer = wanted;
                Ok(())
            })?;
        }
        return Ok(Some(other.host.local_transport().bytes(true)));
    }
    if state.ns != 1 {
        if wanted.loopback() || address_owned(state.ns, wanted)? {
            return Err(crate::ECONNREFUSED);
        }
        let network = crate::route_state::snapshot_for(state.ns)?;
        if !network
            .routes
            .iter()
            .any(|r| r.family as u16 == wanted.family && r.dst_len == 0 && r.kind == 1)
        {
            return Err(ENETUNREACH);
        }
        if !reachable(state.ns, 1)? {
            return Err(ENETUNREACH);
        }
        if !state.bound {
            let mut local = primary(state.ns, wanted.family)?;
            local.port = 0;
            let (bytes, len) = local.bytes(true);
            bind_address(fd, raw, &bytes[..len as usize])?;
        }
        return egress_bound(fd, raw, wanted, connecting);
    }
    // Backend listener ports are internal implementation addresses, even to
    // initial-namespace guests; they cannot be used to cross namespace borders.
    if wanted.loopback()
        && records()?
            .iter()
            .any(|(_, r)| r.ns != 1 && r.host.family == wanted.family && r.host.port == wanted.port)
    {
        return Err(crate::ECONNREFUSED);
    }
    Ok(None)
}
fn egress_bound(
    fd: i32,
    raw: usize,
    wanted: Address,
    connecting: bool,
) -> Result<Option<([u8; 128], i32)>, i32> {
    let item = endpoint(fd)?.ok_or(crate::EBADF)?;
    let state = read(&item)?;
    if state.kind == 1 {
        if !connecting {
            return Err(crate::EISCONN);
        }
        if state.connecting {
            return Err(crate::EALREADY);
        }
        edit(&item, |s| {
            s.connecting = true;
            s.error = 0;
            s.peer = wanted;
            Ok(())
        })?;
        if let Err(error) = crate::usernet_broker::request(1, item.store.id(), wanted, raw) {
            connection_result(item.store.id(), error)?;
            return Err(error);
        }
        return Err(crate::EINPROGRESS);
    }
    if let Some((_, relay)) = state.relays.iter().find(|(remote, _)| *remote == wanted) {
        return Ok(Some(relay.bytes(true)));
    }
    let relay = crate::usernet_broker::request(2, item.store.id(), wanted, raw)?;
    edit(&item, |s| {
        s.relays.push((wanted, relay));
        if connecting {
            s.peer = wanted;
        }
        Ok(())
    })?;
    Ok(Some(relay.bytes(true)))
}
pub(crate) fn lease_exists(id: u64) -> bool {
    Store::user_object(id, false).is_ok()
}
pub(crate) fn transport_matches(id: u64, port: u16) -> bool {
    Store::user_object(id, false)
        .and_then(|s| State::decode(&s.read()?.1))
        .is_ok_and(|s| s.host.port == port)
}
pub(crate) fn connection_result(id: u64, error: i32) -> Result<(), i32> {
    Store::user_object(id, false)?.update(|bytes| {
        let mut state = State::decode(bytes)?;
        state.connecting = false;
        state.error = error;
        Ok((state.encode(), ()))
    })
}
pub(crate) fn socket_error(fd: i32) -> Result<Option<i32>, i32> {
    let Some(item) = endpoint(fd)? else {
        return Ok(None);
    };
    edit(&item, |s| {
        if s.error != 0 {
            let e = s.error;
            s.error = 0;
            Ok(Some(e))
        } else {
            Ok(None)
        }
    })
}
pub(crate) fn sent(fd: i32, raw: usize) -> Result<(), i32> {
    if let Some(item) = endpoint(fd)? {
        let host = host_address(raw)?;
        if host.port != 0 {
            edit(&item, |s| {
                s.host = host;
                if s.local.port == 0 {
                    s.local.port = host.port;
                }
                s.bound = true;
                Ok(())
            })?;
        }
    }
    Ok(())
}
pub(crate) fn name(fd: i32, peer: bool) -> Result<Option<([u8; 128], i32)>, i32> {
    let Some(item) = endpoint(fd)? else {
        return Ok(None);
    };
    let state = read(&item)?;
    if state.packet.is_none() && state.ns == 1 && (!peer || state.peer.family == 0) {
        return Ok(None);
    }
    let address = if peer { state.peer } else { state.local };
    if peer && address.family == 0 {
        return Err(crate::ENOTCONN);
    }
    Ok(Some(address.bytes(true)))
}
pub(crate) fn source(fd: i32, source: &mut [u8; 128], length: &mut i32) -> Result<(), i32> {
    let Some(item) = endpoint(fd)? else {
        return Ok(());
    };
    let state = read(&item)?;
    let addr = Address::parse(&source[..*length as usize])?;
    if let Some((remote, _)) = state.relays.iter().find(|(_, relay)| {
        relay.family == addr.family && relay.port == addr.port && addr.loopback()
    }) {
        (*source, *length) = remote.bytes(true);
        return Ok(());
    }
    if addr.loopback() {
        if let Some((_, other)) = records()?.into_iter().find(|(_, r)| {
            r.kind == state.kind
                && r.host.family == addr.family
                && r.host.port == addr.port
                && r.bound
        }) {
            let mut local = other.local;
            if local.any() {
                local = addr;
                local.port = other.local.port;
            }
            (*source, *length) = local.bytes(true);
        }
    }
    Ok(())
}
pub(crate) fn accepted(listener: i32, fd: i32, raw: usize, source: &[u8]) -> Result<(), i32> {
    let description = crate::get(fd)?.description_id;
    let Some(parent) = endpoint(listener)? else {
        return Ok(());
    };
    let state = read(&parent)?;
    let store = Arc::new(shared::new_object()?);
    let peer = Address::parse(source)?;
    let new = State {
        peer,
        host: host_address(raw)?,
        listening: false,
        ..state.clone()
    };
    store.update(|_| Ok((new.encode(), ())))?;
    publish(
        fd,
        description,
        Endpoint::open(store, crate::namespaces::pin_network(state.ns)?)?,
    )?;
    Ok(())
}
pub(crate) fn reuse(fd: i32, value: bool) -> Result<(), i32> {
    if let Some(item) = endpoint(fd)? {
        edit(&item, |s| {
            s.reuse = value;
            Ok(())
        })?;
    }
    Ok(())
}
pub(crate) fn rights_reference(entry: crate::FdEntry) -> Result<Option<Arc<Endpoint>>, i32> {
    Ok(ENDPOINTS
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned())
}
pub(crate) fn rights_id(endpoint: &Endpoint) -> u64 {
    endpoint.store.id()
}
pub(crate) fn rights_token(
    endpoint: &Endpoint,
) -> Result<Option<Arc<crate::fs::object::Object>>, i32> {
    Ok(endpoint.token.lock().map_err(|_| EIO)?.clone())
}
pub(crate) fn pin_packet_rights(id: u64) -> Result<Vec<crate::fs::object::Object>, i32> {
    let store = Store::user_object(id, false)?;
    let state = State::decode(&store.read()?.1)?;
    let packet = state.packet.ok_or(EIO)?;
    let mut pins = vec![
        store.pin()?,
        crate::namespaces::pin_network(state.ns)?.pin()?,
        stack::shared::SharedEndpoint::pin_reference(
            kinakaze_runtime::authority::domain_id(),
            packet.arena,
        )?,
    ];
    if packet.managed {
        pins.push(stack::shared::SharedEndpoint::pin_allocation_directory()?);
    }
    Ok(pins)
}
pub(crate) fn import_rights(
    entry: crate::FdEntry,
    id: u64,
    token: Option<crate::fs::object::Object>,
) -> Result<(), i32> {
    if id == 0 {
        return Ok(());
    }
    let store = Arc::new(Store::user_object(id, false)?);
    let state = State::decode(&store.read()?.1)?;
    if state.packet.is_some() != entry.flags.contains(crate::FdFlags::PACKET_SOCKET) {
        return Err(EIO);
    }
    let endpoint = Endpoint::open(store, crate::namespaces::pin_network(state.ns)?)?;
    if state.packet.is_some_and(|packet| packet.managed) != token.is_some() {
        return Err(EIO);
    }
    if let Some(token) = token {
        crate::platform::try_set_inheritable(token.raw() as usize, true)?;
        *endpoint.token.lock().map_err(|_| EIO)? = Some(Arc::new(token));
    }
    let table = crate::table().read().map_err(|_| EIO)?;
    if !table.slots.iter().flatten().any(|current| {
        current.description_id == entry.description_id && current.generation == entry.generation
    }) {
        return Err(crate::EBADF);
    }
    if let Some(previous) = ENDPOINTS
        .lock()
        .map_err(|_| EIO)?
        .insert(entry.description_id, endpoint)
    {
        previous.exclude_inheritance();
    }
    Ok(())
}
pub(crate) fn serialize(mut keep: impl FnMut(i32) -> bool) -> Result<Vec<u8>, i32> {
    let entries: Vec<_> = crate::table()
        .read()
        .map_err(|_| EIO)?
        .slots
        .enumerated()
        .filter_map(|(fd, e)| {
            e.filter(|e| e.kind == crate::FdKind::Socket)
                .map(|e| (fd as i32, e.description_id))
        })
        .collect();
    let endpoints = ENDPOINTS.lock().map_err(|_| EIO)?;
    let mut bytes = b"CYNETFD5".to_vec();
    let mut seen = std::collections::HashSet::new();
    for (fd, description) in entries {
        if keep(fd) && seen.insert(description) {
            if let Some(item) = endpoints.get(&description) {
                bytes.extend_from_slice(&fd.to_le_bytes());
                bytes.extend_from_slice(&item.store.id().to_le_bytes());
                let pins = item.handoff_pins()?;
                for pin in &pins {
                    bytes.extend_from_slice(&(pin.raw() as u64).to_le_bytes());
                }
                for _ in pins.len()..5 {
                    bytes.extend_from_slice(&0u64.to_le_bytes());
                }
            }
        }
    }
    Ok(bytes)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    let run = || -> Result<(), i32> {
        if bytes.len() < 8 || &bytes[..8] != b"CYNETFD5" || (bytes.len() - 8) % 52 != 0 {
            return Err(EINVAL);
        }
        let mut endpoints = BTreeMap::new();
        for b in bytes[8..].chunks_exact(52) {
            let fd = i32::from_le_bytes(b[..4].try_into().unwrap());
            let id = u64::from_le_bytes(b[4..12].try_into().unwrap());
            // Native inheritance pins the objects before the parent may exit.
            // Open our own references before releasing those handoff handles.
            let mut inherited = vec![
                crate::fs::object::Object::owned(
                    u64::from_le_bytes(b[12..20].try_into().unwrap()) as _,
                )?,
                crate::fs::object::Object::owned(
                    u64::from_le_bytes(b[20..28].try_into().unwrap()) as _,
                )?,
            ];
            let packet_pin = u64::from_le_bytes(b[28..36].try_into().unwrap());
            if packet_pin != 0 {
                inherited.push(crate::fs::object::Object::owned(packet_pin as _)?);
            }
            let allocation_pin = u64::from_le_bytes(b[36..44].try_into().unwrap());
            if allocation_pin != 0 {
                inherited.push(crate::fs::object::Object::owned(allocation_pin as _)?);
            }
            let token_pin = u64::from_le_bytes(b[44..52].try_into().unwrap());
            if token_pin != 0 {
                inherited.push(crate::fs::object::Object::owned(token_pin as _)?);
            }
            let store = Arc::new(Store::user_object(id, false)?);
            let state = State::decode(&store.read()?.1)?;
            if state.packet.is_some() != (packet_pin != 0) {
                return Err(EIO);
            }
            if state.packet.is_some_and(|packet| packet.managed) != (allocation_pin != 0) {
                return Err(EIO);
            }
            if (allocation_pin != 0) != (token_pin != 0) {
                return Err(EIO);
            }
            if state.packet.is_some()
                != crate::get(fd)?
                    .flags
                    .contains(crate::FdFlags::PACKET_SOCKET)
            {
                return Err(EIO);
            }
            let endpoint = Endpoint::open(store, crate::namespaces::pin_network(state.ns)?)?;
            for pin in &inherited {
                crate::platform::try_set_inheritable(pin.raw() as usize, true)?;
            }
            let inherited: Vec<_> = inherited.into_iter().map(Arc::new).collect();
            if token_pin != 0 {
                *endpoint.token.lock().map_err(|_| EIO)? = inherited.last().cloned();
            }
            *endpoint.pins.lock().map_err(|_| EIO)? = Some(inherited);
            endpoints.insert(crate::get(fd)?.description_id, endpoint);
        }
        *ENDPOINTS.lock().map_err(|_| EIO)? = endpoints;
        Ok(())
    };
    run().is_ok()
}

#[cfg(test)]
mod bridge_tests;
#[cfg(test)]
mod handoff_tests;

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::socket::{self, AF_INET, SOCK_DGRAM};

    #[test]
    fn publication_rejects_another_description_and_concurrent_churn_completes() {
        let first = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();
        let original = crate::get(first).unwrap().description_id;
        let saved = endpoint(first).unwrap().unwrap();
        crate::close(first).unwrap();
        let second = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();
        assert_eq!(publish(second, original, saved), Err(crate::EBADF));
        assert!(!ENDPOINTS.lock().unwrap().contains_key(&original));
        crate::close(second).unwrap();

        let start = Arc::new(std::sync::Barrier::new(6));
        let (done, completion) = std::sync::mpsc::channel();
        let workers: Vec<_> = (0..6)
            .map(|_| {
                let start = start.clone();
                let done = done.clone();
                std::thread::spawn(move || {
                    start.wait();
                    for _ in 0..32 {
                        let fd = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();
                        assert!(endpoint(fd).unwrap().is_some());
                        crate::close(fd).unwrap();
                    }
                    done.send(()).unwrap();
                })
            })
            .collect();
        for _ in 0..workers.len() {
            completion
                .recv_timeout(std::time::Duration::from_secs(15))
                .expect("socket publication/close lock order deadlocked");
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }
}
