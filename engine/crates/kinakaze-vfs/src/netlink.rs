//! Linux netlink sockets backed by explicit host-state translation.
//!
//! Netlink is not an Internet socket and therefore must never be passed to
//! Winsock.  This module owns a separate datagram endpoint, parses Linux
//! `nlmsghdr`/route attributes, and produces kernel-shaped replies.  Only
//! operations whose result can be established from the host are accepted;
//! unknown protocols, message types and mutations fail at their request
//! boundary instead of being acknowledged and ignored.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

use crate::{
    EADDRINUSE, EAGAIN, EDESTADDRREQ, EDOM, EEXIST, EFAULT, EINTR, EINVAL, EIO, EMSGSIZE, ENOBUFS,
    ENODEV, ENOENT, ENOPROTOOPT, EOPNOTSUPP, EPROTONOSUPPORT, FdFlags, FdKind, interrupt, signal,
};

pub(crate) mod multicast;
pub(crate) mod routes;

fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_SOCKET_TRACE").is_some())
}

fn route_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_ROUTE_TRACE").is_some())
}

pub const AF_NETLINK: i32 = 16;
pub const NETLINK_ROUTE: i32 = 0;
pub const NETLINK_XFRM: i32 = 6;
pub const NETLINK_NETFILTER: i32 = 12;
pub const NETLINK_KOBJECT_UEVENT: i32 = 15;

const SOCK_RAW: i32 = 3;
const SOCK_DGRAM: i32 = 2;
const SOCK_NONBLOCK: i32 = 0o4000;
const SOCK_CLOEXEC: i32 = 0o2000000;

const MSG_PEEK: i32 = 0x02;
const MSG_DONTWAIT: i32 = 0x40;

const SOL_SOCKET: i32 = 1;
const SO_TYPE: i32 = 3;
const SO_ERROR: i32 = 4;
const SO_RCVBUF: i32 = 8;
const SO_PASSCRED: i32 = 16;
const SO_RCVTIMEO: i32 = 20;
const SO_SNDTIMEO: i32 = 21;
const SO_PROTOCOL: i32 = 38;

const NLMSG_NOOP: u16 = 1;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x0001;
const NLM_F_MULTI: u16 = 0x0002;
const NLM_F_ACK: u16 = 0x0004;
const NLM_F_ROOT: u16 = 0x0100;
const NLM_F_MATCH: u16 = 0x0200;
const NLM_F_EXCL: u16 = 0x0200;
const NLM_F_CREATE: u16 = 0x0400;
const NLM_F_DUMP: u16 = NLM_F_ROOT | NLM_F_MATCH;

const NFNL_SUBSYS_NFTABLES: u16 = 10;
const NFNL_SUBSYS_NFT_COMPAT: u16 = 11;
const NFNETLINK_V0: u8 = 0;
const NFNL_MSG_BATCH_BEGIN: u16 = 16;
const NFNL_MSG_BATCH_END: u16 = 17;
const NFNL_BATCH_GENID: u16 = 1;
const NFT_MSG_NEWTABLE: u16 = 0;
const NFT_MSG_GETTABLE: u16 = 1;
const NFT_MSG_DELTABLE: u16 = 2;
const NFT_MSG_NEWCHAIN: u16 = 3;
const NFT_MSG_GETCHAIN: u16 = 4;
const NFT_MSG_DELCHAIN: u16 = 5;
const NFT_MSG_NEWRULE: u16 = 6;
const NFT_MSG_GETRULE: u16 = 7;
const NFT_MSG_DELRULE: u16 = 8;
const NFT_MSG_NEWGEN: u16 = 15;
const NFT_MSG_GETGEN: u16 = 16;
const NFT_MSG_GETSET: u16 = 10;
const NFT_MSG_GETOBJ: u16 = 19;
const NFT_MSG_GETFLOWTABLE: u16 = 23;
const NFTA_GEN_ID: u16 = 1;
const NFTA_TABLE_NAME: u16 = 1;
const NFTA_TABLE_USE: u16 = 3;
const NFTA_TABLE_HANDLE: u16 = 4;
const NFTA_CHAIN_TABLE: u16 = 1;
const NFTA_CHAIN_HANDLE: u16 = 2;
const NFTA_CHAIN_NAME: u16 = 3;
const NFTA_CHAIN_USE: u16 = 6;
const NFTA_RULE_TABLE: u16 = 1;
const NFTA_RULE_CHAIN: u16 = 2;
const NFTA_RULE_HANDLE: u16 = 3;
const NFNL_MSG_COMPAT_GET: u16 = 0;
const NFTA_COMPAT_NAME: u16 = 1;
const NFTA_COMPAT_REV: u16 = 2;
const NFTA_COMPAT_TYPE: u16 = 3;

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_GETLINK: u16 = 18;
const RTM_SETLINK: u16 = 19;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_GETROUTE: u16 = 26;

const IFLA_ADDRESS: u16 = 1;
const IFLA_BROADCAST: u16 = 2;
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_LINK: u16 = 5;
const IFLA_MASTER: u16 = 10;
const IFLA_OPERSTATE: u16 = 16;
const IFLA_LINKINFO: u16 = 18;
const IFLA_NET_NS_FD: u16 = 28;
const IFLA_EXT_MASK: u16 = 29;

const IFLA_INFO_KIND: u16 = 1;
const IFLA_INFO_DATA: u16 = 2;
const VETH_INFO_PEER: u16 = 1;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;
const IFA_BROADCAST: u16 = 4;
const IFA_FLAGS: u16 = 8;

const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_TABLE: u16 = 15;

const NLA_F_NESTED: u16 = 1 << 15;

const IFF_UP: u32 = 0x0001;
const IFF_BROADCAST: u32 = 0x0002;
const IFF_LOOPBACK: u32 = 0x0008;
const IFF_RUNNING: u32 = 0x0040;
const IFF_MULTICAST: u32 = 0x1000;

const ARPHRD_ETHER: u16 = 1;
const ARPHRD_LOOPBACK: u16 = 772;
const IF_OPER_DOWN: u8 = 2;
const IF_OPER_UP: u8 = 6;

const AF_UNSPEC: u8 = 0;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;

const SOCKADDR_NL_LEN: usize = 12;
const NLMSG_HEADER_LEN: usize = 16;
const IFINFO_MSG_LEN: usize = 16;
const IFADDR_MSG_LEN: usize = 8;
const RTMSG_LEN: usize = 12;
const RTATTR_HEADER_LEN: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NetlinkAddress {
    port: u32,
    groups: u32,
}

impl NetlinkAddress {
    const KERNEL: Self = Self { port: 0, groups: 0 };

    unsafe fn read(address: *const u8, length: i32) -> Result<Self, i32> {
        if address.is_null() {
            return Err(EFAULT);
        }
        if length < SOCKADDR_NL_LEN as i32 {
            return Err(EINVAL);
        }
        // SAFETY: the caller promised at least one complete sockaddr_nl.
        let bytes = unsafe { core::slice::from_raw_parts(address, SOCKADDR_NL_LEN) };
        if u16::from_ne_bytes([bytes[0], bytes[1]]) != AF_NETLINK as u16 {
            return Err(EINVAL);
        }
        Ok(Self {
            port: u32::from_ne_bytes(bytes[4..8].try_into().map_err(|_| EINVAL)?),
            groups: u32::from_ne_bytes(bytes[8..12].try_into().map_err(|_| EINVAL)?),
        })
    }

    unsafe fn write(self, address: *mut u8, length: *mut i32) -> Result<(), i32> {
        if address.is_null() || length.is_null() {
            return Err(EFAULT);
        }
        // SAFETY: the caller promised a readable length pointer.
        let capacity = unsafe { *length };
        if capacity < 0 {
            return Err(EINVAL);
        }
        let mut bytes = [0u8; SOCKADDR_NL_LEN];
        bytes[0..2].copy_from_slice(&(AF_NETLINK as u16).to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.port.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.groups.to_ne_bytes());
        let copied = (capacity as usize).min(bytes.len());
        // SAFETY: `copied` is bounded by the caller's stated capacity.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), address, copied);
            *length = SOCKADDR_NL_LEN as i32;
        }
        Ok(())
    }
}

struct Datagram {
    bytes: Vec<u8>,
    source: NetlinkAddress,
}

struct EndpointState {
    multicast_cursor: u64,
    multicast_overflow: bool,
    bound: Option<NetlinkAddress>,
    peer: Option<NetlinkAddress>,
    queue: VecDeque<Datagram>,
    receive_timeout_us: Option<u64>,
    send_timeout_us: Option<u64>,
    pass_credentials: bool,
    receive_buffer: i32,
}

struct Endpoint {
    description: std::sync::atomic::AtomicU64,
    namespace: u64,
    _namespace_pin: Arc<crate::mount::shared::Store>,
    protocol: i32,
    socket_type: i32,
    event: HANDLE,
    state: Mutex<EndpointState>,
}

// HANDLE is an opaque kernel-object pointer. Access is synchronized by `state`,
// and the object remains live for the entire Arc lifetime.
unsafe impl Send for Endpoint {}
unsafe impl Sync for Endpoint {}

impl Endpoint {
    fn new(protocol: i32, socket_type: i32) -> Result<Self, i32> {
        // Manual-reset and initially nonsignalled. It represents queue nonempty.
        // SAFETY: null security attributes and name request a private event.
        let event = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) };
        if event.is_null() {
            return Err(EIO);
        }
        let namespace = crate::usernet::current()?;
        Ok(Self {
            description: std::sync::atomic::AtomicU64::new(0),
            namespace,
            _namespace_pin: crate::namespaces::pin_network(namespace)?,
            protocol,
            socket_type,
            event,
            state: Mutex::new(EndpointState {
                multicast_cursor: 0,
                multicast_overflow: false,
                bound: None,
                peer: None,
                queue: VecDeque::new(),
                receive_timeout_us: None,
                send_timeout_us: None,
                pass_credentials: false,
                receive_buffer: 212_992,
            }),
        })
    }

    fn enqueue(&self, mut datagram: Datagram) -> Result<(), i32> {
        let mut state = self.state.lock().map_err(|_| EIO)?;
        if let Some(filter) = crate::socket::filter::for_description(
            self.description.load(std::sync::atomic::Ordering::Acquire),
        )? {
            let Some(length) = filter.run(&crate::socket::filter::LocalPacket(&datagram.bytes), 0)
            else {
                return Ok(());
            };
            datagram.bytes.truncate(length);
        }
        state.queue.push_back(datagram);
        // SAFETY: `event` is live for this Endpoint's lifetime.
        if unsafe { SetEvent(self.event) } == 0 {
            state.queue.pop_back();
            return Err(EIO);
        }
        Ok(())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: the Endpoint owns this event exactly once.
        unsafe { CloseHandle(self.event) };
    }
}

fn endpoints() -> &'static Mutex<HashMap<i32, Arc<Endpoint>>> {
    static ENDPOINTS: OnceLock<Mutex<HashMap<i32, Arc<Endpoint>>>> = OnceLock::new();
    ENDPOINTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn endpoint(fd: i32) -> Result<Arc<Endpoint>, i32> {
    if crate::get(fd)?.kind != FdKind::NetlinkSocket {
        return Err(crate::ENOTSOCK);
    }
    endpoints()
        .lock()
        .map_err(|_| EIO)?
        .get(&fd)
        .cloned()
        .ok_or(crate::EBADF)
}

pub fn is_netlink_socket(fd: i32) -> bool {
    crate::get(fd).is_ok_and(|entry| entry.kind == FdKind::NetlinkSocket)
}

pub fn socket(socket_type: i32, protocol: i32) -> Result<i32, i32> {
    if !matches!(
        protocol,
        NETLINK_ROUTE | NETLINK_NETFILTER | NETLINK_XFRM | NETLINK_KOBJECT_UEVENT
    ) {
        return Err(EPROTONOSUPPORT);
    }
    let base_type = socket_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    if !matches!(base_type, SOCK_RAW | SOCK_DGRAM) {
        return Err(EOPNOTSUPP);
    }
    let mut flags = FdFlags::NONE;
    if socket_type & SOCK_NONBLOCK != 0 {
        flags = flags.union(FdFlags::NONBLOCK);
    }
    if socket_type & SOCK_CLOEXEC != 0 {
        flags = flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    let item = Arc::new(Endpoint::new(protocol, base_type)?);
    let fd = crate::install_handleless_with(FdKind::NetlinkSocket, flags, |fd| {
        endpoints()
            .lock()
            .map_err(|_| EIO)?
            .insert(fd, item.clone());
        Ok(())
    })?;
    item.description.store(
        crate::get(fd)?.description_id,
        std::sync::atomic::Ordering::Release,
    );
    Ok(fd)
}

pub(crate) fn filter_boundary(fd: i32) -> Result<(), i32> {
    let item = endpoint(fd)?;
    let mut state = item.state.lock().map_err(|_| EIO)?;
    multicast::refresh(&item, &mut state)
}

fn allocate_port(state: &HashMap<i32, Arc<Endpoint>>, item: &Endpoint) -> Result<u32, i32> {
    let process_port = crate::job::process_id();
    let used = |candidate| -> Result<bool, i32> {
        for endpoint in state.values() {
            if endpoint.namespace != item.namespace || endpoint.protocol != item.protocol {
                continue;
            }
            if endpoint
                .state
                .lock()
                .map_err(|_| EIO)?
                .bound
                .is_some_and(|address| address.port == candidate)
            {
                return Ok(true);
            }
        }
        Ok(false)
    };
    if process_port != 0 && !used(process_port)? {
        return Ok(process_port);
    }
    static NEXT_PORT: AtomicU32 = AtomicU32::new(0x8000_0000);
    for _ in 0..u16::MAX {
        let candidate = NEXT_PORT.fetch_add(1, Ordering::Relaxed).max(1);
        if !used(candidate)? {
            return Ok(candidate);
        }
    }
    Err(ENOBUFS)
}

pub unsafe fn bind(fd: i32, address: *const u8, length: i32) -> Result<(), i32> {
    // SAFETY: forwarded from this function's contract.
    let mut requested = unsafe { NetlinkAddress::read(address, length)? };
    let item = endpoint(fd)?;
    // The guest device tree is static: a uevent monitor has a real waitable
    // endpoint but receives no hotplug events until guest devices change.
    // In particular, do not feed route multicast traffic into a udev monitor.
    let allowed_groups = match item.protocol {
        NETLINK_ROUTE => 1,
        NETLINK_KOBJECT_UEVENT => 3,
        _ => 0,
    };
    if requested.groups & !allowed_groups != 0 {
        return Err(EOPNOTSUPP);
    }
    let table = endpoints().lock().map_err(|_| EIO)?;
    if item.state.lock().map_err(|_| EIO)?.bound.is_some() {
        return Err(EINVAL);
    }
    if requested.port == 0 {
        requested.port = allocate_port(&table, &item)?;
    } else {
        for (other_fd, other) in table.iter() {
            if *other_fd != fd
                && other.namespace == item.namespace
                && other.protocol == item.protocol
                && other
                    .state
                    .lock()
                    .map_err(|_| EIO)?
                    .bound
                    .is_some_and(|bound| bound.port == requested.port)
            {
                return Err(EADDRINUSE);
            }
        }
    }
    let mut state = item.state.lock().map_err(|_| EIO)?;
    if state.bound.is_some() {
        return Err(EINVAL);
    }
    if requested.groups != 0 && item.protocol == NETLINK_ROUTE {
        state.multicast_cursor = crate::route_state::snapshot_for(item.namespace)?
            .notifications
            .sequence;
    }
    state.bound = Some(requested);
    if trace_enabled() {
        eprintln!(
            "kinakaze netlink: bind fd={fd} port={} groups={:#x}",
            requested.port, requested.groups
        );
    }
    Ok(())
}

pub unsafe fn getsockname(fd: i32, address: *mut u8, length: *mut i32) -> Result<(), i32> {
    let item = endpoint(fd)?;
    let bound = item.state.lock().map_err(|_| EIO)?.bound;
    let bound = match bound {
        Some(bound) => bound,
        None => auto_bind(fd, &item)?,
    };
    // SAFETY: forwarded from this function's contract.
    unsafe { bound.write(address, length) }
}

pub unsafe fn getpeername(fd: i32, address: *mut u8, length: *mut i32) -> Result<(), i32> {
    let item = endpoint(fd)?;
    let peer = item
        .state
        .lock()
        .map_err(|_| EIO)?
        .peer
        .ok_or(crate::ENOTCONN)?;
    // SAFETY: forwarded from this function's contract.
    unsafe { peer.write(address, length) }
}

pub unsafe fn connect(fd: i32, address: *const u8, length: i32) -> Result<(), i32> {
    // SAFETY: forwarded from this function's contract.
    let peer = unsafe { NetlinkAddress::read(address, length)? };
    let item = endpoint(fd)?;
    if item.state.lock().map_err(|_| EIO)?.bound.is_none() {
        auto_bind(fd, &item)?;
    }
    item.state.lock().map_err(|_| EIO)?.peer = Some(peer);
    Ok(())
}

fn auto_bind(fd: i32, item: &Arc<Endpoint>) -> Result<NetlinkAddress, i32> {
    if let Some(bound) = item.state.lock().map_err(|_| EIO)?.bound {
        return Ok(bound);
    }
    let table = endpoints().lock().map_err(|_| EIO)?;
    let bound = NetlinkAddress {
        port: allocate_port(&table, item)?,
        groups: 0,
    };
    let mut state = item.state.lock().map_err(|_| EIO)?;
    if let Some(existing) = state.bound {
        return Ok(existing);
    }
    state.bound = Some(bound);
    let _ = fd;
    Ok(bound)
}

#[derive(Clone, Copy)]
struct MessageHeader {
    length: u32,
    kind: u16,
    flags: u16,
    sequence: u32,
    port: u32,
}

fn parse_header(bytes: &[u8]) -> Result<MessageHeader, i32> {
    if bytes.len() < NLMSG_HEADER_LEN {
        return Err(EINVAL);
    }
    let header = MessageHeader {
        length: u32::from_ne_bytes(bytes[0..4].try_into().map_err(|_| EINVAL)?),
        kind: u16::from_ne_bytes(bytes[4..6].try_into().map_err(|_| EINVAL)?),
        flags: u16::from_ne_bytes(bytes[6..8].try_into().map_err(|_| EINVAL)?),
        sequence: u32::from_ne_bytes(bytes[8..12].try_into().map_err(|_| EINVAL)?),
        port: u32::from_ne_bytes(bytes[12..16].try_into().map_err(|_| EINVAL)?),
    };
    if header.length < NLMSG_HEADER_LEN as u32 || header.length as usize > bytes.len() {
        return Err(EINVAL);
    }
    Ok(header)
}

fn align4(length: usize) -> Result<usize, i32> {
    length.checked_add(3).map(|value| value & !3).ok_or(EINVAL)
}

fn append_header(
    out: &mut Vec<u8>,
    kind: u16,
    flags: u16,
    sequence: u32,
    port: u32,
    payload_length: usize,
) -> Result<(), i32> {
    let length = NLMSG_HEADER_LEN
        .checked_add(payload_length)
        .and_then(|length| u32::try_from(length).ok())
        .ok_or(EMSGSIZE)?;
    out.extend_from_slice(&length.to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(&flags.to_ne_bytes());
    out.extend_from_slice(&sequence.to_ne_bytes());
    out.extend_from_slice(&port.to_ne_bytes());
    Ok(())
}

fn append_attribute(out: &mut Vec<u8>, kind: u16, value: &[u8]) -> Result<(), i32> {
    let length = RTATTR_HEADER_LEN.checked_add(value.len()).ok_or(EMSGSIZE)?;
    let aligned = align4(length)?;
    out.extend_from_slice(&u16::try_from(length).map_err(|_| EMSGSIZE)?.to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(value);
    out.resize(out.len().checked_add(aligned - length).ok_or(EMSGSIZE)?, 0);
    Ok(())
}

struct Loopback {
    index: u32,
    mtu: u32,
    flags: u32,
    operational: bool,
    address: Vec<u8>,
}

fn host_loopback() -> Result<Loopback, i32> {
    let namespace = crate::usernet::current()?;
    if namespace != 1 {
        let flags = crate::route_state::snapshot()?.loopback_flags;
        return Ok(Loopback {
            index: 1,
            mtu: 65536,
            flags,
            operational: flags & 1 != 0,
            address: vec![0; 6],
        });
    }
    let host = crate::hostnet::interfaces(namespace)?
        .into_iter()
        .find(|i| i.hardware_type == ARPHRD_LOOPBACK)
        .ok_or(ENODEV)?;
    Ok(Loopback {
        index: host.link.index,
        mtu: host.link.mtu,
        flags: host.link.flags,
        operational: host.operstate == IF_OPER_UP,
        address: host.link.address,
    })
}

fn parse_attributes(bytes: &[u8]) -> Result<Vec<(u16, &[u8])>, i32> {
    let mut attributes = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor..].iter().all(|byte| *byte == 0) {
            break;
        }
        if cursor
            .checked_add(RTATTR_HEADER_LEN)
            .is_none_or(|end| end > bytes.len())
        {
            return Err(EINVAL);
        }
        let length =
            u16::from_ne_bytes(bytes[cursor..cursor + 2].try_into().map_err(|_| EINVAL)?) as usize;
        let kind = u16::from_ne_bytes(
            bytes[cursor + 2..cursor + 4]
                .try_into()
                .map_err(|_| EINVAL)?,
        ) & 0x3fff;
        if length < RTATTR_HEADER_LEN
            || cursor
                .checked_add(length)
                .is_none_or(|end| end > bytes.len())
        {
            return Err(EINVAL);
        }
        attributes.push((kind, &bytes[cursor + RTATTR_HEADER_LEN..cursor + length]));
        cursor = cursor.checked_add(align4(length)?).ok_or(EINVAL)?;
    }
    Ok(attributes)
}

fn route_attributes(bytes: &[u8]) -> Result<Vec<crate::route_state::Attribute>, i32> {
    let mut attributes = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor..].iter().all(|byte| *byte == 0) {
            break;
        }
        let header = bytes
            .get(cursor..cursor + RTATTR_HEADER_LEN)
            .ok_or(EINVAL)?;
        let length = u16::from_ne_bytes(header[0..2].try_into().map_err(|_| EINVAL)?) as usize;
        let kind = u16::from_ne_bytes(header[2..4].try_into().map_err(|_| EINVAL)?);
        if length < RTATTR_HEADER_LEN {
            return Err(EINVAL);
        }
        let end = cursor.checked_add(length).ok_or(EINVAL)?;
        let value = bytes
            .get(cursor + RTATTR_HEADER_LEN..end)
            .ok_or(EINVAL)?
            .to_vec();
        attributes.push(crate::route_state::Attribute { kind, value });
        cursor = cursor.checked_add(align4(length)?).ok_or(EINVAL)?;
    }
    Ok(attributes)
}

fn route_attribute(attributes: &[crate::route_state::Attribute], kind: u16) -> Option<&[u8]> {
    attributes
        .iter()
        .find(|attribute| attribute.kind & 0x3fff == kind)
        .map(|attribute| attribute.value.as_slice())
}

fn route_u32(attributes: &[crate::route_state::Attribute], kind: u16) -> Result<Option<u32>, i32> {
    route_attribute(attributes, kind)
        .map(|value| {
            let bytes: [u8; 4] = value.try_into().map_err(|_| EINVAL)?;
            Ok(u32::from_ne_bytes(bytes))
        })
        .transpose()
}

fn route_string<'a>(
    attributes: &'a [crate::route_state::Attribute],
    kind: u16,
) -> Result<Option<&'a str>, i32> {
    route_attribute(attributes, kind)
        .map(|value| {
            // NLA_STRING is length-delimited.  Linux accepts either the raw
            // bytes or one trailing NUL (whereas embedded NULs are malformed).
            // Current rtnetlink clients use both encodings: IFLA_IFNAME is
            // commonly terminated, while IFLA_INFO_KIND commonly is not.
            let value = value.strip_suffix(&[0]).unwrap_or(value);
            if value.contains(&0) {
                return Err(EINVAL);
            }
            core::str::from_utf8(value).map_err(|_| EINVAL)
        })
        .transpose()
}

fn link_kind(attributes: &[crate::route_state::Attribute]) -> Result<Option<String>, i32> {
    let Some(link_info) = route_attribute(attributes, IFLA_LINKINFO) else {
        return Ok(None);
    };
    let nested = route_attributes(link_info)?;
    Ok(route_string(&nested, IFLA_INFO_KIND)?.map(str::to_owned))
}

fn default_mac(index: u32) -> Vec<u8> {
    let bytes = index.to_be_bytes();
    vec![0x02, 0x43, bytes[0], bytes[1], bytes[2], bytes[3]]
}

fn stored_link_reply(
    header: MessageHeader,
    local_port: u32,
    link: &crate::route_state::Link,
    multipart: bool,
) -> Result<Vec<u8>, i32> {
    let mut payload = Vec::new();
    payload.push(AF_UNSPEC);
    payload.push(0);
    payload.extend_from_slice(&ARPHRD_ETHER.to_ne_bytes());
    payload.extend_from_slice(&(link.index as i32).to_ne_bytes());
    payload.extend_from_slice(&link.flags.to_ne_bytes());
    payload.extend_from_slice(&0u32.to_ne_bytes());
    let mut name = link.name.as_bytes().to_vec();
    name.push(0);
    append_attribute(&mut payload, IFLA_IFNAME, &name)?;
    append_attribute(&mut payload, IFLA_MTU, &link.mtu.to_ne_bytes())?;
    if !link.address.is_empty() {
        append_attribute(&mut payload, IFLA_ADDRESS, &link.address)?;
    }
    if !link.broadcast.is_empty() {
        append_attribute(&mut payload, IFLA_BROADCAST, &link.broadcast)?;
    }
    if link.master != 0 {
        append_attribute(&mut payload, IFLA_MASTER, &link.master.to_ne_bytes())?;
    }
    if link.peer != 0 {
        append_attribute(&mut payload, IFLA_LINK, &link.peer.to_ne_bytes())?;
    }
    // Requests may use the length-delimited NLA_STRING form for INFO_KIND, but
    // rtnetlink replies conventionally terminate it.  Rebuild LINKINFO from the
    // semantic kind instead of echoing request bytes so every netlink client
    // observes the same canonical kernel representation. Preserve the remaining
    // kind-specific nested attributes verbatim.
    let mut link_info = Vec::new();
    let mut kind = link.kind.as_bytes().to_vec();
    kind.push(0);
    append_attribute(&mut link_info, IFLA_INFO_KIND, &kind)?;
    if let Some(stored) = route_attribute(&link.attributes, IFLA_LINKINFO) {
        for attribute in route_attributes(stored)? {
            if attribute.kind & 0x3fff != IFLA_INFO_KIND {
                append_attribute(&mut link_info, attribute.kind, &attribute.value)?;
            }
        }
    }
    append_attribute(&mut payload, IFLA_LINKINFO, &link_info)?;
    append_attribute(
        &mut payload,
        IFLA_OPERSTATE,
        &[if link.flags & IFF_UP != 0 {
            IF_OPER_UP
        } else {
            IF_OPER_DOWN
        }],
    )?;
    for attribute in &link.attributes {
        let kind = attribute.kind & 0x3fff;
        if matches!(
            kind,
            IFLA_IFNAME
                | IFLA_MTU
                | IFLA_ADDRESS
                | IFLA_BROADCAST
                | IFLA_MASTER
                | IFLA_LINK
                | IFLA_OPERSTATE
                | IFLA_EXT_MASK
                | IFLA_LINKINFO
        ) {
            continue;
        }
        append_attribute(&mut payload, attribute.kind, &attribute.value)?;
    }

    let mut reply = Vec::new();
    append_header(
        &mut reply,
        RTM_NEWLINK,
        if multipart { NLM_F_MULTI } else { 0 },
        header.sequence,
        local_port,
        payload.len(),
    )?;
    reply.extend_from_slice(&payload);
    Ok(reply)
}

fn interface_reply(
    header: MessageHeader,
    local_port: u32,
    interface: &crate::hostnet::Interface,
    multipart: bool,
) -> Result<Vec<u8>, i32> {
    if !interface.host {
        if interface.hardware_type == ARPHRD_LOOPBACK {
            let link = &interface.link;
            return loopback_link_reply(
                header,
                local_port,
                &Loopback {
                    index: link.index,
                    mtu: link.mtu,
                    flags: link.flags,
                    operational: interface.operstate == IF_OPER_UP,
                    address: link.address.clone(),
                },
                multipart,
            );
        }
        return stored_link_reply(header, local_port, &interface.link, multipart);
    }
    let link = &interface.link;
    let mut payload = vec![AF_UNSPEC, 0];
    payload.extend_from_slice(&interface.hardware_type.to_ne_bytes());
    payload.extend_from_slice(&link.index.to_ne_bytes());
    payload.extend_from_slice(&link.flags.to_ne_bytes());
    payload.extend_from_slice(&0u32.to_ne_bytes());
    let mut name = link.name.as_bytes().to_vec();
    name.push(0);
    append_attribute(&mut payload, IFLA_IFNAME, &name)?;
    append_attribute(&mut payload, IFLA_MTU, &link.mtu.to_ne_bytes())?;
    if !link.address.is_empty() {
        append_attribute(&mut payload, IFLA_ADDRESS, &link.address)?;
    }
    if !link.broadcast.is_empty() {
        append_attribute(&mut payload, IFLA_BROADCAST, &link.broadcast)?;
    }
    append_attribute(&mut payload, IFLA_OPERSTATE, &[interface.operstate])?;
    for attribute in &link.attributes {
        append_attribute(&mut payload, attribute.kind, &attribute.value)?;
    }
    if let Some(stats) = &interface.stats {
        append_attribute(&mut payload, 23, &stats.rtnl_stats64())?;
    }
    let mut reply = Vec::new();
    append_header(
        &mut reply,
        RTM_NEWLINK,
        if multipart { NLM_F_MULTI } else { 0 },
        header.sequence,
        local_port,
        payload.len(),
    )?;
    reply.extend_from_slice(&payload);
    Ok(reply)
}

fn loopback_link_reply(
    header: MessageHeader,
    local_port: u32,
    loopback: &Loopback,
    multipart: bool,
) -> Result<Vec<u8>, i32> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&[AF_UNSPEC, 0]);
    payload.extend_from_slice(&ARPHRD_LOOPBACK.to_ne_bytes());
    payload.extend_from_slice(&(loopback.index as i32).to_ne_bytes());
    payload.extend_from_slice(&loopback.flags.to_ne_bytes());
    payload.extend_from_slice(&0u32.to_ne_bytes());
    append_attribute(&mut payload, IFLA_IFNAME, b"lo\0")?;
    append_attribute(&mut payload, IFLA_MTU, &loopback.mtu.to_ne_bytes())?;
    if !loopback.address.is_empty() {
        append_attribute(&mut payload, IFLA_ADDRESS, &loopback.address)?;
        append_attribute(
            &mut payload,
            IFLA_BROADCAST,
            &vec![0; loopback.address.len()],
        )?;
    }
    append_attribute(
        &mut payload,
        IFLA_OPERSTATE,
        &[if loopback.operational {
            IF_OPER_UP
        } else {
            IF_OPER_DOWN
        }],
    )?;
    let mut reply = Vec::new();
    append_header(
        &mut reply,
        RTM_NEWLINK,
        if multipart { NLM_F_MULTI } else { 0 },
        header.sequence,
        local_port,
        payload.len(),
    )?;
    reply.extend_from_slice(&payload);
    Ok(reply)
}

fn address_reply(
    header: MessageHeader,
    local_port: u32,
    address: &crate::route_state::Address,
    multipart: bool,
) -> Result<Vec<u8>, i32> {
    let mut payload = vec![
        address.family,
        address.prefix_len,
        (address.flags & 0xff) as u8,
        address.scope,
    ];
    payload.extend_from_slice(&address.index.to_ne_bytes());
    for attribute in &address.attributes {
        append_attribute(&mut payload, attribute.kind, &attribute.value)?;
    }
    if address.flags > u8::MAX as u32 && route_attribute(&address.attributes, IFA_FLAGS).is_none() {
        append_attribute(&mut payload, IFA_FLAGS, &address.flags.to_ne_bytes())?;
    }
    let mut reply = Vec::new();
    append_header(
        &mut reply,
        RTM_NEWADDR,
        if multipart { NLM_F_MULTI } else { 0 },
        header.sequence,
        local_port,
        payload.len(),
    )?;
    reply.extend_from_slice(&payload);
    Ok(reply)
}

fn route_reply(
    header: MessageHeader,
    local_port: u32,
    route: &crate::route_state::Route,
    multipart: bool,
) -> Result<Vec<u8>, i32> {
    let mut payload = vec![
        route.family,
        route.dst_len,
        route.src_len,
        route.tos,
        route.table,
        route.protocol,
        route.scope,
        route.kind,
    ];
    payload.extend_from_slice(&route.flags.to_ne_bytes());
    for attribute in &route.attributes {
        append_attribute(&mut payload, attribute.kind, &attribute.value)?;
    }
    let mut reply = Vec::new();
    append_header(
        &mut reply,
        RTM_NEWROUTE,
        if multipart { NLM_F_MULTI } else { 0 },
        header.sequence,
        local_port,
        payload.len(),
    )?;
    reply.extend_from_slice(&payload);
    Ok(reply)
}

fn error_reply(
    header: MessageHeader,
    request: &[u8],
    local_port: u32,
    error: i32,
) -> Result<Vec<u8>, i32> {
    let original = request.get(..NLMSG_HEADER_LEN).ok_or(EINVAL)?;
    let mut payload = Vec::with_capacity(4 + original.len());
    payload.extend_from_slice(&error.checked_neg().unwrap_or(error).to_ne_bytes());
    payload.extend_from_slice(original);
    let mut reply = Vec::new();
    append_header(
        &mut reply,
        NLMSG_ERROR,
        0,
        header.sequence,
        local_port,
        payload.len(),
    )?;
    reply.extend_from_slice(&payload);
    Ok(reply)
}

fn nft_message_type(message: u16) -> u16 {
    (NFNL_SUBSYS_NFTABLES << 8) | message
}

fn parse_owned_attributes(bytes: &[u8]) -> Result<Vec<crate::nftables::Attribute>, i32> {
    let mut attributes = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor..].iter().all(|byte| *byte == 0) {
            break;
        }
        let header = bytes
            .get(cursor..cursor + RTATTR_HEADER_LEN)
            .ok_or(EINVAL)?;
        let length = u16::from_ne_bytes(header[0..2].try_into().map_err(|_| EINVAL)?) as usize;
        let kind = u16::from_ne_bytes(header[2..4].try_into().map_err(|_| EINVAL)?);
        if length < RTATTR_HEADER_LEN {
            return Err(EINVAL);
        }
        let end = cursor.checked_add(length).ok_or(EINVAL)?;
        let value = bytes
            .get(cursor + RTATTR_HEADER_LEN..end)
            .ok_or(EINVAL)?
            .to_vec();
        attributes.push(crate::nftables::Attribute { kind, value });
        cursor = cursor.checked_add(align4(length)?).ok_or(EINVAL)?;
    }
    Ok(attributes)
}

fn nft_attribute(attributes: &[crate::nftables::Attribute], kind: u16) -> Option<&[u8]> {
    attributes
        .iter()
        .find(|attribute| attribute.kind & 0x3fff == kind)
        .map(|attribute| attribute.value.as_slice())
}

fn nft_string(attributes: &[crate::nftables::Attribute], kind: u16) -> Result<&str, i32> {
    let bytes = nft_attribute(attributes, kind).ok_or(EINVAL)?;
    if bytes.last() != Some(&0) || bytes[..bytes.len().saturating_sub(1)].contains(&0) {
        return Err(EINVAL);
    }
    core::str::from_utf8(&bytes[..bytes.len() - 1]).map_err(|_| EINVAL)
}

fn parse_nft_message(
    header: MessageHeader,
    request: &[u8],
) -> Result<(u8, Vec<crate::nftables::Attribute>), i32> {
    if header.flags & NLM_F_REQUEST == 0 || header.flags & NLM_F_MULTI != 0 {
        return Err(EINVAL);
    }
    let fixed = request
        .get(NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4)
        .ok_or(EINVAL)?;
    if fixed[1] != NFNETLINK_V0 || u16::from_be_bytes([fixed[2], fixed[3]]) != 0 {
        return Err(EINVAL);
    }
    let attributes = parse_owned_attributes(&request[NLMSG_HEADER_LEN + 4..])?;
    Ok((fixed[0], attributes))
}

fn same_string_attribute(object: &crate::nftables::Object, kind: u16, value: &str) -> bool {
    nft_attribute(&object.attributes, kind).is_some_and(|stored| {
        stored.last() == Some(&0) && stored.get(..stored.len() - 1) == Some(value.as_bytes())
    })
}

fn table_exists(state: &crate::nftables::Ruleset, family: u8, table: &str) -> bool {
    state.objects.iter().any(|object| {
        object.message == NFT_MSG_NEWTABLE
            && object.family == family
            && same_string_attribute(object, NFTA_TABLE_NAME, table)
    })
}

fn chain_exists(state: &crate::nftables::Ruleset, family: u8, table: &str, chain: &str) -> bool {
    state.objects.iter().any(|object| {
        object.message == NFT_MSG_NEWCHAIN
            && object.family == family
            && same_string_attribute(object, NFTA_CHAIN_TABLE, table)
            && same_string_attribute(object, NFTA_CHAIN_NAME, chain)
    })
}

fn apply_nft_mutation(
    state: &mut crate::nftables::Ruleset,
    header: MessageHeader,
    request: &[u8],
) -> Result<(), i32> {
    let message = header.kind & 0x00ff;
    if header.kind >> 8 != NFNL_SUBSYS_NFTABLES {
        return Err(EINVAL);
    }
    let (family, attributes) = parse_nft_message(header, request)?;
    if !matches!(family, 1 | 2 | 3 | 5 | 7 | 10) {
        return Err(97); // EAFNOSUPPORT
    }
    match message {
        NFT_MSG_NEWTABLE => {
            let name = nft_string(&attributes, NFTA_TABLE_NAME)?.to_owned();
            let existing = state.objects.iter().position(|object| {
                object.message == NFT_MSG_NEWTABLE
                    && object.family == family
                    && same_string_attribute(object, NFTA_TABLE_NAME, &name)
            });
            if let Some(index) = existing {
                if header.flags & NLM_F_EXCL != 0 {
                    return Err(EEXIST);
                }
                state.objects[index].attributes = attributes;
                return Ok(());
            }
            if header.flags & NLM_F_CREATE == 0 {
                return Err(ENOENT);
            }
            let handle = state.allocate_handle()?;
            state.objects.push(crate::nftables::Object {
                message,
                family,
                handle,
                attributes,
            });
            Ok(())
        }
        NFT_MSG_DELTABLE => {
            let name = nft_string(&attributes, NFTA_TABLE_NAME)?.to_owned();
            if !table_exists(state, family, &name) {
                return Err(ENOENT);
            }
            state.objects.retain(|object| {
                if object.family != family {
                    return true;
                }
                if object.message == NFT_MSG_NEWTABLE {
                    return !same_string_attribute(object, NFTA_TABLE_NAME, &name);
                }
                let table_kind = match object.message {
                    NFT_MSG_NEWCHAIN => NFTA_CHAIN_TABLE,
                    NFT_MSG_NEWRULE => NFTA_RULE_TABLE,
                    _ => return true,
                };
                !same_string_attribute(object, table_kind, &name)
            });
            Ok(())
        }
        NFT_MSG_NEWCHAIN => {
            let table = nft_string(&attributes, NFTA_CHAIN_TABLE)?.to_owned();
            let name = nft_string(&attributes, NFTA_CHAIN_NAME)?.to_owned();
            if !table_exists(state, family, &table) {
                return Err(ENOENT);
            }
            let existing = state.objects.iter().position(|object| {
                object.message == NFT_MSG_NEWCHAIN
                    && object.family == family
                    && same_string_attribute(object, NFTA_CHAIN_TABLE, &table)
                    && same_string_attribute(object, NFTA_CHAIN_NAME, &name)
            });
            if let Some(index) = existing {
                if header.flags & NLM_F_EXCL != 0 {
                    return Err(EEXIST);
                }
                state.objects[index].attributes = attributes;
                return Ok(());
            }
            let handle = state.allocate_handle()?;
            state.objects.push(crate::nftables::Object {
                message,
                family,
                handle,
                attributes,
            });
            Ok(())
        }
        NFT_MSG_DELCHAIN => {
            let table = nft_string(&attributes, NFTA_CHAIN_TABLE)?.to_owned();
            let name = nft_string(&attributes, NFTA_CHAIN_NAME)?.to_owned();
            if !chain_exists(state, family, &table, &name) {
                return Err(ENOENT);
            }
            state.objects.retain(|object| {
                !(object.family == family
                    && ((object.message == NFT_MSG_NEWCHAIN
                        && same_string_attribute(object, NFTA_CHAIN_TABLE, &table)
                        && same_string_attribute(object, NFTA_CHAIN_NAME, &name))
                        || (object.message == NFT_MSG_NEWRULE
                            && same_string_attribute(object, NFTA_RULE_TABLE, &table)
                            && same_string_attribute(object, NFTA_RULE_CHAIN, &name))))
            });
            Ok(())
        }
        NFT_MSG_NEWRULE => {
            let table = nft_string(&attributes, NFTA_RULE_TABLE)?.to_owned();
            let chain = nft_string(&attributes, NFTA_RULE_CHAIN)?.to_owned();
            if !chain_exists(state, family, &table, &chain) {
                return Err(ENOENT);
            }
            let handle = state.allocate_handle()?;
            state.objects.push(crate::nftables::Object {
                message,
                family,
                handle,
                attributes,
            });
            Ok(())
        }
        NFT_MSG_DELRULE => {
            let table = nft_string(&attributes, NFTA_RULE_TABLE)?.to_owned();
            let chain = nft_string(&attributes, NFTA_RULE_CHAIN)?.to_owned();
            if !chain_exists(state, family, &table, &chain) {
                return Err(ENOENT);
            }
            let handle = nft_attribute(&attributes, NFTA_RULE_HANDLE)
                .map(|bytes| {
                    let value: [u8; 8] = bytes.try_into().map_err(|_| EINVAL)?;
                    Ok::<u64, i32>(u64::from_be_bytes(value))
                })
                .transpose()?;
            let before = state.objects.len();
            state.objects.retain(|object| {
                !(object.message == NFT_MSG_NEWRULE
                    && object.family == family
                    && same_string_attribute(object, NFTA_RULE_TABLE, &table)
                    && same_string_attribute(object, NFTA_RULE_CHAIN, &chain)
                    && handle.is_none_or(|handle| object.handle == handle))
            });
            if handle.is_some() && state.objects.len() == before {
                return Err(ENOENT);
            }
            Ok(())
        }
        _ => Err(EOPNOTSUPP),
    }
}

fn nft_object_reply(
    object: &crate::nftables::Object,
    sequence: u32,
    local_port: u32,
    multipart: bool,
    state: &crate::nftables::Ruleset,
) -> Result<Vec<u8>, i32> {
    let mut payload = vec![object.family, NFNETLINK_V0, 0, 0];
    for attribute in &object.attributes {
        append_attribute(&mut payload, attribute.kind, &attribute.value)?;
    }
    match object.message {
        NFT_MSG_NEWTABLE => {
            if nft_attribute(&object.attributes, NFTA_TABLE_USE).is_none() {
                let name = nft_string(&object.attributes, NFTA_TABLE_NAME)?;
                let uses = state
                    .objects
                    .iter()
                    .filter(|child| {
                        child.message == NFT_MSG_NEWCHAIN
                            && child.family == object.family
                            && same_string_attribute(child, NFTA_CHAIN_TABLE, name)
                    })
                    .count() as u32;
                append_attribute(&mut payload, NFTA_TABLE_USE, &uses.to_be_bytes())?;
            }
            if nft_attribute(&object.attributes, NFTA_TABLE_HANDLE).is_none() {
                append_attribute(
                    &mut payload,
                    NFTA_TABLE_HANDLE,
                    &object.handle.to_be_bytes(),
                )?;
            }
        }
        NFT_MSG_NEWCHAIN => {
            if nft_attribute(&object.attributes, NFTA_CHAIN_USE).is_none() {
                append_attribute(&mut payload, NFTA_CHAIN_USE, &0u32.to_be_bytes())?;
            }
            if nft_attribute(&object.attributes, NFTA_CHAIN_HANDLE).is_none() {
                append_attribute(
                    &mut payload,
                    NFTA_CHAIN_HANDLE,
                    &object.handle.to_be_bytes(),
                )?;
            }
        }
        NFT_MSG_NEWRULE => {
            if nft_attribute(&object.attributes, NFTA_RULE_HANDLE).is_none() {
                append_attribute(&mut payload, NFTA_RULE_HANDLE, &object.handle.to_be_bytes())?;
            }
        }
        _ => {}
    }
    let mut reply = Vec::new();
    append_header(
        &mut reply,
        nft_message_type(object.message),
        if multipart { NLM_F_MULTI } else { 0 },
        sequence,
        local_port,
        payload.len(),
    )?;
    reply.extend_from_slice(&payload);
    Ok(reply)
}

fn done_reply(sequence: u32, local_port: u32) -> Result<Vec<u8>, i32> {
    let mut reply = Vec::new();
    append_header(&mut reply, NLMSG_DONE, NLM_F_MULTI, sequence, local_port, 0)?;
    Ok(reply)
}

fn compat_revisions(family: u8, name: &str, target: bool) -> &'static [u8] {
    // Extension revision negotiation for the stored nf_tables control plane.
    // This is not evidence of a complete packet-level firewall evaluator.
    // iptables-nft checks tcp/udp here before translating ports to native nft
    // payload/cmp expressions. Unknown revisions must remain ENOENT.
    match (family, name, target) {
        (2 | 10, "tcp" | "udp", false) => &[0],
        (2 | 10, "addrtype", false) => &[0, 1],
        // Linux registers the xt_conntrack match for NFPROTO_UNSPEC at
        // revisions 1, 2, and 3.  IPv4 and IPv6 therefore expose the same
        // revision set through NFT_COMPAT_GET; revision 0 is not registered.
        (2 | 10, "conntrack", false) => &[1, 2, 3],
        // xt_nat revision 0 uses the IPv4-only range layout; revisions 1/2
        // share the IPv4/IPv6 layout. iptables-nft translates these to nat
        // expressions after this query, including Docker's published ports.
        (2, "DNAT" | "SNAT", true) => &[0, 1, 2],
        (10, "DNAT" | "SNAT", true) => &[1, 2],
        (2 | 10, "MASQUERADE", true) => &[0],
        _ => &[],
    }
}

fn handle_compat_query(
    header: MessageHeader,
    request: &[u8],
    local_port: u32,
) -> Result<Vec<Vec<u8>>, i32> {
    if header.kind != (NFNL_SUBSYS_NFT_COMPAT << 8) | NFNL_MSG_COMPAT_GET {
        return Ok(vec![error_reply(header, request, local_port, EOPNOTSUPP)?]);
    }
    let (family, attributes) = match parse_nft_message(header, request) {
        Ok(parsed) => parsed,
        Err(error) => return Ok(vec![error_reply(header, request, local_port, error)?]),
    };
    let name = nft_string(&attributes, NFTA_COMPAT_NAME)?;
    let requested = nft_attribute(&attributes, NFTA_COMPAT_REV).ok_or(EINVAL)?;
    let requested = u32::from_be_bytes(requested.try_into().map_err(|_| EINVAL)?);
    let Ok(requested) = u8::try_from(requested) else {
        return Ok(vec![error_reply(header, request, local_port, ENOENT)?]);
    };
    let extension_type = nft_attribute(&attributes, NFTA_COMPAT_TYPE).ok_or(EINVAL)?;
    let extension_type = u32::from_be_bytes(extension_type.try_into().map_err(|_| EINVAL)?);
    let target = match extension_type {
        0 => false,
        1 => true,
        _ => return Ok(vec![error_reply(header, request, local_port, EINVAL)?]),
    };
    let revisions = compat_revisions(family, name, target);
    if !revisions.contains(&requested) {
        return Ok(vec![error_reply(header, request, local_port, ENOENT)?]);
    }
    let best = *revisions.iter().max().ok_or(ENOENT)?;
    let mut payload = vec![family, NFNETLINK_V0, 0, 0];
    let mut terminated_name = name.as_bytes().to_vec();
    terminated_name.push(0);
    append_attribute(&mut payload, NFTA_COMPAT_NAME, &terminated_name)?;
    append_attribute(
        &mut payload,
        NFTA_COMPAT_REV,
        &u32::from(best).to_be_bytes(),
    )?;
    append_attribute(
        &mut payload,
        NFTA_COMPAT_TYPE,
        &extension_type.to_be_bytes(),
    )?;
    let mut reply = Vec::new();
    append_header(
        &mut reply,
        header.kind,
        NLM_F_MULTI,
        header.sequence,
        local_port,
        payload.len(),
    )?;
    reply.extend_from_slice(&payload);
    let mut replies = vec![reply];
    if header.flags & NLM_F_ACK != 0 {
        replies.push(error_reply(header, request, local_port, 0)?);
    }
    Ok(replies)
}

fn handle_netfilter_query(
    header: MessageHeader,
    request: &[u8],
    local_port: u32,
) -> Result<Vec<Vec<u8>>, i32> {
    if header.flags & NLM_F_REQUEST == 0 || header.flags & NLM_F_MULTI != 0 {
        return Ok(vec![error_reply(header, request, local_port, EINVAL)?]);
    }
    if header.kind >> 8 == NFNL_SUBSYS_NFT_COMPAT {
        return handle_compat_query(header, request, local_port);
    }
    let state = crate::nftables::snapshot()?;
    netfilter_query_snapshot(header, request, local_port, &state)
}

fn netfilter_query_snapshot(
    header: MessageHeader,
    request: &[u8],
    local_port: u32,
    state: &crate::nftables::Ruleset,
) -> Result<Vec<Vec<u8>>, i32> {
    if header.kind == nft_message_type(NFT_MSG_GETGEN) {
        let (family, attributes) = parse_nft_message(header, request)?;
        if family != 0 || !attributes.is_empty() {
            return Ok(vec![error_reply(header, request, local_port, EINVAL)?]);
        }
        let mut payload = vec![0, NFNETLINK_V0, 0, 0];
        append_attribute(&mut payload, NFTA_GEN_ID, &state.generation.to_be_bytes())?;
        let mut reply = Vec::new();
        append_header(
            &mut reply,
            nft_message_type(NFT_MSG_NEWGEN),
            0,
            header.sequence,
            local_port,
            payload.len(),
        )?;
        reply.extend_from_slice(&payload);
        return Ok(vec![reply]);
    }

    let get_message = header.kind & 0x00ff;
    if header.kind >> 8 != NFNL_SUBSYS_NFTABLES {
        return Ok(vec![error_reply(header, request, local_port, EINVAL)?]);
    }
    if matches!(
        get_message,
        NFT_MSG_GETSET | NFT_MSG_GETOBJ | NFT_MSG_GETFLOWTABLE
    ) {
        // These object types cannot be created by apply_nft_mutation yet.
        // Listing their actual empty collection is a valid read operation;
        // rejecting it prevented nft from even reading existing table/rule data.
        // Never hide an object written by a future or incompatible producer.
        let (family, attributes) = parse_nft_message(header, request)?;
        if state
            .objects
            .iter()
            .any(|object| object.message == get_message - 1)
        {
            return Ok(vec![error_reply(header, request, local_port, EOPNOTSUPP)?]);
        }
        if nft_attribute(&attributes, 1).is_some() {
            let table = nft_string(&attributes, 1)?;
            if !table_exists(state, family, table) {
                return Ok(vec![error_reply(header, request, local_port, ENOENT)?]);
            }
        }
        return Ok(vec![if header.flags & NLM_F_DUMP == NLM_F_DUMP {
            done_reply(header.sequence, local_port)?
        } else {
            error_reply(header, request, local_port, ENOENT)?
        }]);
    }
    let object_message = match get_message {
        NFT_MSG_GETTABLE => NFT_MSG_NEWTABLE,
        NFT_MSG_GETCHAIN => NFT_MSG_NEWCHAIN,
        NFT_MSG_GETRULE => NFT_MSG_NEWRULE,
        _ => return Ok(vec![error_reply(header, request, local_port, EOPNOTSUPP)?]),
    };
    if header.kind >> 8 != NFNL_SUBSYS_NFTABLES {
        return Ok(vec![error_reply(header, request, local_port, EINVAL)?]);
    }
    let (family, attributes) = match parse_nft_message(header, request) {
        Ok(parsed) => parsed,
        Err(error) => return Ok(vec![error_reply(header, request, local_port, error)?]),
    };
    let dump = header.flags & NLM_F_DUMP == NLM_F_DUMP;
    let table_filter = match object_message {
        NFT_MSG_NEWTABLE => nft_attribute(&attributes, NFTA_TABLE_NAME)
            .map(|_| nft_string(&attributes, NFTA_TABLE_NAME))
            .transpose()?,
        NFT_MSG_NEWCHAIN => nft_attribute(&attributes, NFTA_CHAIN_TABLE)
            .map(|_| nft_string(&attributes, NFTA_CHAIN_TABLE))
            .transpose()?,
        NFT_MSG_NEWRULE => nft_attribute(&attributes, NFTA_RULE_TABLE)
            .map(|_| nft_string(&attributes, NFTA_RULE_TABLE))
            .transpose()?,
        _ => None,
    };
    let name_filter = match object_message {
        NFT_MSG_NEWCHAIN => nft_attribute(&attributes, NFTA_CHAIN_NAME)
            .map(|_| nft_string(&attributes, NFTA_CHAIN_NAME))
            .transpose()?,
        NFT_MSG_NEWRULE => nft_attribute(&attributes, NFTA_RULE_CHAIN)
            .map(|_| nft_string(&attributes, NFTA_RULE_CHAIN))
            .transpose()?,
        _ => None,
    };
    let table_kind = match object_message {
        NFT_MSG_NEWTABLE => NFTA_TABLE_NAME,
        NFT_MSG_NEWCHAIN => NFTA_CHAIN_TABLE,
        NFT_MSG_NEWRULE => NFTA_RULE_TABLE,
        _ => 0,
    };
    let name_kind = if object_message == NFT_MSG_NEWCHAIN {
        NFTA_CHAIN_NAME
    } else {
        NFTA_RULE_CHAIN
    };
    let selected: Vec<_> = state
        .objects
        .iter()
        .filter(|object| {
            object.message == object_message && (family == 0 || object.family == family)
        })
        .filter(|object| {
            table_filter.is_none_or(|table| same_string_attribute(object, table_kind, table))
        })
        .filter(|object| {
            name_filter.is_none_or(|name| same_string_attribute(object, name_kind, name))
        })
        .collect();
    if selected.is_empty() && !dump {
        return Ok(vec![error_reply(header, request, local_port, ENOENT)?]);
    }
    let mut replies = Vec::new();
    for object in selected {
        replies.push(nft_object_reply(
            object,
            header.sequence,
            local_port,
            dump,
            &state,
        )?);
    }
    if dump {
        replies.push(done_reply(header.sequence, local_port)?);
    } else if header.flags & NLM_F_ACK != 0 {
        replies.push(error_reply(header, request, local_port, 0)?);
    }
    Ok(replies)
}

fn parse_batch_generation(header: MessageHeader, request: &[u8]) -> Result<u32, i32> {
    if header.kind != NFNL_MSG_BATCH_BEGIN || header.flags & NLM_F_REQUEST == 0 {
        return Err(EINVAL);
    }
    let fixed = request
        .get(NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4)
        .ok_or(EINVAL)?;
    if fixed[0] != 0
        || fixed[1] != NFNETLINK_V0
        || u16::from_be_bytes([fixed[2], fixed[3]]) != NFNL_SUBSYS_NFTABLES
    {
        return Err(EINVAL);
    }
    let attributes = parse_owned_attributes(&request[NLMSG_HEADER_LEN + 4..])?;
    let bytes = nft_attribute(&attributes, NFNL_BATCH_GENID).ok_or(EINVAL)?;
    Ok(u32::from_be_bytes(bytes.try_into().map_err(|_| EINVAL)?))
}

fn enqueue_netfilter_replies(item: &Endpoint, replies: Vec<Vec<u8>>) -> Result<(), i32> {
    for bytes in replies {
        item.enqueue(Datagram {
            bytes,
            source: NetlinkAddress::KERNEL,
        })?;
    }
    Ok(())
}

fn dispatch_netfilter(item: &Arc<Endpoint>, request: &[u8], local_port: u32) -> Result<(), i32> {
    let first = parse_header(request)?;
    if !crate::user_namespace::capable(crate::namespaces::network_owner(item.namespace)?, 12) {
        return enqueue_netfilter_replies(
            item,
            vec![error_reply(first, request, local_port, crate::EPERM)?],
        );
    }
    if first.kind != NFNL_MSG_BATCH_BEGIN {
        let length = first.length as usize;
        if align4(length)? != request.len() {
            return Err(EINVAL);
        }
        if trace_enabled() {
            eprintln!(
                "kinakaze netlink: request type={} flags={:#x} seq={} pid={} len={}",
                first.kind, first.flags, first.sequence, first.port, first.length
            );
            eprintln!("kinakaze netlink: request bytes={request:02x?}");
        }
        return enqueue_netfilter_replies(
            item,
            handle_netfilter_query(first, &request[..length], local_port)?,
        );
    }

    let begin_length = first.length as usize;
    let generation = match parse_batch_generation(first, &request[..begin_length]) {
        Ok(generation) => generation,
        Err(error) => {
            return enqueue_netfilter_replies(
                item,
                vec![error_reply(
                    first,
                    &request[..begin_length],
                    local_port,
                    error,
                )?],
            );
        }
    };
    let mut frames = Vec::new();
    let mut cursor = align4(begin_length)?;
    let mut end_frame = None;
    while cursor < request.len() {
        let header = parse_header(&request[cursor..])?;
        let length = header.length as usize;
        let frame = request.get(cursor..cursor + length).ok_or(EINVAL)?;
        if trace_enabled() {
            eprintln!(
                "kinakaze netlink: request type={} flags={:#x} seq={} pid={} len={}",
                header.kind, header.flags, header.sequence, header.port, header.length
            );
            eprintln!("kinakaze netlink: request bytes={frame:02x?}");
        }
        if header.kind == NFNL_MSG_BATCH_END {
            if end_frame.is_some() || align4(length)? + cursor != request.len() {
                return Err(EINVAL);
            }
            end_frame = Some((header, frame.to_vec()));
        } else {
            if end_frame.is_some() {
                return Err(EINVAL);
            }
            frames.push((header, frame.to_vec()));
        }
        cursor = cursor.checked_add(align4(length)?).ok_or(EINVAL)?;
    }
    let Some((end_header, end_request)) = end_frame else {
        return enqueue_netfilter_replies(
            item,
            vec![error_reply(
                first,
                &request[..begin_length],
                local_port,
                EINVAL,
            )?],
        );
    };
    let end_fixed = end_request
        .get(NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4)
        .ok_or(EINVAL)?;
    if end_header.flags & NLM_F_REQUEST == 0
        || end_fixed[0] != 0
        || end_fixed[1] != NFNETLINK_V0
        || u16::from_be_bytes([end_fixed[2], end_fixed[3]]) != NFNL_SUBSYS_NFTABLES
        || end_request.len() != NLMSG_HEADER_LEN + 4
    {
        return enqueue_netfilter_replies(
            item,
            vec![error_reply(
                first,
                &request[..begin_length],
                local_port,
                EINVAL,
            )?],
        );
    }

    enum BatchAbort {
        Operation(Vec<Vec<u8>>),
        Internal(i32),
    }
    let outcome = crate::nftables::transaction(generation, |state| {
        let mut replies = Vec::new();
        let mut failed = false;
        if first.flags & NLM_F_ACK != 0 {
            replies.push(
                error_reply(first, &request[..begin_length], local_port, 0)
                    .map_err(BatchAbort::Internal)?,
            );
        }
        for (header, frame) in &frames {
            let error = apply_nft_mutation(state, *header, frame).err();
            if error.is_some() {
                failed = true;
            }
            if header.flags & NLM_F_ACK != 0 || error.is_some() {
                replies.push(
                    error_reply(*header, frame, local_port, error.unwrap_or(0))
                        .map_err(BatchAbort::Internal)?,
                );
            }
        }
        if end_header.flags & NLM_F_ACK != 0 {
            replies.push(
                error_reply(end_header, &end_request, local_port, 0)
                    .map_err(BatchAbort::Internal)?,
            );
        }
        if failed {
            Err(BatchAbort::Operation(replies))
        } else {
            Ok(replies)
        }
    });
    match outcome {
        Err(error) => enqueue_netfilter_replies(
            item,
            vec![error_reply(
                first,
                &request[..begin_length],
                local_port,
                error,
            )?],
        ),
        Ok(Ok(replies)) | Ok(Err(BatchAbort::Operation(replies))) => {
            enqueue_netfilter_replies(item, replies)
        }
        Ok(Err(BatchAbort::Internal(error))) => Err(error),
    }
}

fn route_error(
    header: MessageHeader,
    request: &[u8],
    local_port: u32,
    error: i32,
) -> Result<Vec<Vec<u8>>, i32> {
    Ok(vec![error_reply(header, request, local_port, error)?])
}

fn route_ack(header: MessageHeader, request: &[u8], local_port: u32) -> Result<Vec<Vec<u8>>, i32> {
    if header.flags & NLM_F_ACK != 0 {
        route_error(header, request, local_port, 0)
    } else {
        Ok(Vec::new())
    }
}

fn valid_link_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 16
        && !name.contains(['/', ':'])
        && name
            .as_bytes()
            .iter()
            .all(|byte| *byte != 0 && !byte.is_ascii_whitespace())
}

fn address_matches(
    stored: &crate::route_state::Address,
    wanted: &crate::route_state::Address,
) -> bool {
    stored.index == wanted.index
        && stored.family == wanted.family
        && stored.prefix_len == wanted.prefix_len
        && (wanted.scope == 0 || stored.scope == wanted.scope)
        && wanted.attributes.iter().all(|wanted_attribute| {
            stored.attributes.iter().any(|stored_attribute| {
                stored_attribute.kind & 0x3fff == wanted_attribute.kind & 0x3fff
                    && stored_attribute.value == wanted_attribute.value
            })
        })
}

fn route_matches(stored: &crate::route_state::Route, wanted: &crate::route_state::Route) -> bool {
    stored.family == wanted.family
        && stored.dst_len == wanted.dst_len
        && stored.src_len == wanted.src_len
        && stored.tos == wanted.tos
        && (wanted.table == 0 || stored.table == wanted.table)
        && (wanted.protocol == 0 || stored.protocol == wanted.protocol)
        && (wanted.scope == 0 || stored.scope == wanted.scope)
        && (wanted.kind == 0 || stored.kind == wanted.kind)
        && wanted.attributes.iter().all(|wanted_attribute| {
            stored.attributes.iter().any(|stored_attribute| {
                stored_attribute.kind & 0x3fff == wanted_attribute.kind & 0x3fff
                    && stored_attribute.value == wanted_attribute.value
            })
        })
}

fn peer_description(
    attributes: &[crate::route_state::Attribute],
) -> Result<Option<Vec<crate::route_state::Attribute>>, i32> {
    let Some(link_info) = route_attribute(attributes, IFLA_LINKINFO) else {
        return Ok(None);
    };
    let link_info = route_attributes(link_info)?;
    let Some(data) = route_attribute(&link_info, IFLA_INFO_DATA) else {
        return Ok(None);
    };
    let data = route_attributes(data)?;
    let Some(peer) = route_attribute(&data, VETH_INFO_PEER) else {
        return Ok(None);
    };
    let peer_attributes = peer.get(IFINFO_MSG_LEN..).ok_or(EINVAL)?;
    route_attributes(peer_attributes).map(Some)
}

/// Legacy dump clients (including Go net.Interfaces) send rtgenmsg, not the
/// complete ifinfomsg/ifaddrmsg. Only dump reads accept that layout; writes keep
/// the complete-header validation. Attribute alignment is relative to rtgenmsg.
fn route_fixed<const N: usize>(
    header: MessageHeader,
    request: &[u8],
    get: u16,
) -> Result<([u8; N], &[u8]), i32> {
    let body = request.get(NLMSG_HEADER_LEN..).ok_or(EINVAL)?;
    let mut fixed = [0; N];
    if body.len() >= N {
        fixed.copy_from_slice(&body[..N]);
        return Ok((fixed, &body[N..]));
    }
    if header.kind != get || header.flags & NLM_F_DUMP != NLM_F_DUMP || body.is_empty() {
        return Err(EINVAL);
    }
    fixed[0] = body[0];
    Ok((fixed, body.get(4..).unwrap_or(&[])))
}

fn handle_route_message(
    header: MessageHeader,
    request: &[u8],
    local_port: u32,
) -> Result<Vec<Vec<u8>>, i32> {
    if matches!(
        header.kind,
        RTM_NEWLINK
            | RTM_SETLINK
            | RTM_DELLINK
            | RTM_NEWADDR
            | RTM_DELADDR
            | RTM_NEWROUTE
            | RTM_DELROUTE
    ) && !crate::user_namespace::capable(
        crate::namespaces::network_owner(crate::usernet::current()?)?,
        12,
    ) {
        return route_error(header, request, local_port, crate::EPERM);
    }
    if header.flags & NLM_F_REQUEST == 0 || header.flags & NLM_F_MULTI != 0 {
        return route_error(header, request, local_port, EINVAL);
    }
    if header.kind == NLMSG_NOOP || header.kind == NLMSG_DONE || header.kind == NLMSG_ERROR {
        return route_error(header, request, local_port, EOPNOTSUPP);
    }

    match header.kind {
        RTM_GETLINK | RTM_NEWLINK | RTM_SETLINK | RTM_DELLINK => {
            let (info, attribute_bytes) =
                match route_fixed::<IFINFO_MSG_LEN>(header, request, RTM_GETLINK) {
                    Ok(value) => value,
                    Err(error) => return route_error(header, request, local_port, error),
                };
            let index = i32::from_ne_bytes(info[4..8].try_into().map_err(|_| EINVAL)?);
            let flags = u32::from_ne_bytes(info[8..12].try_into().map_err(|_| EINVAL)?);
            let change = u32::from_ne_bytes(info[12..16].try_into().map_err(|_| EINVAL)?);
            let attributes = route_attributes(attribute_bytes)?;
            let requested_name = route_string(&attributes, IFLA_IFNAME)?;
            if header.kind == RTM_GETLINK {
                let dump = header.flags & NLM_F_DUMP == NLM_F_DUMP;
                let interfaces = network_interfaces(crate::usernet::current()?)?;
                if dump {
                    let mut replies = Vec::new();
                    for interface in &interfaces {
                        replies.push(interface_reply(header, local_port, interface, true)?);
                    }
                    replies.push(done_reply(header.sequence, local_port)?);
                    return Ok(replies);
                }
                let selected = interfaces.iter().find(|i| {
                    (index == 0 || index as u32 == i.link.index)
                        && requested_name.is_none_or(|name| name == i.link.name)
                });
                return match selected {
                    Some(i) => Ok(vec![interface_reply(header, local_port, i, false)?]),
                    None => route_error(header, request, local_port, ENODEV),
                };
            }
            let loopback = host_loopback()?;
            let host_interfaces = crate::hostnet::interfaces(crate::usernet::current()?)?;
            if host_interfaces.iter().any(|i| {
                i.hardware_type != ARPHRD_LOOPBACK
                    && ((index > 0 && index as u32 == i.link.index)
                        || requested_name == Some(i.link.name.as_str()))
            }) {
                return route_error(header, request, local_port, EOPNOTSUPP);
            }

            if (index > 0 && index as u32 == loopback.index)
                || requested_name.is_some_and(|name| name == "lo")
            {
                if index != 0 && index as u32 != loopback.index
                    || requested_name.is_some_and(|name| name != "lo")
                {
                    return route_error(header, request, local_port, ENODEV);
                }
                if header.kind == RTM_DELLINK {
                    return route_error(header, request, local_port, EOPNOTSUPP);
                }
                if crate::usernet::current()? == 1 {
                    // The initial namespace exposes the real host loopback.
                    // Ensuring already-observed flags is idempotent, as used by
                    // Docker's host sandbox. Never acknowledge a host mutation
                    // or an unimplemented attribute merely because it says lo.
                    if (flags ^ loopback.flags) & change == 0
                        && attributes.iter().all(|a| a.kind == IFLA_IFNAME)
                    {
                        return route_ack(header, request, local_port);
                    }
                    return route_error(header, request, local_port, EOPNOTSUPP);
                }
                let outcome = crate::route_state::transaction(|state| {
                    state.set_loopback_flags(flags, change);
                    Ok::<_, i32>(())
                })?;
                return match outcome {
                    Ok(()) => route_ack(header, request, local_port),
                    Err(e) => route_error(header, request, local_port, e),
                };
            }

            if header.kind == RTM_DELLINK {
                let outcome = crate::route_state::transaction(|state| {
                    let selected = state.links.iter().find(|link| {
                        (index == 0 || index as u32 == link.index)
                            && requested_name.is_none_or(|name| name == link.name)
                    });
                    let Some(selected) = selected else {
                        return Err(ENODEV);
                    };
                    let selected_index = selected.index;
                    let peer = selected.peer;
                    state.links.retain(|link| {
                        link.index != selected_index && (peer == 0 || link.index != peer)
                    });
                    state.addresses.retain(|address| {
                        address.index != selected_index && (peer == 0 || address.index != peer)
                    });
                    state.routes.retain(|route| {
                        route_u32(&route.attributes, RTA_OIF)
                            .ok()
                            .flatten()
                            .is_none_or(|oif| oif != selected_index && (peer == 0 || oif != peer))
                    });
                    for link in &mut state.links {
                        if link.master == selected_index || (peer != 0 && link.master == peer) {
                            link.master = 0;
                        }
                    }
                    Ok(())
                })?;
                return match outcome {
                    Ok(()) => route_ack(header, request, local_port),
                    Err(error) => route_error(header, request, local_port, error),
                };
            }

            if let Some(fd) = route_u32(&attributes, IFLA_NET_NS_FD)?.filter(|_| {
                !(header.kind == RTM_NEWLINK && header.flags & NLM_F_CREATE != 0 && index == 0)
            }) {
                let target = crate::namespaces::descriptor_id(fd as i32, crate::namespaces::NET)?;
                let result = move_link(
                    crate::usernet::current()?,
                    target,
                    index as u32,
                    requested_name,
                );
                return match result {
                    Ok(()) => route_ack(header, request, local_port),
                    Err(e) => route_error(header, request, local_port, e),
                };
            }
            let requested_kind = link_kind(&attributes)?;
            let create =
                header.kind == RTM_NEWLINK && header.flags & NLM_F_CREATE != 0 && index == 0;
            let before = crate::route_state::snapshot()?;
            let peer_target = peer_description(&attributes)?
                .map(|attrs| route_u32(&attrs, IFLA_NET_NS_FD))
                .transpose()?
                .flatten()
                .map(|fd| crate::namespaces::descriptor_id(fd as i32, crate::namespaces::NET))
                .transpose()?;
            let target = route_u32(&attributes, IFLA_NET_NS_FD)?
                .map(|fd| crate::namespaces::descriptor_id(fd as i32, crate::namespaces::NET))
                .transpose()?;
            let outcome = crate::route_state::transaction(|state| {
                let existing = state.links.iter().position(|link| {
                    (index > 0 && index as u32 == link.index)
                        || requested_name.is_some_and(|name| name == link.name)
                });
                if create && existing.is_some() && header.flags & NLM_F_EXCL != 0 {
                    return Err(EEXIST);
                }
                if let Some(position) = existing {
                    let new_name = requested_name
                        .map(str::to_owned)
                        .unwrap_or_else(|| state.links[position].name.clone());
                    if !valid_link_name(&new_name)
                        || state
                            .links
                            .iter()
                            .enumerate()
                            .any(|(other, link)| other != position && link.name == new_name)
                    {
                        return Err(if valid_link_name(&new_name) {
                            EEXIST
                        } else {
                            EINVAL
                        });
                    }
                    if let Some(kind) = requested_kind.as_deref()
                        && kind != state.links[position].kind
                    {
                        return Err(EOPNOTSUPP);
                    }
                    let master = route_u32(&attributes, IFLA_MASTER)?;
                    if master.is_some_and(|master| {
                        master != 0
                            && master != loopback.index
                            && state.links.iter().all(|link| link.index != master)
                    }) {
                        return Err(ENODEV);
                    }
                    let link = &mut state.links[position];
                    link.name = new_name;
                    if change != 0 {
                        link.flags = (link.flags & !change) | (flags & change);
                    }
                    if let Some(mtu) = route_u32(&attributes, IFLA_MTU)? {
                        if mtu == 0 {
                            return Err(EINVAL);
                        }
                        link.mtu = mtu;
                    }
                    if let Some(address) = route_attribute(&attributes, IFLA_ADDRESS) {
                        if address.is_empty() {
                            return Err(EINVAL);
                        }
                        link.address = address.to_vec();
                    }
                    if let Some(broadcast) = route_attribute(&attributes, IFLA_BROADCAST) {
                        link.broadcast = broadcast.to_vec();
                    }
                    if let Some(master) = master {
                        link.master = master;
                    }
                    link.attributes = attributes.clone();
                    return Ok(());
                }
                if !create {
                    return Err(ENODEV);
                }
                let Some(name) = requested_name else {
                    return Err(EINVAL);
                };
                if !valid_link_name(name) || name == "lo" {
                    return Err(EINVAL);
                }
                if state.links.iter().any(|link| link.name == name) {
                    return Err(EEXIST);
                }
                let kind = requested_kind.as_deref().ok_or(EINVAL)?;
                if !matches!(kind, "bridge" | "veth" | "dummy") {
                    return Err(EOPNOTSUPP);
                }
                let index = u32::try_from(crate::mount::shared::next_group()?)
                    .map_err(|_| crate::ENOSPC)?
                    .checked_add(0x10000)
                    .ok_or(crate::ENOSPC)?;
                if host_interfaces.iter().any(|i| i.link.index == index) {
                    return Err(EEXIST);
                }
                let mtu = route_u32(&attributes, IFLA_MTU)?.unwrap_or(1500);
                if mtu == 0 {
                    return Err(EINVAL);
                }
                let address = route_attribute(&attributes, IFLA_ADDRESS)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| default_mac(index));
                let broadcast = route_attribute(&attributes, IFLA_BROADCAST)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| vec![0xff; address.len()]);
                let master = route_u32(&attributes, IFLA_MASTER)?.unwrap_or(0);
                if master != 0
                    && master != loopback.index
                    && state.links.iter().all(|link| link.index != master)
                {
                    return Err(ENODEV);
                }
                let mut peer_index = 0;
                let peer = if kind == "veth" {
                    let peer_attributes = peer_description(&attributes)?.ok_or(EINVAL)?;
                    let peer_name = route_string(&peer_attributes, IFLA_IFNAME)?.ok_or(EINVAL)?;
                    if !valid_link_name(peer_name)
                        || peer_name == "lo"
                        || peer_name == name
                        || state.links.iter().any(|link| link.name == peer_name)
                    {
                        return Err(EINVAL);
                    }
                    peer_index = u32::try_from(crate::mount::shared::next_group()?)
                        .map_err(|_| crate::ENOSPC)?
                        .checked_add(0x10000)
                        .ok_or(crate::ENOSPC)?;
                    if host_interfaces
                        .iter()
                        .any(|i| i.link.index == peer_index || i.link.name == peer_name)
                    {
                        return Err(EEXIST);
                    }
                    let peer_mtu = route_u32(&peer_attributes, IFLA_MTU)?.unwrap_or(mtu);
                    let peer_address = route_attribute(&peer_attributes, IFLA_ADDRESS)
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| default_mac(peer_index));
                    Some(crate::route_state::Link {
                        index: peer_index,
                        name: peer_name.to_owned(),
                        kind: "veth".into(),
                        flags: IFF_BROADCAST | IFF_MULTICAST,
                        mtu: peer_mtu,
                        broadcast: vec![0xff; peer_address.len()],
                        address: peer_address,
                        master: 0,
                        peer: index,
                        attributes: peer_attributes,
                    })
                } else {
                    None
                };
                state.links.push(crate::route_state::Link {
                    index,
                    name: name.to_owned(),
                    kind: kind.to_owned(),
                    flags: flags | IFF_BROADCAST | IFF_MULTICAST,
                    mtu,
                    address,
                    broadcast,
                    master,
                    peer: peer_index,
                    attributes: attributes.clone(),
                });
                if let Some(peer) = peer {
                    state.links.push(peer);
                }
                Ok(())
            })?;
            let outcome = outcome.and_then(|()| {
                let after = crate::route_state::snapshot()?;
                let link = after
                    .links
                    .iter()
                    .find(|l| {
                        (index > 0 && l.index == index as u32)
                            || requested_name == Some(l.name.as_str())
                    })
                    .ok_or(ENODEV)?;
                if let Some(target) = peer_target {
                    move_link(crate::usernet::current()?, target, link.peer, None)?;
                }
                if let Some(target) = target {
                    move_link(crate::usernet::current()?, target, link.index, None)?;
                }
                Ok(())
            });
            if outcome.is_err() {
                let _ = crate::route_state::transaction(|state| {
                    *state = before;
                    Ok::<_, i32>(())
                });
            }
            match outcome {
                Ok(()) => route_ack(header, request, local_port),
                Err(error) => route_error(header, request, local_port, error),
            }
        }
        RTM_GETADDR | RTM_NEWADDR | RTM_DELADDR => {
            let (fixed, attribute_bytes) =
                match route_fixed::<IFADDR_MSG_LEN>(header, request, RTM_GETADDR) {
                    Ok(value) => value,
                    Err(error) => return route_error(header, request, local_port, error),
                };
            let family = fixed[0];
            let prefix_len = fixed[1];
            let legacy_flags = u32::from(fixed[2]);
            let scope = fixed[3];
            let index = u32::from_ne_bytes(fixed[4..8].try_into().map_err(|_| EINVAL)?);
            let attributes = route_attributes(attribute_bytes)?;
            let extended_flags = route_u32(&attributes, IFA_FLAGS)?.unwrap_or(legacy_flags);
            if header.kind == RTM_GETADDR {
                let dump = header.flags & NLM_F_DUMP == NLM_F_DUMP;
                let state = crate::route_state::snapshot()?;
                let mut addresses = state.addresses;
                let namespace = crate::usernet::current()?;
                let interfaces = network_interfaces(namespace)?;
                addresses.extend(crate::hostnet::addresses(namespace, &interfaces)?);
                let selected: Vec<_> = addresses
                    .iter()
                    .filter(|address| family == AF_UNSPEC || address.family == family)
                    .filter(|address| index == 0 || address.index == index)
                    .collect();
                if !dump && selected.is_empty() {
                    return route_error(header, request, local_port, ENOENT);
                }
                let mut replies = Vec::new();
                for address in selected {
                    replies.push(address_reply(header, local_port, address, dump)?);
                }
                if dump {
                    replies.push(done_reply(header.sequence, local_port)?);
                }
                return Ok(replies);
            }
            if !matches!(family, AF_INET | AF_INET6)
                || index == 0
                || prefix_len > if family == AF_INET { 32 } else { 128 }
                || attributes.iter().any(|a| {
                    matches!(a.kind & 0x3fff, IFA_ADDRESS | IFA_LOCAL | IFA_BROADCAST)
                        && a.value.len() != if family == AF_INET { 4 } else { 16 }
                })
                || route_attribute(&attributes, IFA_ADDRESS)
                    .or_else(|| route_attribute(&attributes, IFA_LOCAL))
                    .is_none()
            {
                return route_error(header, request, local_port, EINVAL);
            }
            let loopback = host_loopback()?;
            if index == loopback.index {
                return route_error(header, request, local_port, EOPNOTSUPP);
            }
            let wanted = crate::route_state::Address {
                index,
                family,
                prefix_len,
                flags: extended_flags,
                scope,
                attributes,
            };
            let outcome = crate::route_state::transaction(|state| {
                if state.links.iter().all(|link| link.index != index) {
                    return Err(ENODEV);
                }
                let existing = state
                    .addresses
                    .iter()
                    .position(|address| address_matches(address, &wanted));
                if header.kind == RTM_DELADDR {
                    let Some(position) = existing else {
                        return Err(ENOENT);
                    };
                    state.addresses.remove(position);
                } else if let Some(position) = existing {
                    if header.flags & NLM_F_EXCL != 0 {
                        return Err(EEXIST);
                    }
                    state.addresses[position] = wanted;
                } else {
                    state.addresses.push(wanted);
                }
                Ok(())
            })?;
            match outcome {
                Ok(()) => route_ack(header, request, local_port),
                Err(error) => route_error(header, request, local_port, error),
            }
        }
        RTM_GETROUTE | RTM_NEWROUTE | RTM_DELROUTE => {
            if request.len() < NLMSG_HEADER_LEN + RTMSG_LEN {
                return route_error(header, request, local_port, EINVAL);
            }
            let fixed = &request[NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + RTMSG_LEN];
            let attributes = route_attributes(&request[NLMSG_HEADER_LEN + RTMSG_LEN..])?;
            let mut route = crate::route_state::Route {
                family: fixed[0],
                dst_len: fixed[1],
                src_len: fixed[2],
                tos: fixed[3],
                table: fixed[4],
                protocol: fixed[5],
                scope: fixed[6],
                kind: fixed[7],
                flags: u32::from_ne_bytes(fixed[8..12].try_into().map_err(|_| EINVAL)?),
                attributes,
            };
            if let Some(table) = route_u32(&route.attributes, RTA_TABLE)? {
                route.table = u8::try_from(table).unwrap_or(0);
            }
            if header.kind == RTM_GETROUTE {
                let dump = header.flags & NLM_F_DUMP == NLM_F_DUMP;
                let state = crate::route_state::snapshot()?;
                if !dump {
                    return match routes::lookup(&state, &route) {
                        Ok(selected) => {
                            Ok(vec![route_reply(header, local_port, &selected, false)?])
                        }
                        Err(error) => route_error(header, request, local_port, error),
                    };
                }
                let routes = routes::all(&state)?;
                let selected: Vec<_> = routes
                    .iter()
                    .filter(|stored| route.family == AF_UNSPEC || stored.family == route.family)
                    .filter(|stored| route.table == 0 || stored.table == route.table)
                    .collect();
                let mut replies = Vec::new();
                for stored in selected {
                    replies.push(route_reply(header, local_port, stored, dump)?);
                }
                if dump {
                    replies.push(done_reply(header.sequence, local_port)?);
                }
                return Ok(replies);
            }
            if !matches!(route.family, AF_INET | AF_INET6) {
                return route_error(header, request, local_port, EINVAL);
            }
            let address_size = if route.family == AF_INET { 4 } else { 16 };
            if route.dst_len != 0
                && route_attribute(&route.attributes, RTA_DST)
                    .is_none_or(|dst| dst.len() != address_size)
            {
                return route_error(header, request, local_port, EINVAL);
            }
            let loopback = host_loopback()?;
            let outcome = crate::route_state::transaction(|state| {
                if let Some(oif) = route_u32(&route.attributes, RTA_OIF)?
                    && oif != loopback.index
                    && state.links.iter().all(|link| link.index != oif)
                {
                    return Err(ENODEV);
                }
                let existing = state
                    .routes
                    .iter()
                    .position(|stored| route_matches(stored, &route));
                if header.kind == RTM_DELROUTE {
                    let Some(position) = existing else {
                        return Err(ENOENT);
                    };
                    state.routes.remove(position);
                } else if let Some(position) = existing {
                    if header.flags & NLM_F_EXCL != 0 {
                        return Err(EEXIST);
                    }
                    state.routes[position] = route;
                } else {
                    state.routes.push(route);
                }
                Ok(())
            })?;
            match outcome {
                Ok(()) => route_ack(header, request, local_port),
                Err(error) => route_error(header, request, local_port, error),
            }
        }
        _ => route_error(header, request, local_port, EOPNOTSUPP),
    }
}

fn dispatch(item: &Arc<Endpoint>, request: &[u8]) -> Result<(), i32> {
    let _namespace = crate::usernet::scope(item.namespace);
    let local = {
        let state = item.state.lock().map_err(|_| EIO)?;
        state.bound.ok_or(EDESTADDRREQ)?
    };
    if item.protocol == NETLINK_NETFILTER {
        return dispatch_netfilter(item, request, local.port);
    }
    if item.protocol != NETLINK_ROUTE {
        return Err(EPROTONOSUPPORT);
    }
    let mut cursor = 0usize;
    while cursor < request.len() {
        let header = parse_header(&request[cursor..])?;
        if trace_enabled() {
            eprintln!(
                "kinakaze netlink: request type={} flags={:#x} seq={} pid={} len={}",
                header.kind, header.flags, header.sequence, header.port, header.length
            );
        }
        let length = header.length as usize;
        let message = request.get(cursor..cursor + length).ok_or(EINVAL)?;
        if trace_enabled() {
            eprintln!("kinakaze netlink: request bytes={message:02x?}");
        }
        let responses = match handle_route_message(header, message, local.port) {
            Ok(responses) => responses,
            Err(error) => {
                if route_trace_enabled() {
                    eprintln!(
                        "kinakaze rtnetlink: parse rejected type={} flags={:#x} seq={} error={} request={message:02x?}",
                        header.kind, header.flags, header.sequence, error,
                    );
                }
                return Err(error);
            }
        };
        for response in responses {
            if route_trace_enabled()
                && parse_header(&response).is_ok_and(|response_header| {
                    response_header.kind == NLMSG_ERROR
                        && response
                            .get(NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4)
                            .and_then(|bytes| bytes.try_into().ok())
                            .map(i32::from_ne_bytes)
                            .is_some_and(|error| error != 0)
                })
            {
                let error = i32::from_ne_bytes(
                    response[NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4]
                        .try_into()
                        .map_err(|_| EINVAL)?,
                );
                eprintln!(
                    "kinakaze rtnetlink: rejected type={} flags={:#x} seq={} error={} request={message:02x?}",
                    header.kind, header.flags, header.sequence, -error,
                );
            }
            item.enqueue(Datagram {
                bytes: response,
                source: NetlinkAddress::KERNEL,
            })?;
        }
        cursor = cursor.checked_add(align4(length)?).ok_or(EINVAL)?;
    }
    Ok(())
}

pub unsafe fn sendto(
    fd: i32,
    buffer: *const u8,
    length: usize,
    _flags: i32,
    address: *const u8,
    address_length: i32,
) -> Result<usize, i32> {
    if buffer.is_null() && length != 0 {
        return Err(EFAULT);
    }
    let item = endpoint(fd)?;
    let target = if address.is_null() {
        item.state
            .lock()
            .map_err(|_| EIO)?
            .peer
            .ok_or(EDESTADDRREQ)?
    } else {
        // SAFETY: forwarded from this function's contract.
        unsafe { NetlinkAddress::read(address, address_length)? }
    };
    if target.port != 0
        || (target.groups != 0 && (target.groups & 1 == 0 || item.protocol != NETLINK_ROUTE))
    {
        return Err(EOPNOTSUPP);
    }
    if target.groups != 0
        && !crate::user_namespace::capable(crate::namespaces::network_owner(item.namespace)?, 12)
    {
        return Err(crate::EPERM);
    }
    if item.state.lock().map_err(|_| EIO)?.bound.is_none() {
        auto_bind(fd, &item)?;
    }
    // SAFETY: the caller guarantees `length` readable bytes when nonzero.
    let request = if length == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `length` readable bytes.
        unsafe { core::slice::from_raw_parts(buffer, length) }
    };
    if target.groups != 0 {
        let source = item.state.lock().map_err(|_| EIO)?.bound.ok_or(EIO)?.port;
        crate::route_state::broadcast_link(item.namespace, source, request.to_vec())?;
    }
    dispatch(&item, request)?;
    Ok(length)
}

pub unsafe fn send(fd: i32, buffer: *const u8, length: usize, flags: i32) -> Result<usize, i32> {
    // SAFETY: forwarded from this function's contract.
    unsafe { sendto(fd, buffer, length, flags, core::ptr::null(), 0) }
}

fn wait_for_datagram(item: &Endpoint, timeout_us: Option<u64>) -> Result<(), i32> {
    let (ready, sources) = multicast::prepare(item)?;
    if ready {
        return Ok(());
    }
    let interrupt = interrupt::current();
    let handles: Vec<_> = sources
        .iter()
        .map(crate::epoll::native_wait::Source::raw)
        .collect();
    let timeout_ms = match timeout_us {
        None => u32::MAX,
        Some(microseconds) => microseconds
            .saturating_add(999)
            .checked_div(1000)
            .unwrap_or(u64::MAX)
            .min((u32::MAX - 1) as u64) as u32,
    };
    if !interrupt.is_null() {
        signal::register_waiter();
    }
    let waited = unsafe { crate::epoll::native_wait::wait(&handles, interrupt, timeout_ms) };
    if !interrupt.is_null() {
        signal::unregister_waiter();
    }
    drop(sources);
    match waited? {
        crate::epoll::native_wait::Outcome::Ready => Ok(()),
        crate::epoll::native_wait::Outcome::Timeout => Err(EAGAIN),
        crate::epoll::native_wait::Outcome::Interrupted => match signal::deliver_pending() {
            signal::Delivery::Restart => Ok(()),
            _ => Err(EINTR),
        },
    }
}

pub unsafe fn recvfrom(
    fd: i32,
    buffer: *mut u8,
    length: usize,
    flags: i32,
    address: *mut u8,
    address_length: *mut i32,
) -> Result<usize, i32> {
    if buffer.is_null() && length != 0 {
        return Err(EFAULT);
    }
    let item = endpoint(fd)?;
    let nonblocking =
        crate::get(fd)?.flags.contains(FdFlags::NONBLOCK) || flags & MSG_DONTWAIT != 0;
    loop {
        let mut state = item.state.lock().map_err(|_| EIO)?;
        multicast::refresh(&item, &mut state)?;
        if std::mem::take(&mut state.multicast_overflow) {
            if state.queue.is_empty() {
                unsafe { ResetEvent(item.event) };
            }
            return Err(ENOBUFS);
        }
        let datagram = if flags & MSG_PEEK != 0 {
            state.queue.front().map(|datagram| Datagram {
                bytes: datagram.bytes.clone(),
                source: datagram.source,
            })
        } else {
            state.queue.pop_front()
        };
        if let Some(datagram) = datagram {
            if state.queue.is_empty() {
                // SAFETY: event is live and queue emptiness is serialized here.
                unsafe { ResetEvent(item.event) };
            }
            drop(state);
            if !address.is_null() {
                // SAFETY: forwarded from this function's contract.
                unsafe { datagram.source.write(address, address_length)? };
            }
            let copied = length.min(datagram.bytes.len());
            if trace_enabled() {
                let header = parse_header(&datagram.bytes).ok();
                eprintln!(
                    "kinakaze netlink: recv fd={fd} capacity={length} datagram={} header={:?}",
                    datagram.bytes.len(),
                    header.map(|header| (header.kind, header.flags, header.sequence, header.port))
                );
            }
            // SAFETY: both slices are live for `copied` bytes.
            if copied != 0 {
                unsafe { core::ptr::copy_nonoverlapping(datagram.bytes.as_ptr(), buffer, copied) };
            }
            return if copied < datagram.bytes.len() {
                Err(EMSGSIZE)
            } else {
                Ok(copied)
            };
        }
        drop(state);
        if nonblocking {
            return Err(EAGAIN);
        }
        let timeout = item.state.lock().map_err(|_| EIO)?.receive_timeout_us;
        wait_for_datagram(&item, timeout)?;
    }
}

pub unsafe fn recv(fd: i32, buffer: *mut u8, length: usize, flags: i32) -> Result<usize, i32> {
    // SAFETY: forwarded from this function's contract.
    unsafe {
        recvfrom(
            fd,
            buffer,
            length,
            flags,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    }
}

pub fn poll(fd: i32) -> Result<(bool, bool), i32> {
    let item = endpoint(fd)?;
    let mut state = item.state.lock().map_err(|_| EIO)?;
    multicast::refresh(&item, &mut state)?;
    Ok((!state.queue.is_empty() || state.multicast_overflow, true))
}

pub unsafe fn setsockopt(
    fd: i32,
    level: i32,
    name: i32,
    value: *const u8,
    length: i32,
) -> Result<(), i32> {
    let item = endpoint(fd)?;
    if trace_enabled() {
        eprintln!("kinakaze netlink: setsockopt fd={fd} level={level} name={name}");
    }
    if item.protocol == NETLINK_KOBJECT_UEVENT
        && level == SOL_SOCKET
        && matches!(name, SO_PASSCRED | SO_RCVBUF)
    {
        if value.is_null() {
            return Err(EFAULT);
        }
        if length < 4 {
            return Err(EINVAL);
        }
        let setting = unsafe { value.cast::<i32>().read_unaligned() };
        let mut state = item.state.lock().map_err(|_| EIO)?;
        if name == SO_PASSCRED {
            state.pass_credentials = setting != 0;
        } else {
            // Linux reports twice the requested size for bookkeeping overhead.
            state.receive_buffer = setting.clamp(128, 16 * 1024 * 1024) * 2;
        }
        return Ok(());
    }
    if level != SOL_SOCKET || !matches!(name, SO_RCVTIMEO | SO_SNDTIMEO) {
        return Err(ENOPROTOOPT);
    }
    if value.is_null() {
        return Err(EFAULT);
    }
    if length < 16 {
        return Err(EINVAL);
    }
    // Linux x86_64 timeval is two signed 64-bit fields.
    let seconds = unsafe { value.cast::<i64>().read_unaligned() };
    let microseconds = unsafe { value.add(8).cast::<i64>().read_unaligned() };
    if seconds < 0 || !(0..1_000_000).contains(&microseconds) {
        return Err(EDOM);
    }
    let total = (seconds as u64)
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(microseconds as u64))
        .ok_or(EDOM)?;
    let timeout = (total != 0).then_some(total);
    let mut state = item.state.lock().map_err(|_| EIO)?;
    if name == SO_RCVTIMEO {
        state.receive_timeout_us = timeout;
    } else {
        state.send_timeout_us = timeout;
    }
    Ok(())
}

pub unsafe fn getsockopt(
    fd: i32,
    level: i32,
    name: i32,
    value: *mut u8,
    length: *mut i32,
) -> Result<(), i32> {
    let item = endpoint(fd)?;
    if trace_enabled() {
        eprintln!("kinakaze netlink: getsockopt fd={fd} level={level} name={name}");
    }
    if level != SOL_SOCKET {
        return Err(ENOPROTOOPT);
    }
    if value.is_null() || length.is_null() {
        return Err(EFAULT);
    }
    let scalar = match name {
        SO_TYPE => Some(item.socket_type),
        SO_ERROR => Some(0),
        SO_PROTOCOL => Some(item.protocol),
        SO_PASSCRED if item.protocol == NETLINK_KOBJECT_UEVENT => Some(i32::from(
            item.state.lock().map_err(|_| EIO)?.pass_credentials,
        )),
        SO_RCVBUF if item.protocol == NETLINK_KOBJECT_UEVENT => {
            Some(item.state.lock().map_err(|_| EIO)?.receive_buffer)
        }
        _ => None,
    };
    if let Some(scalar) = scalar {
        if unsafe { *length } < 4 {
            return Err(EINVAL);
        }
        unsafe {
            value.cast::<i32>().write_unaligned(scalar);
            *length = 4;
        }
        return Ok(());
    }
    if !matches!(name, SO_RCVTIMEO | SO_SNDTIMEO) {
        return Err(ENOPROTOOPT);
    }
    if unsafe { *length } < 16 {
        return Err(EINVAL);
    }
    let state = item.state.lock().map_err(|_| EIO)?;
    let total = if name == SO_RCVTIMEO {
        state.receive_timeout_us
    } else {
        state.send_timeout_us
    }
    .unwrap_or(0);
    unsafe {
        value
            .cast::<i64>()
            .write_unaligned((total / 1_000_000) as i64);
        value
            .add(8)
            .cast::<i64>()
            .write_unaligned((total % 1_000_000) as i64);
        *length = 16;
    }
    Ok(())
}

pub fn shutdown(fd: i32, _how: i32) -> Result<(), i32> {
    endpoint(fd)?;
    Err(EOPNOTSUPP)
}

pub fn close(fd: i32) {
    detach(fd);
}

pub fn detach(fd: i32) {
    if let Ok(mut table) = endpoints().lock() {
        table.remove(&fd);
    }
}

pub fn duplicate(oldfd: i32, newfd: i32) -> Result<(), i32> {
    let item = endpoint(oldfd)?;
    let mut table = endpoints().lock().map_err(|_| EIO)?;
    if table.contains_key(&newfd) {
        return Err(EADDRINUSE);
    }
    table.insert(newfd, item);
    Ok(())
}

pub fn serialize_matching(mut keep: impl FnMut(i32) -> bool) -> Result<Vec<u8>, i32> {
    let table = endpoints().lock().map_err(|_| EIO)?;
    let mut payload = Vec::new();
    payload.extend_from_slice(&0u32.to_le_bytes());
    let mut count = 0u32;
    for (fd, item) in table.iter() {
        if !keep(*fd) {
            continue;
        }
        let state = item.state.lock().map_err(|_| EIO)?;
        let bound = state.bound.unwrap_or(NetlinkAddress { port: 0, groups: 0 });
        let peer = state.peer.unwrap_or(NetlinkAddress {
            port: u32::MAX,
            groups: 0,
        });
        payload.extend_from_slice(&fd.to_le_bytes());
        payload.extend_from_slice(&item.protocol.to_le_bytes());
        payload.extend_from_slice(&item.socket_type.to_le_bytes());
        payload.extend_from_slice(&bound.port.to_le_bytes());
        payload.extend_from_slice(&bound.groups.to_le_bytes());
        payload.extend_from_slice(&peer.port.to_le_bytes());
        payload.extend_from_slice(&peer.groups.to_le_bytes());
        payload.extend_from_slice(&state.receive_timeout_us.unwrap_or(0).to_le_bytes());
        payload.extend_from_slice(&state.send_timeout_us.unwrap_or(0).to_le_bytes());
        payload.extend_from_slice(&(state.queue.len() as u32).to_le_bytes());
        payload.extend_from_slice(&item.namespace.to_le_bytes());
        payload.extend_from_slice(&state.multicast_cursor.to_le_bytes());
        payload.extend_from_slice(&u32::from(state.multicast_overflow).to_le_bytes());
        payload.extend_from_slice(&u32::from(state.pass_credentials).to_le_bytes());
        payload.extend_from_slice(&state.receive_buffer.to_le_bytes());
        for datagram in &state.queue {
            payload.extend_from_slice(&datagram.source.port.to_le_bytes());
            payload.extend_from_slice(&datagram.source.groups.to_le_bytes());
            let length = u32::try_from(datagram.bytes.len()).map_err(|_| EMSGSIZE)?;
            payload.extend_from_slice(&length.to_le_bytes());
            payload.extend_from_slice(&datagram.bytes);
            while !payload.len().is_multiple_of(4) {
                payload.push(0);
            }
        }
        count += 1;
    }
    payload[..4].copy_from_slice(&count.to_le_bytes());
    Ok(payload)
}

pub fn restore(payload: &[u8]) -> bool {
    let Some(count_bytes) = payload.get(..4) else {
        return false;
    };
    let count = u32::from_le_bytes(match count_bytes.try_into() {
        Ok(bytes) => bytes,
        Err(_) => return false,
    }) as usize;
    let mut cursor = 4usize;
    let mut restored = HashMap::new();
    for _ in 0..count {
        let Some(header) = payload.get(cursor..cursor + 76) else {
            return false;
        };
        let read_i32 = |at| i32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        let read_u32 = |at| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        let fd = read_i32(0);
        let protocol = read_i32(4);
        let socket_type = read_i32(8);
        let bound_port = read_u32(12);
        let bound_groups = read_u32(16);
        let peer_port = read_u32(20);
        let peer_groups = read_u32(24);
        let receive_timeout_us = u64::from_le_bytes(header[28..36].try_into().unwrap());
        let send_timeout_us = u64::from_le_bytes(header[36..44].try_into().unwrap());
        let queued = read_u32(44) as usize;
        let namespace = u64::from_le_bytes(header[48..56].try_into().unwrap());
        let multicast_cursor = u64::from_le_bytes(header[56..64].try_into().unwrap());
        let multicast_overflow = read_u32(64) != 0;
        let pass_credentials = read_u32(68) != 0;
        let receive_buffer = read_i32(72);
        cursor += 76;
        if restored.contains_key(&fd)
            || crate::get(fd).is_err()
            || crate::get(fd).is_ok_and(|entry| entry.kind != FdKind::NetlinkSocket)
            || !matches!(
                protocol,
                NETLINK_ROUTE | NETLINK_NETFILTER | NETLINK_XFRM | NETLINK_KOBJECT_UEVENT
            )
            || !matches!(socket_type, SOCK_RAW | SOCK_DGRAM)
        {
            return false;
        }
        let mut queue = VecDeque::new();
        for _ in 0..queued {
            let Some(datagram_header) = payload.get(cursor..cursor + 12) else {
                return false;
            };
            let source_port = u32::from_le_bytes(datagram_header[0..4].try_into().unwrap());
            let source_groups = u32::from_le_bytes(datagram_header[4..8].try_into().unwrap());
            let length = u32::from_le_bytes(datagram_header[8..12].try_into().unwrap()) as usize;
            cursor += 12;
            let Some(bytes) = payload.get(cursor..cursor.checked_add(length).unwrap_or(usize::MAX))
            else {
                return false;
            };
            queue.push_back(Datagram {
                bytes: bytes.to_vec(),
                source: NetlinkAddress {
                    port: source_port,
                    groups: source_groups,
                },
            });
            cursor = match cursor
                .checked_add(length)
                .and_then(|value| value.checked_next_multiple_of(4))
            {
                Some(cursor) => cursor,
                None => return false,
            };
        }
        let _namespace = crate::usernet::scope(namespace);
        let Ok(item) = Endpoint::new(protocol, socket_type) else {
            return false;
        };
        let item = Arc::new(item);
        {
            let Ok(mut state) = item.state.lock() else {
                return false;
            };
            state.bound = (bound_port != 0 || bound_groups != 0).then_some(NetlinkAddress {
                port: bound_port,
                groups: bound_groups,
            });
            state.peer = (peer_port != u32::MAX).then_some(NetlinkAddress {
                port: peer_port,
                groups: peer_groups,
            });
            state.receive_timeout_us = (receive_timeout_us != 0).then_some(receive_timeout_us);
            state.multicast_cursor = multicast_cursor;
            state.multicast_overflow = multicast_overflow;
            state.send_timeout_us = (send_timeout_us != 0).then_some(send_timeout_us);
            state.pass_credentials = pass_credentials;
            state.receive_buffer = receive_buffer;
            state.queue = queue;
            if !state.queue.is_empty() && unsafe { SetEvent(item.event) } == 0 {
                return false;
            }
        }
        let Ok(entry) = crate::get(fd) else {
            return false;
        };
        item.description
            .store(entry.description_id, std::sync::atomic::Ordering::Release);
        restored.insert(fd, item);
    }
    if cursor != payload.len() {
        return false;
    }
    let Ok(mut table) = endpoints().lock() else {
        return false;
    };
    *table = restored;
    true
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn nft_cache_dumps_preserve_all_families_and_empty_object_collections() {
        let mut state = crate::nftables::Ruleset::default();
        for (family, name) in [(2, "ipv4"), (10, "ipv6")] {
            state.objects.push(crate::nftables::Object {
                message: NFT_MSG_NEWTABLE,
                family,
                handle: family as u64,
                attributes: vec![crate::nftables::Attribute {
                    kind: NFTA_TABLE_NAME,
                    value: format!("{name}\0").into_bytes(),
                }],
            });
        }
        let query = |kind, flags| {
            let mut request = Vec::new();
            append_header(&mut request, nft_message_type(kind), flags, 17, 0, 4).unwrap();
            request.extend_from_slice(&[0; 4]);
            let header = parse_header(&request).unwrap();
            netfilter_query_snapshot(header, &request, 42, &state).unwrap()
        };
        let tables = query(NFT_MSG_GETTABLE, NLM_F_REQUEST | NLM_F_DUMP);
        assert_eq!(tables.len(), 3);
        assert_eq!(tables[0][16], 2);
        assert_eq!(tables[1][16], 10);
        assert_eq!(parse_header(&tables[2]).unwrap().kind, NLMSG_DONE);
        for kind in [NFT_MSG_GETSET, NFT_MSG_GETOBJ, NFT_MSG_GETFLOWTABLE] {
            let empty = query(kind, NLM_F_REQUEST | NLM_F_DUMP);
            assert_eq!(empty.len(), 1);
            assert_eq!(parse_header(&empty[0]).unwrap().kind, NLMSG_DONE);
            let absent = query(kind, NLM_F_REQUEST);
            assert_eq!(parse_header(&absent[0]).unwrap().kind, NLMSG_ERROR);
            assert_eq!(
                i32::from_ne_bytes(absent[0][16..20].try_into().unwrap()),
                -ENOENT
            );
        }
    }

    fn sockaddr(port: u32) -> [u8; SOCKADDR_NL_LEN] {
        let mut address = [0u8; SOCKADDR_NL_LEN];
        address[..2].copy_from_slice(&(AF_NETLINK as u16).to_ne_bytes());
        address[4..8].copy_from_slice(&port.to_ne_bytes());
        address
    }

    #[test]
    fn malformed_route_frames_fail_without_a_reply() {
        let fd = socket(SOCK_RAW | SOCK_NONBLOCK, NETLINK_ROUTE).unwrap();
        let address = sockaddr(0);
        unsafe { bind(fd, address.as_ptr(), address.len() as i32) }.unwrap();
        let malformed = [0u8; 15];
        assert_eq!(
            unsafe {
                sendto(
                    fd,
                    malformed.as_ptr(),
                    malformed.len(),
                    0,
                    address.as_ptr(),
                    address.len() as i32,
                )
            },
            Err(EINVAL)
        );
        crate::close(fd).unwrap();
    }

    #[test]
    fn go_rtgenmsg_dumps_work_through_the_socket_boundary() {
        let _serial = crate::procfs::sysctl_test_lock();
        let _scope = crate::usernet::scope(1);
        let fd = socket(SOCK_RAW | SOCK_NONBLOCK, NETLINK_ROUTE).unwrap();
        let address = sockaddr(0);
        unsafe { bind(fd, address.as_ptr(), address.len() as i32) }.unwrap();
        for kind in [RTM_GETLINK, RTM_GETADDR] {
            let mut request = Vec::new();
            append_header(&mut request, kind, NLM_F_DUMP | NLM_F_REQUEST, 1, 0, 1).unwrap();
            request.push(AF_UNSPEC);
            assert_eq!(request.len(), 17);
            unsafe {
                sendto(
                    fd,
                    request.as_ptr(),
                    request.len(),
                    0,
                    address.as_ptr(),
                    address.len() as i32,
                )
            }
            .unwrap();
            let queued = endpoint(fd).unwrap();
            let mut state = queued.state.lock().unwrap();
            let replies: Vec<_> = state.queue.drain(..).collect();
            assert!(!replies.is_empty());
            for reply in replies {
                assert_ne!(parse_header(&reply.bytes).unwrap().kind, NLMSG_ERROR);
            }
        }
        crate::close(fd).unwrap();
    }

    #[test]
    fn abbreviated_headers_are_read_dump_only() {
        let mut request = Vec::new();
        append_header(
            &mut request,
            RTM_NEWLINK,
            NLM_F_REQUEST | NLM_F_DUMP,
            1,
            0,
            1,
        )
        .unwrap();
        request.push(0);
        assert_eq!(
            route_fixed::<16>(parse_header(&request).unwrap(), &request, RTM_GETLINK).unwrap_err(),
            EINVAL
        );
        request[4..6].copy_from_slice(&RTM_GETLINK.to_ne_bytes());
        request[6..8].copy_from_slice(&NLM_F_REQUEST.to_ne_bytes());
        assert_eq!(
            route_fixed::<16>(parse_header(&request).unwrap(), &request, RTM_GETLINK).unwrap_err(),
            EINVAL
        );
    }

    #[test]
    fn host_interface_configuration_is_explicitly_unsupported() {
        let _scope = crate::usernet::scope(1);
        for interface in crate::hostnet::interfaces(1).unwrap() {
            if interface.hardware_type == ARPHRD_LOOPBACK {
                continue;
            }
            let mut request = Vec::new();
            append_header(
                &mut request,
                RTM_SETLINK,
                NLM_F_REQUEST | NLM_F_ACK,
                7,
                0,
                IFINFO_MSG_LEN + 8,
            )
            .unwrap();
            request.resize(NLMSG_HEADER_LEN + IFINFO_MSG_LEN, 0);
            request[20..24].copy_from_slice(&interface.link.index.to_ne_bytes());
            append_attribute(&mut request, IFLA_MTU, &576u32.to_ne_bytes()).unwrap();
            let replies =
                handle_route_message(parse_header(&request).unwrap(), &request, 0x1234).unwrap();
            assert_eq!(
                i32::from_ne_bytes(replies[0][16..20].try_into().unwrap()),
                -EOPNOTSUPP
            );
        }
    }

    #[test]
    fn unsupported_route_message_receives_an_explicit_error() {
        let fd = socket(SOCK_RAW | SOCK_NONBLOCK, NETLINK_ROUTE).unwrap();
        let address = sockaddr(0);
        unsafe { bind(fd, address.as_ptr(), address.len() as i32) }.unwrap();
        let local = endpoint(fd).unwrap().state.lock().unwrap().bound.unwrap();
        let mut request = Vec::new();
        append_header(&mut request, 0x7777, NLM_F_REQUEST, 41, 0, IFINFO_MSG_LEN).unwrap();
        request.resize(NLMSG_HEADER_LEN + IFINFO_MSG_LEN, 0);
        unsafe {
            sendto(
                fd,
                request.as_ptr(),
                request.len(),
                0,
                address.as_ptr(),
                address.len() as i32,
            )
        }
        .unwrap();
        let queued = endpoint(fd).unwrap();
        let state = queued.state.lock().unwrap();
        let response = state.queue.front().unwrap();
        let header = parse_header(&response.bytes).unwrap();
        assert_eq!(header.kind, NLMSG_ERROR);
        assert_eq!(header.sequence, 41);
        assert_eq!(header.port, local.port);
        assert_eq!(
            i32::from_ne_bytes(response.bytes[16..20].try_into().unwrap()),
            -EOPNOTSUPP
        );
        drop(state);
        crate::close(fd).unwrap();
    }

    fn compat_request(name: &str, revision: u32, extension_type: u32) -> Vec<u8> {
        let mut payload = vec![2, NFNETLINK_V0, 0, 0];
        let mut terminated_name = name.as_bytes().to_vec();
        terminated_name.push(0);
        append_attribute(&mut payload, NFTA_COMPAT_NAME, &terminated_name).unwrap();
        append_attribute(&mut payload, NFTA_COMPAT_REV, &revision.to_be_bytes()).unwrap();
        append_attribute(
            &mut payload,
            NFTA_COMPAT_TYPE,
            &extension_type.to_be_bytes(),
        )
        .unwrap();
        let mut request = Vec::new();
        append_header(
            &mut request,
            (NFNL_SUBSYS_NFT_COMPAT << 8) | NFNL_MSG_COMPAT_GET,
            NLM_F_REQUEST | NLM_F_ACK,
            93,
            0,
            payload.len(),
        )
        .unwrap();
        request.extend_from_slice(&payload);
        request
    }

    #[test]
    fn nft_compat_tcp_udp_versions_do_not_wrap_or_accept_unknown_targets() {
        for family in [2u8, 10] {
            for name in ["tcp", "udp"] {
                for (version, target, valid) in
                    [(0, 0, true), (1, 0, false), (256, 0, false), (0, 1, false)]
                {
                    let mut request = compat_request(name, version, target);
                    request[NLMSG_HEADER_LEN] = family;
                    let header = parse_header(&request).unwrap();
                    let replies = handle_compat_query(header, &request, 0x1234).unwrap();
                    assert_eq!(
                        parse_header(&replies[0]).unwrap().kind,
                        if valid { header.kind } else { NLMSG_ERROR }
                    );
                    if !valid {
                        assert_eq!(
                            i32::from_ne_bytes(replies[0][16..20].try_into().unwrap()),
                            -ENOENT
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn nft_compat_nat_revisions_respect_family_and_extension_type() {
        for family in [2u8, 10, 1] {
            for name in ["DNAT", "SNAT"] {
                for version in [0, 1, 2, 3, 256] {
                    for target in [0, 1] {
                        let mut request = compat_request(name, version, target);
                        request[NLMSG_HEADER_LEN] = family;
                        let header = parse_header(&request).unwrap();
                        let replies = handle_compat_query(header, &request, 0x1234).unwrap();
                        let valid = target == 1
                            && version <= 2
                            && (family == 2 || (family == 10 && version != 0));
                        let reply_header = parse_header(&replies[0]).unwrap();
                        if valid {
                            assert_eq!(reply_header.kind, header.kind);
                            let (_, attributes) = parse_nft_message(
                                MessageHeader {
                                    flags: NLM_F_REQUEST,
                                    ..reply_header
                                },
                                &replies[0],
                            )
                            .unwrap();
                            assert_eq!(
                                nft_attribute(&attributes, NFTA_COMPAT_REV),
                                Some(&2u32.to_be_bytes()[..])
                            );
                        } else {
                            assert_eq!(reply_header.kind, NLMSG_ERROR);
                            assert_eq!(
                                i32::from_ne_bytes(replies[0][16..20].try_into().unwrap()),
                                -ENOENT
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn nft_compat_reports_the_registered_conntrack_revision_set() {
        for requested in 1..=3 {
            let request = compat_request("conntrack", requested, 0);
            let header = parse_header(&request).unwrap();
            let replies = handle_compat_query(header, &request, 0x1234).unwrap();
            assert_eq!(replies.len(), 2);
            let reply_header = parse_header(&replies[0]).unwrap();
            assert_eq!(reply_header.kind, header.kind);
            assert_eq!(reply_header.flags, NLM_F_MULTI);
            let (family, attributes) = parse_nft_message(
                MessageHeader {
                    flags: NLM_F_REQUEST,
                    ..reply_header
                },
                &replies[0],
            )
            .unwrap();
            assert_eq!(family, 2);
            assert_eq!(
                nft_string(&attributes, NFTA_COMPAT_NAME).unwrap(),
                "conntrack"
            );
            assert_eq!(
                nft_attribute(&attributes, NFTA_COMPAT_REV),
                Some(&3u32.to_be_bytes()[..])
            );
            assert_eq!(
                nft_attribute(&attributes, NFTA_COMPAT_TYPE),
                Some(&0u32.to_be_bytes()[..])
            );
            assert_eq!(parse_header(&replies[1]).unwrap().kind, NLMSG_ERROR);
            assert_eq!(
                i32::from_ne_bytes(
                    replies[1][NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4]
                        .try_into()
                        .unwrap()
                ),
                0
            );
        }

        let request = compat_request("conntrack", 0, 0);
        let header = parse_header(&request).unwrap();
        let replies = handle_compat_query(header, &request, 0x1234).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(parse_header(&replies[0]).unwrap().kind, NLMSG_ERROR);
        assert_eq!(
            i32::from_ne_bytes(
                replies[0][NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4]
                    .try_into()
                    .unwrap()
            ),
            -ENOENT
        );

        let request = compat_request("conntrack", 3, 2);
        let header = parse_header(&request).unwrap();
        let replies = handle_compat_query(header, &request, 0x1234).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(
            i32::from_ne_bytes(
                replies[0][NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4]
                    .try_into()
                    .unwrap()
            ),
            -EINVAL
        );
    }

    #[test]
    fn route_response_queue_is_epoll_readable_and_clears_after_recv() {
        use crate::epoll::{
            EPOLL_CTL_ADD, EPOLLIN, EpollEvent, epoll_create1, epoll_ctl, epoll_wait,
        };

        let fd = socket(SOCK_RAW | SOCK_NONBLOCK, NETLINK_ROUTE).unwrap();
        let address = sockaddr(0);
        unsafe { bind(fd, address.as_ptr(), address.len() as i32) }.unwrap();
        let epoll_fd = epoll_create1(0).unwrap();
        epoll_ctl(
            epoll_fd,
            EPOLL_CTL_ADD,
            fd,
            Some(EpollEvent {
                events: EPOLLIN,
                data: 0x4e4c,
            }),
        )
        .unwrap();

        let mut events = [EpollEvent::default(); 2];
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        let mut request = Vec::new();
        append_header(&mut request, 0x7777, NLM_F_REQUEST, 77, 0, IFINFO_MSG_LEN).unwrap();
        request.resize(NLMSG_HEADER_LEN + IFINFO_MSG_LEN, 0);
        unsafe {
            sendto(
                fd,
                request.as_ptr(),
                request.len(),
                0,
                address.as_ptr(),
                address.len() as i32,
            )
        }
        .unwrap();

        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 1);
        assert_eq!(events[0].events & EPOLLIN, EPOLLIN);
        let cookie = events[0].data;
        assert_eq!(cookie, 0x4e4c);

        let mut reply = [0u8; 256];
        unsafe { recv(fd, reply.as_mut_ptr(), reply.len(), 0) }.unwrap();
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        crate::close(epoll_fd).unwrap();
        crate::close(fd).unwrap();
    }

    #[test]
    fn initial_loopback_accepts_only_observed_flag_ensures() {
        let _scope = crate::usernet::scope(1);
        let loopback = host_loopback().unwrap();
        let request = |kind: u16, flags: u32, extra: bool| {
            let mut bytes = Vec::new();
            append_header(
                &mut bytes,
                kind,
                NLM_F_REQUEST | NLM_F_ACK,
                81,
                0,
                IFINFO_MSG_LEN,
            )
            .unwrap();
            bytes.resize(NLMSG_HEADER_LEN + IFINFO_MSG_LEN, 0);
            bytes[20..24].copy_from_slice(&loopback.index.to_ne_bytes());
            bytes[24..28].copy_from_slice(&flags.to_ne_bytes());
            bytes[28..32].copy_from_slice(&IFF_UP.to_ne_bytes());
            if extra {
                bytes.extend_from_slice(&8u16.to_ne_bytes());
                bytes.extend_from_slice(&IFLA_MTU.to_ne_bytes());
                bytes.extend_from_slice(&loopback.mtu.to_ne_bytes());
                let length = bytes.len() as u32;
                bytes[..4].copy_from_slice(&length.to_ne_bytes());
            }
            bytes
        };
        for (kind, flags, extra, expected) in [
            (RTM_NEWLINK, loopback.flags, false, 0),
            (RTM_SETLINK, loopback.flags, false, 0),
            (RTM_NEWLINK, loopback.flags ^ IFF_UP, false, -EOPNOTSUPP),
            (RTM_NEWLINK, loopback.flags, true, -EOPNOTSUPP),
            (RTM_DELLINK, loopback.flags, false, -EOPNOTSUPP),
        ] {
            let bytes = request(kind, flags, extra);
            let replies =
                handle_route_message(parse_header(&bytes).unwrap(), &bytes, 0x1234).unwrap();
            assert_eq!(
                i32::from_ne_bytes(replies[0][16..20].try_into().unwrap()),
                expected
            );
        }
        assert_eq!(host_loopback().unwrap().flags, loopback.flags);
    }

    #[test]
    fn docker_bridge_newlink_frame_is_accepted_verbatim() {
        // A real RTM_NEWLINK emitted by current libnetwork.  In particular, the
        // empty IFLA_INFO_DATA after IFLA_INFO_KIND is a valid nested attribute,
        // not malformed padding.
        crate::route_state::transaction(|state| {
            state.links.retain(|link| link.name != "docker0");
            Ok::<(), i32>(())
        })
        .unwrap()
        .unwrap();
        let request = [
            0x54, 0x00, 0x00, 0x00, 0x10, 0x00, 0x05, 0x06, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x03, 0x00, b'd', b'o', b'c', b'k', b'e', b'r',
            b'0', 0x00, 0x08, 0x00, 0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x01, 0x00,
            0xea, 0x23, 0x1f, 0x10, 0x3c, 0x30, 0x00, 0x00, 0x14, 0x00, 0x12, 0x00, 0x0a, 0x00,
            0x01, 0x00, b'b', b'r', b'i', b'd', b'g', b'e', 0x00, 0x00, 0x04, 0x00, 0x02, 0x00,
        ];
        let header = parse_header(&request).unwrap();
        let replies = handle_route_message(header, &request, 0x1234).unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(parse_header(&replies[0]).unwrap().kind, NLMSG_ERROR);
        assert_eq!(
            i32::from_ne_bytes(
                replies[0][NLMSG_HEADER_LEN..NLMSG_HEADER_LEN + 4]
                    .try_into()
                    .unwrap()
            ),
            0
        );

        let link = crate::route_state::snapshot()
            .unwrap()
            .links
            .into_iter()
            .find(|link| link.name == "docker0")
            .unwrap();
        let reply = stored_link_reply(header, 0x1234, &link, false).unwrap();
        let attributes = route_attributes(&reply[NLMSG_HEADER_LEN + IFINFO_MSG_LEN..]).unwrap();
        let link_info =
            route_attributes(route_attribute(&attributes, IFLA_LINKINFO).unwrap()).unwrap();
        assert_eq!(
            route_attribute(&link_info, IFLA_INFO_KIND),
            Some(&b"bridge\0"[..])
        );
        crate::route_state::transaction(|state| {
            state.links.retain(|link| link.name != "docker0");
            Ok::<(), i32>(())
        })
        .unwrap()
        .unwrap();
    }

    #[test]
    fn netlink_xfrm_socket_creation_succeeds() {
        let fd =
            socket(SOCK_RAW | SOCK_NONBLOCK, NETLINK_XFRM).expect("NETLINK_XFRM socket failed");
        assert!(is_netlink_socket(fd));
        crate::close(fd).unwrap();
    }
}

fn move_link(source: u64, target: u64, index: u32, name: Option<&str>) -> Result<(), i32> {
    if source == target {
        return Ok(());
    }
    if !crate::user_namespace::capable(crate::namespaces::network_owner(target)?, 12) {
        return Err(crate::EPERM);
    }
    let _guard = crate::mount::shared::topology_guard()?;
    let original = crate::route_state::snapshot_for(source)?;
    let Some(mut link) = original
        .links
        .iter()
        .find(|l| {
            if index != 0 {
                l.index == index
            } else {
                name == Some(l.name.as_str())
            }
        })
        .cloned()
    else {
        return Err(ENODEV);
    };
    if link.kind != "veth" && link.kind != "dummy" {
        return Err(EOPNOTSUPP);
    }
    let link_index = link.index;
    link.master = 0;
    if let Some(name) = name {
        if !valid_link_name(name) {
            return Err(EINVAL);
        }
        link.name = name.to_owned();
    }
    if crate::hostnet::interfaces(target)?
        .iter()
        .any(|i| i.link.index == link.index || i.link.name == link.name)
    {
        return Err(EEXIST);
    }
    let destination = crate::route_state::snapshot_for(target)?;
    if destination
        .links
        .iter()
        .any(|l| l.name == link.name || l.index == link.index)
    {
        return Err(EEXIST);
    }
    crate::route_state::transaction_for(target, |state| {
        state.links.push(link);
        state.addresses.extend(
            original
                .addresses
                .iter()
                .filter(|a| a.index == link_index)
                .cloned(),
        );
        Ok::<_, i32>(())
    })??;
    let result = crate::route_state::transaction_for(source, |state| {
        state.links.retain(|l| l.index != link_index);
        state.addresses.retain(|a| a.index != link_index);
        state
            .routes
            .retain(|r| route_u32(&r.attributes, RTA_OIF).ok().flatten() != Some(link_index));
        Ok::<_, i32>(())
    });
    if let Err(e) = result.and_then(|r| r) {
        let _ = crate::route_state::transaction_for(target, |s| {
            *s = destination;
            Ok::<_, i32>(())
        });
        return Err(e);
    }
    Ok(())
}

/// Shared topology for netlink, sysfs, and the measured subset in procfs.
pub(crate) fn network_interfaces(netns: u64) -> Result<Vec<crate::hostnet::Interface>, i32> {
    let state = crate::route_state::snapshot_for(netns)?;
    let mut interfaces = crate::hostnet::interfaces(netns)?;
    if netns != 1 {
        interfaces.push(crate::hostnet::Interface {
            link: crate::route_state::Link {
                index: 1,
                name: "lo".into(),
                kind: "loopback".into(),
                flags: state.loopback_flags,
                mtu: 65536,
                address: vec![0; 6],
                broadcast: vec![0; 6],
                master: 0,
                peer: 0,
                attributes: Vec::new(),
            },
            hardware_type: ARPHRD_LOOPBACK,
            operstate: if state.loopback_flags & IFF_UP != 0 {
                IF_OPER_UP
            } else {
                IF_OPER_DOWN
            },
            host: false,
            stats: None,
        });
    }
    interfaces.extend(
        state
            .links
            .into_iter()
            .map(|link| crate::hostnet::Interface {
                operstate: if link.flags & IFF_UP != 0 {
                    IF_OPER_UP
                } else {
                    IF_OPER_DOWN
                },
                link,
                hardware_type: ARPHRD_ETHER,
                host: false,
                stats: None,
            }),
    );
    crate::hostnet::validate_unique(&interfaces)?;
    Ok(interfaces)
}
