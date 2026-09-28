//! One open description per native section; queues are never copied on fork.
use super::*;
use crate::mount::shared::{self, Store};
use crate::state_codec::{Reader, bytes, word};

const MAGIC: u64 = u64::from_le_bytes(*b"CYNLFD02");
const DIRECTORY: u64 = u64::MAX - 53;

pub(super) struct Shared {
    pub store: Store,
    pub namespace: u64,
    pub protocol: i32,
    pub socket_type: i32,
}
impl Shared {
    pub fn new(namespace: u64, protocol: i32, socket_type: i32) -> Result<Self, i32> {
        let value = Self {
            store: shared::new_object()?,
            namespace,
            protocol,
            socket_type,
        };
        if kinakaze_runtime::authority::get().is_some() {
            kinakaze_runtime::authority::kernel(
                kinakaze_v2_protocol::kernel::KernelCommand::Lease {
                    object: crate::namespaces::retain_kernel(crate::namespaces::NET, namespace)?,
                    owner: value.store.kernel_key(),
                },
            )?;
        }
        value.store.replace(&value.encode(&EndpointState {
            multicast_cursor: 0,
            multicast_overflow: false,
            bound: None,
            peer: None,
            queue: VecDeque::new(),
            receive_timeout_us: None,
            send_timeout_us: None,
            pass_credentials: false,
            reuse_address: false,
            packet_info: false,
            receive_buffer: 212_992,
            send_buffer: 212_992,
        }))?;
        // Publish the weak catalog entry before any descriptor can escape.
        // A crash during bind must not leave a live, bound socket invisible
        // to the next worker's port-conflict check.
        directory()?.update(|data| {
            let mut r = Reader(data);
            let next = if data.is_empty() {
                0x8000_0000
            } else {
                r.word()?
            };
            let mut out = Vec::new();
            word(&mut out, next);
            while !r.0.is_empty() {
                let id = r.word()?;
                match Store::user_object(id, false) {
                    Ok(_) => word(&mut out, id),
                    Err(crate::ENOENT) => {}
                    Err(error) => return Err(error),
                }
            }
            word(&mut out, value.store.id());
            Ok((out, ()))
        })?;
        Ok(value)
    }
    pub fn open(store: Store) -> Result<Self, i32> {
        let (namespace, protocol, socket_type) = store.read_with(|data| {
            let mut r = Reader(data);
            if r.word()? != MAGIC {
                return Err(EIO);
            }
            Ok((r.word()?, r.word()? as i32, r.word()? as i32))
        })?;
        if namespace == 0
            || !matches!(
                protocol,
                NETLINK_ROUTE | NETLINK_NETFILTER | NETLINK_XFRM | NETLINK_KOBJECT_UEVENT
            )
            || !matches!(socket_type, SOCK_RAW | SOCK_DGRAM)
        {
            return Err(EIO);
        }
        Ok(Self {
            store,
            namespace,
            protocol,
            socket_type,
        })
    }
    fn encode(&self, s: &EndpointState) -> Vec<u8> {
        let mut out = Vec::new();
        for v in [
            MAGIC,
            self.namespace,
            self.protocol as u64,
            self.socket_type as u64,
            s.multicast_cursor,
            u64::from(s.multicast_overflow),
            s.receive_timeout_us.unwrap_or(0),
            s.send_timeout_us.unwrap_or(0),
            u64::from(s.pass_credentials),
            u64::from(s.reuse_address),
            u64::from(s.packet_info),
            s.receive_buffer as u64,
            s.send_buffer as u64,
        ] {
            word(&mut out, v);
        }
        for address in [s.bound, s.peer] {
            word(&mut out, u64::from(address.is_some()));
            let address = address.unwrap_or(NetlinkAddress::KERNEL);
            word(&mut out, address.port as u64);
            word(&mut out, address.groups as u64);
        }
        word(&mut out, s.queue.len() as u64);
        for d in &s.queue {
            word(&mut out, d.source.port as u64);
            word(&mut out, d.source.groups as u64);
            bytes(&mut out, &d.bytes);
        }
        out
    }
    fn decode(&self, data: &[u8]) -> Result<EndpointState, i32> {
        let mut r = Reader(data);
        if [r.word()?, r.word()?, r.word()?, r.word()?]
            != [
                MAGIC,
                self.namespace,
                self.protocol as u64,
                self.socket_type as u64,
            ]
        {
            return Err(EIO);
        }
        let mut s = EndpointState {
            multicast_cursor: r.word()?,
            multicast_overflow: r.word()? != 0,
            receive_timeout_us: match r.word()? {
                0 => None,
                n => Some(n),
            },
            send_timeout_us: match r.word()? {
                0 => None,
                n => Some(n),
            },
            pass_credentials: r.word()? != 0,
            reuse_address: r.word()? != 0,
            packet_info: r.word()? != 0,
            receive_buffer: r.word()? as i32,
            send_buffer: r.word()? as i32,
            bound: None,
            peer: None,
            queue: VecDeque::new(),
        };
        for address in [&mut s.bound, &mut s.peer] {
            let present = r.word()? != 0;
            let value = NetlinkAddress {
                port: r.word()? as u32,
                groups: r.word()? as u32,
            };
            *address = present.then_some(value);
        }
        let count = usize::try_from(r.word()?).map_err(|_| EIO)?;
        if count > r.0.len() / 24
            || !(256..=32 * 1024 * 1024).contains(&s.receive_buffer)
            || !(4608..=32 * 1024 * 1024).contains(&s.send_buffer)
        {
            return Err(EIO);
        }
        for _ in 0..count {
            let source = NetlinkAddress {
                port: r.word()? as u32,
                groups: r.word()? as u32,
            };
            s.queue.push_back(Datagram {
                source,
                bytes: r.bytes()?.to_vec(),
            });
        }
        r.end()?;
        Ok(s)
    }
    pub fn read(&self) -> Result<EndpointState, i32> {
        self.store.read_with(|bytes| self.decode(bytes))
    }
    pub fn update<T>(
        &self,
        action: impl FnOnce(&mut EndpointState) -> Result<T, i32>,
    ) -> Result<T, i32> {
        self.store.update(|bytes| {
            let mut state = self.decode(bytes)?;
            // ENOBUFS, for example, must still commit the overrun indicator.
            let result = action(&mut state);
            Ok((self.encode(&state), result))
        })?
    }
}

fn directory() -> Result<Arc<Store>, i32> {
    static DIRECTORY_STORE: OnceLock<Arc<Store>> = OnceLock::new();
    if let Some(store) = DIRECTORY_STORE.get() {
        return Ok(store.clone());
    }
    let store = Arc::new(Store::user_object(DIRECTORY, true)?);
    store.retain_kernel(false, Vec::new())?;
    let _ = DIRECTORY_STORE.set(store);
    Ok(DIRECTORY_STORE.get().unwrap().clone())
}

pub(super) fn bind(
    item: &Endpoint,
    mut requested: NetlinkAddress,
    automatic: bool,
) -> Result<NetlinkAddress, i32> {
    directory()?.update(|data| {
        let mut r = Reader(data);
        let mut next = if data.is_empty() {
            0x8000_0000
        } else {
            r.word()? as u32
        };
        let mut live = Vec::new();
        let mut used = std::collections::BTreeSet::new();
        while !r.0.is_empty() {
            let id = r.word()?;
            let store = match Store::user_object(id, false) {
                Ok(store) => store,
                Err(crate::ENOENT) => continue,
                Err(error) => return Err(error),
            };
            let other = Shared::open(store)?;
            if other.namespace == item.namespace && other.protocol == item.protocol {
                if let Some(bound) = other.read()?.bound {
                    used.insert(bound.port);
                }
            }
            live.push(id);
        }
        let bound = item.state.update(|state| {
            if let Some(bound) = state.bound.filter(|b| b.port != 0) {
                return if automatic { Ok(bound) } else { Err(EINVAL) };
            }
            if requested.port == 0 {
                let pid = crate::job::process_id();
                if pid != 0 && !used.contains(&pid) {
                    requested.port = pid;
                } else {
                    for _ in 0..=used.len() {
                        requested.port = next.max(1);
                        next = requested.port.wrapping_add(1).max(1);
                        if !used.contains(&requested.port) {
                            break;
                        }
                    }
                }
            }
            if used.contains(&requested.port) {
                return Err(EADDRINUSE);
            }
            requested.groups |= state.bound.map_or(0, |b| b.groups);
            if requested.groups != 0 && item.protocol == NETLINK_ROUTE {
                state.multicast_cursor = crate::route_state::snapshot_for(item.namespace)?
                    .notifications
                    .sequence;
            }
            state.bound = Some(requested);
            Ok(requested)
        })?;
        let mut out = Vec::new();
        word(&mut out, next as u64);
        for id in live {
            word(&mut out, id);
        }
        Ok((out, bound))
    })
}
