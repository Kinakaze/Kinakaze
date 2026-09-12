//! Linux sockets mapped onto Winsock2.
//!
//! Two things make this more than a thin forward. First, the constants differ:
//! `AF_INET6` is 10 on Linux and 23 on Windows, `SOL_SOCKET` is 1 against
//! 65535, and most `SO_*` values disagree, so every option crossing the border
//! is translated rather than passed through.
//!
//! Second, every socket is put in non-blocking mode at the OS level regardless
//! of what the guest asked for, and blocking semantics are reimplemented here as
//! "wait for readiness, then retry". That is what lets a blocking `accept` or
//! `recv` be interrupted by a signal: the wait includes the thread's interrupt
//! event, exactly like the overlapped file path. It is also the readiness model
//! `epoll` needs, so the two share one mechanism.

use std::sync::{Mutex, OnceLock};

pub(crate) mod connect_policy;
pub(crate) mod filter;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Networking::WinSock::{
    FD_ACCEPT, FD_CLOSE, FD_CONNECT, FD_READ, FD_WRITE, FIONBIO, FROM_PROTOCOL_INFO,
    INVALID_SOCKET, SOCKET, SOCKET_ERROR, WSA_FLAG_OVERLAPPED, WSACleanup, WSADATA,
    WSADuplicateSocketW, WSAEWOULDBLOCK, WSAEventSelect, WSAGetLastError, WSAPROTOCOL_INFOW,
    WSASocketW, WSAStartup, accept as wsa_accept, bind as wsa_bind, closesocket,
    connect as wsa_connect, getpeername as wsa_getpeername, getsockname as wsa_getsockname,
    getsockopt as wsa_getsockopt, ioctlsocket, listen as wsa_listen, recv as wsa_recv,
    recvfrom as wsa_recvfrom, send as wsa_send, sendto as wsa_sendto, setsockopt as wsa_setsockopt,
    shutdown as wsa_shutdown, socket as wsa_socket,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::{
    EAFNOSUPPORT, EAGAIN, EBADF, EFAULT, EINPROGRESS, EINTR, EINVAL, EIO, ENOPROTOOPT, ENOTCONN,
    ENOTSOCK, EOPNOTSUPP, FdFlags, FdKind, get, install, interrupt, signal,
};

#[inline]
fn socket_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_SOCKET_TRACE").is_some())
}

// Linux x86_64 address families.
pub const AF_UNSPEC: i32 = 0;
pub const AF_UNIX: i32 = 1;
pub const AF_INET: i32 = 2;
/// Linux uses 10 here; Windows uses 23. Translation is mandatory.
pub const AF_INET6: i32 = 10;
pub use crate::netlink::AF_NETLINK;

// Linux socket types. The low bits carry the type; the high bits carry flags.
pub const SOCK_STREAM: i32 = 1;
pub const SOCK_DGRAM: i32 = 2;
pub const SOCK_RAW: i32 = 3;
/// Reliable, ordered, record-preserving. Only meaningful for `AF_UNIX` here.
pub const SOCK_SEQPACKET: i32 = 5;
/// Linux-specific type flags that are ORed into the type argument.
pub const SOCK_NONBLOCK: i32 = 0o4000;
pub const SOCK_CLOEXEC: i32 = 0o2000000;

pub const IPPROTO_IP: i32 = 0;
pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;
pub const IPPROTO_IPV6: i32 = 41;

/// Linux `SOL_SOCKET`. Windows spells this 65535.
pub const SOL_SOCKET: i32 = 1;

// Linux `SO_*` option names, almost all of which differ from Windows.
pub const SO_DEBUG: i32 = 1;
pub const SO_REUSEADDR: i32 = 2;
pub const SO_TYPE: i32 = 3;
pub const SO_ERROR: i32 = 4;
pub const SO_DONTROUTE: i32 = 5;
pub const SO_BROADCAST: i32 = 6;
pub const SO_SNDBUF: i32 = 7;
pub const SO_RCVBUF: i32 = 8;
pub const SO_KEEPALIVE: i32 = 9;
pub const SO_OOBINLINE: i32 = 10;
pub const SO_LINGER: i32 = 13;
/// `AF_UNIX` only: the peer's credentials.
pub const SO_PEERCRED: i32 = 17;
pub const SO_REUSEPORT: i32 = 15;
pub const SO_RCVTIMEO: i32 = 20;
pub const SO_SNDTIMEO: i32 = 21;
pub const SO_ACCEPTCONN: i32 = 30;
pub const SO_PROTOCOL: i32 = 38;
pub const SO_DOMAIN: i32 = 39;
pub use filter::{
    SO_ATTACH_FILTER, SO_DETACH_BPF, SO_DETACH_FILTER, SO_GET_FILTER, SO_LOCK_FILTER,
};

// Linux message flags.
pub const MSG_OOB: i32 = 0x01;
pub const MSG_PEEK: i32 = 0x02;
pub const MSG_DONTROUTE: i32 = 0x04;
pub const MSG_DONTWAIT: i32 = 0x40;
/// Linux uses 0x100; Windows uses 8.
pub const MSG_WAITALL: i32 = 0x100;
pub const MSG_NOSIGNAL: i32 = 0x4000;

// Linux `shutdown` directions, which happen to match Windows.
pub const SHUT_RD: i32 = 0;
pub const SHUT_WR: i32 = 1;
pub const SHUT_RDWR: i32 = 2;

/// Readiness conditions a socket can be waited on for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Readiness(pub u32);

impl Readiness {
    pub const READABLE: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const ERROR: Self = Self(1 << 2);
    pub const HANGUP: Self = Self(1 << 3);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Initializes Winsock exactly once per process.
///
/// A forked child is a fresh process and runs this again on first use, so no
/// handoff is required for the library itself; only the sockets travel.
pub(crate) fn ensure_winsock() -> Result<(), i32> {
    static STARTED: OnceLock<bool> = OnceLock::new();
    let started = *STARTED.get_or_init(|| {
        // SAFETY: `data` is a writable local and 2.2 is a supported version.
        let mut data: WSADATA = unsafe { std::mem::zeroed() };
        // SAFETY: version 2.2 with a valid out-pointer.
        let result = unsafe { WSAStartup(0x0202, &mut data) };
        if crate::fork_trace_enabled() {
            eprintln!(
                "kinakaze vfs: WSAStartup pid={} result={result}",
                std::process::id()
            );
        }
        result == 0
    });
    if started { Ok(()) } else { Err(EIO) }
}

const FORK_SOCKET_MAGIC: u64 = 0x4352_5953_4f43_4b32; // "CRYSOCK2"

/// A Winsock reference must be closed by its provider, never CloseHandle.
pub(crate) struct RightsSocket(pub(crate) usize);
impl Drop for RightsSocket {
    fn drop(&mut self) {
        unsafe {
            closesocket(self.0);
        }
    }
}
impl RightsSocket {
    pub(crate) fn into_raw(self) -> usize {
        let raw = self.0;
        std::mem::forget(self);
        raw
    }
}
pub(crate) fn export_rights(raw: usize, pid: u32) -> Result<Vec<u8>, i32> {
    ensure_winsock()?;
    let mut info: WSAPROTOCOL_INFOW = unsafe { std::mem::zeroed() };
    if unsafe { WSADuplicateSocketW(raw, pid, &mut info) } == SOCKET_ERROR {
        return Err(errno_from_wsa(unsafe { WSAGetLastError() }));
    }
    Ok(unsafe {
        std::slice::from_raw_parts(
            (&raw const info).cast::<u8>(),
            size_of::<WSAPROTOCOL_INFOW>(),
        )
    }
    .to_vec())
}
pub(crate) fn import_rights(bytes: &[u8]) -> Result<RightsSocket, i32> {
    ensure_winsock()?;
    if bytes.len() != size_of::<WSAPROTOCOL_INFOW>() {
        return Err(EIO);
    }
    let info = unsafe { bytes.as_ptr().cast::<WSAPROTOCOL_INFOW>().read_unaligned() };
    let raw = unsafe {
        WSASocketW(
            FROM_PROTOCOL_INFO,
            FROM_PROTOCOL_INFO,
            FROM_PROTOCOL_INFO,
            &info,
            0,
            WSA_FLAG_OVERLAPPED,
        )
    };
    if raw == INVALID_SOCKET {
        return Err(errno_from_wsa(unsafe { WSAGetLastError() }));
    }
    let owned = RightsSocket(raw);
    set_non_blocking(raw)?;
    Ok(owned)
}
const FORK_SOCKET_PAYLOAD_LEN: usize = 40;
const FORK_SOCKET_ACK_TIMEOUT_MS: u32 = 10_000;

struct PendingSocketFork {
    read_pipe: usize,
    write_pipe: usize,
    acknowledged: usize,
    entries: Vec<(i32, usize)>,
}

struct ForkSocketAcknowledgement(usize);

impl Drop for ForkSocketAcknowledgement {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: this child inherited the event and owns its local handle.
            unsafe {
                SetEvent(self.0 as HANDLE);
                CloseHandle(self.0 as HANDLE);
            }
        }
    }
}

fn pending_socket_fork() -> &'static Mutex<Option<PendingSocketFork>> {
    static PENDING: OnceLock<Mutex<Option<PendingSocketFork>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(None))
}

#[repr(C)]
struct ForkSocketRecord {
    fd: i32,
    error: i32,
    protocol: WSAPROTOCOL_INFOW,
}

fn close_pipe(handle: usize) {
    if handle != 0 {
        // SAFETY: fork pipes are owned by this handoff and closed once per process.
        unsafe { CloseHandle(handle as HANDLE) };
    }
}

fn write_pipe_all(handle: usize, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        let mut written = 0u32;
        // SAFETY: `bytes` is readable and this is the parent-owned pipe writer.
        let ok = unsafe {
            WriteFile(
                handle as HANDLE,
                bytes.as_ptr(),
                bytes.len().min(u32::MAX as usize) as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 {
            return false;
        }
        bytes = &bytes[written as usize..];
    }
    true
}

fn read_pipe_all(handle: usize, mut bytes: &mut [u8]) -> bool {
    while !bytes.is_empty() {
        let mut read = 0u32;
        // SAFETY: `bytes` is writable and this is the child-owned pipe reader.
        let ok = unsafe {
            ReadFile(
                handle as HANDLE,
                bytes.as_mut_ptr(),
                bytes.len().min(u32::MAX as usize) as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            return false;
        }
        bytes = &mut bytes[read as usize..];
    }
    true
}

/// Prepares an inheritable control pipe for the process-specific Winsock
/// duplication records produced once the fork coordinator knows the child PID.
pub(crate) fn prepare_process_fork() -> Result<(), i32> {
    prepare_socket_transfer(crate::fork_socket_entries()?)
}

/// Prepares socket duplication for an `execve` replacement. Descriptors marked
/// close-on-exec are intentionally absent from both the reconstructed table and
/// the Winsock transfer.
pub(crate) fn prepare_process_exec() -> Result<(), i32> {
    prepare_socket_transfer(crate::exec_socket_entries()?)
}

fn prepare_socket_transfer(entries: Vec<(i32, usize)>) -> Result<(), i32> {
    let mut pending = pending_socket_fork().lock().map_err(|_| EIO)?;
    if let Some(stale) = pending.take() {
        close_pipe(stale.read_pipe);
        close_pipe(stale.write_pipe);
        close_pipe(stale.acknowledged);
    }
    if entries.is_empty() {
        return Ok(());
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let mut read_pipe: HANDLE = std::ptr::null_mut();
    let mut write_pipe: HANDLE = std::ptr::null_mut();
    // SAFETY: both out-pointers and the security attributes are valid.
    if unsafe { CreatePipe(&mut read_pipe, &mut write_pipe, &security, 0) } == 0 {
        return Err(EIO);
    }
    // Manual-reset and initially unsignalled. The parent must not expose its
    // fork return to guest code until every target socket descriptor exists.
    let acknowledged = unsafe { CreateEventW(&security, 1, 0, std::ptr::null()) };
    if acknowledged.is_null() {
        close_pipe(read_pipe as usize);
        close_pipe(write_pipe as usize);
        return Err(EIO);
    }
    *pending = Some(PendingSocketFork {
        read_pipe: read_pipe as usize,
        write_pipe: write_pipe as usize,
        acknowledged: acknowledged as usize,
        entries,
    });
    Ok(())
}

/// Serializes the inherited pipe handles used for socket reconstruction.
pub(crate) fn serialize_process_fork() -> Result<Vec<u8>, i32> {
    let pending = pending_socket_fork().lock().map_err(|_| EIO)?;
    let (read_pipe, write_pipe, acknowledged, count) = match pending.as_ref() {
        Some(state) => (
            state.read_pipe,
            state.write_pipe,
            state.acknowledged,
            u32::try_from(state.entries.len()).map_err(|_| EIO)?,
        ),
        None => (0, 0, 0, 0),
    };
    let mut payload = Vec::with_capacity(FORK_SOCKET_PAYLOAD_LEN);
    payload.extend_from_slice(&FORK_SOCKET_MAGIC.to_le_bytes());
    payload.extend_from_slice(&(read_pipe as u64).to_le_bytes());
    payload.extend_from_slice(&(write_pipe as u64).to_le_bytes());
    payload.extend_from_slice(&(acknowledged as u64).to_le_bytes());
    payload.extend_from_slice(&count.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    Ok(payload)
}

/// Supplies `WSAPROTOCOL_INFO` records after the fork coordinator learns the
/// destination PID.  Unlike raw handle inheritance, this gives the child its
/// own Winsock provider reference, matching Linux's independent post-fork fd.
pub(crate) fn finish_process_fork(result: i32) {
    let state = pending_socket_fork()
        .lock()
        .ok()
        .and_then(|mut pending| pending.take());
    let Some(state) = state else { return };
    close_pipe(state.read_pipe);
    if result <= 0 || ensure_winsock().is_err() {
        close_pipe(state.write_pipe);
        close_pipe(state.acknowledged);
        return;
    }
    for (fd, raw) in state.entries {
        // WSAEventSelect associates readiness state with the underlying socket,
        // not with one descriptor. A duplicate must not retain an event left by
        // the parent's poll/epoll path.
        unsafe { WSAEventSelect(raw as SOCKET, 0, 0) };
        // SAFETY: this plain-old-data record is initialized before it is sent.
        let mut record: ForkSocketRecord = unsafe { std::mem::zeroed() };
        record.fd = fd;
        // SAFETY: the socket is live in this process and `result` is the target PID.
        if unsafe { WSADuplicateSocketW(raw as SOCKET, result as u32, &mut record.protocol) }
            == SOCKET_ERROR
        {
            // SAFETY: WSAGetLastError has no preconditions.
            record.error = unsafe { WSAGetLastError() };
        }
        // SAFETY: the record is fully initialized and contains no references.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&record as *const ForkSocketRecord).cast::<u8>(),
                size_of::<ForkSocketRecord>(),
            )
        };
        if !write_pipe_all(state.write_pipe, bytes) {
            break;
        }
    }
    close_pipe(state.write_pipe);
    // WSADuplicateSocketW only produces a recipe. Keep the source descriptor
    // alive until the child confirms that WSASocketW consumed every recipe.
    unsafe { WaitForSingleObject(state.acknowledged as HANDLE, FORK_SOCKET_ACK_TIMEOUT_MS) };
    close_pipe(state.acknowledged);
}

/// Reconstructs each inherited socket as a real Winsock socket owned by this
/// process.  The replacement retains the same Linux fd and descriptor flags.
pub(crate) fn restore_process_fork(payload: &[u8]) -> bool {
    if payload.len() != FORK_SOCKET_PAYLOAD_LEN
        || u64::from_le_bytes(payload[0..8].try_into().unwrap_or_default()) != FORK_SOCKET_MAGIC
    {
        return false;
    }
    let read_pipe = u64::from_le_bytes(payload[8..16].try_into().unwrap_or_default()) as usize;
    let write_pipe = u64::from_le_bytes(payload[16..24].try_into().unwrap_or_default()) as usize;
    let acknowledged = u64::from_le_bytes(payload[24..32].try_into().unwrap_or_default()) as usize;
    let count = u32::from_le_bytes(payload[32..36].try_into().unwrap_or_default()) as usize;
    if count == 0 {
        return read_pipe == 0 && write_pipe == 0 && acknowledged == 0;
    }
    if read_pipe == 0 || write_pipe == 0 || acknowledged == 0 {
        close_pipe(read_pipe);
        close_pipe(write_pipe);
        close_pipe(acknowledged);
        return false;
    }
    let _acknowledgement = ForkSocketAcknowledgement(acknowledged);
    if ensure_winsock().is_err() {
        close_pipe(read_pipe);
        close_pipe(write_pipe);
        return false;
    }
    // The child never writes protocol records. Closing its inherited writer also
    // makes a premature parent close observable as EOF instead of a deadlock.
    close_pipe(write_pipe);
    let entries = crate::fork_socket_entries().unwrap_or_default();
    if entries.len() != count {
        close_pipe(read_pipe);
        return false;
    }
    for (fd, inherited) in entries {
        // SAFETY: ReadFile fills every byte before the record is inspected.
        let mut record: ForkSocketRecord = unsafe { std::mem::zeroed() };
        // SAFETY: the record is plain writable storage with its exact byte size.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                (&mut record as *mut ForkSocketRecord).cast::<u8>(),
                size_of::<ForkSocketRecord>(),
            )
        };
        if !read_pipe_all(read_pipe, bytes) || record.fd != fd || record.error != 0 {
            close_pipe(read_pipe);
            return false;
        }
        // SAFETY: the parent generated this protocol record specifically for
        // this process with WSADuplicateSocketW.
        let replacement = unsafe {
            WSASocketW(
                FROM_PROTOCOL_INFO,
                FROM_PROTOCOL_INFO,
                FROM_PROTOCOL_INFO,
                &record.protocol,
                0,
                WSA_FLAG_OVERLAPPED,
            )
        };
        if replacement != INVALID_SOCKET {
            // Event selection is shared by duplicated sockets. Clear the
            // association before this process starts using the replacement.
            unsafe { WSAEventSelect(replacement, 0, 0) };
        }
        if replacement == INVALID_SOCKET || set_non_blocking(replacement).is_err() {
            if replacement != INVALID_SOCKET {
                // SAFETY: the replacement was created in this process.
                unsafe { closesocket(replacement) };
            }
            close_pipe(read_pipe);
            return false;
        }
        if crate::fork_trace_enabled() {
            eprintln!(
                "kinakaze vfs: restored socket fd={fd} inherited={inherited:#x} replacement={replacement:#x} t={}ms",
                crate::fork_trace_elapsed_ms()
            );
        }
        // `inherited` is only the serialized parent handle value. Socket handles
        // are deliberately non-inheritable, so there is no raw child handle to
        // close; `replacement` is the sole socket owned by this process.
        if crate::replace_fork_socket(fd, inherited, replacement as usize).is_err() {
            // SAFETY: installation failed, so this function still owns it.
            unsafe { closesocket(replacement) };
            close_pipe(read_pipe);
            return false;
        }
    }
    close_pipe(read_pipe);
    true
}

/// Shuts Winsock down. Only useful at process teardown.
pub fn cleanup() {
    // SAFETY: WSACleanup has no preconditions beyond a prior WSAStartup.
    unsafe { WSACleanup() };
}

/// Translates a Linux address family to the Windows value.
fn family_to_windows(family: i32) -> Result<i32, i32> {
    match family {
        AF_UNSPEC => Ok(0),
        AF_INET => Ok(2),
        // The one that actually differs.
        AF_INET6 => Ok(23),
        // AF_UNIX never reaches Winsock: it is emulated on named pipes by
        // `crate::unix`, so a caller that gets here has already been dispatched
        // past this point.
        AF_UNIX => Err(EAFNOSUPPORT),
        _ => Err(EAFNOSUPPORT),
    }
}

/// Translates a Windows address family back to the Linux value.
fn family_to_linux(family: u16) -> i32 {
    match family {
        2 => AF_INET,
        23 => AF_INET6,
        1 => AF_UNIX,
        _ => AF_UNSPEC,
    }
}

/// Translates a Linux `(level, name)` option pair to Windows.
///
/// Returns `None` for options Windows has no equivalent of, which callers report
/// as `ENOPROTOOPT` rather than silently ignoring.
fn option_to_windows(level: i32, name: i32) -> Option<(i32, i32)> {
    match level {
        SOL_SOCKET => {
            let windows_name = match name {
                SO_DEBUG => 1,
                SO_REUSEADDR => 4,
                SO_TYPE => 0x1008,
                SO_ERROR => 0x1007,
                SO_DONTROUTE => 16,
                SO_BROADCAST => 32,
                SO_SNDBUF => 0x1001,
                SO_RCVBUF => 0x1002,
                SO_KEEPALIVE => 8,
                SO_OOBINLINE => 0x0100,
                SO_LINGER => 0x0080,
                SO_RCVTIMEO => 0x1006,
                SO_SNDTIMEO => 0x1005,
                SO_ACCEPTCONN => 2,
                // Windows has no SO_REUSEPORT; SO_REUSEADDR already allows the
                // rebinding most callers want it for.
                SO_REUSEPORT => 4,
                _ => return None,
            };
            // Windows spells SOL_SOCKET as 0xffff.
            Some((0xffff, windows_name))
        }
        IPPROTO_IP => {
            let windows_name = match name {
                1 => 8,   // Linux IP_TOS -> Winsock IP_TOS (8)
                2 => 4,   // Linux IP_TTL -> Winsock IP_TTL (4)
                3 => 2,   // Linux IP_HDRINCL -> Winsock IP_HDRINCL (2)
                4 => 1,   // Linux IP_OPTIONS -> Winsock IP_OPTIONS (1)
                8 => 19,  // Linux IP_PKTINFO -> Winsock IP_PKTINFO (19)
                32 => 9,  // Linux IP_MULTICAST_IF -> Winsock IP_MULTICAST_IF (9)
                33 => 10, // Linux IP_MULTICAST_TTL -> Winsock IP_MULTICAST_TTL (10)
                34 => 11, // Linux IP_MULTICAST_LOOP -> Winsock IP_MULTICAST_LOOP (11)
                35 => 12, // Linux IP_ADD_MEMBERSHIP -> Winsock IP_ADD_MEMBERSHIP (12)
                36 => 13, // Linux IP_DROP_MEMBERSHIP -> Winsock IP_DROP_MEMBERSHIP (13)
                _ => name,
            };
            Some((0, windows_name))
        }
        IPPROTO_IPV6 => {
            let windows_name = match name {
                16 => 4,       // Linux IPV6_UNICAST_HOPS -> Winsock IPV6_UNICAST_HOPS (4)
                17 => 9,       // Linux IPV6_MULTICAST_IF -> Winsock IPV6_MULTICAST_IF (9)
                18 => 10,      // Linux IPV6_MULTICAST_HOPS -> Winsock IPV6_MULTICAST_HOPS (10)
                19 => 11,      // Linux IPV6_MULTICAST_LOOP -> Winsock IPV6_MULTICAST_LOOP (11)
                20 => 12,      // Linux IPV6_ADD_MEMBERSHIP -> Winsock IPV6_ADD_MEMBERSHIP (12)
                21 => 13,      // Linux IPV6_DROP_MEMBERSHIP -> Winsock IPV6_DROP_MEMBERSHIP (13)
                26 => 27,      // Linux IPV6_V6ONLY -> Winsock IPV6_V6ONLY (27)
                49 | 50 => 19, // Linux IPV6_PKTINFO -> Winsock IPV6_PKTINFO (19)
                51 | 52 => 21, // Linux IPV6_HOPLIMIT -> Winsock IPV6_HOPLIMIT (21)
                53 | 54 => 1,  // Linux IPV6_HOPOPTS -> Winsock IPV6_HOPOPTS (1)
                _ => name,
            };
            Some((41, windows_name))
        }
        IPPROTO_TCP => {
            let windows_name = match name {
                1 => 1,   // TCP_NODELAY
                2 => 4,   // TCP_MAXSEG
                4 => 3,   // Linux TCP_KEEPIDLE -> Winsock TCP_KEEPIDLE (3)
                5 => 17,  // Linux TCP_KEEPINTVL -> Winsock TCP_KEEPINTVL (17)
                6 => 16,  // Linux TCP_KEEPCNT -> Winsock TCP_KEEPCNT (16)
                23 => 15, // Linux TCP_FASTOPEN -> Winsock TCP_FASTOPEN (15)
                _ => name,
            };
            Some((6, windows_name))
        }
        _ => None,
    }
}

/// Translates Linux message flags to Windows.
///
/// `MSG_DONTWAIT` and `MSG_NOSIGNAL` have no Windows counterpart: the first is
/// handled here by skipping the readiness wait, and the second is irrelevant
/// because this layer never raises `SIGPIPE` from a send.
fn message_flags_to_windows(flags: i32) -> i32 {
    let mut translated = 0;
    if flags & MSG_OOB != 0 {
        translated |= 1;
    }
    if flags & MSG_PEEK != 0 {
        translated |= 2;
    }
    if flags & MSG_DONTROUTE != 0 {
        translated |= 4;
    }
    if flags & MSG_WAITALL != 0 {
        translated |= 8;
    }
    translated
}

/// Maps a Winsock error to the closest Linux errno.
pub(crate) fn errno_from_wsa(error: i32) -> i32 {
    match error {
        10004 => EINTR,         // WSAEINTR
        10009 => EBADF,         // WSAEBADF
        10013 => crate::EACCES, // WSAEACCES
        10014 => EFAULT,        // WSAEFAULT
        10022 => EINVAL,        // WSAEINVAL
        10024 => crate::EMFILE, // WSAEMFILE
        10035 => EAGAIN,        // WSAEWOULDBLOCK
        10036 => EINPROGRESS,   // WSAEINPROGRESS
        10037 => crate::EALREADY,
        10038 => ENOTSOCK,
        10039 => crate::EDESTADDRREQ,
        10040 => crate::EMSGSIZE,
        10041 => crate::EPROTOTYPE,
        10042 => ENOPROTOOPT,
        10043 => crate::EPROTONOSUPPORT,
        10044 => crate::ESOCKTNOSUPPORT,
        10045 => EOPNOTSUPP,
        10047 => EAFNOSUPPORT,
        10048 => crate::EADDRINUSE,
        10049 => crate::EADDRNOTAVAIL,
        10050 => crate::ENETDOWN,
        10051 => crate::ENETUNREACH,
        10052 => crate::ENETRESET,
        10053 => crate::ECONNABORTED,
        10054 => crate::ECONNRESET,
        10055 => crate::ENOBUFS,
        10056 => crate::EISCONN,
        10057 => ENOTCONN,
        10058 => crate::EPIPE,
        10060 => crate::ETIMEDOUT,
        10061 => crate::ECONNREFUSED,
        10064 => crate::EHOSTDOWN,
        10065 => crate::EHOSTUNREACH,
        _ => EIO,
    }
}

/// Returns the last Winsock error as an errno.
pub(crate) fn last_wsa_errno() -> i32 {
    // SAFETY: WSAGetLastError has no preconditions.
    errno_from_wsa(unsafe { WSAGetLastError() })
}

/// Copies a guest `sockaddr` into a Windows one, patching the family.
///
/// The `sockaddr_in` and `sockaddr_in6` payloads are byte-identical between the
/// two systems, so only the leading 16-bit family needs rewriting.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
unsafe fn sockaddr_to_windows(address: *const u8, length: i32) -> Result<([u8; 128], i32), i32> {
    if address.is_null() || length < 2 || length as usize > 128 {
        return Err(EINVAL);
    }
    let mut buffer = [0u8; 128];
    // SAFETY: the caller guarantees `length` readable bytes and the bound was
    // checked against the buffer size.
    unsafe { std::ptr::copy_nonoverlapping(address, buffer.as_mut_ptr(), length as usize) };

    let linux_family = i32::from(u16::from_le_bytes([buffer[0], buffer[1]]));
    let windows_family = family_to_windows(linux_family)?;
    buffer[..2].copy_from_slice(&(windows_family as u16).to_le_bytes());
    let actual_length = match windows_family {
        2 => 16,
        23 => 28,
        _ => length,
    };
    Ok((buffer, actual_length))
}

/// Copies a Windows `sockaddr` back to the guest, patching the family.
///
/// # Safety
///
/// `out` must be writable for `*out_length` bytes and `out_length` must be
/// readable and writable.
unsafe fn sockaddr_to_linux(
    source: &[u8],
    valid: i32,
    out: *mut u8,
    out_length: *mut i32,
) -> Result<(), i32> {
    if out.is_null() || out_length.is_null() {
        // A caller that does not want the address is not an error.
        return Ok(());
    }
    // SAFETY: the caller guarantees the length out-pointer is readable.
    let capacity = unsafe { *out_length };
    if capacity < 0 {
        return Err(EINVAL);
    }
    let mut buffer = [0u8; 128];
    let valid = (valid as usize).min(source.len()).min(buffer.len());
    buffer[..valid].copy_from_slice(&source[..valid]);
    if valid >= 2 {
        let windows_family = u16::from_le_bytes([buffer[0], buffer[1]]);
        let linux_family = family_to_linux(windows_family) as u16;
        buffer[..2].copy_from_slice(&linux_family.to_le_bytes());
    }
    // Truncation is signalled by reporting the untruncated length, as POSIX
    // specifies for `accept` and `getsockname`.
    let copied = valid.min(capacity as usize);
    // SAFETY: `copied` is bounded by the caller's stated capacity.
    unsafe {
        std::ptr::copy_nonoverlapping(buffer.as_ptr(), out, copied);
        *out_length = valid as i32;
    }
    Ok(())
}

/// Owns a Winsock event used for readiness notification.
struct WsaEvent(HANDLE);

impl WsaEvent {
    fn new() -> Option<Self> {
        // Manual reset, initially unsignalled: WSAEventSelect requires manual
        // reset semantics because it signals on edge and the waiter clears it.
        // SAFETY: null security descriptor and name request an unnamed event.
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if handle.is_null() {
            None
        } else {
            Some(Self(handle))
        }
    }
}

impl Drop for WsaEvent {
    fn drop(&mut self) {
        // SAFETY: this type owns the event it created.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Puts a socket into non-blocking mode.
///
/// Every socket is non-blocking at the OS level so this layer can implement
/// blocking itself and keep it interruptible.
fn set_non_blocking(socket: SOCKET) -> Result<(), i32> {
    let mut enabled: u32 = 1;
    // SAFETY: FIONBIO takes a writable u32 and the socket is live.
    if unsafe { ioctlsocket(socket, FIONBIO, &mut enabled) } == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    Ok(())
}

/// Waits until a socket is ready, or a signal interrupts the wait.
///
/// `timeout_ms` of `None` waits indefinitely. Returns the readiness observed, or
/// `Err(EINTR)` when a signal handler ran without `SA_RESTART`, or `Err(EAGAIN)`
/// when the timeout expired.
pub fn wait_readiness(
    socket: SOCKET,
    interest: Readiness,
    timeout_ms: Option<u32>,
) -> Result<Readiness, i32> {
    let event = WsaEvent::new().ok_or(EIO)?;
    let mut mask = 0i32;
    if interest.contains(Readiness::READABLE) {
        // FD_ACCEPT is the readable condition for a listening socket, and
        // FD_CLOSE is how a peer disconnect surfaces as readable-at-EOF.
        mask |= (FD_READ | FD_ACCEPT | FD_CLOSE) as i32;
    }
    if interest.contains(Readiness::WRITABLE) {
        mask |= (FD_WRITE | FD_CONNECT) as i32;
    }

    // SAFETY: the socket is live and the event outlives the association below.
    // WSAEVENT is an isize alias of the same underlying handle.
    if unsafe { WSAEventSelect(socket, event.0 as isize, mask) } == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }

    let interrupt = interrupt::current();
    let timeout = timeout_ms.unwrap_or(u32::MAX);
    let waited = if interrupt.is_null() {
        // SAFETY: the event is live for the duration of the wait.
        unsafe { WaitForMultipleObjects(1, &event.0, 0, timeout) }
    } else {
        signal::register_waiter();
        let handles = [event.0, interrupt];
        // SAFETY: both handles are live for the duration of the wait.
        let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout) };
        signal::unregister_waiter();
        result
    };

    // Dissociating restores the socket to plain non-blocking mode so a later
    // operation is not affected by this event registration.
    // SAFETY: passing a null event and zero mask cancels the association.
    unsafe { WSAEventSelect(socket, 0, 0) };

    if waited == WAIT_TIMEOUT {
        return Err(EAGAIN);
    }
    if waited == WAIT_OBJECT_0 {
        return Ok(interest);
    }
    if waited == WAIT_OBJECT_0 + 1 {
        // A signal arrived. Handlers run here; SA_RESTART turns into a retry at
        // the call site, which is why the caller loops on EINTR.
        return match signal::deliver_pending() {
            signal::Delivery::Restart => Err(EAGAIN),
            _ => Err(EINTR),
        };
    }
    Err(EIO)
}

/// Runs `operation`, waiting for readiness and retrying while it would block.
///
/// This is where the guest's blocking semantics are reconstructed: the OS socket
/// is always non-blocking, so a `WSAEWOULDBLOCK` means "wait, then try again"
/// unless the descriptor or the call itself asked not to block.
fn blocking_retry<T>(
    socket: SOCKET,
    non_blocking: bool,
    interest: Readiness,
    mut operation: impl FnMut() -> Result<T, i32>,
) -> Result<T, i32> {
    loop {
        match operation() {
            Err(EAGAIN) if !non_blocking => {
                // Wait for the condition, then retry. An interrupted wait that
                // requested a restart also lands here and retries.
                match wait_readiness(socket, interest, None) {
                    Ok(_) => continue,
                    Err(EAGAIN) => continue,
                    Err(error) => return Err(error),
                }
            }
            other => return other,
        }
    }
}

/// Returns the Winsock handle behind a descriptor, if it is a socket.
pub fn socket_of(fd: i32) -> Result<(SOCKET, bool), i32> {
    // A fork child is a fresh Windows process. Even after WSASocketW rebuilds
    // the descriptor from the parent's protocol record, Winsock requires one
    // WSAStartup call in that process before any operation on it.
    ensure_winsock()?;
    let entry = get(fd)?;
    if entry.kind != FdKind::Socket {
        return Err(ENOTSOCK);
    }
    Ok((entry.raw as SOCKET, entry.flags.contains(FdFlags::NONBLOCK)))
}

/// Creates a socket and installs it in the descriptor table.
pub fn socket(family: i32, socket_type: i32, protocol: i32) -> Result<i32, i32> {
    if socket_trace_enabled() {
        eprintln!(
            "kinakaze socket: pid={} socket(family={family}, type={socket_type:#x}, protocol={protocol})",
            std::process::id()
        );
    }
    // AF_UNIX is emulated on named pipes and never touches Winsock.
    if family == AF_UNIX {
        let result = crate::unix::socket(socket_type, protocol);
        if socket_trace_enabled() {
            eprintln!("kinakaze socket: unix result={result:?}");
        }
        return result;
    }
    if family == AF_NETLINK {
        let result = crate::netlink::socket(socket_type, protocol);
        if socket_trace_enabled() {
            eprintln!("kinakaze socket: netlink result={result:?}");
        }
        return result;
    }
    ensure_winsock()?;
    let windows_family = family_to_windows(family).inspect_err(|error| {
        if socket_trace_enabled() {
            eprintln!("kinakaze socket: unsupported family={family} error={error}");
        }
    })?;
    // Linux ORs SOCK_NONBLOCK and SOCK_CLOEXEC into the type argument.
    let non_blocking = socket_type & SOCK_NONBLOCK != 0;
    let close_on_exec = socket_type & SOCK_CLOEXEC != 0;
    let base_type = socket_type & !(SOCK_NONBLOCK | SOCK_CLOEXEC);
    if !matches!(base_type, SOCK_STREAM | SOCK_DGRAM | SOCK_RAW) {
        return Err(EINVAL);
    }

    // SAFETY: the arguments are validated translations of the guest's request.
    let packet = crate::usernet::packet::selected(family, base_type, protocol);
    let handle = unsafe {
        wsa_socket(
            windows_family,
            if packet { SOCK_DGRAM } else { base_type },
            if packet { 17 } else { protocol },
        )
    };
    if handle == INVALID_SOCKET {
        return Err(last_wsa_errno());
    }
    // The OS socket is always non-blocking; the guest's choice is recorded in
    // the descriptor flags and honoured by this layer instead.
    set_non_blocking(handle)?;

    let mut flags = FdFlags::NONE;
    if non_blocking {
        flags = flags.union(FdFlags::NONBLOCK);
    }
    if close_on_exec {
        flags = flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    let result = install(handle as usize, FdKind::Socket, flags).inspect_err(|_| {
        // SAFETY: installation failed, so this function still owns the socket.
        unsafe { closesocket(handle) };
    });
    let result = result.and_then(|fd| {
        if let Err(e) = crate::usernet::attach(fd, family, base_type) {
            let _ = crate::close(fd);
            return Err(e);
        }
        Ok(fd)
    });
    let result = result.and_then(|fd| {
        if crate::usernet::packet::selected(family, base_type, protocol) {
            if let Err(error) = crate::usernet::packet::initialize(fd) {
                let _ = crate::close(fd);
                return Err(error);
            }
        }
        Ok(fd)
    });
    if socket_trace_enabled() {
        eprintln!("kinakaze socket: winsock result={result:?}");
    }
    result
}

/// `bind`.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
pub unsafe fn bind(fd: i32, address: *const u8, length: i32) -> Result<(), i32> {
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::netlink::bind(fd, address, length) };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::bind(fd, address, length) };
    }
    let (socket, _) = socket_of(fd)?;
    // SAFETY: forwarded from this function's contract.
    let (buffer, length) = unsafe { sockaddr_to_windows(address, length)? };
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        return crate::usernet::packet::bind(
            fd,
            &owner,
            crate::usernet::Address::parse(&buffer[..length as usize])?,
        );
    }
    if crate::usernet::bind_address(fd, socket, &buffer[..length as usize])? {
        return Ok(());
    }
    // SAFETY: the buffer holds a valid sockaddr of `length` bytes.
    if unsafe { wsa_bind(socket, buffer.as_ptr().cast(), length) } == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    crate::usernet::bound_native(fd, socket)
}

/// `listen`.
pub fn listen(fd: i32, backlog: i32) -> Result<(), i32> {
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        return crate::usernet::packet::listen(fd, &owner, backlog);
    }
    if crate::netlink::is_netlink_socket(fd) {
        return Err(EOPNOTSUPP);
    }
    if crate::unix::is_unix_socket(fd) {
        return crate::unix::listen(fd, backlog);
    }
    let (socket, _) = socket_of(fd)?;
    crate::usernet::listening(fd, socket)?;
    // SAFETY: the socket is live and any backlog is accepted by Winsock.
    if unsafe { wsa_listen(socket, backlog) } == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    crate::usernet::listening(fd, socket)
}

/// `accept4`, which `accept` calls with no flags.
///
/// # Safety
///
/// `address` and `length` must be null or a writable pair.
pub unsafe fn accept(fd: i32, address: *mut u8, length: *mut i32, flags: i32) -> Result<i32, i32> {
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        let (child, peer) = crate::usernet::packet::accept(fd, &owner, flags)?;
        let (bytes, size) = peer.bytes(true);
        if let Err(error) = unsafe { sockaddr_to_linux(&bytes, size, address, length) } {
            let _ = crate::close(child);
            return Err(error);
        }
        return Ok(child);
    }
    if crate::netlink::is_netlink_socket(fd) {
        return Err(EOPNOTSUPP);
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::accept(fd, address, length, flags) };
    }
    let (socket, non_blocking) = socket_of(fd)?;
    let mut storage = [0u8; 128];
    let mut storage_length = storage.len() as i32;

    let accepted = blocking_retry(socket, non_blocking, Readiness::READABLE, || {
        // SAFETY: the out-parameters are local and correctly sized.
        let accepted =
            unsafe { wsa_accept(socket, storage.as_mut_ptr().cast(), &mut storage_length) };
        let error = if accepted == INVALID_SOCKET {
            unsafe { WSAGetLastError() }
        } else {
            0
        };
        crate::epoll::notify_read(fd);
        if accepted == INVALID_SOCKET {
            // SAFETY: WSAGetLastError has no preconditions.
            if error == WSAEWOULDBLOCK {
                return Err(EAGAIN);
            }
            return Err(errno_from_wsa(error));
        }
        Ok(accepted)
    })?;

    // Inherited sockets are blocking by default in Linux unless SOCK_NONBLOCK
    // is requested, but the OS handle must still be non-blocking here.
    if set_non_blocking(accepted).is_err() {
        // SAFETY: this function owns the accepted socket until it is installed.
        unsafe { closesocket(accepted) };
        return Err(EIO);
    }

    if let Err(e) = crate::usernet::source(fd, &mut storage, &mut storage_length) {
        unsafe {
            closesocket(accepted);
        }
        return Err(e);
    }
    // SAFETY: `address`/`length` satisfy this function's contract.
    if let Err(error) = unsafe { sockaddr_to_linux(&storage, storage_length, address, length) } {
        // SAFETY: this function still owns the accepted socket.
        unsafe { closesocket(accepted) };
        return Err(error);
    }

    let mut child_flags = FdFlags::NONE;
    if flags & SOCK_NONBLOCK != 0 {
        child_flags = child_flags.union(FdFlags::NONBLOCK);
    }
    if flags & SOCK_CLOEXEC != 0 {
        child_flags = child_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    let child = install(accepted, FdKind::Socket, child_flags).inspect_err(|_| {
        // SAFETY: installation failed, so the socket is still owned here.
        unsafe { closesocket(accepted) };
    })?;
    if let Err(e) =
        crate::usernet::accepted(fd, child, accepted, &storage[..storage_length as usize])
    {
        let _ = crate::close(child);
        return Err(e);
    }
    Ok(child)
}

/// `connect`.
///
/// A non-blocking socket reports `EINPROGRESS` on the first call, matching
/// Linux, and the caller then waits for writability.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
pub unsafe fn connect(fd: i32, address: *const u8, length: i32) -> Result<(), i32> {
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::netlink::connect(fd, address, length) };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::connect(fd, address, length) };
    }
    let (socket, non_blocking) = socket_of(fd)?;
    // SAFETY: forwarded from this function's contract.
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        if address.is_null() || length < 2 {
            return Err(EINVAL);
        }
        let family = unsafe { address.cast::<u16>().read_unaligned() };
        let target = if family == 0 {
            None
        } else {
            let (bytes, size) = unsafe { sockaddr_to_windows(address, length)? };
            Some(crate::usernet::Address::parse(&bytes[..size as usize])?)
        };
        return crate::usernet::packet::connect(fd, &owner, target);
    }
    let (mut buffer, mut length) = unsafe { sockaddr_to_windows(address, length)? };
    let started = match crate::usernet::destination(fd, socket, &buffer[..length as usize], true) {
        Ok(Some((mapped, size))) => {
            buffer = mapped;
            length = size;
            false
        }
        Ok(None) => false,
        Err(EINPROGRESS) => true,
        Err(e) => return Err(e),
    };
    if !started {
        let _ = connect_policy::configure(socket, &buffer[..length as usize]);
    }
    let result = if started {
        SOCKET_ERROR
    } else {
        unsafe { wsa_connect(socket, buffer.as_ptr().cast(), length) }
    };
    let error = if started {
        WSAEWOULDBLOCK
    } else if result == SOCKET_ERROR {
        unsafe { WSAGetLastError() }
    } else {
        0
    };
    crate::usernet::sent(fd, socket)?;
    if result != SOCKET_ERROR {
        return Ok(());
    }
    // SAFETY: WSAGetLastError has no preconditions.
    // WSAEWOULDBLOCK from connect means the handshake started.
    if error != WSAEWOULDBLOCK {
        return Err(errno_from_wsa(error));
    }
    if non_blocking {
        return Err(EINPROGRESS);
    }

    // Blocking connect: wait for writability, then read SO_ERROR to learn
    // whether the handshake actually succeeded. Writability alone does not
    // distinguish success from a refused connection.
    loop {
        match wait_readiness(socket, Readiness::WRITABLE, None) {
            Ok(_) => break,
            Err(EAGAIN) => continue,
            Err(error) => return Err(error),
        }
    }
    let mut pending: i32 = 0;
    let mut pending_length = size_of::<i32>() as i32;
    // SAFETY: SO_ERROR writes one i32 and both out-pointers are local.
    let queried = unsafe {
        wsa_getsockopt(
            socket,
            0xffff,
            0x1007,
            (&raw mut pending).cast(),
            &mut pending_length,
        )
    };
    if queried == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    if let Some(error) = crate::usernet::socket_error(fd)? {
        return Err(error);
    }
    if pending != 0 {
        return Err(errno_from_wsa(pending));
    }
    Ok(())
}

/// `send`/`recv` direction, so one helper serves both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Send,
    Receive,
}
/// Packet sockets report queued guest payload, never the size of an outer IP frame.
pub fn packet_queued_bytes(fd: i32) -> Result<Option<usize>, i32> {
    crate::usernet::packet::description(fd)?
        .map(|owner| crate::usernet::packet::queued_bytes(fd, &owner))
        .transpose()
}

// Preserve an existing read edge while bytes remain queued. Once a receive
// drains the queue, arm the next arrival even if the caller did not probe EAGAIN.
fn notify_receive_progress(fd: i32, socket: SOCKET, moved: i32, error: i32, flags: i32) {
    if error == WSAEWOULDBLOCK {
        crate::epoll::notify_read(fd);
    } else if moved >= 0 && flags & MSG_PEEK == 0 {
        let mut available = 0;
        if unsafe {
            ioctlsocket(
                socket,
                windows_sys::Win32::Networking::WinSock::FIONREAD,
                &mut available,
            )
        } == 0
            && available == 0
        {
            crate::epoll::notify_read(fd);
        }
    }
}

/// Transfers data on a connected socket.
///
/// # Safety
///
/// `buffer` must be valid for `len` bytes in the requested direction.
unsafe fn transfer(
    fd: i32,
    buffer: *mut u8,
    len: usize,
    flags: i32,
    direction: Direction,
) -> Result<usize, i32> {
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract and selected by the
        // same direction that describes the caller's access guarantee.
        return unsafe {
            match direction {
                Direction::Send => crate::netlink::send(fd, buffer, len, flags),
                Direction::Receive => crate::netlink::recv(fd, buffer, len, flags),
            }
        };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract; the direction selects
        // which of the two access modes the caller guaranteed.
        return unsafe {
            match direction {
                Direction::Send => crate::unix::send(fd, buffer, len, flags),
                Direction::Receive => crate::unix::recv(fd, buffer, len, flags),
            }
        };
    }
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        if len != 0 && buffer.is_null() {
            return Err(EFAULT);
        }
        return unsafe {
            match direction {
                Direction::Send => crate::usernet::packet::send(
                    fd,
                    &owner,
                    if len == 0 {
                        &[]
                    } else {
                        std::slice::from_raw_parts(buffer, len)
                    },
                    flags,
                    None,
                ),
                Direction::Receive => crate::usernet::packet::receive(
                    fd,
                    &owner,
                    if len == 0 {
                        &mut []
                    } else {
                        std::slice::from_raw_parts_mut(buffer, len)
                    },
                    flags,
                )
                .map(|(copied, _, full)| if flags & 0x20 != 0 { full } else { copied }),
            }
        };
    }
    let (socket, socket_non_blocking) = socket_of(fd)?;
    // MSG_DONTWAIT makes a single call non-blocking regardless of the socket.
    let non_blocking = socket_non_blocking || flags & MSG_DONTWAIT != 0;
    let windows_flags = message_flags_to_windows(flags);
    let amount = len.min(i32::MAX as usize) as i32;
    let interest = match direction {
        Direction::Send => Readiness::WRITABLE,
        Direction::Receive => Readiness::READABLE,
    };

    let result = blocking_retry(socket, non_blocking, interest, || {
        // SAFETY: the caller guarantees the buffer for `amount` bytes.
        let moved = unsafe {
            match direction {
                Direction::Send => wsa_send(socket, buffer, amount, windows_flags),
                Direction::Receive => wsa_recv(socket, buffer, amount, windows_flags),
            }
        };
        let error = if moved == SOCKET_ERROR {
            unsafe { WSAGetLastError() }
        } else {
            0
        };
        if direction == Direction::Receive {
            notify_receive_progress(fd, socket, moved, error, flags);
        }
        if direction == Direction::Send
            && (error == WSAEWOULDBLOCK || (moved >= 0 && moved < amount))
        {
            crate::epoll::notify_write(fd);
        }
        if moved == SOCKET_ERROR {
            // SAFETY: WSAGetLastError has no preconditions.
            if error == WSAEWOULDBLOCK {
                return Err(EAGAIN);
            }
            return Err(errno_from_wsa(error));
        }
        Ok(moved as usize)
    });
    if crate::fork_trace_enabled() {
        eprintln!(
            "kinakaze vfs: pid {} socket {:?} fd={fd} handle={socket:#x} requested={len} result={result:?} t={}ms",
            std::process::id(),
            direction,
            crate::fork_trace_elapsed_ms()
        );
    }
    result
}

/// `send`.
///
/// # Safety
///
/// `buffer` must be readable for `len` bytes.
pub unsafe fn send(fd: i32, buffer: *const u8, len: usize, flags: i32) -> Result<usize, i32> {
    // SAFETY: the transfer only reads from the pointer in this direction.
    unsafe { transfer(fd, buffer.cast_mut(), len, flags, Direction::Send) }
}

/// `recv`.
///
/// # Safety
///
/// `buffer` must be writable for `len` bytes.
pub unsafe fn recv(fd: i32, buffer: *mut u8, len: usize, flags: i32) -> Result<usize, i32> {
    // SAFETY: forwarded from this function's contract.
    unsafe { transfer(fd, buffer, len, flags, Direction::Receive) }
}

/// `sendto`.
///
/// # Safety
///
/// `buffer` must be readable for `len` bytes and `address` readable for
/// `address_length` bytes when non-null.
pub unsafe fn sendto(
    fd: i32,
    buffer: *const u8,
    len: usize,
    flags: i32,
    address: *const u8,
    address_length: i32,
) -> Result<usize, i32> {
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::netlink::sendto(fd, buffer, len, flags, address, address_length) };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::sendto(fd, buffer, len, flags, address, address_length) };
    }
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        if len != 0 && buffer.is_null() {
            return Err(EFAULT);
        }
        let target = if address.is_null() {
            None
        } else {
            let (bytes, length) = unsafe { sockaddr_to_windows(address, address_length)? };
            Some(crate::usernet::Address::parse(&bytes[..length as usize])?)
        };
        return crate::usernet::packet::send(
            fd,
            &owner,
            if len == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(buffer, len) }
            },
            flags,
            target,
        );
    }
    if address.is_null() {
        // SAFETY: forwarded from this function's contract.
        return unsafe { send(fd, buffer, len, flags) };
    }
    let (socket, socket_non_blocking) = socket_of(fd)?;
    let non_blocking = socket_non_blocking || flags & MSG_DONTWAIT != 0;
    // SAFETY: forwarded from this function's contract.
    let (mut target, mut target_length) = unsafe { sockaddr_to_windows(address, address_length)? };
    if let Some((mapped, size)) =
        crate::usernet::destination(fd, socket, &target[..target_length as usize], false)?
    {
        target = mapped;
        target_length = size;
    }
    let windows_flags = message_flags_to_windows(flags);
    let amount = len.min(i32::MAX as usize) as i32;

    let result = blocking_retry(socket, non_blocking, Readiness::WRITABLE, || {
        // SAFETY: the buffer and the translated address are both valid.
        let sent = unsafe {
            wsa_sendto(
                socket,
                buffer,
                amount,
                windows_flags,
                target.as_ptr().cast(),
                target_length,
            )
        };
        let error = if sent == SOCKET_ERROR {
            unsafe { WSAGetLastError() }
        } else {
            0
        };
        if error == WSAEWOULDBLOCK {
            crate::epoll::notify_write(fd);
        }
        if sent == SOCKET_ERROR {
            // SAFETY: WSAGetLastError has no preconditions.
            if error == WSAEWOULDBLOCK {
                return Err(EAGAIN);
            }
            return Err(errno_from_wsa(error));
        }
        Ok(sent as usize)
    });
    crate::usernet::sent(fd, socket)?;
    result
}

/// `recvfrom`.
///
/// # Safety
///
/// `buffer` must be writable for `len` bytes; `address` and `address_length`
/// must be null or a writable pair.
pub unsafe fn recvfrom(
    fd: i32,
    buffer: *mut u8,
    len: usize,
    flags: i32,
    address: *mut u8,
    address_length: *mut i32,
) -> Result<usize, i32> {
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe {
            crate::netlink::recvfrom(fd, buffer, len, flags, address, address_length)
        };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::recvfrom(fd, buffer, len, flags, address, address_length) };
    }
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        if (len != 0 && buffer.is_null()) || (!address.is_null() && address_length.is_null()) {
            return Err(EFAULT);
        }
        let (copied, peer, full) = crate::usernet::packet::receive(
            fd,
            &owner,
            if len == 0 {
                &mut []
            } else {
                unsafe { std::slice::from_raw_parts_mut(buffer, len) }
            },
            flags,
        )?;
        if let Some(peer) = peer {
            let (bytes, length) = peer.bytes(true);
            unsafe {
                sockaddr_to_linux(&bytes, length, address, address_length)?;
            }
        }
        return Ok(if flags & 0x20 != 0 { full } else { copied });
    }
    if address.is_null() {
        // SAFETY: forwarded from this function's contract.
        return unsafe { recv(fd, buffer, len, flags) };
    }
    if address_length.is_null() {
        return Err(EFAULT);
    }
    let (socket, socket_non_blocking) = socket_of(fd)?;
    let non_blocking = socket_non_blocking || flags & MSG_DONTWAIT != 0;
    let windows_flags = message_flags_to_windows(flags);
    let amount = len.min(i32::MAX as usize) as i32;
    let mut storage = [0u8; 128];
    let mut storage_length = storage.len() as i32;

    let received = blocking_retry(socket, non_blocking, Readiness::READABLE, || {
        // SAFETY: the buffer and both out-parameters are valid.
        let received = unsafe {
            wsa_recvfrom(
                socket,
                buffer,
                amount,
                windows_flags,
                storage.as_mut_ptr().cast(),
                &mut storage_length,
            )
        };
        let error = if received == SOCKET_ERROR {
            unsafe { WSAGetLastError() }
        } else {
            0
        };
        notify_receive_progress(fd, socket, received, error, flags);
        if received == SOCKET_ERROR {
            // SAFETY: WSAGetLastError has no preconditions.
            if error == WSAEWOULDBLOCK {
                return Err(EAGAIN);
            }
            return Err(errno_from_wsa(error));
        }
        Ok(received as usize)
    })?;

    // SAFETY: forwarded from this function's contract.
    crate::usernet::source(fd, &mut storage, &mut storage_length)?;
    unsafe { sockaddr_to_linux(&storage, storage_length, address, address_length)? };
    Ok(received)
}

/// `shutdown`.
pub fn shutdown(fd: i32, how: i32) -> Result<(), i32> {
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        return crate::usernet::packet::shutdown(fd, &owner, how);
    }
    if crate::netlink::is_netlink_socket(fd) {
        return crate::netlink::shutdown(fd, how);
    }
    if crate::unix::is_unix_socket(fd) {
        return crate::unix::shutdown(fd, how);
    }
    let (socket, _) = socket_of(fd)?;
    if !matches!(how, SHUT_RD | SHUT_WR | SHUT_RDWR) {
        return Err(EINVAL);
    }
    // The direction constants happen to agree between Linux and Windows.
    // SAFETY: the socket is live and `how` was validated.
    if unsafe { wsa_shutdown(socket, how) } == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    Ok(())
}

/// `setsockopt`.
///
/// # Safety
///
/// `value` must be readable for `length` bytes.
pub unsafe fn setsockopt(
    fd: i32,
    level: i32,
    name: i32,
    value: *const u8,
    length: i32,
) -> Result<(), i32> {
    if level == SOL_SOCKET && matches!(name, SO_ATTACH_FILTER | SO_DETACH_FILTER | SO_LOCK_FILTER) {
        return unsafe { filter::set(fd, name, value, length) };
    }
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::netlink::setsockopt(fd, level, name, value, length) };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::setsockopt(fd, level, name, value, length) };
    }
    let (socket, _) = socket_of(fd)?;
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        return unsafe { crate::usernet::packet::set_option(&owner, level, name, value, length) };
    }
    let Some((windows_level, windows_name)) = option_to_windows(level, name) else {
        return Err(ENOPROTOOPT);
    };
    // SAFETY: the caller guarantees `length` readable bytes.
    let applied = unsafe { wsa_setsockopt(socket, windows_level, windows_name, value, length) };
    if applied == SOCKET_ERROR {
        let err = last_wsa_errno();
        if (level == IPPROTO_IP && name == 1) || (level == IPPROTO_IPV6 && name == 67) {
            return Ok(());
        }
        return Err(err);
    }
    if level == SOL_SOCKET
        && matches!(name, SO_REUSEADDR | SO_REUSEPORT)
        && !value.is_null()
        && length >= 4
    {
        crate::usernet::reuse(fd, unsafe { value.cast::<i32>().read_unaligned() } != 0)?;
    }
    Ok(())
}

/// `getsockopt`.
///
/// # Safety
///
/// `value` must be writable for `*length` bytes and `length` must be a valid
/// in-out pointer.
pub unsafe fn getsockopt(
    fd: i32,
    level: i32,
    name: i32,
    value: *mut u8,
    length: *mut i32,
) -> Result<(), i32> {
    if level == SOL_SOCKET && matches!(name, SO_GET_FILTER | SO_LOCK_FILTER) {
        return unsafe { filter::get(fd, name, value, length) };
    }
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::netlink::getsockopt(fd, level, name, value, length) };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::getsockopt(fd, level, name, value, length) };
    }
    let (socket, _) = socket_of(fd)?;
    if level == IPPROTO_IP && name == 1 {
        // Linux IP_TOS is 1. Default to 0.
        if !value.is_null() && !length.is_null() {
            if unsafe { *length } >= 4 {
                unsafe {
                    value.cast::<i32>().write_unaligned(0);
                    *length = 4;
                }
            } else if unsafe { *length } >= 1 {
                unsafe {
                    *value = 0;
                    *length = 1;
                }
            }
        }
        return Ok(());
    }
    if level == IPPROTO_IP && name == 4 {
        // Linux IP_OPTIONS is 4. When queried on Linux without IP options set, returns *length = 0.
        // Winsock returns 4 bytes of dummy data which causes OpenSSH to reject the connection.
        if !length.is_null() {
            unsafe { *length = 0 };
        }
        return Ok(());
    }
    if level == IPPROTO_IPV6 && matches!(name, 54 | 57 | 59) {
        // Linux IPV6_RTHDR (57) / IPV6_HOPOPTS (54) / IPV6_DSTOPTS (59)
        if !length.is_null() {
            unsafe { *length = 0 };
        }
        return Ok(());
    }
    if (level == IPPROTO_IP || level == IPPROTO_IPV6) && (name == 66 || name == 67) {
        // IPT_SO_GET_REVISION_MATCH (66), IPT_SO_GET_REVISION_TARGET (67)
        // IP6T_SO_GET_REVISION_MATCH (66), IP6T_SO_GET_REVISION_TARGET (67)
        // struct xt_get_revision { char name[29]; u8 revision; }
        if length.is_null() || value.is_null() {
            return Err(EFAULT);
        }
        if unsafe { *length } < 30 {
            return Err(EINVAL);
        }
        return Ok(());
    }
    // Linux control-plane queries above are independent of the data transport.
    // xtables probes extension revisions using an ordinary stream descriptor.
    if let Some(owner) = crate::usernet::packet::description(fd)? {
        return unsafe {
            crate::usernet::packet::get_option(fd, &owner, level, name, value, length)
        };
    }
    let Some((windows_level, windows_name)) = option_to_windows(level, name) else {
        return Err(ENOPROTOOPT);
    };
    // SAFETY: the caller guarantees the value/length pair.
    let queried = unsafe { wsa_getsockopt(socket, windows_level, windows_name, value, length) };
    if queried == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    // SO_ERROR carries a Winsock code that the guest will compare against
    // errno values, so it has to be translated in place.
    if level == SOL_SOCKET
        && name == SO_ERROR
        && !value.is_null()
        // SAFETY: the caller guarantees the length pointer is readable.
        && unsafe { *length } >= size_of::<i32>() as i32
    {
        // SAFETY: the option wrote at least one i32 into `value`.
        unsafe {
            let raw = value.cast::<i32>().read_unaligned();
            let translated = crate::usernet::socket_error(fd)?
                .unwrap_or_else(|| if raw == 0 { 0 } else { errno_from_wsa(raw) });
            value.cast::<i32>().write_unaligned(translated);
        }
    }
    Ok(())
}

/// `getsockname`.
///
/// # Safety
///
/// `address` and `length` must be a writable pair.
pub unsafe fn getsockname(fd: i32, address: *mut u8, length: *mut i32) -> Result<(), i32> {
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::netlink::getsockname(fd, address, length) };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::getsockname(fd, address, length) };
    }
    if let Some((bytes, size)) = crate::usernet::name(fd, false)? {
        return unsafe { sockaddr_to_linux(&bytes, size, address, length) };
    }
    let (socket, _) = socket_of(fd)?;
    let mut storage = [0u8; 128];
    let mut storage_length = storage.len() as i32;
    // SAFETY: both out-parameters are local and correctly sized.
    let queried =
        unsafe { wsa_getsockname(socket, storage.as_mut_ptr().cast(), &mut storage_length) };
    if queried == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    // SAFETY: forwarded from this function's contract.
    unsafe { sockaddr_to_linux(&storage, storage_length, address, length) }
}

/// `getpeername`.
///
/// # Safety
///
/// `address` and `length` must be a writable pair.
pub unsafe fn getpeername(fd: i32, address: *mut u8, length: *mut i32) -> Result<(), i32> {
    if crate::netlink::is_netlink_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::netlink::getpeername(fd, address, length) };
    }
    if crate::unix::is_unix_socket(fd) {
        // SAFETY: forwarded from this function's contract.
        return unsafe { crate::unix::getpeername(fd, address, length) };
    }
    if let Some((bytes, size)) = crate::usernet::name(fd, true)? {
        return unsafe { sockaddr_to_linux(&bytes, size, address, length) };
    }
    let (socket, _) = socket_of(fd)?;
    let mut storage = [0u8; 128];
    let mut storage_length = storage.len() as i32;
    // SAFETY: both out-parameters are local and correctly sized.
    let queried =
        unsafe { wsa_getpeername(socket, storage.as_mut_ptr().cast(), &mut storage_length) };
    if queried == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    // SAFETY: forwarded from this function's contract.
    unsafe { sockaddr_to_linux(&storage, storage_length, address, length) }
}

/// Closes a socket handle. Used by the descriptor table.
pub fn close_socket(raw: usize) -> Result<(), i32> {
    ensure_winsock()?;
    // SAFETY: the table owns this socket and is releasing it.
    if unsafe { closesocket(raw as SOCKET) } == SOCKET_ERROR {
        return Err(last_wsa_errno());
    }
    Ok(())
}
