use super::super::stack::{IpAddress, IpEndpoint, packet_endpoints, transport::PacketRouter};
use super::*;
use std::net::{IpAddr, SocketAddr};

pub(super) struct Router {
    pub namespace: u64,
    owner: Option<u64>,
    peer: Mutex<Option<Arc<Peer>>>,
}
struct Peer {
    _store: Option<Arc<Store>>,
    state: State,
    root: Arc<SharedEndpoint>,
}
impl Router {
    pub(super) fn new(namespace: u64, owner: Option<u64>) -> Self {
        Self {
            namespace,
            owner,
            peer: Mutex::new(None),
        }
    }
}

pub(super) fn address(endpoint: IpEndpoint) -> Address {
    let mut result = Address {
        port: endpoint.port,
        ..Address::default()
    };
    match endpoint.addr {
        IpAddress::Ipv4(ip) => {
            result.family = 2;
            result.ip[..4].copy_from_slice(&ip.octets());
        }
        IpAddress::Ipv6(ip) => {
            result.family = 10;
            result.ip.copy_from_slice(&ip.octets());
        }
    }
    result
}
pub(super) fn endpoint(address: Address) -> Result<IpEndpoint, i32> {
    let ip = match address.family {
        2 => IpAddress::v4(address.ip[0], address.ip[1], address.ip[2], address.ip[3]),
        10 => IpAddress::Ipv6(std::net::Ipv6Addr::from(address.ip)),
        _ => return Err(crate::EAFNOSUPPORT),
    };
    Ok(IpEndpoint::new(ip, address.port))
}
fn native(address: Address) -> Result<SocketAddr, i32> {
    let ip = match address.family {
        2 => IpAddr::V4(std::net::Ipv4Addr::from(
            <[u8; 4]>::try_from(&address.ip[..4]).unwrap(),
        )),
        10 => IpAddr::V6(std::net::Ipv6Addr::from(address.ip)),
        _ => return Err(crate::EAFNOSUPPORT),
    };
    Ok(SocketAddr::new(ip, address.port))
}
fn matches_local(state: &State, target: Address, kind: i32) -> bool {
    state.packet.is_some()
        && state.bound
        && state.kind == kind
        && state.local.family == target.family
        && state.local.port == target.port
        && (state.local.any() || state.local.ip == target.ip)
}
impl PacketRouter for Router {
    fn poll_peers(&self) -> Result<Option<i64>, i32> {
        let Some(owner) = self.owner else {
            return Ok(None);
        };
        let state = match Store::user_object(owner, false) {
            Ok(store) => State::decode(&store.read()?.1)?,
            Err(crate::ENOENT) => return Ok(None),
            Err(error) => return Err(error),
        };
        if state.kind != 1 || state.peer.port == 0 {
            return Ok(None);
        }
        // An established host flow has a fixed counterpart. It cannot become a
        // different guest connection because a matching address appears later.
        if gateway::relay_for(&state, state.peer)?.is_some() {
            return Ok(None);
        }
        let existing = self.peer.lock().map_err(|_| EIO)?.clone();
        let peer = if let Some(peer) = existing {
            peer
        } else {
            let mut peers = records()?;
            let candidate = |peer: &State| -> Result<bool, i32> {
                Ok(matches_local(peer, state.peer, 1)
                    && peer.packet.is_some_and(|packet| packet.managed)
                    && (!state.peer.loopback() || peer.ns == state.ns)
                    && reachable(state.ns, peer.ns)?
                    && (peer.listening
                        || (peer.peer.port == state.local.port
                            && (state.local.any() || peer.peer.ip == state.local.ip))))
            };
            if !peers.iter().try_fold(false, |found, (_, peer)| {
                Ok::<_, i32>(found || candidate(peer)?)
            })? {
                let live: std::collections::HashSet<_> = peers.iter().map(|(id, _)| *id).collect();
                for (id, bytes) in SharedEndpoint::retained_descriptions()? {
                    if !live.contains(&id) {
                        peers.push((id, State::decode(&bytes)?));
                    }
                }
            }
            peers.sort_by_key(|(_, peer)| peer.listening);
            let mut chosen = None;
            for (id, peer) in peers {
                if !candidate(&peer)? {
                    continue;
                }
                let arena = peer.packet.ok_or(EIO)?.arena;
                let root = SharedEndpoint::open(kinakaze_runtime::authority::domain_id(), arena)?;
                let store = match Store::user_object(id, false) {
                    Ok(store) => Some(Arc::new(store)),
                    Err(crate::ENOENT) => None,
                    Err(error) => return Err(error),
                };
                chosen = Some(Arc::new(Peer {
                    _store: store,
                    state: peer,
                    root,
                }));
                break;
            }
            *self.peer.lock().map_err(|_| EIO)? = chosen.clone();
            let Some(peer) = chosen else { return Ok(None) };
            peer
        };
        let mut output = lifetime::reconcile(&peer.root, peer.state.packet.ok_or(EIO)?.arena)?;
        let (_, frames, delay) = if let Some(listener) =
            crate::usernet::stack::shared::Listener::reopen(peer.root.clone())?
        {
            listener.poll(None)?
        } else {
            peer.root.poll(None)?
        };
        output.extend(frames);
        let router = Router::new(peer.state.ns, None);
        for packet in output {
            router.deliver_with_source(&packet, Some(peer.state.clone()))?;
        }
        Ok(delay.map(|delay| {
            crate::usernet::stack::shared::now_ms()
                .saturating_add(delay.min(i64::MAX as u64) as i64)
        }))
    }
    fn lifetime_arenas(&self) -> Result<Vec<u64>, i32> {
        let mut arenas = Vec::new();
        if let Some(owner) = self.owner {
            let store = Store::user_object(owner, false)?;
            if let Some(packet) = State::decode(&store.read()?.1)?
                .packet
                .filter(|packet| packet.managed)
            {
                arenas.push(packet.arena);
            }
        }
        if let Some(peer) = &*self.peer.lock().map_err(|_| EIO)? {
            arenas.push(peer.state.packet.ok_or(EIO)?.arena);
        }
        arenas.sort_unstable();
        arenas.dedup();
        Ok(arenas)
    }
    fn deliver(&self, frame: &[u8]) -> Result<bool, i32> {
        let source = self
            .owner
            .map(|id| {
                Store::user_object(id, false).and_then(|store| State::decode(&store.read()?.1))
            })
            .transpose()?;
        self.deliver_with_source(frame, source)
    }
    fn route(&self, frame: &[u8]) -> Result<Option<SocketAddr>, i32> {
        let state = self
            .owner
            .map(|owner| {
                Store::user_object(owner, false).and_then(|store| State::decode(&store.read()?.1))
            })
            .transpose()?;
        if let Some(state) = &state {
            if let Some(host) = tcp_gateway(&state, frame)? {
                return native(host).map(Some);
            }
        }
        if let Some(state) = find(self.namespace, frame).inspect_err(|e| {
            gateway::trace(self.owner.unwrap_or(0), format_args!("find route {e}"))
        })? {
            return native(state.host.local_transport()).map(Some);
        }
        // TCP connect establishes a host relay before emitting its SYN. A
        // reply/retransmission without either that relay or a guest peer is a
        // stale flow, not authority to open a new host connection. Discard this
        // packet so it cannot pin the listener's output queue on a route error.
        if state.as_ref().is_some_and(|state| state.kind == 1) {
            return Ok(None);
        }
        let Some(owner) = self.owner else {
            return Ok(None);
        };
        let Some((_, destination, _)) = packet_endpoints(frame).map_err(|_| EIO)? else {
            return Ok(None);
        };
        gateway::route(owner, address(destination))
            .and_then(native)
            .map(Some)
    }
    fn accepts(&self, frame: &[u8], transport: SocketAddr) -> Result<bool, i32> {
        if !transport.ip().is_loopback() {
            return Ok(false);
        }
        let Some((source, destination, protocol)) = packet_endpoints(frame).ok().flatten() else {
            return Ok(false);
        };
        let source = address(source);
        let destination = address(destination);
        let kind = if protocol == 6 { 1 } else { 2 };
        if let Some(owner) = self.owner {
            let state = State::decode(&Store::user_object(owner, false)?.read()?.1)?;
            if state.kind == kind
                && matches_local(&state, destination, kind)
                && gateway::relay(&state, source).map(native).transpose()? == Some(transport)
            {
                return Ok(true);
            }
            // Newly accepted host flows register on the listener's portable
            // descriptor before sending their SYN. Accepted views share its link.
            if let Some(reference) = state.packet {
                if reference.arena != owner {
                    if let Ok(store) = Store::user_object(reference.arena, false) {
                        let listener = State::decode(&store.read()?.1)?;
                        if matches_local(&listener, destination, kind)
                            && gateway::relay(&listener, source).map(native).transpose()?
                                == Some(transport)
                        {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        if !address_owned(self.namespace, destination)? {
            return Ok(false);
        }
        for (_, state) in records()? {
            if !matches_local(&state, source, kind)
                || native(state.host.local_transport())? != transport
            {
                continue;
            }
            if (source.loopback() || destination.loopback()) && state.ns != self.namespace {
                continue;
            }
            if reachable(self.namespace, state.ns)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
impl Router {
    fn deliver_with_source(&self, frame: &[u8], source: Option<State>) -> Result<bool, i32> {
        if source
            .as_ref()
            .map(|state| tcp_gateway(state, frame))
            .transpose()?
            .flatten()
            .is_some()
        {
            return Ok(false);
        }
        let Some(first) = find(self.namespace, frame)? else {
            return Ok(false);
        };
        if !first.packet.is_some_and(|packet| packet.managed) {
            return Ok(false);
        }
        // Packets still traverse checksum validation, BPF and the receiving
        // TCP/IP implementation. No Winsock TCP ACK can bypass a guest filter.
        // A bounded work list avoids recursion while allowing connect-before-
        // accept and ACK/window progress without a permanent worker process.
        let mut pending = std::collections::VecDeque::from([(first, frame.to_vec(), source)]);
        for _ in 0..256 {
            let Some((target, packet, origin)) = pending.pop_front() else {
                break;
            };
            let reference = target.packet.ok_or(EIO)?;
            if !reference.managed {
                return Err(crate::EOPNOTSUPP);
            }
            let root = match SharedEndpoint::open(
                kinakaze_runtime::authority::domain_id(),
                reference.arena,
            ) {
                Ok(root) => root,
                Err(crate::ENOENT) => continue, // the receiving description closed
                Err(error) => return Err(error),
            };
            let (_, output, _) = if let Some(listener) =
                crate::usernet::stack::shared::Listener::reopen(root.clone())?
            {
                listener.poll(Some(packet))?
            } else {
                root.poll(Some(packet))?
            };
            for packet in output {
                let next = find(target.ns, &packet)?.or_else(|| {
                    let source = origin.as_ref()?;
                    let (peer, destination, protocol) = packet_endpoints(&packet).ok().flatten()?;
                    (matches_local(
                        source,
                        address(destination),
                        if protocol == 6 { 1 } else { 2 },
                    ) && (source.listening || source.peer == address(peer)))
                    .then(|| source.clone())
                });
                if let Some(next) = next {
                    pending.push_back((next, packet, Some(target.clone())));
                }
            }
        }
        // Work exceeding this network burst is a packet drop. TCP retransmits
        // from its shared queue; UDP has no protocol-generated response burst.
        Ok(true)
    }
}

fn tcp_gateway(state: &State, frame: &[u8]) -> Result<Option<Address>, i32> {
    if state.kind != 1 {
        return Ok(None);
    }
    let Some((source, target, protocol)) = packet_endpoints(frame).map_err(|_| EIO)? else {
        return Ok(None);
    };
    if protocol != 6 || source.port != state.local.port {
        return Ok(None);
    }
    gateway::relay_for(state, address(target))
}

fn find(namespace: u64, frame: &[u8]) -> Result<Option<State>, i32> {
    let Some((source, destination, protocol)) = packet_endpoints(frame).map_err(|e| {
        gateway::trace(0, format_args!("find packet {e:?}"));
        EIO
    })?
    else {
        return Ok(None);
    };
    let source = address(source);
    let destination = address(destination);
    let kind = if protocol == 6 { 1 } else { 2 };
    let rows = records().inspect_err(|e| gateway::trace(0, format_args!("find records {e}")))?;
    let matches = |id: u64, state: &State| -> Result<bool, i32> {
        if !matches_local(state, destination, kind) || state.host.port == 0 {
            return Ok(false);
        }
        if destination.loopback() && state.ns != namespace {
            return Ok(false);
        }
        if !destination.loopback() && !address_owned(state.ns, destination)? {
            return Ok(false);
        }
        if !reachable(namespace, state.ns)? {
            return Ok(false);
        }
        if kind == 1 && !state.listening && state.peer != source {
            return Ok(false);
        }
        if state.packet.is_some_and(|packet| packet.managed)
            && (kind != 1 || state.listening)
            && !lifetime::present(state.packet.ok_or(EIO)?.arena)?.contains(&id)
        {
            return Ok(false);
        }
        Ok(true)
    };
    for (id, state) in &rows {
        if matches(*id, state)
            .inspect_err(|e| gateway::trace(*id, format_args!("find matches {e}")))?
        {
            return Ok(Some(state.clone()));
        }
    }
    // The peer's fd table can disappear before accept. Its protocol state and
    // compact routing identity remain in the domain's shared storage section.
    let live: std::collections::HashSet<_> = rows.iter().map(|(id, _)| *id).collect();
    for (id, bytes) in SharedEndpoint::retained_descriptions()
        .inspect_err(|e| gateway::trace(0, format_args!("find retained {e}")))?
    {
        if live.contains(&id) {
            continue;
        }
        let state = State::decode(&bytes)
            .inspect_err(|e| gateway::trace(id, format_args!("find retained decode {e}")))?;
        if matches(id, &state)? {
            return Ok(Some(state));
        }
    }
    if std::env::var_os("KINAKAZE_GATEWAY_TRACE_DIR").is_some() {
        static TRACED: std::sync::OnceLock<Mutex<std::collections::HashSet<u64>>> =
            std::sync::OnceLock::new();
        if TRACED
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| EIO)?
            .insert(namespace)
        {
            let topology: Vec<_> = namespace_ids()?
                .into_iter()
                .chain([namespace])
                .map(|id| {
                    (
                        id,
                        crate::route_state::snapshot_for(id).map(|net| {
                            net.links
                                .into_iter()
                                .map(|link| {
                                    (link.index, link.name, link.flags, link.master, link.peer)
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                })
                .collect();
            gateway::trace(0, format_args!("routing topology {topology:?}"));
        }
        let candidates: Vec<_> = rows
            .iter()
            .filter(|(_, state)| state.local.port == destination.port)
            .map(|(id, state)| {
                (
                    *id,
                    state.ns,
                    state.local,
                    state.peer,
                    state.listening,
                    state.bound,
                    state.host,
                    state.packet,
                    address_owned(state.ns, destination),
                    reachable(namespace, state.ns),
                )
            })
            .collect();
        gateway::trace(
            0,
            format_args!(
                "unrouted ns={namespace} {source:?} -> {destination:?} candidates={candidates:?}"
            ),
        );
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
