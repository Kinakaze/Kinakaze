//! Name resolution, address presentation and scatter/gather socket messages.
//!
//! Three kinds of work happen here, and they carry very different amounts of
//! risk.
//!
//! The address-presentation functions — `inet_pton`, `inet_ntop`, `inet_aton`
//! and `inet_ntoa` — are written out longhand rather than forwarded to Winsock.
//! That is deliberate. `inet_aton` is specified to accept the historical short
//! and non-decimal forms (`127.1`, `0x7f000001`, `0177.0.0.1`) that Windows'
//! `inet_addr` rejects, and BusyBox's `ping` and `ifconfig` hand them straight
//! through from the command line. `inet_ntop` has to emit the RFC 5952
//! canonical form for IPv6, which Windows' own formatter does not always agree
//! with. All four are pure byte arithmetic with no platform behind them, so
//! implementing them here makes them exactly testable and takes the host out of
//! the answer entirely.
//!
//! The resolver functions forward to Winsock but translate in both directions.
//! `struct addrinfo` is the trap: Linux orders it `ai_addr` then
//! `ai_canonname`, Windows' `ADDRINFOW` orders it `ai_canonname` then `ai_addr`,
//! and Windows widens `ai_addrlen` from `socklen_t` to `size_t`. The two structs
//! are the same size, so handing a Linux `addrinfo` to Windows compiles, links,
//! runs, and quietly swaps a pointer for a string. Every field crossing that
//! boundary is copied out by name below, never memcpy'd.
//!
//! Service lookup and enumeration share the guest `/etc/services` database.
//! Missing files remain errors; host assignments and built-in defaults are not used.
//! Database ownership, parsing and the servent ABI live in `services`.
//!
//! Unix ancillary records are carried separately from the payload stream.

use core::ffi::{CStr, c_char, c_int, c_uint, c_void};
use core::ptr;
mod netgroup;
use std::cell::RefCell;
use std::ffi::CString;
use std::sync::OnceLock;

use kinakaze_vfs::socket::{self, AF_INET, AF_INET6, AF_UNSPEC, SOCK_DGRAM, SOCK_STREAM};
use kinakaze_vfs::{EAFNOSUPPORT, EFAULT, EINVAL, EMSGSIZE, ENOMEM, ENOSPC, EOPNOTSUPP};

use crate::set_errno;

// ---------------------------------------------------------------------------
// Linux ABI constants.
//
// Every number here is what the guest was compiled against. Where the Windows
// value differs it is named at the translation site rather than here, so the two
// spellings are never one constant that has to be remembered as ambiguous.
// ---------------------------------------------------------------------------

/// `getaddrinfo` failure codes, as Linux numbers them.
///
/// Windows returns Winsock codes from `GetAddrInfoW` instead, in the 11001..
/// range. [`eai_from_wsa`] is the only place the two meet.
pub const EAI_BADFLAGS: c_int = -1;
pub const EAI_NONAME: c_int = -2;
pub const EAI_AGAIN: c_int = -3;
pub const EAI_FAIL: c_int = -4;
/// Deprecated on Linux but still defined, and `gethostbyname` needs to tell
/// "no such name" apart from "name exists with no address of this type".
pub const EAI_NODATA: c_int = -5;
pub const EAI_FAMILY: c_int = -6;
pub const EAI_SOCKTYPE: c_int = -7;
pub const EAI_SERVICE: c_int = -8;
pub const EAI_MEMORY: c_int = -10;
pub const EAI_SYSTEM: c_int = -11;
pub const EAI_OVERFLOW: c_int = -12;

/// Linux `AI_*` hint flags. Only the first three share Windows' values.
pub const AI_PASSIVE: c_int = 0x0001;
pub const AI_CANONNAME: c_int = 0x0002;
pub const AI_NUMERICHOST: c_int = 0x0004;
pub const AI_V4MAPPED: c_int = 0x0008;
pub const AI_ALL: c_int = 0x0010;
pub const AI_ADDRCONFIG: c_int = 0x0020;
pub const AI_IDN: c_int = 0x0040;
pub const AI_CANONIDN: c_int = 0x0080;
/// Deprecated GNU extension, retained as part of the glibc ABI.
pub const AI_IDN_ALLOW_UNASSIGNED: c_int = 0x0100;
/// Deprecated GNU extension, retained as part of the glibc ABI.
pub const AI_IDN_USE_STD3_ASCII_RULES: c_int = 0x0200;
pub const AI_NUMERICSERV: c_int = 0x0400;
/// Every flag this layer recognizes; anything else is `EAI_BADFLAGS`.
const AI_KNOWN: c_int = AI_PASSIVE
    | AI_CANONNAME
    | AI_NUMERICHOST
    | AI_V4MAPPED
    | AI_ALL
    | AI_ADDRCONFIG
    | AI_IDN
    | AI_CANONIDN
    | AI_IDN_ALLOW_UNASSIGNED
    | AI_IDN_USE_STD3_ASCII_RULES
    | AI_NUMERICSERV;

/// Linux `NI_*` flags. Windows assigns these the same five bits in a different
/// order, so passing them through would silently swap `NI_NUMERICHOST` for
/// `NI_NOFQDN`.
pub const NI_NUMERICHOST: c_int = 1;
pub const NI_NUMERICSERV: c_int = 2;
pub const NI_NOFQDN: c_int = 4;
pub const NI_NAMEREQD: c_int = 8;
pub const NI_DGRAM: c_int = 16;

/// `h_errno` values, which are a separate namespace from `errno`.
pub const HOST_NOT_FOUND: c_int = 1;
pub const TRY_AGAIN: c_int = 2;
pub const NO_RECOVERY: c_int = 3;
pub const NO_DATA: c_int = 4;

/// `getifaddrs` interface flags, from Linux's `<net/if.h>`.
pub const IFF_UP: c_uint = 0x1;
pub const IFF_BROADCAST: c_uint = 0x2;
pub const IFF_LOOPBACK: c_uint = 0x8;
pub const IFF_POINTOPOINT: c_uint = 0x10;
pub const IFF_RUNNING: c_uint = 0x40;
pub const IFF_MULTICAST: c_uint = 0x1000;

/// `cmsg_level` values. `SOL_SOCKET` is 1 on Linux; Windows spells it 0xffff.
pub const SOL_SOCKET: c_int = 1;
/// Descriptor passing through Unix sockets.
pub const SCM_RIGHTS: c_int = 1;
/// Peer credential passing. Refused for the same reason.
pub const SCM_CREDENTIALS: c_int = 2;

/// `recvmsg` result flags. `MSG_TRUNC` and `MSG_CTRUNC` are reported.
pub const MSG_TRUNC: c_int = 0x20;
pub const MSG_CTRUNC: c_int = 0x08;

/// `ENODEV`, which `if_nametoindex` reports for a name no interface carries.
const ENODEV: i32 = 19;

// ---------------------------------------------------------------------------
// Linux x86_64 struct layouts.
//
// Guest code reads these offsets directly, so each one is stated as a byte
// budget and pinned by a test. A wrong offset here is not a compile error; it is
// a pointer read as an integer.
// ---------------------------------------------------------------------------

/// Linux `struct sockaddr_in`, exactly 16 bytes.
///
/// ```text
///  0  u16 sin_family    host order
///  2  u16 sin_port      NETWORK order
///  4  u32 sin_addr      NETWORK order
///  8  u8  sin_zero[8]
/// ```
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SockaddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

/// Linux `struct sockaddr_in6`, exactly 28 bytes.
///
/// ```text
///  0  u16 sin6_family    host order
///  2  u16 sin6_port      NETWORK order
///  4  u32 sin6_flowinfo
///  8  u8  sin6_addr[16]  wire order
/// 24  u32 sin6_scope_id
/// ```
///
/// 28 is not a multiple of the 4-byte alignment padding rule people expect for
/// this struct: it really is 28, because every member is at most 4-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SockaddrIn6 {
    pub sin6_family: u16,
    pub sin6_port: u16,
    pub sin6_flowinfo: u32,
    pub sin6_addr: [u8; 16],
    pub sin6_scope_id: u32,
}

/// Linux `struct addrinfo`, 48 bytes.
///
/// ```text
///  0  i32       ai_flags
///  4  i32       ai_family
///  8  i32       ai_socktype
/// 12  i32       ai_protocol
/// 16  u32       ai_addrlen     socklen_t, NOT size_t
/// 20  (4 bytes padding)
/// 24  sockaddr* ai_addr        <-- BEFORE ai_canonname
/// 32  char*     ai_canonname
/// 40  addrinfo* ai_next
/// ```
///
/// Windows' `ADDRINFOW` is also 48 bytes and also 8-aligned, and its
/// `ai_canonname` sits at offset 24 with `ai_addr` at 32. The sizes matching is
/// what makes the mistake silent, so nothing in this module ever copies one onto
/// the other.
#[repr(C)]
pub struct AddrInfo {
    pub ai_flags: c_int,
    pub ai_family: c_int,
    pub ai_socktype: c_int,
    pub ai_protocol: c_int,
    pub ai_addrlen: u32,
    pub ai_addr: *mut c_void,
    pub ai_canonname: *mut c_char,
    pub ai_next: *mut AddrInfo,
}

/// Linux `struct hostent`, 32 bytes.
///
/// ```text
///  0  char*   h_name
///  8  char**  h_aliases
/// 16  i32     h_addrtype
/// 20  i32     h_length
/// 24  char**  h_addr_list
/// ```
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Hostent {
    pub h_name: *mut c_char,
    pub h_aliases: *mut *mut c_char,
    pub h_addrtype: c_int,
    pub h_length: c_int,
    pub h_addr_list: *mut *mut c_char,
}

/// Linux `struct servent`, 32 bytes.
///
/// ```text
///  0  char*   s_name
///  8  char**  s_aliases
/// 16  i32     s_port      NETWORK order
/// 20  (4 bytes padding)
/// 24  char*   s_proto
/// ```
///
/// `s_port` being network order in an `int` field is a real wart of the
/// interface: the value stored is `htons(port)` widened to 32 bits, so on a
/// little-endian host the number a debugger prints is byte-swapped.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Servent {
    pub s_name: *mut c_char,
    pub s_aliases: *mut *mut c_char,
    pub s_port: c_int,
    pub s_proto: *mut c_char,
}

/// Linux `struct iovec`: base first, then length.
///
/// Windows' `WSABUF` is the other way round — `{ u32 len; char *buf; }` — which
/// is why [`gather`] and [`scatter`] never reinterpret one as the other.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoVec {
    pub iov_base: *mut c_void,
    pub iov_len: usize,
}

/// Linux x86_64 `struct msghdr`, 56 bytes.
///
/// ```text
///  0  void*   msg_name
///  8  u32     msg_namelen
/// 12  (4 bytes padding)          <-- easy to miss
/// 16  iovec*  msg_iov
/// 24  usize   msg_iovlen         size_t on Linux x86_64, int elsewhere
/// 32  void*   msg_control
/// 40  usize   msg_controllen     size_t, not int
/// 48  i32     msg_flags
/// 52  (4 bytes tail padding)
/// ```
///
/// Windows' `WSAMSG` is a different struct entirely: its `dwBufferCount` and
/// `Control` length are `u32`, and it carries no `msg_flags` in the same place.
#[repr(C)]
pub struct MsgHdr {
    pub msg_name: *mut c_void,
    pub msg_namelen: u32,
    pub msg_iov: *mut IoVec,
    pub msg_iovlen: usize,
    pub msg_control: *mut c_void,
    pub msg_controllen: usize,
    pub msg_flags: c_int,
}

/// Linux `struct cmsghdr`, 16 bytes, followed by data aligned to `size_t`.
///
/// ```text
///  0  usize cmsg_len    header + data, NOT padded
///  8  i32   cmsg_level
/// 12  i32   cmsg_type
/// 16  data...
/// ```
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CmsgHdr {
    pub cmsg_len: usize,
    pub cmsg_level: c_int,
    pub cmsg_type: c_int,
}

/// Linux `struct ifaddrs`, 56 bytes.
///
/// ```text
///  0  ifaddrs*  ifa_next
///  8  char*     ifa_name
/// 16  u32       ifa_flags
/// 20  (4 bytes padding)
/// 24  sockaddr* ifa_addr
/// 32  sockaddr* ifa_netmask
/// 40  sockaddr* ifa_ifu       broadcast or destination, a union on Linux
/// 48  void*     ifa_data
/// ```
#[repr(C)]
pub struct Ifaddrs {
    pub ifa_next: *mut Ifaddrs,
    pub ifa_name: *mut c_char,
    pub ifa_flags: c_uint,
    pub ifa_addr: *mut c_void,
    pub ifa_netmask: *mut c_void,
    pub ifa_ifu: *mut c_void,
    pub ifa_data: *mut c_void,
}

// ---------------------------------------------------------------------------
// Win32 entry points.
//
// Declared inline, as `crate::userdb` does, because the crate's `windows-sys`
// feature set covers `Win32_System_Threading` only and these live in
// `Win32_Networking_WinSock` and `Win32_NetworkManagement_IpHelper`. Widening
// that feature list would mean editing the shared manifest.
// ---------------------------------------------------------------------------

/// Windows `ADDRINFOW`, the wide-character result of `GetAddrInfoW`.
///
/// The field order is the whole reason this type exists separately:
/// `ai_canonname` is at offset 24 and `ai_addr` at 32, the reverse of Linux.
/// `ai_addrlen` is a `size_t` here, so it occupies offsets 16..24 with no
/// padding, where Linux has a `u32` and four bytes of padding.
#[repr(C)]
#[derive(Clone, Copy)]
struct AddrInfoW {
    ai_flags: c_int,
    ai_family: c_int,
    ai_socktype: c_int,
    ai_protocol: c_int,
    /// `size_t` on Windows, `socklen_t` on Linux.
    ai_addrlen: usize,
    /// Offset 24. Linux puts `ai_addr` here.
    ai_canonname: *mut u16,
    /// Offset 32. Linux puts `ai_canonname` here.
    ai_addr: *mut c_void,
    ai_next: *mut AddrInfoW,
}

/// Windows `SOCKET_ADDRESS`: a pointer to a `sockaddr` and its length.
#[repr(C)]
#[derive(Clone, Copy)]
struct SocketAddress {
    sockaddr: *mut c_void,
    sockaddr_length: c_int,
}

/// Windows `IP_ADAPTER_UNICAST_ADDRESS_LH`.
///
/// ```text
///  0  u32           Length
///  4  u32           Flags
///  8  self*         Next
/// 16  SOCKET_ADDRESS Address           (16 bytes)
/// 32  i32           PrefixOrigin
/// 36  i32           SuffixOrigin
/// 40  i32           DadState
/// 44  u32           ValidLifetime
/// 48  u32           PreferredLifetime
/// 52  u32           LeaseLifetime
/// 56  u8            OnLinkPrefixLength
/// ```
#[repr(C)]
struct IpAdapterUnicastAddress {
    length: u32,
    flags: u32,
    next: *mut IpAdapterUnicastAddress,
    address: SocketAddress,
    prefix_origin: c_int,
    suffix_origin: c_int,
    dad_state: c_int,
    valid_lifetime: u32,
    preferred_lifetime: u32,
    lease_lifetime: u32,
    /// The netmask, expressed as a prefix length. Windows reports no mask
    /// directly, so [`mask_from_prefix`] reconstructs one.
    on_link_prefix_length: u8,
}

/// Windows `IP_ADAPTER_ADDRESSES_LH`, truncated after the last field read.
///
/// The real struct continues to 448 bytes on x64. Only the prefix up to
/// `FirstPrefix` is described because that is all this module reads, and the
/// buffer is owned by the OS: a short declaration reads the same bytes at the
/// same offsets, it simply names fewer of them. `Next` at offset 8 is what makes
/// the truncation safe, since traversal never needs the tail.
///
/// ```text
///   0  u32     Length
///   4  u32     IfIndex            <-- the index if_nametoindex reports
///   8  self*   Next
///  16  char*   AdapterName        GUID string, ANSI
///  24  ptr     FirstUnicastAddress
///  32  ptr     FirstAnycastAddress
///  40  ptr     FirstMulticastAddress
///  48  ptr     FirstDnsServerAddress
///  56  u16*    DnsSuffix
///  64  u16*    Description
///  72  u16*    FriendlyName
///  80  u8      PhysicalAddress[8]  <-- the MAC gethostid derives from
///  88  u32     PhysicalAddressLength
///  92  u32     Flags
///  96  u32     Mtu
/// 100  u32     IfType             <-- IANA type; 24 is loopback
/// 104  i32     OperStatus         <-- 1 is IfOperStatusUp
/// 108  u32     Ipv6IfIndex
/// 112  u32     ZoneIndices[16]
/// 176  ptr     FirstPrefix
/// ```
#[repr(C)]
struct IpAdapterAddresses {
    length: u32,
    if_index: u32,
    next: *mut IpAdapterAddresses,
    adapter_name: *mut c_char,
    first_unicast_address: *mut IpAdapterUnicastAddress,
    first_anycast_address: *mut c_void,
    first_multicast_address: *mut c_void,
    first_dns_server_address: *mut c_void,
    dns_suffix: *mut u16,
    description: *mut u16,
    friendly_name: *mut u16,
    physical_address: [u8; 8],
    physical_address_length: u32,
    flags: u32,
    mtu: u32,
    if_type: u32,
    oper_status: c_int,
    ipv6_if_index: u32,
    zone_indices: [u32; 16],
    first_prefix: *mut c_void,
}

// IANA interface types, as `IfType` reports them.
const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
const IF_TYPE_PPP: u32 = 23;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
const IF_TYPE_IEEE80211: u32 = 71;
const IF_TYPE_TUNNEL: u32 = 131;

/// `IfOperStatusUp`, the only `OperStatus` value that means the link is usable.
const IF_OPER_STATUS_UP: c_int = 1;

/// `IP_ADAPTER_NO_MULTICAST`: set when the adapter cannot do multicast, so
/// `IFF_MULTICAST` is its absence rather than its presence.
const IP_ADAPTER_NO_MULTICAST: u32 = 0x0010;

/// `GetAdaptersAddresses` filters. Anycast, multicast and DNS entries are asked
/// for by their absence: Linux `getifaddrs` reports unicast addresses only.
const GAA_FLAG_SKIP_ANYCAST: u32 = 0x0002;
const GAA_FLAG_SKIP_MULTICAST: u32 = 0x0004;
const GAA_FLAG_SKIP_DNS_SERVER: u32 = 0x0008;

/// `ERROR_BUFFER_OVERFLOW`, which `GetAdaptersAddresses` returns with the
/// required size so the caller can retry.
const ERROR_BUFFER_OVERFLOW: u32 = 111;
const ERROR_SUCCESS: u32 = 0;

#[link(name = "ws2_32")]
unsafe extern "system" {
    /// `WSAStartup`. Winsock is reference-counted per process, so calling it
    /// here as well as in the socket layer is correct rather than redundant: a
    /// guest may resolve a name before it ever creates a socket, and
    /// `GetAddrInfoW` fails with `WSANOTINITIALISED` if nothing started Winsock.
    /// The socket layer's own initializer is private to that crate.
    fn WSAStartup(version: u16, data: *mut u8) -> c_int;
    /// `GetAddrInfoW`. Returns an EAI-shaped code directly, not through
    /// `WSAGetLastError`, and the codes are Winsock's own.
    fn GetAddrInfoW(
        node: *const u16,
        service: *const u16,
        hints: *const AddrInfoW,
        result: *mut *mut AddrInfoW,
    ) -> c_int;
    /// `FreeAddrInfoW`, which releases what `GetAddrInfoW` allocated. The guest
    /// never sees these blocks; they are translated and freed inside one call.
    fn FreeAddrInfoW(info: *mut AddrInfoW);
    /// `GetNameInfoW`. `socklen_t` is a plain `int` on Windows.
    fn GetNameInfoW(
        address: *const c_void,
        address_length: c_int,
        host: *mut u16,
        host_length: u32,
        service: *mut u16,
        service_length: u32,
        flags: c_int,
    ) -> c_int;
}

#[link(name = "iphlpapi")]
unsafe extern "system" {
    /// `GetAdaptersAddresses`: the only supported way to enumerate interfaces
    /// with their addresses on a modern Windows.
    fn GetAdaptersAddresses(
        family: u32,
        flags: u32,
        reserved: *mut c_void,
        addresses: *mut IpAdapterAddresses,
        size: *mut u32,
    ) -> u32;
}

unsafe extern "sysv64" {
    /// `__h_errno_location`, defined in [`crate::startup`].
    ///
    /// Referenced through its exported name rather than a Rust path because the
    /// definition sits in a private `windows` submodule there. Going through the
    /// symbol guarantees this module writes the very slot the guest reads; a
    /// second thread-local would be a different variable with the same purpose,
    /// which is the bug this avoids.
    #[link_name = "kinakaze_abi___h_errno_location"]
    fn h_errno_location() -> *mut c_int;
}

/// Sets `h_errno`, the resolver's error variable.
fn set_h_errno(value: c_int) {
    // SAFETY: the callee returns a pointer to this thread's own live slot.
    unsafe { *h_errno_location() = value };
}

/// Initializes Winsock once for this module's own use. See [`WSAStartup`].
fn ensure_winsock() -> bool {
    static STARTED: OnceLock<bool> = OnceLock::new();
    *STARTED.get_or_init(|| {
        // WSADATA is 408 bytes on x64. The contents are not read, so an
        // oversized zeroed buffer is passed rather than restating the layout.
        let mut data = [0u8; 512];
        // SAFETY: 2.2 is supported by every Windows this targets and the buffer
        // exceeds the size the callee writes.
        unsafe { WSAStartup(0x0202, data.as_mut_ptr()) == 0 }
    })
}

/// Encodes a Rust string as a NUL-terminated UTF-16 buffer for the W APIs.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Decodes a NUL-terminated UTF-16 buffer a W API wrote.
///
/// # Safety
///
/// `text` must be null or a NUL-terminated UTF-16 string.
unsafe fn from_wide(text: *const u16) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut length = 0;
    // SAFETY: the caller guarantees a terminator, which bounds this walk.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units precede the terminator just found.
    let units = unsafe { core::slice::from_raw_parts(text, length) };
    Some(String::from_utf16_lossy(units))
}

// ---------------------------------------------------------------------------
// Address presentation, done by hand.
//
// Nothing in this section calls the platform. Every function is a total mapping
// from bytes to bytes, which is what makes the malformed cases testable rather
// than dependent on whichever resolver happens to be installed.
// ---------------------------------------------------------------------------

/// Parses one `inet_aton` component: hex `0x…`, octal `0…`, or decimal.
///
/// This is the historical part of the interface. `inet_pton` accepts none of
/// these forms; `inet_aton` must accept all of them, because `ping 0x7f000001`
/// and `route add 010.0.0.0` are documented usages that predate CIDR.
///
/// Returns the value and rejects anything that is not wholly consumed. The bound
/// is `u32` because a one-component address is a full 32-bit value.
fn parse_component(text: &str) -> Option<u32> {
    let (digits, radix) =
        if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            (rest, 16)
        } else if text.len() > 1 && text.starts_with('0') {
            (&text[1..], 8)
        } else {
            (text, 10)
        };
    if digits.is_empty() {
        // A lone "0" reaches here with the decimal branch, so an empty string at
        // this point means a bare "0x" or a stray separator.
        return if radix == 8 { Some(0) } else { None };
    }
    // `from_str_radix` would accept a leading sign, which no address form has.
    if !digits.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    u32::from_str_radix(digits, radix).ok()
}

/// `inet_aton`'s parse: returns the address in **host** order.
///
/// The four accepted shapes, all of which BusyBox can produce:
///
/// | form      | meaning                                       |
/// |-----------|-----------------------------------------------|
/// | `a`       | the whole 32-bit address                      |
/// | `a.b`     | `a` in the top octet, `b` in the low 24 bits  |
/// | `a.b.c`   | `a`, `b`, then `c` in the low 16 bits         |
/// | `a.b.c.d` | one octet each                                |
///
/// Each leading component must fit in 8 bits and the last in whatever remains,
/// which is what rejects `1.2.3.256` while accepting `1.2.3` (where 3 is a
/// 16-bit tail, not an octet).
fn parse_inet_aton(text: &str) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut values = Vec::with_capacity(parts.len());
    for part in &parts {
        // An empty component means a doubled or trailing dot: "1..2", "1.2.3.4.".
        if part.is_empty() {
            return None;
        }
        values.push(parse_component(part)?);
    }

    // Every component but the last is one octet.
    let leading = values.len() - 1;
    for value in &values[..leading] {
        if *value > 0xff {
            return None;
        }
    }
    // The last component fills all the octets the leading ones did not.
    let tail_bits = 32 - 8 * leading as u32;
    let tail = values[leading];
    if tail_bits < 32 && tail >= (1u32 << tail_bits) {
        return None;
    }

    let mut address = tail;
    for (index, value) in values[..leading].iter().enumerate() {
        address |= value << (32 - 8 * (index as u32 + 1));
    }
    Some(address)
}

/// `inet_pton(AF_INET)`: strict dotted quad, four decimal octets, nothing else.
///
/// Deliberately much stricter than [`parse_inet_aton`]. `inet_pton` is what
/// validates user input that must be unambiguous, so `010.1.1.1` is rejected
/// rather than read as octal — the whole reason the two functions coexist.
fn parse_ipv4_strict(text: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut seen = 0;
    for part in text.split('.') {
        if seen == 4 {
            // A fifth component: "1.2.3.4.5".
            return None;
        }
        // One to three digits, no sign, no leading zero on a multi-digit group.
        if part.is_empty() || part.len() > 3 {
            return None;
        }
        if !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
        let value: u32 = part.parse().ok()?;
        if value > 255 {
            return None;
        }
        octets[seen] = value as u8;
        seen += 1;
    }
    (seen == 4).then_some(octets)
}

/// `inet_pton(AF_INET6)`.
///
/// Handles `::` compression, an embedded IPv4 tail (`::ffff:1.2.3.4`), and
/// rejects a second `::`, which is the ambiguity that makes `::1::2` meaningless
/// rather than merely unusual.
fn parse_ipv6(text: &str) -> Option<[u8; 16]> {
    let mut bytes = [0u8; 16];
    // Where the run of implied zeros begins, if a `::` was seen.
    let mut gap: Option<usize> = None;
    let mut filled = 0usize;

    let mut rest = text;
    // A leading `::` is the one case where an empty group before the separator is
    // legal, so it is consumed up front and the general loop then rejects every
    // other empty group.
    if let Some(after) = rest.strip_prefix("::") {
        gap = Some(0);
        rest = after;
        if rest.is_empty() {
            // "::" alone is the all-zero address.
            return Some(bytes);
        }
    } else if rest.starts_with(':') {
        // A single leading colon is never valid: ":1::" and ":1:2:...".
        return None;
    }

    let mut groups = rest.split(':');
    let mut pending: Option<&str> = groups.next();
    while let Some(group) = pending {
        pending = groups.next();

        if group.is_empty() {
            // An empty group is the `::` separator seen from inside the loop:
            // "1::2" splits to ["1", "", "2"]. A second one is the ambiguity that
            // makes the address meaningless.
            if gap.is_some() {
                return None;
            }
            gap = Some(filled);
            if pending == Some("") {
                // A trailing "::" splits to two empty groups ("1::" gives
                // ["1", "", ""]). Consume the second; anything after it means a
                // third colon, as in "1:::2".
                if groups.next().is_some() {
                    return None;
                }
                break;
            }
            // A lone trailing colon is not a `::`. This rejects "1:" and "",
            // both of which would otherwise be read as a compressed address.
            if pending.is_none() {
                return None;
            }
            continue;
        }

        // An embedded IPv4 tail occupies the last two groups.
        if group.contains('.') {
            // It has to actually be last: "::1.2.3.4:5" is malformed.
            if pending.is_some() {
                return None;
            }
            let quad = parse_ipv4_strict(group)?;
            if filled + 4 > 16 {
                return None;
            }
            bytes[filled..filled + 4].copy_from_slice(&quad);
            filled += 4;
            break;
        }

        // A group is one to four hex digits. Five is an overflow, not a wrap.
        if group.len() > 4 || !group.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let value = u16::from_str_radix(group, 16).ok()?;
        if filled + 2 > 16 {
            return None;
        }
        bytes[filled..filled + 2].copy_from_slice(&value.to_be_bytes());
        filled += 2;
    }

    match gap {
        // Without compression the address must be exactly full.
        None => (filled == 16).then_some(bytes),
        Some(start) => {
            // With compression it must be short, or the `::` stood for nothing —
            // which RFC 5952 forbids and which would make two spellings of one
            // address.
            if filled >= 16 {
                return None;
            }
            // Slide everything after the gap down to the end, zeroing the middle.
            let tail = filled - start;
            for index in 0..tail {
                bytes[16 - tail + index] = bytes[start + index];
                bytes[start + index] = 0;
            }
            Some(bytes)
        }
    }
}

/// Formats an IPv4 address as dotted decimal.
fn format_ipv4(octets: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3])
}

/// Formats an IPv6 address in the RFC 5952 canonical form.
///
/// The rules that make the output canonical rather than merely valid: lowercase
/// hex, no leading zeros in a group, the **longest** run of zero groups
/// compressed to `::`, the **leftmost** such run on a tie, and a run of exactly
/// one group never compressed. The last rule matters because `::` standing for a
/// single zero group would give two spellings of the same address.
fn format_ipv6(bytes: [u8; 16]) -> String {
    let mut words = [0u16; 8];
    for (index, word) in words.iter_mut().enumerate() {
        *word = u16::from_be_bytes([bytes[index * 2], bytes[index * 2 + 1]]);
    }

    // Longest run of zeros, leftmost on a tie. `>` rather than `>=` is what
    // keeps the leftmost run when two are the same length.
    let mut best_start = 0usize;
    let mut best_length = 0usize;
    let mut current_start = 0usize;
    let mut current_length = 0usize;
    for (index, word) in words.iter().enumerate() {
        if *word == 0 {
            if current_length == 0 {
                current_start = index;
            }
            current_length += 1;
            if current_length > best_length {
                best_start = current_start;
                best_length = current_length;
            }
        } else {
            current_length = 0;
        }
    }
    // A single zero group is spelled `0`, not `::`.
    if best_length < 2 {
        best_length = 0;
    }

    // The mixed `::ffff:1.2.3.4` form. RFC 5952 keeps the dotted tail for
    // IPv4-mapped addresses, and the IPv4-compatible `::a.b.c.d` form is printed
    // the same way by convention. `::1` is not one of these: a six-group run puts
    // a non-zero word at index 6 or 7, and the loopback address has a run of
    // seven, so it falls through to the general path and prints as `::1`.
    let mapped = best_start == 0 && ((best_length == 5 && words[5] == 0xffff) || best_length == 6);
    if mapped {
        let quad = format_ipv4([bytes[12], bytes[13], bytes[14], bytes[15]]);
        return if best_length == 5 {
            format!("::ffff:{quad}")
        } else {
            format!("::{quad}")
        };
    }

    // Groups are emitted with a leading colon for every group but the first. The
    // compressed run is emitted as a single colon, which combines with the
    // leading colon of the group that follows to form `::`; a run that reaches
    // the end has no following group, so it supplies the second colon itself.
    let mut out = String::with_capacity(39);
    let mut index = 0;
    while index < 8 {
        if best_length > 0 && index == best_start {
            out.push(':');
            if best_start + best_length == 8 {
                out.push(':');
            }
            index += best_length;
            continue;
        }
        if index > 0 {
            out.push(':');
        }
        // Lowercase and unpadded: `{:x}` is exactly the canonical group spelling.
        out.push_str(&format!("{:x}", words[index]));
        index += 1;
    }
    out
}

/// Builds an IPv4 netmask from a prefix length.
///
/// Windows reports `OnLinkPrefixLength` and no mask, but Linux `getifaddrs`
/// callers read `ifa_netmask`, so the mask is reconstructed. A prefix of 0 must
/// not shift by 32, which is undefined in C and a panic in debug Rust.
fn mask_from_prefix_v4(prefix: u8) -> u32 {
    if prefix == 0 {
        return 0;
    }
    let prefix = prefix.min(32) as u32;
    u32::MAX << (32 - prefix)
}

/// Builds an IPv6 netmask from a prefix length, byte by byte.
fn mask_from_prefix_v6(prefix: u8) -> [u8; 16] {
    let mut mask = [0u8; 16];
    let mut remaining = u32::from(prefix.min(128));
    for byte in &mut mask {
        let bits = remaining.min(8);
        // `0xff << (8 - bits)` for bits in 0..=8, without the 0-shift hazard.
        *byte = (0xffu16 << (8 - bits)) as u8;
        remaining -= bits;
    }
    mask
}

/// Borrows a NUL-terminated argument as a `str`.
///
/// # Safety
///
/// `text` must be null or a NUL-terminated string.
unsafe fn borrow(text: *const c_char) -> Option<&'static str> {
    if text.is_null() {
        return None;
    }
    // SAFETY: forwarded from this function's contract.
    unsafe { CStr::from_ptr(text) }.to_str().ok()
}

/// The historical one-to-four component syntax; failure returns INADDR_NONE.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_inet_addr(text: *const c_char) -> u32 {
    if text.is_null() {
        return u32::MAX;
    }
    unsafe { CStr::from_ptr(text) }
        .to_str()
        .ok()
        .and_then(parse_inet_aton)
        .map_or(u32::MAX, u32::to_be)
}

/// `inet_aton`, which accepts the historical address forms.
///
/// Returns 1 on success and **0** on failure — not -1, and errno is not set.
/// That inverted convention is part of the interface and BusyBox tests it
/// directly, so a caller reading -1 would treat every valid address as a
/// failure.
///
/// # Safety
///
/// `text` must be null or NUL-terminated, and `address` must be null or point at
/// a writable `struct in_addr`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_inet_aton(
    text: *const c_char,
    address: *mut u32,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    let Some(text) = (unsafe { borrow(text) }) else {
        return 0;
    };
    let Some(host_order) = parse_inet_aton(text) else {
        return 0;
    };
    // `struct in_addr` holds the address in network order. POSIX allows a null
    // out-parameter, which turns this into a pure validity check.
    if !address.is_null() {
        // SAFETY: the caller guarantees a writable in_addr when non-null.
        unsafe { address.write_unaligned(host_order.to_be()) };
    }
    1
}

/// `inet_ntoa`, which formats an address into per-thread static storage.
///
/// The returned pointer is valid until this thread calls `inet_ntoa` again, which
/// is glibc's documented contract. Per-thread rather than process-wide storage is
/// a strict improvement: it removes a data race the interface would otherwise
/// have, without changing anything a single-threaded caller can observe.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_inet_ntoa(address: u32) -> *mut c_char {
    // 16 bytes holds "255.255.255.255" and its terminator.
    thread_local! {
        static BUFFER: core::cell::UnsafeCell<[u8; 16]> = const {
            core::cell::UnsafeCell::new([0; 16])
        };
    }
    // The argument is in network byte order in memory; SysV ABI passes the struct
    // in_addr in register EDI, so its native-endian byte representation gives the
    // bytes in original network order.
    let text = format_ipv4(address.to_ne_bytes());
    BUFFER.with(|slot| {
        let buffer = slot.get();
        // SAFETY: single-threaded access to this thread's own cell, and the
        // formatted text is at most 15 bytes plus a terminator.
        unsafe {
            let bytes = text.as_bytes();
            ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), bytes.len());
            (*buffer)[bytes.len()] = 0;
            buffer.cast::<c_char>()
        }
    })
}

/// `inet_pton`.
///
/// Returns 1 on success, 0 for a well-formed call whose text is not a valid
/// address, and -1 with `EAFNOSUPPORT` for a family this does not know. The
/// three-way return is load-bearing: 0 and -1 mean different things to the
/// caller, and collapsing them would hide a programming error as bad input.
///
/// # Safety
///
/// `text` must be null or NUL-terminated, and `destination` must be writable for
/// 4 bytes for `AF_INET` or 16 for `AF_INET6`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_inet_pton(
    family: c_int,
    text: *const c_char,
    destination: *mut c_void,
) -> c_int {
    if family != AF_INET && family != AF_INET6 {
        set_errno(EAFNOSUPPORT);
        return -1;
    }
    if destination.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    // SAFETY: forwarded from this function's contract.
    let Some(text) = (unsafe { borrow(text) }) else {
        return 0;
    };

    if family == AF_INET {
        let Some(octets) = parse_ipv4_strict(text) else {
            return 0;
        };
        // SAFETY: the caller guarantees four writable bytes for AF_INET.
        unsafe { ptr::copy_nonoverlapping(octets.as_ptr(), destination.cast::<u8>(), 4) };
        return 1;
    }

    let Some(bytes) = parse_ipv6(text) else {
        return 0;
    };
    // SAFETY: the caller guarantees sixteen writable bytes for AF_INET6.
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), destination.cast::<u8>(), 16) };
    1
}

/// `inet_ntop`.
///
/// Returns `destination` on success, or null with `ENOSPC` when the text does not
/// fit. `ENOSPC` rather than `ERANGE` is what POSIX specifies here.
///
/// # Safety
///
/// `source` must be readable for 4 bytes for `AF_INET` or 16 for `AF_INET6`, and
/// `destination` must be writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_inet_ntop(
    family: c_int,
    source: *const c_void,
    destination: *mut c_char,
    size: u32,
) -> *const c_char {
    if source.is_null() || destination.is_null() {
        set_errno(EFAULT);
        return ptr::null();
    }
    let text = match family {
        AF_INET => {
            let mut octets = [0u8; 4];
            // SAFETY: the caller guarantees four readable bytes for AF_INET.
            unsafe { ptr::copy_nonoverlapping(source.cast::<u8>(), octets.as_mut_ptr(), 4) };
            format_ipv4(octets)
        }
        AF_INET6 => {
            let mut bytes = [0u8; 16];
            // SAFETY: the caller guarantees sixteen readable bytes for AF_INET6.
            unsafe { ptr::copy_nonoverlapping(source.cast::<u8>(), bytes.as_mut_ptr(), 16) };
            format_ipv6(bytes)
        }
        _ => {
            set_errno(EAFNOSUPPORT);
            return ptr::null();
        }
    };

    let bytes = text.as_bytes();
    if bytes.len() + 1 > size as usize {
        set_errno(ENOSPC);
        return ptr::null();
    }
    // SAFETY: the bound above proved the text and its terminator fit.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), destination.cast::<u8>(), bytes.len());
        *destination.add(bytes.len()) = 0;
    }
    destination
}

// ---------------------------------------------------------------------------
// getaddrinfo.
//
// Forwarded to GetAddrInfoW, with every field copied across by name. The two
// `addrinfo` structs are the same size and different shapes, so this is where the
// module earns its keep.
// ---------------------------------------------------------------------------

/// Translates a Linux address family to the Windows value.
///
/// `AF_INET6` is the one that differs: 10 on Linux, 23 on Windows.
fn family_to_windows(family: c_int) -> Option<c_int> {
    match family {
        AF_UNSPEC => Some(0),
        AF_INET => Some(2),
        AF_INET6 => Some(23),
        _ => None,
    }
}

/// Translates a Windows address family back to the Linux value.
fn family_to_linux(family: c_int) -> c_int {
    match family {
        2 => AF_INET,
        23 => AF_INET6,
        _ => AF_UNSPEC,
    }
}

/// Translates Linux `AI_*` hint flags to Windows.
///
/// Only the first three bits agree. `AI_V4MAPPED` is 0x0008 on Linux and 0x0800
/// on Windows, `AI_ALL` 0x0010 against 0x0100, `AI_ADDRCONFIG` 0x0020 against
/// 0x0400, and `AI_NUMERICSERV` 0x0400 against 0x0008 — so Linux's
/// `AI_NUMERICSERV` is bit-for-bit Windows' `AI_V4MAPPED`. Passing these through
/// would not fail; it would resolve a different question.
fn ai_flags_to_windows(flags: c_int) -> c_int {
    const WINDOWS_AI_PASSIVE: c_int = 0x0001;
    const WINDOWS_AI_CANONNAME: c_int = 0x0002;
    const WINDOWS_AI_NUMERICHOST: c_int = 0x0004;
    const WINDOWS_AI_NUMERICSERV: c_int = 0x0008;
    const WINDOWS_AI_ALL: c_int = 0x0100;
    const WINDOWS_AI_ADDRCONFIG: c_int = 0x0400;
    const WINDOWS_AI_V4MAPPED: c_int = 0x0800;

    let mut translated = 0;
    if flags & AI_PASSIVE != 0 {
        translated |= WINDOWS_AI_PASSIVE;
    }
    if flags & AI_CANONNAME != 0 {
        translated |= WINDOWS_AI_CANONNAME;
    }
    if flags & AI_NUMERICHOST != 0 {
        translated |= WINDOWS_AI_NUMERICHOST;
    }
    if flags & AI_NUMERICSERV != 0 {
        translated |= WINDOWS_AI_NUMERICSERV;
    }
    if flags & AI_ALL != 0 {
        translated |= WINDOWS_AI_ALL;
    }
    if flags & AI_ADDRCONFIG != 0 {
        translated |= WINDOWS_AI_ADDRCONFIG;
    }
    if flags & AI_V4MAPPED != 0 {
        translated |= WINDOWS_AI_V4MAPPED;
    }
    translated
}

/// Maps a Winsock resolver error onto the Linux `EAI_*` code.
///
/// `GetAddrInfoW` returns these directly rather than through `WSAGetLastError`,
/// and they are Winsock error numbers, not EAI codes: `WSAHOST_NOT_FOUND` is
/// 11001, where Linux's `EAI_NONAME` is -2.
fn eai_from_wsa(error: c_int) -> c_int {
    match error {
        // WSATRY_AGAIN: the name server is reachable but did not answer.
        11002 => EAI_AGAIN,
        // WSANO_RECOVERY: a non-recoverable server failure.
        11003 => EAI_FAIL,
        // WSANO_DATA: the name is valid but carries no record of this type.
        11004 => EAI_NODATA,
        // WSAHOST_NOT_FOUND.
        11001 => EAI_NONAME,
        // WSATYPE_NOT_FOUND: the service name is not in the database.
        10109 => EAI_SERVICE,
        // WSAEAFNOSUPPORT / WSAEPFNOSUPPORT.
        10047 | 10046 => EAI_FAMILY,
        // WSAESOCKTNOSUPPORT.
        10044 => EAI_SOCKTYPE,
        // WSA_NOT_ENOUGH_MEMORY.
        8 => EAI_MEMORY,
        // WSAEINVAL: reached when the hint flags are not a valid combination.
        10022 => EAI_BADFLAGS,
        // WSANOTINITIALISED and anything unrecognized. EAI_FAIL rather than
        // EAI_SYSTEM because errno carries nothing useful for these.
        _ => EAI_FAIL,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HostsAnswer {
    address: String,
    canonical_name: String,
}

/// Parses the `files` source used by the ordinary Linux `hosts: files dns`
/// NSS policy.  The first name after the address is canonical; every remaining
/// name is an alias for the same address.
fn parse_hosts_answers(text: &str, query: &str) -> Vec<HostsAnswer> {
    let mut answers = Vec::new();
    for line in text.lines() {
        let record = line.split('#').next().unwrap_or_default();
        let mut fields = record.split_whitespace();
        let Some(address) = fields.next() else {
            continue;
        };
        if address.parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        let Some(canonical_name) = fields.next() else {
            continue;
        };
        if canonical_name.eq_ignore_ascii_case(query)
            || fields.any(|alias| alias.eq_ignore_ascii_case(query))
        {
            answers.push(HostsAnswer {
                address: address.to_owned(),
                canonical_name: canonical_name.to_owned(),
            });
        }
    }
    answers
}

/// Looks up a name in the guest's `/etc/hosts`, not the host Windows file.
/// This preserves Linux's default `files dns` ordering and also keeps each
/// guest root's resolver configuration isolated from the machine running it.
fn hosts_answers(query: &str) -> Vec<HostsAnswer> {
    let text = kinakaze_vfs::path::resolve_linux_path("/etc/hosts")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_else(|| "127.0.0.1 localhost\n::1 localhost\n".to_owned());
    parse_hosts_answers(&text, query)
}

/// Allocates one guest-visible `addrinfo` node in a single block.
///
/// The layout is `[AddrInfo][sockaddr][canonname]`, so one allocation holds the
/// node and everything it points at and [`kinakaze_abi_freeaddrinfo`] releases it
/// with one `free`. The alternative — three allocations per node — would work,
/// but the guest is entitled to call `freeaddrinfo` on a list this module built
/// and nothing else, so keeping the ownership in one block removes the chance of
/// a partial release.
///
/// The `AddrInfo` header is first, which is what makes the node pointer the
/// allocation pointer.
fn allocate_node(address: &[u8], canonname: Option<&str>) -> *mut AddrInfo {
    let header = size_of::<AddrInfo>();
    // The sockaddr must stay 8-aligned for a guest that reads sin6_scope_id, and
    // the header is a multiple of 8, so no interior padding is needed.
    let address_offset = header;
    let name_offset = address_offset + address.len();
    let name_length = canonname.map_or(0, |name| name.len() + 1);
    let total = name_offset + name_length;

    // SAFETY: the block is handed to the guest and released by freeaddrinfo.
    let block = unsafe { kinakaze_alloc::c::malloc(total) };
    if block.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: `total` bytes were just allocated.
    unsafe { ptr::write_bytes(block, 0, total) };

    // SAFETY: the allocation covers the address at its offset.
    let address_pointer = unsafe { block.add(address_offset) };
    // SAFETY: same allocation, and `address.len()` bytes were reserved.
    unsafe { ptr::copy_nonoverlapping(address.as_ptr(), address_pointer, address.len()) };

    let name_pointer = match canonname {
        Some(name) => {
            // SAFETY: the allocation covers the name at its offset.
            let target = unsafe { block.add(name_offset) };
            // SAFETY: `name.len() + 1` bytes were reserved, and the block was
            // zeroed, so the terminator is already in place.
            unsafe { ptr::copy_nonoverlapping(name.as_ptr(), target, name.len()) };
            target.cast::<c_char>()
        }
        None => ptr::null_mut(),
    };

    let node = block.cast::<AddrInfo>();
    // SAFETY: the block begins with space for an AddrInfo and is 16-aligned.
    unsafe {
        node.write(AddrInfo {
            ai_flags: 0,
            ai_family: AF_UNSPEC,
            ai_socktype: 0,
            ai_protocol: 0,
            ai_addrlen: address.len() as u32,
            ai_addr: address_pointer.cast::<c_void>(),
            ai_canonname: name_pointer,
            ai_next: ptr::null_mut(),
        });
    }
    node
}

/// Rewrites the family word of a Windows `sockaddr` to the Linux value.
///
/// The payloads are byte-identical between the two systems; only the leading
/// 16-bit family differs, and only for IPv6.
fn sockaddr_family_to_linux(buffer: &mut [u8]) {
    if buffer.len() < 2 {
        return;
    }
    let windows_family = c_int::from(u16::from_le_bytes([buffer[0], buffer[1]]));
    let linux_family = family_to_linux(windows_family) as u16;
    buffer[..2].copy_from_slice(&linux_family.to_le_bytes());
}

/// `getaddrinfo`.
///
/// Both `node` and `service` being null is `EAI_NONAME`, which is what makes
/// `getaddrinfo(NULL, NULL, ...)` an error rather than a request for every
/// address on the machine.
///
/// `AI_PASSIVE` is handled by Windows, which returns the wildcard address
/// (`0.0.0.0` or `::`) for a null `node` when the flag is set and the loopback
/// address when it is not — the same rule Linux follows.
///
/// # Safety
///
/// `node` and `service` must be null or NUL-terminated, `hints` must be null or
/// point at a readable `struct addrinfo`, and `result` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getaddrinfo(
    node: *const c_char,
    service: *const c_char,
    hints: *const AddrInfo,
    result: *mut *mut AddrInfo,
) -> c_int {
    if result.is_null() {
        return EAI_SYSTEM;
    }
    // SAFETY: the caller guarantees a writable out-pointer.
    unsafe { *result = ptr::null_mut() };

    // SAFETY: forwarded from this function's contract.
    let node_text = unsafe { borrow(node) };
    // SAFETY: forwarded from this function's contract.
    let service_text = unsafe { borrow(service) };
    if node.is_null() && service.is_null() {
        return EAI_NONAME;
    }
    // A non-null pointer that did not decode is not a name that can be resolved.
    if (!node.is_null() && node_text.is_none()) || (!service.is_null() && service_text.is_none()) {
        return EAI_NONAME;
    }

    // The hints, read field by field. A Linux `addrinfo` cannot be handed to
    // Windows even as an input: `ai_addrlen` is a different width and the two
    // pointer fields are transposed.
    let (hint_flags, hint_family, hint_socktype, hint_protocol) = if hints.is_null() {
        (0, AF_UNSPEC, 0, 0)
    } else {
        // SAFETY: the caller guarantees a readable struct addrinfo.
        let hints = unsafe { &*hints };
        (
            hints.ai_flags,
            hints.ai_family,
            // Linux ORs SOCK_NONBLOCK and SOCK_CLOEXEC into a socket type
            // elsewhere; a resolver hint carrying them would be rejected by
            // Windows, so only the type bits are forwarded.
            hints.ai_socktype & 0xff,
            hints.ai_protocol,
        )
    };

    if hint_flags & !AI_KNOWN != 0 {
        return EAI_BADFLAGS;
    }
    let Some(windows_family) = family_to_windows(hint_family) else {
        return EAI_FAMILY;
    };
    if !matches!(hint_socktype, 0 | SOCK_STREAM | SOCK_DGRAM | 3) {
        return EAI_SOCKTYPE;
    }

    if let Some(name) = service_text
        && !name.is_empty()
        && !name.as_bytes().iter().all(u8::is_ascii_digit)
    {
        return unsafe {
            services::resolve_addresses(
                node,
                name.as_bytes(),
                services::AddressHints {
                    flags: hint_flags,
                    family: hint_family,
                    socktype: hint_socktype,
                    protocol: hint_protocol,
                },
                result,
            )
        };
    }

    let trace = crate::fork_trace_enabled();
    if trace {
        eprintln!(
            "kinakaze netdb: getaddrinfo node={node_text:?} service={service_text:?} flags={hint_flags:#x} family={hint_family} socktype={hint_socktype} protocol={hint_protocol}"
        );
    }

    if !ensure_winsock() {
        if trace {
            eprintln!("kinakaze netdb: WSAStartup failed");
        }
        return EAI_SYSTEM;
    }

    let windows_hints = AddrInfoW {
        ai_flags: ai_flags_to_windows(hint_flags),
        ai_family: windows_family,
        // SOCK_STREAM, SOCK_DGRAM and SOCK_RAW agree between the two systems.
        ai_socktype: hint_socktype,
        // IPPROTO_TCP and IPPROTO_UDP also agree.
        ai_protocol: hint_protocol,
        ai_addrlen: 0,
        ai_canonname: ptr::null_mut(),
        ai_addr: ptr::null_mut(),
        ai_next: ptr::null_mut(),
    };

    let node_wide = node_text.map(wide);
    let service_wide = service_text.map(wide);

    // glibc's default NSS policy checks the guest's files database before DNS.
    // Resolve each matching address as a numeric host through Winsock so service
    // names, requested socket types, AI_ADDRCONFIG and AI_V4MAPPED still receive
    // exactly the same validation as the DNS path below.
    if hint_flags & AI_NUMERICHOST == 0
        && let Some(name) = node_text
    {
        let file_answers = hosts_answers(name);
        if !file_answers.is_empty() {
            let mut head: *mut AddrInfo = ptr::null_mut();
            let mut tail: *mut AddrInfo = ptr::null_mut();
            let mut last_error = EAI_NONAME;
            let mut canonical_pending = hint_flags & AI_CANONNAME != 0;

            for answer in &file_answers {
                let numeric_node = wide(&answer.address);
                let mut file_hints = windows_hints;
                // Numeric lookup must not perform another name-service query.
                // Suppress Windows' numeric canonname so the guest receives the
                // canonical first name from its own hosts record instead.
                file_hints.ai_flags = (file_hints.ai_flags | 0x0004) & !0x0002;
                let mut windows_file_result: *mut AddrInfoW = ptr::null_mut();
                let code = unsafe {
                    GetAddrInfoW(
                        numeric_node.as_ptr(),
                        service_wide
                            .as_ref()
                            .map_or(ptr::null(), |text| text.as_ptr()),
                        &raw const file_hints,
                        &raw mut windows_file_result,
                    )
                };
                if code != 0 {
                    last_error = eai_from_wsa(code);
                    continue;
                }

                let canonical = canonical_pending.then_some(answer.canonical_name.as_str());
                let translated =
                    unsafe { translate_result_list(windows_file_result, hint_flags, canonical) };
                unsafe { FreeAddrInfoW(windows_file_result) };
                let new_head = match translated {
                    Ok(node) => node,
                    Err(error) => {
                        if !head.is_null() {
                            unsafe { kinakaze_abi_freeaddrinfo(head) };
                        }
                        return error;
                    }
                };
                canonical_pending = false;

                if head.is_null() {
                    head = new_head;
                } else {
                    unsafe { (*tail).ai_next = new_head };
                }
                tail = new_head;
                while unsafe { !(*tail).ai_next.is_null() } {
                    tail = unsafe { (*tail).ai_next };
                }
            }

            if !head.is_null() {
                unsafe { *result = head };
                return 0;
            }
            return last_error;
        }
    }

    let mut windows_result: *mut AddrInfoW = ptr::null_mut();
    // SAFETY: both name pointers are null or NUL-terminated wide buffers that
    // outlive the call, the hints are a live local, and the out-pointer is local.
    let mut code = unsafe {
        GetAddrInfoW(
            node_wide.as_ref().map_or(ptr::null(), |text| text.as_ptr()),
            service_wide
                .as_ref()
                .map_or(ptr::null(), |text| text.as_ptr()),
            &raw const windows_hints,
            &raw mut windows_result,
        )
    };
    if code != 0 && hint_flags & AI_ADDRCONFIG != 0 {
        let mut retry_hints = windows_hints;
        retry_hints.ai_flags &= !0x0400; // clear WINDOWS_AI_ADDRCONFIG
        let retry_code = unsafe {
            GetAddrInfoW(
                node_wide.as_ref().map_or(ptr::null(), |text| text.as_ptr()),
                service_wide
                    .as_ref()
                    .map_or(ptr::null(), |text| text.as_ptr()),
                &raw const retry_hints,
                &raw mut windows_result,
            )
        };
        if retry_code == 0 {
            code = 0;
        }
    }
    if code != 0 {
        if trace {
            eprintln!(
                "kinakaze netdb: GetAddrInfoW returned {code}, mapped to {}",
                eai_from_wsa(code)
            );
        }
        return eai_from_wsa(code);
    }

    // SAFETY: the call succeeded, so the list is a valid chain this scope owns.
    let outcome = unsafe { translate_result_list(windows_result, hint_flags, None) };
    // SAFETY: the Windows list is released whether or not translation succeeded;
    // the guest never sees these nodes.
    unsafe { FreeAddrInfoW(windows_result) };

    match outcome {
        Ok(head) => {
            // SAFETY: `result` was checked non-null above.
            unsafe { *result = head };
            0
        }
        Err(error) => error,
    }
}

/// Copies a Windows result chain into guest-owned Linux `addrinfo` nodes.
///
/// # Safety
///
/// `list` must be null or a valid `ADDRINFOW` chain.
unsafe fn translate_result_list(
    list: *mut AddrInfoW,
    requested_flags: c_int,
    canonical_override: Option<&str>,
) -> Result<*mut AddrInfo, c_int> {
    let mut nodes: Vec<*mut AddrInfo> = Vec::new();

    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: the chain is valid and `cursor` is non-null.
        let entry = unsafe { &*cursor };
        cursor = entry.ai_next;

        let length = entry.ai_addrlen.min(128);
        if entry.ai_addr.is_null() || length < 2 {
            continue;
        }
        let mut address = vec![0u8; length];
        unsafe {
            ptr::copy_nonoverlapping(entry.ai_addr.cast::<u8>(), address.as_mut_ptr(), length)
        };
        sockaddr_family_to_linux(&mut address);

        let canonname = if requested_flags & AI_CANONNAME != 0 && nodes.is_empty() {
            canonical_override
                .map(str::to_owned)
                .or_else(|| unsafe { from_wide(entry.ai_canonname) })
        } else {
            None
        };

        let node = allocate_node(&address, canonname.as_deref());
        if node.is_null() {
            for &n in &nodes {
                unsafe { kinakaze_abi_freeaddrinfo(n) };
            }
            return Err(EAI_MEMORY);
        }
        unsafe {
            (*node).ai_flags = requested_flags;
            (*node).ai_family = family_to_linux(entry.ai_family);
            (*node).ai_socktype = entry.ai_socktype;
            (*node).ai_protocol = entry.ai_protocol;
            (*node).ai_next = ptr::null_mut();
        }
        nodes.push(node);
    }

    if nodes.is_empty() {
        return Err(EAI_NONAME);
    }

    // Stable sort: prioritize IPv4 (AF_INET = 2) for immediate connection success in mixed networks
    nodes.sort_by_key(|&n| unsafe { if (*n).ai_family == 2 { 0 } else { 1 } });

    for i in 0..nodes.len() - 1 {
        unsafe {
            (*nodes[i]).ai_next = nodes[i + 1];
        }
    }

    Ok(nodes[0])
}

/// `freeaddrinfo`.
///
/// Each node was allocated as one block containing its `ai_addr` and
/// `ai_canonname`, so one `free` per node releases everything. A guest must not
/// free those two fields separately, which is also true of glibc.
///
/// # Safety
///
/// `list` must be null or a chain this module's `getaddrinfo` returned, and must
/// not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_freeaddrinfo(list: *mut AddrInfo) {
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: the caller guarantees a chain this module built, so the node is
        // readable and `ai_next` is the next such node or null.
        let next = unsafe { (*cursor).ai_next };
        // SAFETY: the node pointer is the allocation pointer by construction in
        // `allocate_node`, and the guest is releasing it.
        unsafe { kinakaze_alloc::c::free(cursor.cast::<u8>()) };
        cursor = next;
    }
}

// ---------------------------------------------------------------------------
// The service database.
//
// Shared lookup and iteration, with explicit open/reset/close lifetime.
// ---------------------------------------------------------------------------

mod ether;
mod networks;
mod protocols;
mod records;
mod reentrant;
mod services;
pub use ether::{
    kinakaze_abi_ether_aton, kinakaze_abi_ether_aton_r, kinakaze_abi_ether_ntoa,
    kinakaze_abi_ether_ntoa_r,
};
pub use networks::{
    Netent, kinakaze_abi_endnetent, kinakaze_abi_getnetbyaddr, kinakaze_abi_getnetbyname,
    kinakaze_abi_getnetent, kinakaze_abi_inet_network, kinakaze_abi_setnetent,
};
pub use protocols::{
    kinakaze_abi_getprotobyname, kinakaze_abi_getprotobyname_r, kinakaze_abi_getprotobynumber,
};
use services::find_service_by_port;
pub use services::{
    kinakaze_abi_endservent, kinakaze_abi_getservbyname, kinakaze_abi_getservbyname_r,
    kinakaze_abi_getservbyport, kinakaze_abi_getservbyport_r, kinakaze_abi_getservent,
    kinakaze_abi_setservent,
};

// ---------------------------------------------------------------------------
// getnameinfo.
// ---------------------------------------------------------------------------

/// Reads the family, port and address bytes out of a guest `sockaddr`.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
unsafe fn read_sockaddr(
    address: *const c_void,
    length: c_int,
) -> Option<(c_int, u16, Vec<u8>, u32)> {
    if address.is_null() || length < 2 {
        return None;
    }
    // SAFETY: the caller guarantees two readable bytes at minimum.
    let family = c_int::from(unsafe { address.cast::<u16>().read_unaligned() });
    match family {
        AF_INET if length >= size_of::<SockaddrIn>() as c_int => {
            // SAFETY: the length check proved a whole sockaddr_in is readable.
            let inet = unsafe { address.cast::<SockaddrIn>().read_unaligned() };
            Some((
                AF_INET,
                u16::from_be(inet.sin_port),
                inet.sin_addr.to_ne_bytes().to_vec(),
                0,
            ))
        }
        AF_INET6 if length >= size_of::<SockaddrIn6>() as c_int => {
            // SAFETY: the length check proved a whole sockaddr_in6 is readable.
            let inet6 = unsafe { address.cast::<SockaddrIn6>().read_unaligned() };
            Some((
                AF_INET6,
                u16::from_be(inet6.sin6_port),
                inet6.sin6_addr.to_vec(),
                inet6.sin6_scope_id,
            ))
        }
        _ => None,
    }
}

/// Asks Windows to reverse-resolve a `sockaddr` to a host name.
///
/// Only the host half goes through `GetNameInfoW`. The service half is answered
/// from this module's own table so that `getnameinfo` and `getservbyport` can
/// never disagree about a port, and the numeric host form is produced by
/// [`format_ipv6`] so it is RFC 5952 canonical rather than whatever Windows
/// prefers.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
unsafe fn reverse_resolve(address: *const c_void, length: c_int) -> Option<String> {
    if !ensure_winsock() {
        return None;
    }
    // The family word has to be Windows' before Winsock sees it.
    let mut buffer = vec![0u8; length as usize];
    // SAFETY: the caller guarantees `length` readable bytes.
    unsafe { ptr::copy_nonoverlapping(address.cast::<u8>(), buffer.as_mut_ptr(), buffer.len()) };
    if buffer.len() >= 2 {
        let linux_family = c_int::from(u16::from_le_bytes([buffer[0], buffer[1]]));
        let windows_family = family_to_windows(linux_family)? as u16;
        buffer[..2].copy_from_slice(&windows_family.to_le_bytes());
    }

    // NI_MAXHOST on Linux, and the bound Windows documents for this buffer.
    let mut host = [0u16; 1025];
    // Windows' own NI_NAMEREQD is 4, not Linux's 8. Requesting it here means a
    // failure is reported rather than the numeric form being returned silently,
    // which is what lets the caller apply its own NI_NAMEREQD policy.
    const WINDOWS_NI_NAMEREQD: c_int = 4;
    // SAFETY: the translated address is a live local of `buffer.len()` bytes and
    // the host buffer is writable for its stated length.
    let code = unsafe {
        GetNameInfoW(
            buffer.as_ptr().cast::<c_void>(),
            buffer.len() as c_int,
            host.as_mut_ptr(),
            host.len() as u32,
            ptr::null_mut(),
            0,
            WINDOWS_NI_NAMEREQD,
        )
    };
    if code != 0 {
        return None;
    }
    // SAFETY: a successful call leaves a NUL-terminated wide string.
    unsafe { from_wide(host.as_ptr()) }.filter(|name| !name.is_empty())
}

/// Copies text into a caller buffer, reporting `EAI_OVERFLOW` if it will not fit.
///
/// # Safety
///
/// `out` must be writable for `capacity` bytes.
unsafe fn copy_out(out: *mut c_char, capacity: u32, text: &str) -> Result<(), c_int> {
    unsafe { copy_out_bytes(out, capacity, text.as_bytes()) }
}

unsafe fn copy_out_bytes(out: *mut c_char, capacity: u32, bytes: &[u8]) -> Result<(), c_int> {
    if bytes.len() + 1 > capacity as usize {
        return Err(EAI_OVERFLOW);
    }
    // SAFETY: the bound above proved the text and terminator fit.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), bytes.len());
        *out.add(bytes.len()) = 0;
    }
    Ok(())
}

/// `getnameinfo`.
///
/// The flag values differ between the two systems — Linux orders them
/// `NUMERICHOST, NUMERICSERV, NOFQDN, NAMEREQD, DGRAM` while Windows orders them
/// `NOFQDN, NUMERICHOST, NAMEREQD, NUMERICSERV, DGRAM` — so they are interpreted
/// here rather than forwarded. Only `NI_NAMEREQD` reaches Winsock at all, and it
/// is translated on the way.
///
/// # Safety
///
/// `address` must be readable for `address_length` bytes, `host` writable for
/// `host_length` bytes when non-null, and `service` writable for
/// `service_length` bytes when non-null.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getnameinfo(
    address: *const c_void,
    address_length: c_int,
    host: *mut c_char,
    host_length: u32,
    service: *mut c_char,
    service_length: u32,
    flags: c_int,
) -> c_int {
    const NI_KNOWN: c_int = NI_NUMERICHOST | NI_NUMERICSERV | NI_NOFQDN | NI_NAMEREQD | NI_DGRAM;
    if flags & !NI_KNOWN != 0 {
        return EAI_BADFLAGS;
    }
    // Asking for neither half is a caller error, not an empty success.
    if (host.is_null() || host_length == 0) && (service.is_null() || service_length == 0) {
        return EAI_NONAME;
    }
    // SAFETY: forwarded from this function's contract.
    let Some((family, port, bytes, scope)) = (unsafe { read_sockaddr(address, address_length) })
    else {
        // A family this layer cannot describe, or a length too short for the
        // family the caller declared.
        return EAI_FAMILY;
    };

    if !host.is_null() && host_length > 0 {
        // The numeric form is produced here rather than by Windows so it matches
        // `inet_ntop` exactly.
        let numeric = if family == AF_INET {
            format_ipv4([bytes[0], bytes[1], bytes[2], bytes[3]])
        } else {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&bytes[..16]);
            let text = format_ipv6(octets);
            // A link-local address is ambiguous without its scope, and glibc
            // appends it in the `%index` form that `ping -I` accepts back.
            if scope != 0 {
                format!("{text}%{scope}")
            } else {
                text
            }
        };

        let resolved = if flags & NI_NUMERICHOST != 0 {
            None
        } else {
            // SAFETY: forwarded from this function's contract.
            unsafe { reverse_resolve(address, address_length) }
        };

        let text = match resolved {
            Some(name) if flags & NI_NOFQDN != 0 => {
                // NI_NOFQDN asks for the unqualified name, which is the first
                // label. A name with no dot is already unqualified.
                name.split('.').next().unwrap_or(&name).to_string()
            }
            Some(name) => name,
            None if flags & NI_NAMEREQD != 0 => {
                // The caller said a numeric answer is not acceptable.
                return EAI_NONAME;
            }
            None => numeric,
        };
        // SAFETY: forwarded from this function's contract.
        if let Err(error) = unsafe { copy_out(host, host_length, &text) } {
            return error;
        }
    }

    if !service.is_null() && service_length > 0 {
        // NI_DGRAM selects the UDP row of the database, which matters for the
        // ports where the two protocols name different services.
        let protocol = if flags & NI_DGRAM != 0 {
            b"udp"
        } else {
            b"tcp"
        };
        let found = if flags & NI_NUMERICSERV != 0 {
            None
        } else {
            match find_service_by_port(port, Some(protocol)) {
                Ok(found) => found,
                Err(kinakaze_vfs::ENOENT) => None,
                Err(error) => {
                    set_errno(error);
                    return EAI_SYSTEM;
                }
            }
        };
        // Numeric output for an unnamed port is getnameinfo's documented
        // contract, not another service database or a host lookup.
        let numeric;
        let bytes = if let Some(found) = &found {
            found.name()
        } else {
            numeric = port.to_string();
            numeric.as_bytes()
        };
        // SAFETY: forwarded from this function's contract.
        if let Err(error) = unsafe { copy_out_bytes(service, service_length, bytes) } {
            return error;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// The host database.
//
// `gethostbyname` and `gethostbyaddr` are built on this module's own
// `getaddrinfo` and `getnameinfo` rather than on Windows' deprecated
// `gethostbyname`, so there is one resolution path and one set of translations to
// be right about. Both report failure through `h_errno`, which is a separate
// variable from `errno` because a name that does not resolve is not an OS error.
// ---------------------------------------------------------------------------

/// Per-thread storage backing a returned `struct hostent`.
///
/// Same contract and same reason for boxing as [`ServentStorage`].
struct HostentStorage {
    entry: Hostent,
    name: CString,
    aliases: Vec<*mut c_char>,
    addresses: Vec<Vec<u8>>,
    address_pointers: Vec<*mut c_char>,
}

thread_local! {
    static HOSTENT: RefCell<Option<Box<HostentStorage>>> = const { RefCell::new(None) };
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endhostent() {
    // Hosts lookups close their source descriptor immediately; release the
    // thread's retained result storage as well when the caller ends its use.
    HOSTENT.with(|slot| {
        slot.borrow_mut().take();
    });
}

/// Publishes a host as a `struct hostent` in this thread's storage.
///
/// `addresses` must all be `length` bytes, which is what `h_length` promises the
/// caller about every entry in `h_addr_list`.
fn publish_hostent(name: &str, family: c_int, addresses: Vec<Vec<u8>>) -> *mut Hostent {
    let length = if family == AF_INET { 4 } else { 16 };
    HOSTENT.with(|slot| {
        let Ok(name) = CString::new(name) else {
            set_h_errno(NO_RECOVERY);
            return ptr::null_mut();
        };
        let mut storage = Box::new(HostentStorage {
            entry: Hostent {
                h_name: ptr::null_mut(),
                h_aliases: ptr::null_mut(),
                h_addrtype: family,
                h_length: length,
                h_addr_list: ptr::null_mut(),
            },
            name,
            // Always a valid empty array rather than null: callers walk
            // `h_aliases` unconditionally, and glibc never returns null here.
            aliases: vec![ptr::null_mut()],
            addresses,
            address_pointers: Vec::new(),
        });

        storage.address_pointers = storage
            .addresses
            .iter()
            .map(|address| address.as_ptr().cast_mut().cast::<c_char>())
            .chain(core::iter::once(ptr::null_mut()))
            .collect();

        storage.entry = Hostent {
            h_name: storage.name.as_ptr().cast_mut(),
            h_aliases: storage.aliases.as_mut_ptr(),
            h_addrtype: family,
            h_length: length,
            h_addr_list: storage.address_pointers.as_mut_ptr(),
        };

        let published = (&raw mut storage.entry).cast::<Hostent>();
        *slot.borrow_mut() = Some(storage);
        published
    })
}

/// Maps a `getaddrinfo` failure onto the `h_errno` value that describes it.
fn h_errno_from_eai(error: c_int) -> c_int {
    match error {
        EAI_AGAIN => TRY_AGAIN,
        EAI_NODATA => NO_DATA,
        EAI_NONAME => HOST_NOT_FOUND,
        // EAI_FAIL, EAI_MEMORY, EAI_SYSTEM and the argument errors are all
        // conditions a retry will not fix.
        _ => NO_RECOVERY,
    }
}

/// Resolves `name` to a list of addresses of one family.
///
/// Shared by `gethostbyname` and nothing else yet, but kept separate because it
/// is the piece that has to go through this module's `getaddrinfo` rather than
/// Windows'.
fn resolve_addresses(name: &str, family: c_int) -> Result<(String, Vec<Vec<u8>>), c_int> {
    let Ok(node) = CString::new(name) else {
        return Err(EAI_NONAME);
    };
    let hints = AddrInfo {
        ai_flags: AI_CANONNAME,
        ai_family: family,
        // One socket type, or every address is reported three times.
        ai_socktype: SOCK_STREAM,
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_addr: ptr::null_mut(),
        ai_canonname: ptr::null_mut(),
        ai_next: ptr::null_mut(),
    };
    let mut list: *mut AddrInfo = ptr::null_mut();
    // SAFETY: the node is a live NUL-terminated string, the hints are a live
    // local, and the out-pointer is local.
    let code = unsafe {
        kinakaze_abi_getaddrinfo(node.as_ptr(), ptr::null(), &raw const hints, &raw mut list)
    };
    if code != 0 {
        return Err(code);
    }

    let mut canonical = name.to_string();
    let mut addresses = Vec::new();
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: the list is one this module built and still owns.
        let entry = unsafe { &*cursor };
        // SAFETY: same.
        cursor = entry.ai_next;

        if !entry.ai_canonname.is_null()
            // SAFETY: a non-null canonname is NUL-terminated by construction.
            && let Some(text) = unsafe { borrow(entry.ai_canonname) }
            && !text.is_empty()
        {
            canonical = text.to_string();
        }
        // SAFETY: `ai_addr` is readable for `ai_addrlen` bytes by construction.
        if let Some((entry_family, _, bytes, _)) =
            unsafe { read_sockaddr(entry.ai_addr, entry.ai_addrlen as c_int) }
            && entry_family == family
            && !addresses.contains(&bytes)
        {
            addresses.push(bytes);
        }
    }
    // SAFETY: the list is this module's and is not referenced after this point.
    unsafe { kinakaze_abi_freeaddrinfo(list) };

    if addresses.is_empty() {
        // The name resolved but produced no address of the requested family.
        return Err(EAI_NODATA);
    }
    Ok((canonical, addresses))
}

/// `gethostbyname`.
///
/// Resolves `AF_INET` only, which is what the interface can express: `h_length`
/// is a single number for the whole list, so one call cannot describe both
/// address sizes. A caller that needs IPv6 has to use `getaddrinfo`, which is
/// also true on glibc.
///
/// A numeric dotted quad is recognized without a lookup, matching glibc, so
/// `ping 127.0.0.1` does not depend on a resolver being reachable.
///
/// # Safety
///
/// `name` must be null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gethostbyname(name: *const c_char) -> *mut Hostent {
    // SAFETY: forwarded from this function's contract.
    let Some(name) = (unsafe { borrow(name) }) else {
        set_h_errno(HOST_NOT_FOUND);
        return ptr::null_mut();
    };

    // A literal address is its own answer. glibc uses the strict form here, so
    // `010.1.1.1` is a name to be looked up rather than an octal address.
    if let Some(octets) = parse_ipv4_strict(name) {
        return publish_hostent(name, AF_INET, vec![octets.to_vec()]);
    }

    match resolve_addresses(name, AF_INET) {
        Ok((canonical, addresses)) => publish_hostent(&canonical, AF_INET, addresses),
        Err(error) => {
            set_h_errno(h_errno_from_eai(error));
            ptr::null_mut()
        }
    }
}

/// `gethostbyaddr`.
///
/// The reverse direction, through this module's `getnameinfo`. `length` must
/// match the family exactly — 4 for `AF_INET`, 16 for `AF_INET6` — because a
/// shorter buffer would be read past its end.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gethostbyaddr(
    address: *const c_void,
    length: c_uint,
    family: c_int,
) -> *mut Hostent {
    let expected = match family {
        AF_INET => 4u32,
        AF_INET6 => 16u32,
        _ => {
            // Not a family this layer can describe. glibc reports this through
            // errno as well as h_errno, because it is a caller error rather than
            // a lookup outcome.
            set_errno(EAFNOSUPPORT);
            set_h_errno(NO_RECOVERY);
            return ptr::null_mut();
        }
    };
    if address.is_null() || length != expected {
        set_errno(EINVAL);
        set_h_errno(NO_RECOVERY);
        return ptr::null_mut();
    }

    // SAFETY: the length was checked against the family above.
    let bytes = unsafe { core::slice::from_raw_parts(address.cast::<u8>(), length as usize) };

    // The reverse lookup needs a full sockaddr, not just the address bytes.
    let mut storage = [0u8; 28];
    let sockaddr_length = if family == AF_INET {
        let inet = SockaddrIn {
            sin_family: AF_INET as u16,
            sin_port: 0,
            sin_addr: u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            sin_zero: [0; 8],
        };
        // SAFETY: `storage` is larger than a sockaddr_in and is byte-aligned.
        unsafe {
            storage
                .as_mut_ptr()
                .cast::<SockaddrIn>()
                .write_unaligned(inet)
        };
        size_of::<SockaddrIn>() as c_int
    } else {
        let mut address_bytes = [0u8; 16];
        address_bytes.copy_from_slice(bytes);
        let inet6 = SockaddrIn6 {
            sin6_family: AF_INET6 as u16,
            sin6_port: 0,
            sin6_flowinfo: 0,
            sin6_addr: address_bytes,
            sin6_scope_id: 0,
        };
        // SAFETY: `storage` is exactly a sockaddr_in6 and is byte-aligned.
        unsafe {
            storage
                .as_mut_ptr()
                .cast::<SockaddrIn6>()
                .write_unaligned(inet6)
        };
        size_of::<SockaddrIn6>() as c_int
    };

    // SAFETY: the sockaddr was just built in a local of sufficient size.
    let resolved = unsafe { reverse_resolve(storage.as_ptr().cast::<c_void>(), sockaddr_length) };
    match resolved {
        Some(name) => publish_hostent(&name, family, vec![bytes.to_vec()]),
        None => {
            // No PTR record. This is a lookup outcome, not an OS failure, so only
            // h_errno is set.
            set_h_errno(HOST_NOT_FOUND);
            ptr::null_mut()
        }
    }
}

/// `hstrerror`, which describes an `h_errno` value.
///
/// The strings are glibc's, so a guest that prints them produces the output its
/// users recognize. The returned pointer addresses static storage and is valid
/// for the life of the process.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_hstrerror(error: c_int) -> *const c_char {
    let text: &'static CStr = match error {
        0 => c"Resolver Error 0 (no error)",
        HOST_NOT_FOUND => c"Unknown host",
        TRY_AGAIN => c"Host name lookup failure",
        NO_RECOVERY => c"Unknown server error",
        NO_DATA => c"No address associated with name",
        _ => c"Unknown resolver error",
    };
    text.as_ptr()
}

// ---------------------------------------------------------------------------
// Interfaces.
//
// Windows names adapters with a GUID (`{4A1B...}`) and a localized friendly name
// ("Ethernet 2"). Neither is a Linux interface name, and BusyBox's `ifconfig` and
// `ip` print the name and then look it up again, so whatever is reported has to
// round-trip.
//
// The naming rule: interfaces are ordered by their Windows `IfIndex`, and each is
// given the conventional Linux name for its IANA type plus its ordinal within
// that type — `lo` for the first loopback, then `eth0`, `eth1` for Ethernet,
// `wlan0` for 802.11, `ppp0` for PPP, `tunl0` for tunnels, and `if<IfIndex>` for
// anything else. Ordering by `IfIndex` is what makes the assignment stable across
// calls within a boot.
//
// `if_nametoindex` reverses exactly this rule, and also accepts a bare `IfIndex`
// spelled as a name, so a guest that learned an index from a routing table can
// still use it. The value it returns is the Windows `IfIndex`, which is what the
// `IP_MULTICAST_IF` and `IPV6_MULTICAST_IF` socket options expect.
// ---------------------------------------------------------------------------

/// One interface, as this module describes it.
struct Interface {
    /// The Linux-shaped name; see the section comment for the rule.
    name: String,
    /// The Windows `IfIndex`, which is what `if_nametoindex` reports.
    index: u32,
    flags: c_uint,
    /// The MAC, when the adapter has one. Used by `gethostid`.
    physical: Vec<u8>,
    /// Unicast addresses, each with the prefix length Windows reported.
    addresses: Vec<(Vec<u8>, u8)>,
}

/// Calls `GetAdaptersAddresses`, growing the buffer until it fits.
///
/// The size is an in-out parameter and the first call is expected to fail with
/// `ERROR_BUFFER_OVERFLOW`; Windows documents 15 KiB as the recommended starting
/// size, which usually makes a single call enough.
fn adapter_buffer() -> Option<Vec<u8>> {
    const FLAGS: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 15 * 1024;
    // Two attempts, plus a little slack for an adapter appearing between them.
    for _ in 0..4 {
        let mut buffer = vec![0u8; size as usize];
        // SAFETY: `buffer` is writable for `size` bytes and `size` is a writable
        // local, which is the documented contract. AF_UNSPEC asks for both
        // families.
        let result = unsafe {
            GetAdaptersAddresses(
                0,
                FLAGS,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast::<IpAdapterAddresses>(),
                &raw mut size,
            )
        };
        match result {
            ERROR_SUCCESS => return Some(buffer),
            // `size` now holds the length actually required.
            ERROR_BUFFER_OVERFLOW => continue,
            _ => return None,
        }
    }
    None
}

/// Derives the Linux-shaped name for an adapter of `if_type`.
///
/// `ordinal` counts adapters of the same type already named, so the first
/// loopback is `lo` and the first Ethernet adapter is `eth0`.
fn interface_name(if_type: u32, ordinal: usize, if_index: u32) -> String {
    match if_type {
        // The first loopback is `lo`, unnumbered, which is the name every Linux
        // tool special-cases.
        IF_TYPE_SOFTWARE_LOOPBACK if ordinal == 0 => String::from("lo"),
        IF_TYPE_SOFTWARE_LOOPBACK => format!("lo{ordinal}"),
        IF_TYPE_ETHERNET_CSMACD => format!("eth{ordinal}"),
        IF_TYPE_IEEE80211 => format!("wlan{ordinal}"),
        IF_TYPE_PPP => format!("ppp{ordinal}"),
        IF_TYPE_TUNNEL => format!("tunl{ordinal}"),
        // No conventional Linux name for this type, so the real index is used.
        // It is still stable and still round-trips.
        _ => format!("if{if_index}"),
    }
}

/// Translates Windows adapter state into Linux interface flags.
fn interface_flags(if_type: u32, oper_status: c_int, adapter_flags: u32) -> c_uint {
    let mut flags = 0;
    if oper_status == IF_OPER_STATUS_UP {
        // Linux distinguishes administratively up from carrier-present; Windows
        // reports one operational status, so both bits follow it. A caller
        // checking either gets the same true answer.
        flags |= IFF_UP | IFF_RUNNING;
    }
    if if_type == IF_TYPE_SOFTWARE_LOOPBACK {
        flags |= IFF_LOOPBACK;
    } else if if_type == IF_TYPE_PPP || if_type == IF_TYPE_TUNNEL {
        flags |= IFF_POINTOPOINT;
    } else {
        // Broadcast is a property of the link layer, and every non-loopback,
        // non-point-to-point type Windows reports here is a broadcast medium.
        flags |= IFF_BROADCAST;
    }
    // Reported by its absence: the flag names the adapters that cannot.
    if adapter_flags & IP_ADAPTER_NO_MULTICAST == 0 {
        flags |= IFF_MULTICAST;
    }
    flags
}

/// Enumerates the host's interfaces.
///
/// Not cached: an adapter can come up or go down at any time, and a stale answer
/// here would make `ifconfig` describe a link that no longer exists.
fn enumerate_interfaces() -> Vec<Interface> {
    let Some(buffer) = adapter_buffer() else {
        return Vec::new();
    };

    // Collected first so the list can be ordered by IfIndex before names are
    // assigned; the ordinal in a name depends on that order.
    struct Raw {
        index: u32,
        if_type: u32,
        oper_status: c_int,
        adapter_flags: u32,
        physical: Vec<u8>,
        addresses: Vec<(Vec<u8>, u8)>,
    }
    let mut raw: Vec<Raw> = Vec::new();

    let mut adapter = buffer.as_ptr().cast::<IpAdapterAddresses>();
    while !adapter.is_null() {
        // SAFETY: the OS filled this buffer as a chain of these structs and
        // `adapter` is non-null.
        let entry = unsafe { &*adapter };
        // SAFETY: same; `next` is the next adapter or null.
        adapter = entry.next;

        let mut addresses = Vec::new();
        let mut unicast = entry.first_unicast_address;
        while !unicast.is_null() {
            // SAFETY: the chain is part of the same OS-filled buffer.
            let address = unsafe { &*unicast };
            // SAFETY: same.
            unicast = address.next;

            let length = address.address.sockaddr_length;
            if address.address.sockaddr.is_null() || length < 2 {
                continue;
            }
            let mut bytes = vec![0u8; length as usize];
            // SAFETY: Windows guarantees `sockaddr_length` readable bytes there.
            unsafe {
                ptr::copy_nonoverlapping(
                    address.address.sockaddr.cast::<u8>(),
                    bytes.as_mut_ptr(),
                    bytes.len(),
                )
            };
            addresses.push((bytes, address.on_link_prefix_length));
        }

        let physical_length = (entry.physical_address_length as usize).min(8);
        raw.push(Raw {
            index: entry.if_index,
            if_type: entry.if_type,
            oper_status: entry.oper_status,
            adapter_flags: entry.flags,
            physical: entry.physical_address[..physical_length].to_vec(),
            addresses,
        });
    }

    // The naming rule depends on this order; see the section comment.
    raw.sort_by_key(|adapter| adapter.index);

    let mut counts: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    raw.into_iter()
        .map(|adapter| {
            let ordinal = counts.entry(adapter.if_type).or_insert(0);
            let name = interface_name(adapter.if_type, *ordinal, adapter.index);
            *ordinal += 1;
            Interface {
                name,
                index: adapter.index,
                flags: interface_flags(adapter.if_type, adapter.oper_status, adapter.adapter_flags),
                physical: adapter.physical,
                addresses: adapter.addresses,
            }
        })
        .collect()
}

/// `if_nametoindex`.
///
/// Reverses the naming rule in the section comment above, so a name this module's
/// `getifaddrs` printed resolves back to the interface it came from. A bare
/// decimal index is also accepted, since a guest reading `/proc/net/route` on a
/// real Linux would have one.
///
/// Returns 0 on failure, which is this interface's convention — there is no valid
/// interface index 0 — and sets `ENODEV`.
///
/// # Safety
///
/// `name` must be null or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_if_nametoindex(name: *const c_char) -> c_uint {
    // SAFETY: forwarded from this function's contract.
    let Some(name) = (unsafe { borrow(name) }) else {
        set_errno(EFAULT);
        return 0;
    };
    if name.is_empty() {
        set_errno(ENODEV);
        return 0;
    }

    let interfaces = enumerate_interfaces();
    if let Some(found) = interfaces.iter().find(|entry| entry.name == name) {
        return found.index;
    }
    // The `if<index>` spelling and a bare decimal index both name an interface by
    // its Windows index, so both are checked against the live list rather than
    // returned unvalidated.
    let numeric = name.strip_prefix("if").unwrap_or(name);
    if let Ok(index) = numeric.parse::<u32>()
        && interfaces.iter().any(|entry| entry.index == index)
    {
        return index;
    }
    set_errno(ENODEV);
    0
}

/// One `getifaddrs` list node, allocated as a single block.
///
/// The `Ifaddrs` header is first, so the node pointer is the allocation pointer
/// and `freeifaddrs` can release each node with one `free`. The three sockaddr
/// slots are inline at 28 bytes each — the size of a `sockaddr_in6`, the largest
/// this module produces — padded to keep the following field aligned.
#[repr(C)]
struct IfaddrsNode {
    public: Ifaddrs,
    address: [u8; 32],
    netmask: [u8; 32],
    broadcast: [u8; 32],
    /// `IFNAMSIZ` on Linux is 16, and every name this module generates is far
    /// shorter, but the buffer is sized so a name can never be truncated.
    name: [u8; 32],
}

/// Builds a `sockaddr` for one address, and the netmask and broadcast that go
/// with it.
///
/// Returns the three byte images and the length that is valid in each. The
/// broadcast address is only meaningful for IPv4 — IPv6 has no broadcast, using
/// multicast instead — so it is `None` for v6, and `ifa_ifu` is then left null
/// rather than filled with something that does not exist.
fn address_triple(
    windows_sockaddr: &[u8],
    prefix: u8,
) -> Option<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> {
    if windows_sockaddr.len() < 2 {
        return None;
    }
    let windows_family = c_int::from(u16::from_le_bytes([
        windows_sockaddr[0],
        windows_sockaddr[1],
    ]));
    let family = family_to_linux(windows_family);

    if family == AF_INET {
        if windows_sockaddr.len() < size_of::<SockaddrIn>() {
            return None;
        }
        // The payload is identical between the two systems; only the family word
        // differs, and for AF_INET it happens to be the same number.
        // SAFETY: the length check proved a whole sockaddr_in is readable, and
        // `read_unaligned` makes no alignment demand on the source.
        let mut inet = unsafe {
            windows_sockaddr
                .as_ptr()
                .cast::<SockaddrIn>()
                .read_unaligned()
        };
        inet.sin_family = AF_INET as u16;
        inet.sin_zero = [0; 8];

        let mask = mask_from_prefix_v4(prefix);
        let netmask = SockaddrIn {
            sin_family: AF_INET as u16,
            sin_port: 0,
            sin_addr: mask.to_be(),
            sin_zero: [0; 8],
        };
        // The broadcast address is the host part set to all ones.
        let host_order = u32::from_be(inet.sin_addr);
        let broadcast = SockaddrIn {
            sin_family: AF_INET as u16,
            sin_port: 0,
            sin_addr: (host_order | !mask).to_be(),
            sin_zero: [0; 8],
        };
        return Some((
            struct_bytes(&inet),
            struct_bytes(&netmask),
            Some(struct_bytes(&broadcast)),
        ));
    }

    if family == AF_INET6 {
        if windows_sockaddr.len() < size_of::<SockaddrIn6>() {
            return None;
        }
        // SAFETY: the length check proved a whole sockaddr_in6 is readable.
        let mut inet6 = unsafe {
            windows_sockaddr
                .as_ptr()
                .cast::<SockaddrIn6>()
                .read_unaligned()
        };
        // This is the family that actually differs: 23 on the wire from Windows,
        // 10 in the struct the guest reads.
        inet6.sin6_family = AF_INET6 as u16;
        let netmask = SockaddrIn6 {
            sin6_family: AF_INET6 as u16,
            sin6_port: 0,
            sin6_flowinfo: 0,
            sin6_addr: mask_from_prefix_v6(prefix),
            sin6_scope_id: 0,
        };
        return Some((struct_bytes(&inet6), struct_bytes(&netmask), None));
    }
    // A family this module does not describe, such as AF_LINK-style entries.
    None
}

/// Copies a struct's bytes, for writing into an inline sockaddr slot.
fn struct_bytes<T>(value: &T) -> Vec<u8> {
    // SAFETY: reading a live, fully-initialized value as bytes. Every struct
    // passed here is `repr(C)` with no padding that could be uninitialized: both
    // sockaddr types are built field by field from initialized values.
    unsafe { core::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
        .to_vec()
}

/// `getifaddrs`.
///
/// One node per address, as on Linux, so an adapter with both an IPv4 and an IPv6
/// address appears twice under the same `ifa_name`. Interfaces with no address
/// are reported with a null `ifa_addr`, which is also what Linux does and what
/// lets `ifconfig -a` list a down interface.
///
/// `ifa_data` is always null. On Linux it carries `struct rtnl_link_stats`, and
/// Windows' per-adapter counters come from a different API with a different set of
/// fields; publishing a partially-filled statistics struct would be worse than
/// publishing none, since a caller cannot tell which fields were real.
///
/// # Safety
///
/// `list` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getifaddrs(list: *mut *mut Ifaddrs) -> c_int {
    if list.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a writable out-pointer.
    unsafe { *list = ptr::null_mut() };

    let interfaces = enumerate_interfaces();
    if interfaces.is_empty() {
        // A host with no interfaces at all is possible but vanishingly rare, so
        // this almost always means the query itself failed.
        set_errno(kinakaze_vfs::EIO);
        return -1;
    }

    let mut head: *mut Ifaddrs = ptr::null_mut();
    let mut tail: *mut Ifaddrs = ptr::null_mut();

    for interface in &interfaces {
        // An address of a family this module cannot describe is dropped here
        // rather than published with a family word the guest would misread.
        let described: Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> = interface
            .addresses
            .iter()
            .filter_map(|(bytes, prefix)| address_triple(bytes, *prefix))
            .collect();

        // An interface with no usable address still gets exactly one node, with a
        // null `ifa_addr`. That is what Linux reports, and it is what lets
        // `ifconfig -a` list an interface that is down.
        let slots: Vec<Option<&(Vec<u8>, Vec<u8>, Option<Vec<u8>>)>> = if described.is_empty() {
            vec![None]
        } else {
            described.iter().map(Some).collect()
        };

        for slot in slots {
            let node = allocate_ifaddrs_node(interface, slot);
            if node.is_null() {
                // SAFETY: the partial list is owned here and nothing else refers
                // to it.
                unsafe { kinakaze_abi_freeifaddrs(head) };
                set_errno(ENOMEM);
                return -1;
            }
            if tail.is_null() {
                head = node;
            } else {
                // SAFETY: `tail` is a node this loop allocated and still owns.
                unsafe { (*tail).ifa_next = node };
            }
            tail = node;
        }
    }

    if head.is_null() {
        set_errno(kinakaze_vfs::EIO);
        return -1;
    }
    // SAFETY: `list` was checked non-null above.
    unsafe { *list = head };
    0
}

/// Allocates and fills one `getifaddrs` node.
///
/// A `None` triple produces a node with a null `ifa_addr`, which is how an
/// interface with no address is reported.
fn allocate_ifaddrs_node(
    interface: &Interface,
    triple: Option<&(Vec<u8>, Vec<u8>, Option<Vec<u8>>)>,
) -> *mut Ifaddrs {
    // SAFETY: the block is handed to the guest and released by freeifaddrs.
    let block = unsafe { kinakaze_alloc::c::malloc(size_of::<IfaddrsNode>()) };
    if block.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the allocation covers a whole node.
    unsafe { ptr::write_bytes(block, 0, size_of::<IfaddrsNode>()) };
    let node = block.cast::<IfaddrsNode>();

    // The name is truncated to leave room for the terminator, which cannot
    // actually happen for the names this module generates.
    let name = interface.name.as_bytes();
    let name_length = name.len().min(31);
    // SAFETY: the node is allocated and zeroed, so the byte after the copy is
    // already a terminator.
    unsafe {
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            (&raw mut (*node).name).cast::<u8>(),
            name_length,
        );
    }

    let (address_pointer, netmask_pointer, broadcast_pointer) = match triple {
        None => (ptr::null_mut(), ptr::null_mut(), ptr::null_mut()),
        Some((address, netmask, broadcast)) => {
            // SAFETY: each slot is 32 bytes and every sockaddr written is at most
            // 28, so the copies stay inside their slots.
            unsafe {
                let address_slot = (&raw mut (*node).address).cast::<u8>();
                ptr::copy_nonoverlapping(address.as_ptr(), address_slot, address.len().min(32));
                let netmask_slot = (&raw mut (*node).netmask).cast::<u8>();
                ptr::copy_nonoverlapping(netmask.as_ptr(), netmask_slot, netmask.len().min(32));
                let broadcast_slot = match broadcast {
                    // Only filled when the flags claim IFF_BROADCAST, so
                    // `ifa_ifu` is never a value the flags say does not exist.
                    Some(broadcast) if interface.flags & IFF_BROADCAST != 0 => {
                        let slot = (&raw mut (*node).broadcast).cast::<u8>();
                        ptr::copy_nonoverlapping(broadcast.as_ptr(), slot, broadcast.len().min(32));
                        slot.cast::<c_void>()
                    }
                    _ => ptr::null_mut(),
                };
                (
                    address_slot.cast::<c_void>(),
                    netmask_slot.cast::<c_void>(),
                    broadcast_slot,
                )
            }
        }
    };

    // SAFETY: the node is allocated and its name buffer is populated.
    unsafe {
        (*node).public = Ifaddrs {
            ifa_next: ptr::null_mut(),
            ifa_name: (&raw mut (*node).name).cast::<c_char>(),
            ifa_flags: interface.flags,
            ifa_addr: address_pointer,
            ifa_netmask: netmask_pointer,
            ifa_ifu: broadcast_pointer,
            // See the note in `kinakaze_abi_getifaddrs` on why this is null.
            ifa_data: ptr::null_mut(),
        };
    }
    // The public header is first, so this is both the node and the allocation.
    node.cast::<Ifaddrs>()
}

/// `freeifaddrs`.
///
/// Each node is one allocation holding its name and every sockaddr it points at,
/// so one `free` per node releases the lot.
///
/// # Safety
///
/// `list` must be null or a list this module's `getifaddrs` returned, and must not
/// be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_freeifaddrs(list: *mut Ifaddrs) {
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: the caller guarantees a list this module built.
        let next = unsafe { (*cursor).ifa_next };
        // SAFETY: the node pointer is the allocation pointer by construction in
        // `allocate_ifaddrs_node`.
        unsafe { kinakaze_alloc::c::free(cursor.cast::<u8>()) };
        cursor = next;
    }
}

/// `gethostid`.
///
/// Linux returns a 32-bit identifier that is supposed to be unique to the
/// machine and stable across reboots. glibc reads `/etc/hostid` and falls back to
/// resolving the hostname to an address, which is a poor source: a DHCP lease
/// change alters it.
///
/// The value here is an FNV-1a hash of the **MAC address of the lowest-indexed
/// non-loopback interface that has one**. A MAC is burned into the hardware, so
/// it survives reboots, IP changes and renames, which is exactly the property the
/// interface asks for. Ordering by interface index makes the choice deterministic
/// on a machine with several adapters.
///
/// When no adapter reports a MAC — a virtual machine with only synthetic
/// loopback, for instance — the computer name is hashed instead. That is weaker,
/// because renaming the machine changes it, so it is a documented fallback rather
/// than the primary source. It is still derived from something real; no constant
/// is returned.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_gethostid() -> kinakaze_abi::Long {
    static HOST_ID: OnceLock<u32> = OnceLock::new();
    let id = *HOST_ID.get_or_init(|| {
        let interfaces = enumerate_interfaces();
        let mac = interfaces
            .iter()
            .filter(|interface| {
                interface.flags & IFF_LOOPBACK == 0 && !interface.physical.is_empty()
            })
            // Already ordered by index, so the first match is the lowest.
            .map(|interface| interface.physical.clone())
            .next();

        match mac {
            Some(mac) => fnv1a(&mac),
            None => {
                // The documented fallback. `COMPUTERNAME` is set for every
                // process on Windows.
                let name = std::env::var("COMPUTERNAME").unwrap_or_default();
                fnv1a(name.as_bytes())
            }
        }
    });
    // Linux's `gethostid` returns a `long` carrying a 32-bit value. Widening from
    // `u32` keeps it positive rather than sign-extending a high hash into a negative
    // number, which some callers print.
    //
    // The return type is `kinakaze_abi::Long`, not `core::ffi::c_long`: the latter is
    // the *host's* long, which is 32-bit on Windows, so it would return half a value
    // into the 64-bit register the guest reads.
    kinakaze_abi::Long::from(id)
}

/// FNV-1a over 32 bits.
///
/// Chosen because it is fully specified and has no seed, so the same MAC produces
/// the same host id on every run and in every process — which is what makes the
/// identifier stable rather than merely unique.
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

// ---------------------------------------------------------------------------
// Scatter/gather messages.
//
// These go through `kinakaze_vfs::socket`, not Winsock directly, and that is a
// requirement rather than a convenience. The descriptor table is authoritative
// about what an fd is: `socket::sendto` dispatches an `AF_UNIX` fd to the
// named-pipe implementation in `kinakaze_vfs::unix` and a Winsock fd to
// `WSASendTo`, and it reconstructs the guest's blocking semantics on top of a
// socket that is always non-blocking at the OS level. Calling `WSASend` with a
// translated `WSABUF` array would be one copy cheaper and would lose all of that:
// a Unix-domain fd would be handed to Winsock as a handle it does not own.
//
// The cost is that the iovec array is coalesced into one buffer instead of being
// handed to the OS as a vector. For a datagram that is required anyway, since the
// whole message must be one write to preserve the record boundary.
// ---------------------------------------------------------------------------

/// `CMSG_ALIGN`: rounds a length up to a `size_t` boundary.
///
/// This is the arithmetic the `CMSG_*` macros are built from, and it must match
/// glibc exactly or a caller walking a control buffer lands between headers.
const fn cmsg_align(length: usize) -> usize {
    (length + size_of::<usize>() - 1) & !(size_of::<usize>() - 1)
}

/// `__cmsg_nxthdr`, the out-of-line helper behind the `CMSG_NXTHDR` macro.
///
/// Advances past `cmsg` by its aligned length and returns the next header, or null
/// when the buffer holds no room for one. The two bounds checks are what the macro
/// relies on for safety: a control buffer arrives from the network in the
/// `recvmsg` case, so a `cmsg_len` that overruns the buffer is untrusted input, not
/// a programming error.
///
/// # Safety
///
/// `msg` must point at a readable `struct msghdr` and `cmsg` at a readable
/// `struct cmsghdr` inside that message's control buffer.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cmsg_nxthdr(
    msg: *mut MsgHdr,
    cmsg: *mut CmsgHdr,
) -> *mut CmsgHdr {
    if msg.is_null() || cmsg.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a readable msghdr.
    let (control, control_length) = unsafe { ((*msg).msg_control, (*msg).msg_controllen) };
    if control.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a readable cmsghdr.
    let length = unsafe { (*cmsg).cmsg_len };
    // A header shorter than a header cannot be advanced past, and treating it as
    // zero-length would loop forever.
    if length < size_of::<CmsgHdr>() {
        return ptr::null_mut();
    }

    let base = control as usize;
    let end = base + control_length;
    // The next header starts at this one's aligned end. glibc aligns the step, not
    // `cmsg_len` itself, which stays the unpadded header-plus-data length.
    let next = cmsg as usize + cmsg_align(length);
    // Room for a whole header is required, not just for its first byte: the caller
    // will read `cmsg_len` out of whatever this returns.
    if next < base || next + size_of::<CmsgHdr>() > end {
        return ptr::null_mut();
    }
    next as *mut CmsgHdr
}

/// Reads a `msghdr`'s iovec array as a slice.
///
/// # Safety
///
/// `msg` must point at a readable `struct msghdr` whose `msg_iov` names
/// `msg_iovlen` readable entries.
unsafe fn iovecs(msg: *const MsgHdr) -> Result<&'static [IoVec], i32> {
    // SAFETY: the caller guarantees a readable msghdr.
    let (base, count) = unsafe { ((*msg).msg_iov, (*msg).msg_iovlen) };
    if count == 0 {
        return Ok(&[]);
    }
    if base.is_null() {
        return Err(EFAULT);
    }
    // Linux's own limit. A larger count is a caller error rather than a request.
    if count > 1024 {
        return Err(EINVAL);
    }
    // SAFETY: the caller guarantees `count` readable entries at `base`.
    Ok(unsafe { core::slice::from_raw_parts(base, count) })
}

/// Copies every iovec into one contiguous buffer.
///
/// # Safety
///
/// Each iovec must be readable for its stated length.
unsafe fn gather(vectors: &[IoVec]) -> Result<Vec<u8>, i32> {
    let total: usize = vectors.iter().map(|vector| vector.iov_len).sum();
    let mut buffer = Vec::with_capacity(total);
    for vector in vectors {
        if vector.iov_len == 0 {
            continue;
        }
        if vector.iov_base.is_null() {
            return Err(EFAULT);
        }
        // SAFETY: the caller guarantees `iov_len` readable bytes at `iov_base`.
        let slice =
            unsafe { core::slice::from_raw_parts(vector.iov_base.cast::<u8>(), vector.iov_len) };
        buffer.extend_from_slice(slice);
    }
    Ok(buffer)
}

/// Distributes a received buffer back across the iovecs.
///
/// Returns the number of bytes placed, which is `data.len()` unless the vectors
/// hold less room than that.
///
/// # Safety
///
/// Each iovec must be writable for its stated length.
unsafe fn scatter(vectors: &[IoVec], data: &[u8]) -> usize {
    let mut written = 0usize;
    for vector in vectors {
        if written == data.len() {
            break;
        }
        if vector.iov_len == 0 || vector.iov_base.is_null() {
            continue;
        }
        let count = vector.iov_len.min(data.len() - written);
        // SAFETY: the caller guarantees `iov_len` writable bytes, and `count` is
        // bounded by it as well as by the data remaining.
        unsafe {
            ptr::copy_nonoverlapping(
                data.as_ptr().add(written),
                vector.iov_base.cast::<u8>(),
                count,
            );
        }
        written += count;
    }
    written
}

/// Reports whether a control buffer asks for something this layer cannot do.
///
/// Parse and validate all Unix control messages before transferring payload.
///
/// # Safety
///
/// `control` must be readable for `length` bytes.
unsafe fn socket_control(
    control: *const c_void,
    length: usize,
) -> Result<(Vec<i32>, Option<kinakaze_vfs::unix::credentials::Sender>), i32> {
    if length == 0 {
        return Ok((Vec::new(), None));
    }
    if control.is_null() {
        return Err(EFAULT);
    }
    let mut rights = Vec::new();
    let mut credentials = None;
    let mut offset = 0usize;
    while offset + size_of::<CmsgHdr>() <= length {
        // SAFETY: the bound above proved a whole header is readable here.
        let header = unsafe {
            control
                .cast::<u8>()
                .add(offset)
                .cast::<CmsgHdr>()
                .read_unaligned()
        };
        if header.cmsg_len < size_of::<CmsgHdr>() || header.cmsg_len > length - offset {
            return Err(EINVAL);
        }
        let data = header.cmsg_len - size_of::<CmsgHdr>();
        let payload = unsafe { control.cast::<u8>().add(offset + size_of::<CmsgHdr>()) };
        if header.cmsg_level == SOL_SOCKET {
            match header.cmsg_type {
                SCM_RIGHTS => {
                    if !data.is_multiple_of(size_of::<i32>()) || rights.len() + data / 4 > 253 {
                        return Err(EINVAL);
                    }
                    for index in 0..data / 4 {
                        rights
                            .push(unsafe { payload.add(index * 4).cast::<i32>().read_unaligned() });
                    }
                }
                SCM_CREDENTIALS => {
                    use kinakaze_vfs::unix::credentials::{Ucred, validate};
                    if data != size_of::<Ucred>() {
                        return Err(EINVAL);
                    }
                    credentials = Some(validate(unsafe {
                        payload.cast::<Ucred>().read_unaligned()
                    })?);
                }
                _ => return Err(EINVAL),
            }
        }
        offset = offset
            .checked_add(cmsg_align(header.cmsg_len))
            .ok_or(EINVAL)?;
    }
    Ok((rights, credentials))
}

/// `sendmsg`.
///
/// Validates ancillary headers before sending any payload. Descriptor references
/// are escrowed until the receiver accepts them or closes the socket.
///
/// # Safety
///
/// `msg` must point at a readable `struct msghdr` whose iovecs and control buffer
/// are readable for their stated lengths.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sendmsg(
    fd: c_int,
    msg: *const MsgHdr,
    flags: c_int,
) -> isize {
    if msg.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable msghdr.
    let (control, control_length, name, name_length) = unsafe {
        (
            (*msg).msg_control,
            (*msg).msg_controllen,
            (*msg).msg_name,
            (*msg).msg_namelen,
        )
    };

    let (rights, credentials) = match unsafe { socket_control(control, control_length) } {
        Ok(rights) => rights,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };

    // SAFETY: forwarded from this function's contract.
    let vectors = match unsafe { iovecs(msg) } {
        Ok(vectors) => vectors,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    // SAFETY: forwarded from this function's contract.
    let buffer = match unsafe { gather(vectors) } {
        Ok(buffer) => buffer,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };

    // An unconnected datagram send carries its destination in `msg_name`.
    // `socket::sendto` is what routes an AF_UNIX fd to the named-pipe path.
    let result = if !rights.is_empty() || credentials.is_some() {
        if !kinakaze_vfs::unix::is_unix_socket(fd) {
            Err(EOPNOTSUPP)
        } else {
            unsafe {
                kinakaze_vfs::unix::validate_destination(fd, name.cast(), name_length as i32)
                    .and_then(|()| {
                        kinakaze_vfs::unix::send_control(
                            fd,
                            buffer.as_ptr(),
                            buffer.len(),
                            flags,
                            &rights,
                            credentials.as_ref(),
                        )
                    })
            }
        }
    } else if name.is_null() || name_length == 0 {
        // SAFETY: `buffer` is a live Vec of its own length.
        unsafe { socket::send(fd, buffer.as_ptr(), buffer.len(), flags) }
    } else {
        // SAFETY: `buffer` is live, and the caller guarantees `msg_namelen`
        // readable bytes at `msg_name`.
        unsafe {
            socket::sendto(
                fd,
                buffer.as_ptr(),
                buffer.len(),
                flags,
                name.cast::<u8>(),
                name_length as c_int,
            )
        }
    };

    match result {
        Ok(sent) => sent as isize,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `recvmsg`.
///
/// Unix descriptor references honor MSG_CMSG_CLOEXEC and MSG_PEEK. A short
/// control buffer sets MSG_CTRUNC and releases references that do not fit.
///
/// `MSG_TRUNC` **is** reported. Windows and Linux disagree here in a way that
/// matters: an oversized datagram makes `WSARecvFrom` fail with `WSAEMSGSIZE`
/// after filling the buffer, while Linux succeeds, fills the buffer and sets
/// `MSG_TRUNC`. The Windows failure is translated back into the Linux success so a
/// caller reading `msg_flags` learns the truth, rather than seeing an `EMSGSIZE`
/// that Linux never returns from `recvmsg`.
///
/// # Safety
///
/// `msg` must point at a writable `struct msghdr` whose iovecs are writable for
/// their stated lengths.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_recvmsg(
    fd: c_int,
    msg: *mut MsgHdr,
    flags: c_int,
) -> isize {
    if msg.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    // SAFETY: forwarded from this function's contract.
    let vectors = match unsafe { iovecs(msg) } {
        Ok(vectors) => vectors,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    let capacity: usize = vectors.iter().map(|vector| vector.iov_len).sum();

    // SAFETY: the caller guarantees a readable msghdr.
    let (name, name_capacity) = unsafe { ((*msg).msg_name, (*msg).msg_namelen) };
    let mut scratch = vec![0u8; capacity];
    let mut address_length = name_capacity as c_int;

    // `socket::recvfrom` dispatches an AF_UNIX fd to the named-pipe path and a
    // Winsock fd to WSARecvFrom, and reconstructs the blocking semantics the
    // descriptor asked for.
    // SAFETY: `scratch` is a live buffer of `capacity` bytes, and `msg_name` is
    // either null or writable for `msg_namelen` bytes per the caller's contract.
    let control_capacity = unsafe {
        if (*msg).msg_control.is_null() {
            0
        } else {
            (*msg).msg_controllen
        }
    };
    let mut rights = Vec::new();
    let mut credentials = None;
    let mut ancillary_flags = 0;
    let result = if kinakaze_vfs::unix::is_unix_socket(fd) {
        unsafe {
            kinakaze_vfs::unix::recv_control(
                fd,
                scratch.as_mut_ptr(),
                capacity,
                flags,
                control_capacity,
                true,
            )
        }
        .map(|(received, fds, truncated, sender)| {
            rights = fds;
            credentials = sender;
            ancillary_flags = truncated;
            if !name.is_null() {
                if unsafe {
                    kinakaze_vfs::unix::getpeername(fd, name.cast(), &raw mut address_length)
                }
                .is_err()
                {
                    address_length = 0;
                }
            }
            received
        })
    } else {
        unsafe {
            socket::recvfrom(
                fd,
                scratch.as_mut_ptr(),
                capacity,
                flags,
                if name.is_null() {
                    ptr::null_mut()
                } else {
                    name.cast::<u8>()
                },
                if name.is_null() {
                    ptr::null_mut()
                } else {
                    &raw mut address_length
                },
            )
        }
    };

    let (received, truncated) = match result {
        Ok(received) => (received, false),
        // The Windows/Linux divergence this function's documentation describes.
        // The buffer was filled; only the overflow was lost.
        Err(EMSGSIZE) => (capacity, true),
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };

    let placed = if received == 0 {
        0
    } else {
        // SAFETY: forwarded from this function's contract.
        unsafe { scatter(vectors, &scratch[..received.min(capacity)]) }
    };

    // SAFETY: the caller guarantees a writable msghdr.
    unsafe {
        if !name.is_null() {
            // `recvfrom` reports the untruncated length, which is what
            // `msg_namelen` is specified to carry on return.
            (*msg).msg_namelen = address_length.max(0) as u32;
        }
        let mut result_flags = 0;
        if truncated || placed < received {
            result_flags |= MSG_TRUNC;
        }
        result_flags |= ancillary_flags;
        (*msg).msg_flags = result_flags;
        (*msg).msg_controllen = 0;
        if let Some(credentials) = credentials {
            let required = size_of::<CmsgHdr>() + size_of_val(&credentials);
            if control_capacity < required {
                (*msg).msg_flags |= 8;
            }
            if control_capacity >= size_of::<CmsgHdr>() {
                let length = required.min(control_capacity);
                let used = cmsg_align(length).min(control_capacity);
                let out = (*msg).msg_control.cast::<u8>();
                ptr::write_bytes(out, 0, used);
                out.cast::<CmsgHdr>().write_unaligned(CmsgHdr {
                    cmsg_len: length,
                    cmsg_level: SOL_SOCKET,
                    cmsg_type: SCM_CREDENTIALS,
                });
                ptr::copy_nonoverlapping(
                    (&raw const credentials).cast::<u8>(),
                    out.add(size_of::<CmsgHdr>()),
                    length - size_of::<CmsgHdr>(),
                );
                (*msg).msg_controllen = used;
            }
        }
        if !rights.is_empty() {
            let offset = (*msg).msg_controllen;
            let length = size_of::<CmsgHdr>() + rights.len() * 4;
            let used = cmsg_align(length).min(control_capacity - offset);
            ptr::write_bytes((*msg).msg_control.cast::<u8>().add(offset), 0, used);
            (*msg)
                .msg_control
                .cast::<u8>()
                .add(offset)
                .cast::<CmsgHdr>()
                .write_unaligned(CmsgHdr {
                    cmsg_len: length,
                    cmsg_level: SOL_SOCKET,
                    cmsg_type: SCM_RIGHTS,
                });
            for (index, fd) in rights.into_iter().enumerate() {
                (*msg)
                    .msg_control
                    .cast::<u8>()
                    .add(offset + size_of::<CmsgHdr>() + index * 4)
                    .cast::<i32>()
                    .write_unaligned(fd);
            }
            (*msg).msg_controllen = offset + used;
        }
    }
    if flags & MSG_TRUNC != 0 {
        received as isize
    } else {
        placed as isize
    }
}

#[repr(C)]
pub struct IfNameIndex {
    pub if_index: c_uint,
    pub if_name: *mut c_char,
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_if_nameindex() -> *mut IfNameIndex {
    let buf = unsafe { kinakaze_alloc::c::malloc(core::mem::size_of::<IfNameIndex>() * 2) }
        as *mut IfNameIndex;
    if buf.is_null() {
        crate::set_errno(kinakaze_vfs::ENOMEM);
        return ptr::null_mut();
    }
    let lo_name = unsafe { kinakaze_alloc::c::malloc(3) } as *mut c_char;
    if !lo_name.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(b"lo\0".as_ptr() as *const c_char, lo_name, 3);
        }
    }
    unsafe {
        *buf.add(0) = IfNameIndex {
            if_index: 1,
            if_name: lo_name,
        };
        *buf.add(1) = IfNameIndex {
            if_index: 0,
            if_name: ptr::null_mut(),
        };
    }
    buf
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_if_freenameindex(ptr: *mut IfNameIndex) {
    if !ptr.is_null() {
        unsafe {
            let mut cur = ptr;
            while (*cur).if_index != 0 {
                if !(*cur).if_name.is_null() {
                    kinakaze_alloc::c::free((*cur).if_name.cast());
                }
                cur = cur.add(1);
            }
            kinakaze_alloc::c::free(ptr.cast());
        }
    }
}

#[repr(C)]
pub struct Protoent {
    pub p_name: *mut c_char,
    pub p_aliases: *mut *mut c_char,
    pub p_proto: c_int,
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gethostbyname_r(
    name: *const c_char,
    ret: *mut Hostent,
    buf: *mut c_char,
    buflen: usize,
    result: *mut *mut Hostent,
    h_errnop: *mut c_int,
) -> c_int {
    let h = unsafe { kinakaze_abi_gethostbyname(name) };
    unsafe { reentrant::copy_host(h, ret, buf, buflen, result, h_errnop) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gethostbyaddr_r(
    addr: *const c_void,
    len: c_uint,
    addr_type: c_int,
    ret: *mut Hostent,
    buf: *mut c_char,
    buflen: usize,
    result: *mut *mut Hostent,
    h_errnop: *mut c_int,
) -> c_int {
    let h = unsafe { kinakaze_abi_gethostbyaddr(addr, len, addr_type) };
    unsafe { reentrant::copy_host(h, ret, buf, buflen, result, h_errnop) }
}

// ---------------------------------------------------------------------------
// libresolv compatibility.
//
// glibc 2.34 folded the public resolver entry points into libc.  OpenSSH built
// against that version therefore has no DT_NEEDED for libresolv even though it
// imports these four names from GLIBC_2.34.  Keep the implementation here (and
// not in the loader) so every guest gets the same wire-format DNS semantics.
// ---------------------------------------------------------------------------

const RES_INIT: u64 = 0x0000_0001;
const RES_RECURSE: u64 = 0x0000_0040;
const RES_DEFNAMES: u64 = 0x0000_0080;
const RES_DNSRCH: u64 = 0x0000_0200;

#[repr(C)]
struct ResolverSortEntry {
    address: [u8; 4],
    mask: u32,
}

/// glibc x86_64's public `struct __res_state` layout.
///
/// Programs are allowed to inspect its leading fields and OpenSSH does so via
/// `_res.options`.  The opaque tail is nevertheless represented at its real
/// offsets so a guest compiled against `<resolv.h>` can safely use the complete
/// 560-byte object.
#[repr(C)]
pub struct ResState {
    retrans: c_int,
    retry: c_int,
    options: u64,
    nscount: c_int,
    nsaddr_list: [[u8; 16]; 3],
    id: u16,
    dnsrch: [*mut c_char; 7],
    defdname: [c_char; 256],
    pfcode: u64,
    state_flags: u32,
    sort_list: [ResolverSortEntry; 10],
    qhook: *mut c_void,
    rhook: *mut c_void,
    res_h_errno: c_int,
    vcsock: c_int,
    flags: u32,
    extension: [u8; 52],
}

impl ResState {
    const fn new() -> Self {
        Self {
            retrans: 5,
            retry: 2,
            options: RES_INIT | RES_RECURSE | RES_DEFNAMES | RES_DNSRCH,
            nscount: 0,
            nsaddr_list: [[0; 16]; 3],
            id: 0,
            dnsrch: [ptr::null_mut(); 7],
            defdname: [0; 256],
            pfcode: 0,
            state_flags: 0,
            sort_list: [const {
                ResolverSortEntry {
                    address: [0; 4],
                    mask: 0,
                }
            }; 10],
            qhook: ptr::null_mut(),
            rhook: ptr::null_mut(),
            res_h_errno: 0,
            vcsock: -1,
            flags: 0,
            extension: [0; 52],
        }
    }
}

const _: () = assert!(core::mem::size_of::<ResState>() == 560);

thread_local! {
    static RESOLVER_STATE: core::cell::UnsafeCell<ResState> =
        const { core::cell::UnsafeCell::new(ResState::new()) };
}

/// Returns the DNS servers Windows actually configured.
fn resolver_servers() -> Vec<std::net::IpAddr> {
    kinakaze_vfs::fs::active_dns_servers()
}

fn initialize_resolver_state(state: &mut ResState) {
    *state = ResState::new();
    let mut count = 0;
    for server in resolver_servers() {
        if count >= 3 {
            break;
        }
        let std::net::IpAddr::V4(address) = server else {
            continue;
        };
        let slot = &mut state.nsaddr_list[count];
        // Linux sockaddr_in: native-endian family, network-endian port/address.
        slot[0..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
        slot[2..4].copy_from_slice(&53u16.to_be_bytes());
        slot[4..8].copy_from_slice(&address.octets());
        count += 1;
    }
    state.nscount = count as c_int;
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___res_init() -> c_int {
    RESOLVER_STATE.with(|slot| {
        // SAFETY: this is the current thread's unique TLS cell.
        initialize_resolver_state(unsafe { &mut *slot.get() });
    });
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___res_state() -> *mut ResState {
    RESOLVER_STATE.with(core::cell::UnsafeCell::get)
}

fn encode_dns_name(name: &[u8], packet: &mut Vec<u8>) -> Result<(), ()> {
    let name = name.strip_suffix(b".").unwrap_or(name);
    if name.is_empty() {
        packet.push(0);
        return Ok(());
    }
    let start = packet.len();
    for label in name.split(|byte| *byte == b'.') {
        if label.is_empty() || label.len() > 63 {
            return Err(());
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label);
    }
    packet.push(0);
    if packet.len() - start > 255 {
        return Err(());
    }
    Ok(())
}

fn build_dns_query(name: &[u8], class: u16, record_type: u16) -> Result<Vec<u8>, ()> {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT_ID: AtomicU16 = AtomicU16::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut packet = Vec::with_capacity(name.len() + 18);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    packet.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    packet.extend_from_slice(&[0; 6]);
    encode_dns_name(name, &mut packet)?;
    packet.extend_from_slice(&record_type.to_be_bytes());
    packet.extend_from_slice(&class.to_be_bytes());
    Ok(packet)
}

fn receive_dns_udp(server: std::net::IpAddr, query: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::time::Duration;
    let bind = match server {
        std::net::IpAddr::V4(_) => "0.0.0.0:0",
        std::net::IpAddr::V6(_) => "[::]:0",
    };
    let socket = std::net::UdpSocket::bind(bind)?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    socket.connect(std::net::SocketAddr::new(server, 53))?;
    socket.send(query)?;
    let mut answer = vec![0u8; 65_535];
    let length = socket.recv(&mut answer)?;
    answer.truncate(length);
    Ok(answer)
}

fn receive_dns_tcp(server: std::net::IpAddr, query: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Write};
    use std::time::Duration;
    let address = std::net::SocketAddr::new(server, 53);
    let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(&(query.len() as u16).to_be_bytes())?;
    stream.write_all(query)?;
    let mut length = [0u8; 2];
    stream.read_exact(&mut length)?;
    let mut answer = vec![0u8; u16::from_be_bytes(length) as usize];
    stream.read_exact(&mut answer)?;
    Ok(answer)
}

fn exchange_dns(query: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut last_error = None;
    for server in resolver_servers() {
        match receive_dns_udp(server, query) {
            Ok(answer) if answer.len() >= 12 && answer[..2] == query[..2] => {
                // TC is bit 1 of the response flags byte. Retry over TCP exactly
                // as libresolv does instead of handing a truncated RR set back.
                if answer[2] & 0x02 != 0 {
                    match receive_dns_tcp(server, query) {
                        Ok(answer) => return Ok(answer),
                        Err(error) => last_error = Some(error),
                    }
                } else {
                    return Ok(answer);
                }
            }
            Ok(_) => {}
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("no DNS server answered")))
}

/// `res_query`: issue one DNS wire-format question and return the response.
///
/// # Safety
///
/// `dname` must be NUL-terminated and `answer` writable for `answer_length`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_query(
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    answer: *mut u8,
    answer_length: c_int,
) -> c_int {
    if dname.is_null() || answer.is_null() || answer_length < 12 {
        set_errno(EINVAL);
        set_h_errno(NO_RECOVERY);
        return -1;
    }
    // SAFETY: guaranteed by this function's contract.
    let name = unsafe { CStr::from_ptr(dname) }.to_bytes();
    let (Ok(class), Ok(record_type)) = (u16::try_from(class), u16::try_from(record_type)) else {
        set_errno(EINVAL);
        set_h_errno(NO_RECOVERY);
        return -1;
    };
    let Ok(query) = build_dns_query(name, class, record_type) else {
        set_errno(EINVAL);
        set_h_errno(NO_RECOVERY);
        return -1;
    };
    let response = match exchange_dns(&query) {
        Ok(response) if response.len() >= 12 => response,
        Ok(_) => {
            set_h_errno(NO_RECOVERY);
            return -1;
        }
        Err(_) => {
            set_h_errno(TRY_AGAIN);
            return -1;
        }
    };

    let rcode = response[3] & 0x0f;
    if rcode != 0 {
        set_h_errno(if rcode == 3 {
            HOST_NOT_FOUND
        } else {
            NO_RECOVERY
        });
        return -1;
    }
    let capacity = answer_length as usize;
    let copied = response.len().min(capacity);
    // SAFETY: the caller supplied this writable answer buffer.
    unsafe { ptr::copy_nonoverlapping(response.as_ptr(), answer, copied) };
    if response.len() > capacity && copied >= 3 {
        // Tell the wire-format consumer that the local buffer truncated the
        // response, just as a resolver receiving a TC response would.
        unsafe { *answer.add(2) |= 0x02 };
    }
    set_h_errno(0);
    copied as c_int
}

/// `__res_query`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_query(
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    answer: *mut u8,
    answer_length: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_query(dname, class, record_type, answer, answer_length) }
}

/// `res_search`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_search(
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    answer: *mut u8,
    answer_length: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_query(dname, class, record_type, answer, answer_length) }
}

/// `__res_search`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_search(
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    answer: *mut u8,
    answer_length: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_query(dname, class, record_type, answer, answer_length) }
}

/// `res_mkquery`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_mkquery(
    _op: c_int,
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    _data: *const c_void,
    _datalen: c_int,
    _newrr: *const c_void,
    buf: *mut u8,
    buflen: c_int,
) -> c_int {
    if dname.is_null() || buf.is_null() || buflen < 12 {
        set_errno(EINVAL);
        set_h_errno(NO_RECOVERY);
        return -1;
    }
    // SAFETY: guaranteed by this function's contract.
    let name = unsafe { CStr::from_ptr(dname) }.to_bytes();
    let (Ok(class), Ok(record_type)) = (u16::try_from(class), u16::try_from(record_type)) else {
        set_errno(EINVAL);
        set_h_errno(NO_RECOVERY);
        return -1;
    };
    let Ok(query) = build_dns_query(name, class, record_type) else {
        set_errno(EINVAL);
        set_h_errno(NO_RECOVERY);
        return -1;
    };
    if query.len() > buflen as usize {
        set_errno(kinakaze_vfs::EMSGSIZE);
        set_h_errno(NO_RECOVERY);
        return -1;
    }
    unsafe { ptr::copy_nonoverlapping(query.as_ptr(), buf, query.len()) };
    query.len() as c_int
}

/// `__res_mkquery`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_mkquery(
    op: c_int,
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    data: *const c_void,
    datalen: c_int,
    newrr: *const c_void,
    buf: *mut u8,
    buflen: c_int,
) -> c_int {
    unsafe {
        kinakaze_abi_res_mkquery(
            op,
            dname,
            class,
            record_type,
            data,
            datalen,
            newrr,
            buf,
            buflen,
        )
    }
}

/// `res_ninit`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_ninit(state: *mut ResState) -> c_int {
    if state.is_null() {
        return -1;
    }
    initialize_resolver_state(unsafe { &mut *state });
    0
}

/// `__res_ninit`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_ninit(state: *mut ResState) -> c_int {
    unsafe { kinakaze_abi_res_ninit(state) }
}

/// `res_nclose`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_nclose(_state: *mut ResState) {}

/// `__res_nclose`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_nclose(state: *mut ResState) {
    unsafe { kinakaze_abi_res_nclose(state) }
}

/// `res_nmkquery`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_nmkquery(
    _state: *mut ResState,
    op: c_int,
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    data: *const c_void,
    datalen: c_int,
    newrr: *const c_void,
    buf: *mut u8,
    buflen: c_int,
) -> c_int {
    unsafe {
        kinakaze_abi_res_mkquery(
            op,
            dname,
            class,
            record_type,
            data,
            datalen,
            newrr,
            buf,
            buflen,
        )
    }
}

/// `__res_nmkquery`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_nmkquery(
    state: *mut ResState,
    op: c_int,
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    data: *const c_void,
    datalen: c_int,
    newrr: *const c_void,
    buf: *mut u8,
    buflen: c_int,
) -> c_int {
    unsafe {
        kinakaze_abi_res_nmkquery(
            state,
            op,
            dname,
            class,
            record_type,
            data,
            datalen,
            newrr,
            buf,
            buflen,
        )
    }
}

/// `res_nquery`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_nquery(
    _state: *mut ResState,
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    answer: *mut u8,
    answer_length: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_query(dname, class, record_type, answer, answer_length) }
}

/// `__res_nquery`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_nquery(
    state: *mut ResState,
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    answer: *mut u8,
    answer_length: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_nquery(state, dname, class, record_type, answer, answer_length) }
}

/// `res_nsearch`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_nsearch(
    _state: *mut ResState,
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    answer: *mut u8,
    answer_length: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_query(dname, class, record_type, answer, answer_length) }
}

/// `__res_nsearch`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_nsearch(
    state: *mut ResState,
    dname: *const c_char,
    class: c_int,
    record_type: c_int,
    answer: *mut u8,
    answer_length: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_nsearch(state, dname, class, record_type, answer, answer_length) }
}

/// `res_nsend`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_nsend(
    _state: *mut ResState,
    msg: *const u8,
    msglen: c_int,
    answer: *mut u8,
    anslen: c_int,
) -> c_int {
    if msg.is_null() || answer.is_null() || msglen < 12 || anslen < 12 {
        return -1;
    }
    // SAFETY: the caller guarantees readable slice for `msglen`.
    let query = unsafe { core::slice::from_raw_parts(msg, msglen as usize) };
    let response = match exchange_dns(query) {
        Ok(response) if response.len() >= 12 => response,
        _ => return -1,
    };
    let copied = response.len().min(anslen as usize);
    // SAFETY: the caller supplied this writable answer buffer.
    unsafe { ptr::copy_nonoverlapping(response.as_ptr(), answer, copied) };
    copied as c_int
}

/// `__res_nsend`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_nsend(
    state: *mut ResState,
    msg: *const u8,
    msglen: c_int,
    answer: *mut u8,
    anslen: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_nsend(state, msg, msglen, answer, anslen) }
}

/// `res_send`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_res_send(
    msg: *const u8,
    msglen: c_int,
    answer: *mut u8,
    anslen: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_nsend(ptr::null_mut(), msg, msglen, answer, anslen) }
}

/// `__res_send`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___res_send(
    msg: *const u8,
    msglen: c_int,
    answer: *mut u8,
    anslen: c_int,
) -> c_int {
    unsafe { kinakaze_abi_res_send(msg, msglen, answer, anslen) }
}

/// Expands one possibly-compressed DNS name.
///
/// The return value counts bytes consumed at `compressed`, not bytes followed
/// through compression pointers.  That distinction is required to advance to
/// the QTYPE/RDATA field after a compressed owner name.
///
/// # Safety
///
/// The message range must be readable, and `expanded` writable for `length`.
/// Canonical resolver wire-name expansion shared by libc and libresolv.
pub fn expand_dns_name(bytes: &[u8], start: usize) -> Result<(Vec<u8>, usize), i32> {
    let mut at = start;
    let mut consumed = 0;
    let mut jumped = false;
    let mut output = Vec::new();
    let mut wire_length = 1usize;
    let mut seen = vec![false; bytes.len()];
    loop {
        let len = *bytes.get(at).ok_or(90)?;
        if seen[at] {
            return Err(90);
        }
        seen[at] = true;
        if len & 0xc0 == 0xc0 {
            let low = *bytes.get(at + 1).ok_or(90)?;
            if !jumped {
                consumed += 2;
            }
            jumped = true;
            at = (((len & 0x3f) as usize) << 8) | low as usize;
            continue;
        }
        if len & 0xc0 != 0 {
            return Err(90);
        }
        at += 1;
        if !jumped {
            consumed += 1 + len as usize;
        }
        if len == 0 {
            break;
        }
        wire_length += len as usize + 1;
        if wire_length > 255 {
            return Err(90);
        }
        let label = bytes.get(at..at + len as usize).ok_or(90)?;
        if !output.is_empty() {
            output.push(b'.');
        }
        for &octet in label {
            if b"\".;\\()@$".contains(&octet) {
                output.push(b'\\');
                output.push(octet);
            } else if !(0x21..=0x7e).contains(&octet) {
                output.extend_from_slice(&[
                    b'\\',
                    b'0' + octet / 100,
                    b'0' + (octet / 10) % 10,
                    b'0' + octet % 10,
                ]);
            } else {
                output.push(octet);
            }
        }
        at += len as usize;
    }
    if output.is_empty() {
        output.push(b'.');
    }
    Ok((output, consumed))
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ns_name_uncompress(
    message: *const u8,
    message_end: *const u8,
    compressed: *const u8,
    expanded: *mut c_char,
    capacity: usize,
) -> c_int {
    let start = message as usize;
    let end = message_end as usize;
    let source = compressed as usize;
    if start == 0
        || expanded.is_null()
        || end <= start
        || end - start > 65535
        || !(start..end).contains(&source)
    {
        set_errno(EMSGSIZE);
        return -1;
    }
    let bytes = unsafe { core::slice::from_raw_parts(message, end - start) };
    let (name, consumed) = match expand_dns_name(bytes, source - start) {
        Ok(value) => value,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    if name.len() >= capacity {
        set_errno(EMSGSIZE);
        return -1;
    }
    unsafe {
        ptr::copy_nonoverlapping(name.as_ptr(), expanded.cast::<u8>(), name.len());
        expanded.add(name.len()).write(0);
    }
    consumed as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_dn_expand(
    message: *const u8,
    message_end: *const u8,
    compressed: *const u8,
    expanded: *mut c_char,
    length: c_int,
) -> c_int {
    if length <= 0 {
        set_errno(EMSGSIZE);
        return -1;
    }
    let result = unsafe {
        kinakaze_abi_ns_name_uncompress(message, message_end, compressed, expanded, length as usize)
    };
    if result > 0 && unsafe { *expanded == b'.' as c_char && *expanded.add(1) == 0 } {
        unsafe { expanded.write(0) };
    }
    result
}

/// Advance over a wire-format DNS name without following compression pointers.
/// A pointer terminates the encoded name even when its target is outside this
/// slice: validating the referenced name belongs to the expansion routine.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ns_name_skip(
    cursor: *mut *const u8,
    end: *const u8,
) -> c_int {
    if !cursor.is_null() {
        let start = unsafe { cursor.read() };
        let remaining = (end as usize).checked_sub(start as usize);
        if !start.is_null()
            && let Some(length) = remaining
        {
            let mut offset = 0usize;
            while offset < length {
                let label = unsafe { start.add(offset).read() };
                offset += 1;
                let complete = match label {
                    0 => true,
                    1..=63 if usize::from(label) <= length - offset => {
                        offset += usize::from(label);
                        false
                    }
                    192..=255 if offset < length => {
                        offset += 1;
                        true
                    }
                    _ => break,
                };
                if complete {
                    unsafe { cursor.write(start.add(offset)) };
                    return 0;
                }
            }
        }
    }
    set_errno(EMSGSIZE);
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_dn_skipname(start: *const u8, end: *const u8) -> c_int {
    let mut cursor = start;
    if unsafe { kinakaze_abi_ns_name_skip(&mut cursor, end) } != 0 {
        return -1;
    }
    match c_int::try_from(cursor as usize - start as usize) {
        Ok(length) => length,
        Err(_) => {
            set_errno(EMSGSIZE);
            -1
        }
    }
}

#[cfg(test)]
mod resolver_wire_tests {
    use super::*;

    #[test]
    fn uncompressed_dns_names_escape_octets_and_reject_cycles() {
        assert_eq!(
            expand_dns_name(b"\x04a.\\\x01\0", 0).unwrap(),
            (b"a\\.\\\\\\001".to_vec(), 6)
        );
        assert_eq!(expand_dns_name(b"\0", 0).unwrap(), (b".".to_vec(), 1));
        assert_eq!(expand_dns_name(b"\xc0\x00", 0), Err(EMSGSIZE));
        assert_eq!(expand_dns_name(b"\x03ab", 0), Err(EMSGSIZE));
        let message = b"\x01a\0";
        let mut output = [0x7fi8; 2];
        assert_eq!(
            unsafe {
                kinakaze_abi_ns_name_uncompress(
                    message.as_ptr(),
                    message.as_ptr().add(message.len()),
                    message.as_ptr(),
                    output.as_mut_ptr(),
                    1,
                )
            },
            -1
        );
        assert_eq!(output, [0x7f; 2]);
    }

    #[test]
    fn compressed_dns_name_reports_original_consumption() {
        let message = b"\x03www\x07example\x03com\0\x04mail\xc0\x04";
        let mut output = [0i8; 64];
        let consumed = unsafe {
            kinakaze_abi_dn_expand(
                message.as_ptr(),
                message.as_ptr().add(message.len()),
                message.as_ptr().add(17),
                output.as_mut_ptr(),
                output.len() as c_int,
            )
        };
        assert_eq!(consumed, 7);
        assert_eq!(
            unsafe { CStr::from_ptr(output.as_ptr()) }.to_bytes(),
            b"mail.example.com"
        );
    }

    #[test]
    fn resolver_state_matches_glibc_x86_64_layout() {
        assert_eq!(core::mem::size_of::<ResState>(), 560);
        let state = kinakaze_abi___res_state();
        assert!(!state.is_null());
        assert_ne!(unsafe { (*state).options } & RES_RECURSE, 0);
    }

    #[test]
    fn linux_idn_flags_are_accepted_without_colliding_with_windows_flags() {
        let linux_flags = AI_IDN | AI_CANONIDN | AI_ADDRCONFIG;
        assert_eq!(linux_flags & !AI_KNOWN, 0);
        // GetAddrInfoW already consumes and returns Unicode names. The Linux
        // IDN flags therefore need no Windows flag, while ADDRCONFIG still maps
        // to its different Windows bit.
        assert_eq!(ai_flags_to_windows(linux_flags), 0x0400);
    }

    #[test]
    fn hosts_parser_uses_canonical_name_aliases_and_file_order() {
        let text = "# guest-local names\n192.0.2.10 canonical alias ALIAS2\n\
                    malformed ignored\n2001:db8::10 v6-name alias\n192.0.2.11 alias\n";
        assert_eq!(
            parse_hosts_answers(text, "AlIaS"),
            vec![
                HostsAnswer {
                    address: "192.0.2.10".to_owned(),
                    canonical_name: "canonical".to_owned(),
                },
                HostsAnswer {
                    address: "2001:db8::10".to_owned(),
                    canonical_name: "v6-name".to_owned(),
                },
                HostsAnswer {
                    address: "192.0.2.11".to_owned(),
                    canonical_name: "alias".to_owned(),
                },
            ]
        );
    }
}
