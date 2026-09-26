//! `AF_UNIX` sockets built on Windows named pipes.
//!
//! Windows does have an `AF_UNIX` of its own, but it is stream-only, its
//! `sockaddr_un` is laid out differently, and it has no abstract namespace. So
//! rather than forward to Winsock this module reconstructs Unix sockets on named
//! pipes, which are the one Windows primitive that shares the properties that
//! matter: a rendezvous by name, a connection-oriented accept loop, per-client
//! instances, and a byte or message stream that survives being handed to
//! overlapped I/O.
//!
//! The mapping is:
//!
//! | Unix socket concept        | Named pipe mechanism                        |
//! |----------------------------|---------------------------------------------|
//! | `bind(path)`               | `CreateNamedPipeW` with FIRST_PIPE_INSTANCE |
//! | `listen`                   | publish shared backlog and readiness       |
//! | `accept`                   | remove one shared queued connection        |
//! | `connect(path)`            | allocate a private data pipe and enqueue   |
//! | `SOCK_STREAM`              | byte-mode pipe                              |
//! | `SOCK_DGRAM`/`SOCK_SEQPACKET` | message-mode pipe                        |
//! | abstract namespace         | pipe name with no placeholder file          |
//!
//! Because `bind` claims the name with `FILE_FLAG_FIRST_PIPE_INSTANCE`, the
//! pipe namespace itself provides the `EADDRINUSE` check, which is stronger than
//! testing for a leftover file: a stale path from a crashed process does not
//! wedge the name the way it does on Linux.
//!
//! Everything here is deliberately non-blocking at the OS level, with blocking
//! semantics rebuilt on top, matching the design in [`crate::socket`]. That is
//! what keeps a blocking `accept` or `recv` interruptible by a signal.

use std::collections::HashMap;
mod ancillary;
#[cfg(test)]
mod connect_tests;
pub mod credentials;
mod listener;
pub(crate) mod procnet;
pub(crate) mod readiness;
pub use ancillary::{async_enabled, get_owner, set_async, set_owner};
pub fn run_rights_keeper() -> Result<(), i32> {
    ancillary::run_keeper()
}
pub(crate) fn publish_filter(fd: i32, filter: Arc<crate::ofd::Shared>) -> Result<(), i32> {
    ancillary::publish_filter(fd, filter)
}
pub(crate) fn with_filter_publication<T>(
    fd: i32,
    filter: &crate::ofd::Shared,
    update: impl FnOnce(&mut dyn FnMut(&[u8]) -> Result<(), i32>) -> Result<T, i32>,
) -> Result<T, i32> {
    ancillary::with_filter_publication(fd, filter, update)
}
fn publish_existing_filter(fd: i32) -> Result<(), i32> {
    let socket = snapshot(fd)?;
    if let Some(state) = &socket.ancillary {
        state.set_passcred(socket.is_server_end, socket.record.passcred()?)?;
    }
    if let Some(filter) = crate::ofd::existing(get(fd)?.description_id)? {
        publish_filter(fd, filter)?;
    }
    Ok(())
}
pub(crate) fn prepare_process_handoff() -> Result<(), i32> {
    listener::prepare_handoff()
}
pub(crate) fn finish_process_handoff(pid: i32) {
    listener::finish_handoff(pid);
}

fn restart_transfer<T>(mut operation: impl FnMut() -> Result<T, i32>) -> Result<T, i32> {
    loop {
        let result = {
            let _defer = signal::defer_delivery();
            operation()
        };
        if !signal::delivery_deferred()
            && matches!(&result, Err(error) if *error == EINTR)
            && signal::deliver_pending() == signal::Delivery::Restart
        {
            continue;
        }
        return result;
    }
}

/// Send payload and native descriptor references as one ordered operation.
pub unsafe fn send_rights(
    fd: i32,
    buffer: *const u8,
    len: usize,
    flags: i32,
    rights: &[i32],
) -> Result<usize, i32> {
    unsafe { send_control(fd, buffer, len, flags, rights, None) }
}
/// Validated credentials accompany the same payload as descriptor references.
pub unsafe fn send_control(
    fd: i32,
    buffer: *const u8,
    len: usize,
    flags: i32,
    rights: &[i32],
    credentials: Option<&credentials::Sender>,
) -> Result<usize, i32> {
    restart_transfer(|| {
        let description = get(fd)?.description_id;
        let result = unsafe { ancillary::send(fd, buffer, len, flags, rights, credentials) };
        if result == Err(EAGAIN) {
            crate::epoll::readiness_consumed(
                description,
                crate::epoll::EPOLLOUT | crate::epoll::EPOLLWRNORM,
            );
        }
        result
    })
}
pub unsafe fn recv_rights(
    fd: i32,
    buffer: *mut u8,
    len: usize,
    flags: i32,
    max_rights: usize,
) -> Result<(usize, Vec<i32>, i32), i32> {
    unsafe { recv_control(fd, buffer, len, flags, max_rights, false) }
        .map(|(n, fds, flags, _)| (n, fds, flags))
}
/// `control` is buffer capacity for recvmsg, or maximum fds for legacy callers.
pub unsafe fn recv_control(
    fd: i32,
    buffer: *mut u8,
    len: usize,
    flags: i32,
    control: usize,
    recvmsg: bool,
) -> Result<(usize, Vec<i32>, i32, Option<credentials::Ucred>), i32> {
    restart_transfer(|| {
        let description = get(fd)?.description_id;
        let result = unsafe { ancillary::recv(fd, buffer, len, flags, control, recvmsg) };
        if result == Err(EAGAIN) {
            // This includes an empty ancillary peek and temporary serialization
            // contention, which return before recv_payload can retire an old edge.
            crate::epoll::readiness_consumed(
                description,
                crate::epoll::EPOLLIN | crate::epoll::EPOLLRDNORM,
            );
        }
        result
    })
}
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA,
    ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
// PIPE_ACCESS_DUPLEX lives under Storage::FileSystem rather than System::Pipes,
// because it is an access mode for CreateFileW-family calls.
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_READMODE_MESSAGE, PIPE_TYPE_BYTE, PIPE_TYPE_MESSAGE,
    PIPE_UNLIMITED_INSTANCES, PeekNamedPipe, SetNamedPipeHandleState, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForMultipleObjects};

use crate::socket::{
    AF_UNIX, MSG_DONTWAIT, MSG_PEEK, Readiness, SHUT_RD, SHUT_RDWR, SHUT_WR, SO_ERROR, SO_PEERCRED,
    SO_RCVBUF, SO_SNDBUF, SO_TYPE, SOCK_DGRAM, SOCK_NONBLOCK, SOCK_SEQPACKET, SOCK_STREAM,
    SOL_SOCKET,
};
use crate::{
    EADDRINUSE, EAFNOSUPPORT, EAGAIN, EBADF, ECONNREFUSED, ECONNRESET, EDESTADDRREQ, EEXIST,
    EFAULT, EINTR, EINVAL, EIO, EISCONN, ENAMETOOLONG, ENOPROTOOPT, ENOTCONN, ENOTSOCK, EOPNOTSUPP,
    EPIPE, EPROTOTYPE, FdEntry, FdFlags, FdKind, get, interrupt, signal,
};

/// The `sun_path` capacity of Linux's `sockaddr_un`.
const SUN_PATH_MAX: usize = 108;

/// Prefix every emulated Unix socket lives under.
///
/// Namespacing keeps these from colliding with unrelated pipes on the machine.
const PIPE_PREFIX: &str = r"\\.\pipe\kinakaze-unix\";

/// Pipe buffer size requested for each direction.
const PIPE_BUFFER: u32 = 64 * 1024;

/// Longest encoded name accepted before falling back to a hash.
///
/// The whole pipe path is limited to 256 characters, so this leaves comfortable
/// room for the prefix and complete 64-bit manager domain.
const MAX_ENCODED: usize = 200;

fn unix_trace_enabled() -> bool {
    crate::diagnostic_target_enabled("KINAKAZE_UNIX_TRACE")
}

fn trace_unix(message: impl FnOnce() -> String) {
    let directory = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR");
    if !unix_trace_enabled() && directory.is_none() {
        return;
    }
    let message = message();
    if let Some(directory) = directory.as_ref() {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(
            std::path::PathBuf::from(directory).join(format!("unix-{}.log", std::process::id())),
        ) {
            let _ = file.write_all(format!("{message}\n").as_bytes());
        }
    }
    static LINES: AtomicUsize = AtomicUsize::new(0);
    if directory.is_none() && unix_trace_enabled() && LINES.fetch_add(1, Ordering::Relaxed) < 2048 {
        // Inherited output can be a containerd binary bootstrap protocol.
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("unix-{}.log", std::process::id()));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{message}");
        }
    }
}

/// Which namespace a Unix socket address lives in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Namespace {
    /// A filesystem path, which gets a placeholder file so `stat` and `ls` work.
    Pathname,
    /// Linux's abstract namespace, signalled by a leading NUL byte. It has no
    /// filesystem presence at all.
    Abstract,
    /// An unnamed socket, as produced by `socketpair` or an accepted connection.
    Unnamed,
}

/// A parsed `sockaddr_un`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnixAddress {
    pub namespace: Namespace,
    /// The address bytes without the leading NUL for an abstract name.
    pub name: Vec<u8>,
}

impl UnixAddress {
    /// An address with no name, which is what an unbound socket reports.
    pub fn unnamed() -> Self {
        Self {
            namespace: Namespace::Unnamed,
            name: Vec::new(),
        }
    }

    /// Maps the address onto a named pipe path.
    ///
    /// The encoding has to be deterministic, collision-free, and legal as a pipe
    /// name. The wrinkle is that pipe names are case-insensitive while Unix paths
    /// are not, so `/tmp/A` and `/tmp/a` must not land on the same pipe: every
    /// byte that is not unambiguously safe, uppercase letters included, is
    /// escaped as `=hh`. Names that would overrun the pipe path limit fall back
    /// to a hash of the full address, which stays deterministic across processes.
    pub fn pipe_name(&self) -> String {
        self.pipe_name_in(1)
    }

    fn pipe_name_in(&self, network: u64) -> String {
        let mut encoded = String::with_capacity(self.name.len() + 8);
        // An abstract name is a different namespace, so it gets a distinct
        // prefix; `@` matches how Linux tooling renders these.
        if self.namespace == Namespace::Abstract {
            encoded.push('@');
            if network != 1 {
                encoded.push_str(&format!("ns{network:016x}@"));
            }
        }
        for &byte in &self.name {
            match byte {
                b'/' => encoded.push('-'),
                b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' => encoded.push(byte as char),
                other => {
                    encoded.push('=');
                    encoded
                        .push(char::from_digit((u32::from(other) >> 4) & 0xf, 16).unwrap_or('0'));
                    encoded.push(char::from_digit(u32::from(other) & 0xf, 16).unwrap_or('0'));
                }
            }
        }

        if encoded.len() > MAX_ENCODED {
            // FNV-1a over the namespace tag and the raw bytes. Hashing loses
            // readability but keeps long paths usable and still unique.
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            let tag: u8 = if self.namespace == Namespace::Abstract {
                1
            } else {
                0
            };
            for &byte in std::iter::once(&tag).chain(self.name.iter()) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            if self.namespace == Namespace::Abstract {
                for byte in network.to_le_bytes() {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            encoded = format!("h{hash:016x}");
        }

        format!(
            "{PIPE_PREFIX}{:016x}-{encoded}",
            kinakaze_runtime::authority::domain_id()
        )
    }

    /// Returns the filesystem path for a pathname socket.
    pub fn filesystem_path(&self) -> Option<String> {
        if self.namespace != Namespace::Pathname {
            return None;
        }
        String::from_utf8(self.name.clone()).ok()
    }
}

/// Parses a guest `sockaddr_un` into an address.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
pub unsafe fn parse_address(address: *const u8, length: i32) -> Result<UnixAddress, i32> {
    if address.is_null() {
        return Err(EFAULT);
    }
    // Two bytes of family, then up to 108 of path.
    if length < 2 || length as usize > 2 + SUN_PATH_MAX {
        return Err(EINVAL);
    }
    // SAFETY: the caller guarantees `length` readable bytes, bounded above.
    let bytes = unsafe { std::slice::from_raw_parts(address, length as usize) };
    let family = i32::from(u16::from_le_bytes([bytes[0], bytes[1]]));
    if family != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }

    let path = &bytes[2..];
    // A length of exactly 2 is the documented way to request an autobound
    // address; with no name there is nothing to bind to but it is not malformed.
    if path.is_empty() {
        return Ok(UnixAddress::unnamed());
    }

    if path[0] == 0 {
        // Abstract namespace: the name is the remaining bytes verbatim, NULs
        // included, and its length comes from the caller rather than a
        // terminator.
        return Ok(UnixAddress {
            namespace: Namespace::Abstract,
            name: path[1..].to_vec(),
        });
    }

    // Pathname: NUL-terminated, and the terminator may be absent when the
    // caller passed exactly 108 bytes.
    let end = path
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(path.len());
    if end == 0 {
        return Ok(UnixAddress::unnamed());
    }
    Ok(UnixAddress {
        namespace: Namespace::Pathname,
        name: path[..end].to_vec(),
    })
}

/// Writes an address back to the guest in `sockaddr_un` form.
///
/// Returns the untruncated length, which is what POSIX requires `getsockname`
/// and `accept` to report even when the caller's buffer was too small.
///
/// # Safety
///
/// `out` must be writable for `*out_length` bytes, and `out_length` must be
/// readable and writable. Both may be null, which is not an error.
pub unsafe fn write_address(
    address: &UnixAddress,
    out: *mut u8,
    out_length: *mut i32,
) -> Result<(), i32> {
    if out.is_null() || out_length.is_null() {
        return Ok(());
    }
    // SAFETY: the caller guarantees the length out-pointer is readable.
    let capacity = unsafe { *out_length };
    if capacity < 0 {
        return Err(EINVAL);
    }

    let mut buffer = [0u8; 2 + SUN_PATH_MAX];
    buffer[..2].copy_from_slice(&(AF_UNIX as u16).to_le_bytes());
    let valid = match address.namespace {
        // An unnamed socket reports just the family, as Linux does.
        Namespace::Unnamed => 2,
        Namespace::Abstract => {
            let room = SUN_PATH_MAX - 1;
            let copied = address.name.len().min(room);
            // The leading NUL is the namespace marker and is already zero.
            buffer[3..3 + copied].copy_from_slice(&address.name[..copied]);
            2 + 1 + copied
        }
        Namespace::Pathname => {
            // Reserve one byte so the reported length includes the terminator.
            let room = SUN_PATH_MAX - 1;
            let copied = address.name.len().min(room);
            buffer[2..2 + copied].copy_from_slice(&address.name[..copied]);
            2 + copied + 1
        }
    };

    let copied = valid.min(capacity as usize);
    // SAFETY: `copied` is bounded by both the local buffer and the caller's
    // stated capacity.
    unsafe {
        std::ptr::copy_nonoverlapping(buffer.as_ptr(), out, copied);
        *out_length = valid as i32;
    }
    Ok(())
}

/// What a Unix socket descriptor is currently doing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// Created but neither bound nor connected.
    Idle,
    /// Bound to a name, not yet listening.
    Bound,
    /// Listening on a queue shared by all holders of the description.
    Listening,
    /// Connected to a peer; `handle` carries the data.
    Connected,
    /// The peer is gone but the descriptor is still open.
    Disconnected,
}

/// Per-descriptor state for an emulated Unix socket.
///
/// This lives outside the descriptor table because a Unix socket exists before it
/// has any host handle at all: `socket()` returns a usable fd, and only `bind` or
/// `connect` produces the pipe behind it.
#[derive(Clone)]
struct UnixSocket {
    record: Arc<procnet::Record>,
    ancillary: Option<Arc<ancillary::State>>,
    listener: Option<Arc<listener::Lease>>,
    network: u64,
    _network_pin: Arc<crate::mount::shared::Store>,
    /// `SOCK_STREAM`, `SOCK_DGRAM` or `SOCK_SEQPACKET`.
    socket_type: i32,
    state: State,
    /// The pipe instance, once one exists.
    handle: usize,
    /// Address this socket was bound to.
    local: UnixAddress,
    /// Address of the connected peer, when known.
    peer: UnixAddress,
    /// True once the server side owns a placeholder file it must remove.
    owns_file: bool,
    inode: Option<Arc<SocketInode>>,
    /// `shutdown(SHUT_RD)` was called: reads report end of file.
    read_shut: bool,
    /// `shutdown(SHUT_WR)` was called: writes report `EPIPE`.
    write_shut: bool,
    /// Set for the server end of a connection, which must disconnect the pipe
    /// instance rather than just close its handle.
    is_server_end: bool,
}

impl UnixSocket {
    fn new(socket_type: i32, network: u64) -> Result<Self, i32> {
        Ok(Self {
            record: procnet::Record::new(network, socket_type)?,
            ancillary: None,
            listener: None,
            network,
            _network_pin: crate::namespaces::pin_network(network)?,
            socket_type,
            state: State::Idle,
            handle: 0,
            local: UnixAddress::unnamed(),
            peer: UnixAddress::unnamed(),
            owns_file: false,
            inode: None,
            read_shut: false,
            write_shut: false,
            is_server_end: false,
        })
    }

    /// True when the pipe should carry message boundaries.
    ///
    /// `SOCK_DGRAM` and `SOCK_SEQPACKET` both preserve them; `SOCK_STREAM` does
    /// not.
    fn message_mode(&self) -> bool {
        self.socket_type != SOCK_STREAM
    }

    fn refresh_listener(&mut self) -> Result<(), i32> {
        if matches!(self.state, State::Bound | State::Listening) {
            if let Some(listener) = &self.listener {
                self.state = if listener.pool.is_listening()? {
                    State::Listening
                } else {
                    State::Bound
                };
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct SocketInode(usize);
impl SocketInode {
    pub(crate) fn raw(&self) -> usize {
        self.0
    }
}
impl Drop for SocketInode {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0 as HANDLE) };
    }
}

fn inode_pipe_name(handle: HANDLE) -> Result<String, i32> {
    let stat = crate::fs::stat_handle(handle, false)?;
    if stat.st_mode & crate::fs::S_IFMT != crate::fs::S_IFSOCK {
        return Err(ECONNREFUSED);
    }
    Ok(format!(
        "{PIPE_PREFIX}{:016x}-inode-{:016x}-{:016x}",
        kinakaze_runtime::authority::domain_id(),
        stat.st_dev,
        stat.st_ino
    ))
}

/// Called with the fd table locked by exec's inheritance transaction.
pub(crate) fn auxiliary_handles() -> Result<Vec<(i32, Arc<SocketInode>)>, i32> {
    Ok(sockets()
        .lock()
        .map_err(|_| EIO)?
        .iter()
        .filter_map(|(&fd, socket)| socket.inode.as_ref().map(|inode| (fd, Arc::clone(inode))))
        .collect())
}

pub(crate) fn auxiliary_state_handles() -> Result<Vec<(i32, Arc<crate::fs::object::Object>)>, i32> {
    let sockets = sockets().lock().map_err(|_| EIO)?;
    Ok(sockets
        .iter()
        .flat_map(|(&fd, socket)| {
            std::iter::once((fd, socket.record.pin.clone()))
                .chain(
                    socket.ancillary.iter().flat_map(move |state| {
                        state.pins().iter().map(move |pin| (fd, pin.clone()))
                    }),
                )
                .chain(
                    socket
                        .listener
                        .iter()
                        .map(move |listener| (fd, listener.pin.clone())),
                )
        })
        .collect())
}

type SocketTable = HashMap<i32, UnixSocket>;

fn sockets() -> &'static Mutex<SocketTable> {
    static SOCKETS: OnceLock<Mutex<SocketTable>> = OnceLock::new();
    SOCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn namespace_code(namespace: &Namespace) -> u32 {
    match namespace {
        Namespace::Pathname => 1,
        Namespace::Abstract => 2,
        Namespace::Unnamed => 3,
    }
}

/// Serializes the named-pipe side state; the pipe HANDLE itself is inherited by
/// the process creation and is also present in the descriptor-table section.
pub(crate) fn serialize_fork_state() -> Result<Vec<u8>, i32> {
    let descriptors = crate::table().read().map_err(|_| EIO)?;
    let sockets = sockets().lock().map_err(|_| EIO)?;
    let mut active = Vec::with_capacity(sockets.len());
    for (&fd, socket) in sockets.iter() {
        let entry = descriptors
            .slots
            .get(fd as usize)
            .and_then(|entry| *entry)
            .ok_or(EBADF)?;
        if entry.kind != FdKind::UnixSocket {
            return Err(EBADF);
        }
        active.push((fd, socket.clone()));
    }
    drop(sockets);
    drop(descriptors);

    let mut payload = Vec::new();
    payload.extend_from_slice(&(active.len() as u32).to_le_bytes());
    payload.extend_from_slice(&2u32.to_le_bytes());
    payload.extend_from_slice(
        &listener::handoff_token(
            active
                .iter()
                .filter_map(|(_, socket)| socket.listener.clone())
                .collect(),
        )?
        .to_le_bytes(),
    );
    for (fd, socket) in active {
        let state = match socket.state {
            State::Idle => 0u32,
            State::Bound => 1,
            State::Listening => 2,
            State::Connected => 3,
            State::Disconnected => 4,
        };
        let flags = u32::from(socket.owns_file)
            | (u32::from(socket.read_shut) << 1)
            | (u32::from(socket.write_shut) << 2)
            | (u32::from(socket.is_server_end) << 3);
        payload.extend_from_slice(&fd.to_le_bytes());
        payload.extend_from_slice(&socket.socket_type.to_le_bytes());
        payload.extend_from_slice(&state.to_le_bytes());
        payload.extend_from_slice(&flags.to_le_bytes());
        payload.extend_from_slice(&(socket.handle as u64).to_le_bytes());
        payload.extend_from_slice(
            &(socket.inode.as_ref().map_or(0, |pin| pin.0) as u64).to_le_bytes(),
        );
        payload.extend_from_slice(&socket.network.to_le_bytes());
        for word in socket
            .ancillary
            .as_ref()
            .map_or([0; 4], |state| state.inherited())
        {
            payload.extend_from_slice(&word.to_le_bytes());
        }
        payload.extend_from_slice(&socket.record.id().to_le_bytes());
        payload.extend_from_slice(&(socket.record.pin.raw() as u64).to_le_bytes());
        payload.extend_from_slice(
            &socket
                .listener
                .as_ref()
                .map_or(0, |listener| listener.id())
                .to_le_bytes(),
        );
        payload.extend_from_slice(
            &socket
                .listener
                .as_ref()
                .map_or(0, |listener| listener.pin.raw() as u64)
                .to_le_bytes(),
        );
        for address in [&socket.local, &socket.peer] {
            payload.extend_from_slice(&namespace_code(&address.namespace).to_le_bytes());
            payload.extend_from_slice(&(address.name.len() as u32).to_le_bytes());
            payload.extend_from_slice(&address.name);
            while !payload.len().is_multiple_of(8) {
                payload.push(0);
            }
        }
    }
    Ok(payload)
}

pub(crate) fn restore_fork_state(payload: &[u8]) -> bool {
    if payload.len() < 16 || payload[4..8] != 2u32.to_le_bytes() {
        return false;
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default()) as usize;
    let token = u64::from_le_bytes(payload[8..16].try_into().unwrap());
    let mut cursor = 16usize;
    let mut restored = SocketTable::new();
    let mut inodes = HashMap::<usize, Arc<SocketInode>>::new();
    // Retain inherited sections until every surviving alias has reopened them.
    let mut state_pins = HashMap::<u64, crate::fs::object::Object>::new();
    for _ in 0..count {
        if cursor
            .checked_add(104)
            .is_none_or(|end| end > payload.len())
        {
            return false;
        }
        let fd = i32::from_le_bytes(payload[cursor..cursor + 4].try_into().unwrap_or_default());
        let socket_type = i32::from_le_bytes(
            payload[cursor + 4..cursor + 8]
                .try_into()
                .unwrap_or_default(),
        );
        let state = match u32::from_le_bytes(
            payload[cursor + 8..cursor + 12]
                .try_into()
                .unwrap_or_default(),
        ) {
            0 => State::Idle,
            1 => State::Bound,
            2 => State::Listening,
            3 => State::Connected,
            4 => State::Disconnected,
            _ => return false,
        };
        let flags = u32::from_le_bytes(
            payload[cursor + 12..cursor + 16]
                .try_into()
                .unwrap_or_default(),
        );
        let handle = u64::from_le_bytes(
            payload[cursor + 16..cursor + 24]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        let inode_raw =
            u64::from_le_bytes(payload[cursor + 24..cursor + 32].try_into().unwrap()) as usize;
        let network = u64::from_le_bytes(payload[cursor + 32..cursor + 40].try_into().unwrap());
        let Ok(network_pin) = crate::namespaces::pin_network(network) else {
            return false;
        };
        let mut inherited = [0u64; 4];
        for (index, word) in inherited.iter_mut().enumerate() {
            let start = cursor + 40 + index * 8;
            *word = u64::from_le_bytes(payload[start..start + 8].try_into().unwrap());
        }
        let record_id = u64::from_le_bytes(payload[cursor + 72..cursor + 80].try_into().unwrap());
        let record_pin = u64::from_le_bytes(payload[cursor + 80..cursor + 88].try_into().unwrap());
        let listener_id = u64::from_le_bytes(payload[cursor + 88..cursor + 96].try_into().unwrap());
        let listener_pin =
            u64::from_le_bytes(payload[cursor + 96..cursor + 104].try_into().unwrap());
        cursor += 104;
        let mut addresses = Vec::with_capacity(2);
        for _ in 0..2 {
            if cursor + 8 > payload.len() {
                return false;
            }
            let namespace = match u32::from_le_bytes(
                payload[cursor..cursor + 4].try_into().unwrap_or_default(),
            ) {
                1 => Namespace::Pathname,
                2 => Namespace::Abstract,
                3 => Namespace::Unnamed,
                _ => return false,
            };
            let len = u32::from_le_bytes(
                payload[cursor + 4..cursor + 8]
                    .try_into()
                    .unwrap_or_default(),
            ) as usize;
            let start = cursor + 8;
            let Some(end) = start.checked_add(len) else {
                return false;
            };
            let Some(name) = payload.get(start..end) else {
                return false;
            };
            addresses.push(UnixAddress {
                namespace,
                name: name.to_vec(),
            });
            cursor = end.next_multiple_of(8);
        }
        let Ok(entry) = get(fd) else { continue };
        if entry.kind != FdKind::UnixSocket || entry.raw != handle {
            continue;
        }
        for raw in [inherited[1], inherited[3], record_pin, listener_pin] {
            if raw != 0 && !state_pins.contains_key(&raw) {
                let Ok(pin) = crate::fs::object::Object::owned(raw as HANDLE) else {
                    return false;
                };
                state_pins.insert(raw, pin);
            }
        }
        let ancillary = if inherited[0] == 0 {
            None
        } else {
            match ancillary::State::restore([inherited[0], inherited[2]]) {
                Ok(state) => Some(state),
                Err(error) => {
                    trace_unix(|| format!("restore ancillary fd={fd} error={error}"));
                    return false;
                }
            }
        };
        let Ok(record) = procnet::Record::restore(record_id) else {
            return false;
        };
        let listener = if listener_id == 0 {
            None
        } else {
            match listener::Lease::restore(listener_id) {
                Ok(listener) => Some(listener),
                Err(_) => return false,
            }
        };
        trace_unix(|| {
            format!("restored fd={fd} handle={handle:#x} flags={flags:#x} type={socket_type}")
        });
        restored.insert(
            fd,
            UnixSocket {
                record,
                ancillary,
                listener,
                network,
                _network_pin: network_pin,
                socket_type,
                state,
                handle,
                local: addresses.remove(0),
                peer: addresses.remove(0),
                owns_file: flags & 1 != 0,
                inode: if inode_raw == 0 {
                    None
                } else {
                    Some(Arc::clone(
                        inodes
                            .entry(inode_raw)
                            .or_insert_with(|| Arc::new(SocketInode(inode_raw))),
                    ))
                },
                read_shut: flags & 2 != 0,
                write_shut: flags & 4 != 0,
                is_server_end: flags & 8 != 0,
            },
        );
    }
    if cursor != payload.len() {
        return false;
    }
    let Ok(mut sockets) = sockets().lock() else {
        return false;
    };
    *sockets = restored;
    drop(sockets);
    if listener::acknowledge_handoff(token).is_err() {
        return false;
    }
    true
}

/// Reads a snapshot of one socket's state.
fn snapshot(fd: i32) -> Result<UnixSocket, i32> {
    // The descriptor must still be a live Unix socket; a stale side-table entry
    // for a recycled fd would otherwise be readable.
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(usize::try_from(fd).map_err(|_| EBADF)?)
        .and_then(|slot| *slot)
        .ok_or(EBADF)?;
    if entry.kind != FdKind::UnixSocket {
        return Err(ENOTSOCK);
    }
    let mut socket = sockets()
        .lock()
        .map_err(|_| EIO)?
        .get(&fd)
        .cloned()
        .ok_or(EBADF)?;
    drop(table);
    socket.refresh_listener()?;
    Ok(socket)
}

/// Attaches a duplicated named-pipe handle to a second descriptor number.
///
/// `DuplicateHandle` preserves the underlying pipe endpoint, but the Unix
/// socket metadata is keyed by Linux fd and therefore must be copied explicitly.
/// The new table entry owns `handle`; closing either descriptor closes only its
/// own Windows handle, as Linux `dup` closes only one reference to the socket.
pub fn duplicate(oldfd: i32, newfd: i32, handle: usize) -> Result<(), i32> {
    let old_entry = get(oldfd)?;
    let new_entry = get(newfd)?;
    if old_entry.kind != FdKind::UnixSocket
        || new_entry.kind != FdKind::UnixSocket
        || new_entry.raw != handle
    {
        return Err(ENOTSOCK);
    }
    let mut table = sockets().lock().map_err(|_| EIO)?;
    let mut copy = table.get(&oldfd).cloned().ok_or(EBADF)?;
    copy.handle = handle;
    table.insert(newfd, copy);
    Ok(())
}

/// Applies `update` to one socket's state.
fn modify<T>(fd: i32, update: impl FnOnce(&mut UnixSocket) -> Result<T, i32>) -> Result<T, i32> {
    let descriptors = crate::table().read().map_err(|_| EIO)?;
    let entry = descriptors
        .slots
        .get(usize::try_from(fd).map_err(|_| EBADF)?)
        .and_then(|slot| *slot)
        .ok_or(EBADF)?;
    if entry.kind != FdKind::UnixSocket {
        return Err(ENOTSOCK);
    }
    let mut table = sockets().lock().map_err(|_| EIO)?;
    let socket = table.get_mut(&fd).ok_or(EBADF)?;
    let mut updated = socket.clone();
    let result = update(&mut updated)?;
    updated.record.publish(&updated)?;
    *socket = updated;
    Ok(result)
}

/// State detached from a closing descriptor, still holding host resources.
///
/// Returned by [`detach`] so the caller can release it after dropping the
/// descriptor table lock.
pub struct Detached(UnixSocket);

/// Removes a descriptor's state from the side table without releasing it.
///
/// Called by [`crate::close`] while it still holds the descriptor table lock, so
/// the fd cannot be reissued to another thread between the two removals. The
/// host resources are released separately by [`release_detached`], because doing
/// it here would mean blocking on I/O with that lock held.
pub fn detach(fd: i32) -> Option<Detached> {
    sockets().lock().ok()?.remove(&fd).map(Detached)
}

/// Releases the host resources behind detached state.
pub fn release_detached(state: Detached) {
    release(&state.0);
}

/// Releases the host resources behind a socket.
fn release(socket: &UnixSocket) {
    trace_unix(|| {
        format!(
            "release handle={:#x} type={}",
            socket.handle, socket.socket_type
        )
    });
    if socket.handle != 0 {
        // Deliberately not DisconnectNamedPipe, even on the server end: that call
        // discards whatever the peer has not read yet, whereas closing a Unix
        // socket on Linux leaves already-written bytes readable until the peer
        // drains them and only then reports end of file. Closing the handle ends
        // the connection the same way without losing the tail of the stream.
        // SAFETY: this socket owns the handle and it is not used after this.
        unsafe { CloseHandle(socket.handle as HANDLE) };
    }
    // Closing the final socket descriptor does not unlink a pathname socket on
    // Linux. The inode remains until an explicit unlink, regardless of whether
    // shutdown was orderly.
}

/// Encodes a pipe name as the NUL-terminated UTF-16 the Win32 API expects.
fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Maps a Win32 error from a pipe operation to the errno a socket call reports.
///
/// The pipe errors carry socket meanings here: a vanished pipe is a refused
/// connection, and a broken one is a reset peer.
fn errno_from_pipe(error: u32) -> i32 {
    match error {
        ERROR_FILE_NOT_FOUND => ECONNREFUSED,
        ERROR_PIPE_BUSY => EAGAIN,
        ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED => EPIPE,
        ERROR_NO_DATA => EPIPE,
        other => crate::errno_from_win32(other),
    }
}

/// Creates one server-side pipe instance for an address.
///
/// `first` requests `FILE_FLAG_FIRST_PIPE_INSTANCE`, which is what makes `bind`
/// fail with `EADDRINUSE` when another process already owns the name. Subsequent
/// instances for the same listener must not set it.
fn create_instance(
    name: &str,
    message: bool,
    first: bool,
) -> Result<(HANDLE, Arc<ancillary::State>), i32> {
    let _defer = signal::defer_delivery();
    let _publication = InstancePublication::acquire(name)?;
    let handle = create_native_instance(name, message, first)?;
    match ancillary::State::create(handle) {
        Ok(state) => Ok((handle, state)),
        Err(e) => {
            unsafe { CloseHandle(handle) };
            Err(e)
        }
    }
}

/// Serializes publication with clients which opened an instance before its
/// attributes existed. Ownership is native and abandoned-owner aware; there is
/// no timer, process broker, or persistent handle registry. Hash collisions only
/// serialize unrelated short initialization transactions, never identify peers.
struct InstancePublication(crate::fs::object::Object);
impl InstancePublication {
    fn acquire(name: &str) -> Result<Self, i32> {
        use std::hash::{Hash, Hasher};
        use windows_sys::Win32::Foundation::WAIT_ABANDONED;
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        let mut hash = std::hash::DefaultHasher::new();
        name.to_ascii_lowercase().hash(&mut hash);
        let name = wide(&format!(
            "Local\\kinakaze-unix-publish-{:016x}",
            hash.finish()
        ));
        let mutex = crate::fs::object::Object::owned(unsafe {
            CreateMutexW(std::ptr::null(), 0, name.as_ptr())
        })?;
        match unsafe { WaitForSingleObject(mutex.raw(), u32::MAX) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self(mutex)),
            _ => Err(EIO),
        }
    }
}
impl Drop for InstancePublication {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::Threading::ReleaseMutex(self.0.raw()) };
    }
}

fn create_native_instance(name: &str, message: bool, first: bool) -> Result<HANDLE, i32> {
    // Not exported by windows-sys under this name, and it is the flag that makes
    // name ownership exclusive.
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;

    let name = wide(name);
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let pipe_mode = if message {
        PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE
    } else {
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE
    };

    // SAFETY: `name` is a NUL-terminated wide string and the mode flags are a
    // valid combination; a null security descriptor requests the default.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            pipe_mode,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: GetLastError has no preconditions.
        let error = unsafe { GetLastError() };
        return Err(match error {
            // Both mean the name is already taken.
            ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY => EADDRINUSE,
            other => crate::errno_from_win32(other),
        });
    }
    Ok(handle)
}

/// `ERROR_ACCESS_DENIED`, which `CreateNamedPipeW` returns for a name owned by
/// another process.
const ERROR_ACCESS_DENIED: u32 = 5;
/// The NT `IO_STATUS_BLOCK`.
use crate::fs::NativeIoStatus as IoStatusBlock;

/// `FILE_PIPE_LOCAL_INFORMATION`, the read-only view of a pipe instance.
///
/// This is the primitive that makes honest readiness reporting possible. The Win32
/// surface has no way to ask "is a client waiting" or "how much room is left to
/// write"; both are right here, and querying has no side effects.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FilePipeLocalInformation {
    named_pipe_type: u32,
    named_pipe_configuration: u32,
    maximum_instances: u32,
    current_instances: u32,
    inbound_quota: u32,
    read_data_available: u32,
    outbound_quota: u32,
    write_quota_available: u32,
    named_pipe_state: u32,
    named_pipe_end: u32,
}

/// `FilePipeLocalInformation` in the `FILE_INFORMATION_CLASS` enumeration.
const FILE_PIPE_LOCAL_INFORMATION_CLASS: u32 = 24;

// Values of `named_pipe_state`.
#[cfg(test)]
const FILE_PIPE_CONNECTED_STATE: u32 = 3;
const FILE_PIPE_CLOSING_STATE: u32 = 4;

// Not in windows-sys, so declared against ntdll directly, as epoll.rs does for
// the AFD entry points.
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationFile(
        file: HANDLE,
        io_status_block: *mut IoStatusBlock,
        information: *mut core::ffi::c_void,
        length: u32,
        information_class: u32,
    ) -> i32;
}

/// Queries a pipe instance's state without disturbing it.
fn pipe_information(handle: HANDLE) -> Option<FilePipeLocalInformation> {
    let mut information = FilePipeLocalInformation::default();
    let mut status_block = IoStatusBlock::default();
    // SAFETY: the out-parameters are locals of the sizes being declared, and the
    // query is read-only.
    let status = unsafe {
        NtQueryInformationFile(
            handle,
            &raw mut status_block,
            (&raw mut information).cast(),
            size_of::<FilePipeLocalInformation>() as u32,
            FILE_PIPE_LOCAL_INFORMATION_CLASS,
        )
    };
    if status < 0 { None } else { Some(information) }
}

use crate::io_event::IoEvent;

/// `ERROR_OPERATION_ABORTED`, reported for a request cancelled by `CancelIoEx`.
const ERROR_OPERATION_ABORTED: u32 = 995;

/// Waits for a pending pipe request or a signal, cancelling on interruption.
///
/// # Safety
///
/// `overlapped` must describe a request pending on `handle`, and `io_event` must
/// be its completion event.
unsafe fn wait_interruptible(
    handle: HANDLE,
    overlapped: *mut OVERLAPPED,
    io_event: HANDLE,
) -> Result<(), i32> {
    let interrupt = interrupt::current();
    if interrupt.is_null() {
        // SAFETY: the request is pending and the event is live.
        let waited = unsafe { WaitForMultipleObjects(1, &io_event, 0, u32::MAX) };
        return if waited == WAIT_OBJECT_0 {
            Ok(())
        } else {
            // Retire the kernel's stack references before releasing the lease.
            unsafe { CancelIoEx(handle, overlapped) };
            unsafe { WaitForMultipleObjects(1, &io_event, 0, u32::MAX) };
            Err(EIO)
        };
    }

    let handles = [io_event, interrupt];
    // Announce this thread as interruptible so `kill` knows to wake it.
    signal::register_waiter();
    // SAFETY: both handles are live for the duration of the wait.
    let waited = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, u32::MAX) };
    signal::unregister_waiter();

    if waited == WAIT_OBJECT_0 {
        return Ok(());
    }
    if waited != WAIT_OBJECT_0 + 1 {
        unsafe { CancelIoEx(handle, overlapped) };
        unsafe { WaitForMultipleObjects(1, &io_event, 0, u32::MAX) };
        return Err(EIO);
    }

    // The interrupt fired. Cancel and let the kernel retire the request so the
    // result below reflects the final state instead of racing it.
    // SAFETY: `overlapped` names a request pending on this handle.
    unsafe { CancelIoEx(handle, overlapped) };
    // SAFETY: the completion event stays live until the request retires.
    unsafe { WaitForMultipleObjects(1, &io_event, 0, u32::MAX) };
    // Clear the consumed interrupt so it does not abort the next call.
    // SAFETY: the interrupt event belongs to this thread.
    unsafe { ResetEvent(interrupt) };

    // A handler ran. SA_RESTART turns into a retry at the call site.
    match signal::deliver_pending() {
        signal::Delivery::Restart => Err(EAGAIN),
        _ => Err(EINTR),
    }
}

fn check_rebind(current: FdEntry, expected: FdEntry) -> Result<(), i32> {
    if current.generation != expected.generation || current.kind != FdKind::UnixSocket {
        return Err(EBADF);
    }
    if current.raw != expected.raw {
        return Err(EAGAIN);
    }
    Ok(())
}

/// Publish the descriptor and Unix state under the same table guard. In-flight
/// operations pin the previous handle; the descriptor owns the new one only
/// after this transaction succeeds. The update cannot re-enter either table.
fn publish_socket(
    fd: i32,
    expected: FdEntry,
    handle: crate::fs::object::Object,
    update: impl FnOnce(&mut UnixSocket),
) -> Result<(), i32> {
    let mut descriptors = crate::table().write().map_err(|_| EIO)?;
    let entry = descriptors
        .slots
        .get_mut(usize::try_from(fd).map_err(|_| EBADF)?)
        .and_then(|slot| slot.as_mut())
        .ok_or(EBADF)?;
    check_rebind(*entry, expected)?;
    let mut states = sockets().lock().map_err(|_| EIO)?;
    let socket = states.get_mut(&fd).ok_or(EBADF)?;
    if socket.handle != entry.raw {
        return Err(EBADF);
    }
    let inheritance = crate::exec_inheritance::DescriptorInheritance::prepare(
        handle.raw() as usize,
        entry.kind,
        entry.flags,
    )?;
    let mut updated = socket.clone();
    update(&mut updated);
    updated.record.publish(&updated)?;
    *socket = updated;
    entry.raw = handle.into_raw() as usize;
    socket.handle = entry.raw;
    inheritance.commit();
    let entry = *entry;
    drop(states);
    drop(descriptors);
    crate::epoll::descriptor_rebound(fd, entry);
    if expected.raw != 0 {
        // Native waiters own duplicates; only the descriptor's old reference
        // is retired here. Accept uses a separate atomic ownership transfer.
        unsafe { CloseHandle(expected.raw as HANDLE) };
    }
    Ok(())
}

/// `socket(AF_UNIX, ...)`.
///
/// The descriptor is created with no handle at all: on Linux an unbound Unix
/// socket has no kernel rendezvous either, and only `bind` or `connect` creates
/// the pipe.
pub fn socket(socket_type: i32, protocol: i32) -> Result<i32, i32> {
    // Unix sockets have exactly one protocol.
    if protocol != 0 {
        return Err(EPROTOTYPE);
    }
    let non_blocking = socket_type & SOCK_NONBLOCK != 0;
    let close_on_exec = socket_type & crate::socket::SOCK_CLOEXEC != 0;
    let base_type = socket_type & !(SOCK_NONBLOCK | crate::socket::SOCK_CLOEXEC);
    if !matches!(base_type, SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET) {
        return Err(EPROTOTYPE);
    }

    let mut flags = FdFlags::NONE;
    if non_blocking {
        flags = flags.union(FdFlags::NONBLOCK);
    }
    if close_on_exec {
        flags = flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    // Overlapped because every pipe instance is created that way, and the
    // transfer path keys off this flag.
    flags = flags.union(FdFlags::OVERLAPPED);

    // The side-table entry is created while the descriptor table lock is held, so
    // the fd is never visible without its state.
    crate::install_handleless_with(FdKind::UnixSocket, flags, |fd| {
        sockets()
            .lock()
            .map_err(|_| EIO)?
            .insert(fd, UnixSocket::new(base_type, crate::usernet::current()?)?);
        Ok(())
    })
}

/// `bind`.
///
/// Claims the name by creating the first pipe instance, so two binds to one
/// address collide in the pipe namespace rather than through a file check.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
pub unsafe fn bind(fd: i32, address: *const u8, length: i32) -> Result<(), i32> {
    // SAFETY: forwarded from this function's contract.
    let parsed = unsafe { parse_address(address, length) }?;
    if parsed.namespace == Namespace::Unnamed {
        return Err(EINVAL);
    }
    if let Some(path) = parsed.filesystem_path()
        && path.len() > SUN_PATH_MAX
    {
        return Err(ENAMETOOLONG);
    }

    let (entry, current, _pin) = ancillary::pin_socket(fd)?;
    if current.state != State::Idle {
        // Already bound or connected.
        return Err(EINVAL);
    }

    // CREATE_NEW owns a pathname; the pinned inode, not guest path text, names
    // the transport. Bind aliases and chroots therefore reach the same inode,
    // while identical names in independent roots reach different endpoints.
    let mut owns_file = false;
    let inode = if let Some(path) = parsed.filesystem_path() {
        let pin = crate::fs::create_socket_inode(&path)
            .map_err(|error| if error == EEXIST { EADDRINUSE } else { error })?;
        owns_file = true;
        Some(pin)
    } else {
        None
    };
    let pipe_name = match &inode {
        Some(pin) => inode_pipe_name(pin.as_raw_handle().cast())?,
        None => parsed.pipe_name_in(current.network),
    };
    let _publication = InstancePublication::acquire(&pipe_name)?;
    let instance = if current.socket_type == SOCK_DGRAM {
        create_instance(&pipe_name, current.message_mode(), true)
            .map(|(handle, state)| (handle, Some(state)))
    } else {
        create_native_instance(&pipe_name, current.message_mode(), true)
            .map(|handle| (handle, None))
    };
    let (handle, ancillary) = match instance {
        Ok(handle) => handle,
        Err(error) => {
            if let Some(pin) = &inode {
                crate::fs::unlink_inode(pin.as_raw_handle().cast())?;
            }
            return Err(error);
        }
    };
    let inode = inode.map(|pin| Arc::new(SocketInode(pin.into_raw_handle() as usize)));
    if let Some(pin) = &inode {
        if let Err(error) = crate::platform::try_set_inheritable(pin.0, true) {
            unsafe { CloseHandle(handle) };
            crate::fs::unlink_inode(pin.0 as HANDLE)?;
            return Err(error);
        }
    }

    let owned = crate::fs::object::Object::owned(handle)?;
    let listener = if current.socket_type == SOCK_DGRAM {
        None
    } else {
        match listener::Lease::create(&pipe_name, current.socket_type, handle) {
            Ok(listener) => Some(listener),
            Err(error) => {
                if let Some(inode) = &inode {
                    let _ = crate::fs::unlink_inode(inode.0 as HANDLE);
                }
                return Err(error);
            }
        }
    };
    let published = publish_socket(fd, entry, owned, |socket| {
        socket.state = State::Bound;
        socket.ancillary = ancillary;
        socket.listener = listener;
        socket.local = parsed;
        socket.owns_file = owns_file;
        socket.inode = inode.clone();
    });
    if published.is_err()
        && let Some(pin) = &inode
    {
        crate::fs::unlink_inode(pin.0 as HANDLE)?;
    }
    published.and_then(|()| publish_existing_filter(fd))
}

/// `listen`.
///
/// Publish the transition and backlog in the shared bound description.
pub fn listen(fd: i32, backlog: i32) -> Result<(), i32> {
    let socket = snapshot(fd)?;
    if socket.socket_type == SOCK_DGRAM {
        return Err(EOPNOTSUPP);
    }
    if !matches!(socket.state, State::Bound | State::Listening) {
        return Err(EINVAL);
    }
    socket.listener.as_ref().ok_or(EIO)?.pool.listen(backlog)?;
    modify(fd, |socket| {
        if socket.socket_type == SOCK_DGRAM {
            // A datagram socket has no connection to accept.
            return Err(EOPNOTSUPP);
        }
        match socket.state {
            State::Bound | State::Listening => {
                socket.state = State::Listening;
                Ok(())
            }
            // Linux requires a bound address before listening on AF_UNIX.
            _ => Err(EINVAL),
        }
    })
}

/// `accept4`.
///
/// Dequeues a connection while retaining the listener's stable binding.
///
/// # Safety
///
/// `address` and `length` must be null or a writable pair.
pub unsafe fn accept(fd: i32, address: *mut u8, length: *mut i32, flags: i32) -> Result<i32, i32> {
    let (entry, listener, _pin) = ancillary::pin_socket(fd)?;
    if listener.state != State::Listening {
        return Err(EINVAL);
    }
    if let Some(shared) = &listener.listener {
        if flags & !(SOCK_NONBLOCK | crate::socket::SOCK_CLOEXEC) != 0 {
            return Err(EINVAL);
        }
        let reservation = crate::reserve_descriptor()?;
        let result =
            connect_with_restart(|| shared.accept(entry.flags.contains(FdFlags::NONBLOCK)));
        let (handle, state, peer, drained) = match result {
            Ok(result) => result,
            Err(error) => {
                if error == EAGAIN {
                    crate::epoll::readiness_consumed(
                        entry.description_id,
                        crate::epoll::EPOLLIN | crate::epoll::EPOLLRDNORM,
                    );
                }
                return Err(error);
            }
        };
        if drained {
            crate::epoll::readiness_consumed(
                entry.description_id,
                crate::epoll::EPOLLIN | crate::epoll::EPOLLRDNORM,
            );
        }
        let mut client = UnixSocket::new(listener.socket_type, listener.network)?;
        client.state = State::Connected;
        client.ancillary = Some(state);
        client.handle = handle.raw() as usize;
        client.local = listener.local.clone();
        client.inode = listener.inode.clone();
        client.peer = peer.clone();
        client.is_server_end = true;
        client.record.publish(&client)?;
        let passcred = listener.record.passcred()?;
        client.record.set_passcred(passcred)?;
        client
            .ancillary
            .as_ref()
            .ok_or(EIO)?
            .set_passcred(true, passcred)?;
        let mut accepted_flags = FdFlags::OVERLAPPED;
        if flags & SOCK_NONBLOCK != 0 {
            accepted_flags = accepted_flags.union(FdFlags::NONBLOCK);
        }
        if flags & crate::socket::SOCK_CLOEXEC != 0 {
            accepted_flags = accepted_flags.union(FdFlags::CLOSE_ON_EXEC);
        }
        let accepted = reservation.install_with(
            handle.raw() as usize,
            FdKind::UnixSocket,
            accepted_flags,
            |accepted, _| {
                sockets().lock().map_err(|_| EIO)?.insert(accepted, client);
                Ok(())
            },
        )?;
        handle.into_raw();
        if let Err(error) = unsafe { write_address(&peer, address, length) } {
            let _ = crate::close(accepted);
            return Err(error);
        }
        return Ok(accepted);
    }
    Err(EIO)
}

/// `connect`.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
pub unsafe fn connect(fd: i32, address: *const u8, length: i32) -> Result<(), i32> {
    let tracing = crate::diagnostic_target_enabled("KINAKAZE_UNIX_CONNECT_TRACE");
    let began = tracing.then(std::time::Instant::now);
    let before = tracing.then(|| get(fd).ok()).flatten();
    let result = connect_with_restart(|| unsafe { connect_inner(fd, address, length) });
    let result = result.and_then(|()| publish_existing_filter(fd));
    if let Some(began) = began {
        use std::io::Write;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!("unix-connect-{}.log", std::process::id()));
        if let Ok(mut output) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let line = format!(
                "utc_us={} tid={} fd={fd} before={before:?} after={:?} state={:?} elapsed_us={} result={result:?}\n",
                now.as_micros(),
                unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() },
                get(fd).ok(),
                snapshot(fd).ok().map(|s| s.state),
                began.elapsed().as_micros()
            );
            let _ = output.write_all(line.as_bytes());
        }
    }
    result
}

fn connect_with_restart<T>(mut operation: impl FnMut() -> Result<T, i32>) -> Result<T, i32> {
    loop {
        let interrupted_before = signal::nonrestart_epoch();
        // Metadata/native-open failure unwinds every temporary handle before
        // a guest handler can enter the filesystem or fork. No payload has
        // been submitted by connect(), so a restarting signal may retry it.
        let result = {
            let _defer = signal::defer_delivery();
            operation()
        };
        if !matches!(&result, Err(error) if *error == EINTR) || signal::delivery_deferred() {
            return result;
        }
        let delivery = signal::deliver_pending();
        if delivery == signal::Delivery::Interrupted
            || signal::nonrestart_epoch() != interrupted_before
        {
            return result;
        }
        // Ignored/stale interrupts and SA_RESTART must not escape as a phantom
        // asynchronous connect. Go treats EINTR like EINPROGRESS and waits for
        // EPOLLOUT even if the path lookup never opened a pipe.
    }
}

unsafe fn connect_inner(fd: i32, address: *const u8, length: i32) -> Result<(), i32> {
    // SAFETY: forwarded from this function's contract.
    let parsed = unsafe { parse_address(address, length) }?;
    if parsed.namespace == Namespace::Unnamed {
        return Err(EINVAL);
    }

    let (entry, current, _pin) = ancillary::pin_socket(fd)?;
    if current.state == State::Connected {
        return Err(EISCONN);
    }
    let non_blocking = entry.flags.contains(FdFlags::NONBLOCK);

    let path_pin = if let Some(path) = parsed.filesystem_path() {
        let native = crate::fs::resolve(&path)?;
        Some(crate::fs::object::Object::open(
            &native,
            windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES,
        )?)
    } else {
        None
    };
    let pipe_name = match &path_pin {
        Some(pin) => inode_pipe_name(pin.raw())?,
        None => parsed.pipe_name_in(current.network),
    };
    if current.socket_type != SOCK_DGRAM {
        if current.state == State::Listening {
            return Err(EINVAL);
        }
        if let Some(pool) = listener::Pool::lookup(&pipe_name)? {
            let (handle, ancillary) =
                pool.connect(current.socket_type, &current.local, non_blocking)?;
            publish_socket(fd, entry, handle, |socket| {
                socket.state = State::Connected;
                socket.ancillary = Some(ancillary);
                socket.peer = parsed;
            })?;
            return Ok(());
        }
    }
    let name = wide(&pipe_name);
    let handle = loop {
        // SAFETY: `name` is NUL-terminated; OPEN_EXISTING never creates.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            break handle;
        }
        // SAFETY: GetLastError has no preconditions.
        let error = unsafe { GetLastError() };
        if error != ERROR_PIPE_BUSY {
            return Err(errno_from_pipe(error));
        }
        // Every instance is serving another client. Linux would queue against
        // the backlog here, so a blocking socket waits instead of failing.
        if non_blocking {
            // AF_UNIX reports EAGAIN when its accept queue cannot admit this
            // connection. No native open is pending here: EINPROGRESS would
            // strand a caller waiting for an EPOLLOUT completion that cannot
            // occur. The descriptor remains available for an explicit retry.
            return Err(EAGAIN);
        }
        // SAFETY: `name` is a NUL-terminated pipe name.
        if unsafe { WaitNamedPipeW(name.as_ptr(), 5_000) } == 0 {
            // SAFETY: GetLastError has no preconditions.
            let error = unsafe { GetLastError() };
            // A timeout means the server never freed an instance; retrying
            // forever would hide a wedged peer.
            if error == ERROR_SEM_TIMEOUT {
                return Err(crate::ETIMEDOUT);
            }
            return Err(errno_from_pipe(error));
        }
    };

    // A message-mode pipe must be read in message mode from both ends, or a
    // datagram gets split across reads.
    if current.message_mode() {
        let mut mode = PIPE_READMODE_MESSAGE;
        // SAFETY: the handle is a connected client end and `mode` is a valid
        // read-mode request; the remaining out-parameters are optional.
        unsafe {
            SetNamedPipeHandleState(
                handle,
                &raw mut mode,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
    }

    let owned = crate::fs::object::Object::owned(handle)?;
    let ancillary = match ancillary::State::open(handle) {
        // CreateNamedPipe publishes the endpoint before its connection
        // attributes can be initialized. A client may already have opened it.
        // Wait for that short publication transaction only on this race path;
        // normal connections do not acquire another named mutex.
        Err(crate::ENOENT) => {
            let _publication = InstancePublication::acquire(&pipe_name)?;
            ancillary::State::open(handle)?
        }
        other => other?,
    };
    publish_socket(fd, entry, owned, |socket| {
        socket.state = State::Connected;
        socket.ancillary = Some(ancillary);
        socket.peer = parsed.clone();
    })?;
    trace_unix(|| {
        format!(
            "connect fd={fd} handle={:#x} peer={:?}",
            handle as usize, parsed.name
        )
    });
    Ok(())
}

/// `ERROR_SEM_TIMEOUT`, which `WaitNamedPipeW` reports when it gives up.
const ERROR_SEM_TIMEOUT: u32 = 121;

/// Returns how many bytes are buffered on a connected pipe.
///
/// This is what makes non-blocking reads and `epoll` readiness possible: a pipe
/// has no readiness primitive of its own, so availability is polled.
fn available(handle: HANDLE) -> Result<u32, i32> {
    let mut available = 0u32;
    // SAFETY: only the total-available out-parameter is requested; the rest are
    // optional and passed as null.
    let ok = unsafe {
        PeekNamedPipe(
            handle,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &raw mut available,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_pipe(unsafe { GetLastError() }));
    }
    Ok(available)
}

/// Builds the descriptor entry the shared overlapped transfer path expects.
///
/// A pipe has no file position, so `SEEKABLE` is deliberately absent: that keeps
/// the overlapped offset at zero, which is required for pipe handles.
fn transfer_entry(handle: usize) -> FdEntry {
    FdEntry {
        raw: handle,
        kind: FdKind::Pipe,
        flags: FdFlags::OVERLAPPED
            .union(FdFlags::PIPE_READ_END)
            .union(FdFlags::PIPE_WRITE_END),
        generation: 0,
        description_id: 0,
        offset: 0,
    }
}

/// `send`/`write` on a Unix socket.
///
/// # Safety
///
/// `buffer` must be readable for `len` bytes.
pub unsafe fn send(fd: i32, buffer: *const u8, len: usize, flags: i32) -> Result<usize, i32> {
    unsafe { send_rights(fd, buffer, len, flags, &[]) }
}
unsafe fn send_payload(
    fd: i32,
    fd_entry: FdEntry,
    socket: &UnixSocket,
    buffer: *const u8,
    len: usize,
    flags: i32,
) -> Result<usize, i32> {
    if socket.state != State::Connected {
        // A datagram socket that was never connected has no destination.
        return Err(if socket.socket_type == SOCK_DGRAM {
            EDESTADDRREQ
        } else {
            ENOTCONN
        });
    }
    // POSIX would also raise SIGPIPE here; this layer reports the error only,
    // which is what MSG_NOSIGNAL asks for and is safe for callers that do not.
    if socket.write_shut {
        return Err(EPIPE);
    }
    if len == 0 && !socket.message_mode() {
        return Ok(0);
    }

    let entry = transfer_entry(socket.handle);
    let non_blocking = fd_entry.flags.contains(FdFlags::NONBLOCK) || flags & MSG_DONTWAIT != 0;
    let mut observed_quota = None;

    let result = (|| {
        // A stream may accept a short write. Submitting more than the available
        // pipe quota instead queues an all-bytes request, whose cancellation
        // can report zero even though the stream had room for a useful prefix.
        // Message sockets retain their atomic message path, never a truncation.
        let amount = if non_blocking && socket.socket_type == SOCK_STREAM {
            let info = pipe_information(socket.handle as HANDLE).ok_or(EIO)?;
            if info.named_pipe_state == FILE_PIPE_CLOSING_STATE {
                return Err(EPIPE);
            }
            observed_quota = Some(info.write_quota_available as usize);
            len.min(info.write_quota_available as usize)
        } else {
            len
        };
        if amount == 0 && !socket.message_mode() {
            return Err(EAGAIN);
        }
        // SAFETY: the caller provides `len` readable bytes; amount <= len.
        if non_blocking || amount == 0 {
            unsafe { write_without_blocking(socket.handle as HANDLE, buffer, amount) }
        } else {
            unsafe { crate::platform_write(entry, buffer.cast_mut(), amount) }
        }
    })();
    if result == Err(EAGAIN)
        || result.is_ok_and(|written| observed_quota.is_some_and(|quota| written >= quota))
    {
        // The send observed backpressure even if the peer drains the pipe
        // before epoll gets to sample it. Retire the old writable edge for the
        // open description, including registrations through duplicated fds.
        crate::epoll::readiness_consumed(
            fd_entry.description_id,
            crate::epoll::EPOLLOUT | crate::epoll::EPOLLWRNORM,
        );
    }
    let result = match result {
        Ok(written) => Ok(written),
        Err(EPIPE) => {
            // A concurrent close may have recycled fd; the pinned endpoint
            // alone determines this operation and needs no side-table mutation.
            Err(EPIPE)
        }
        Err(error) => Err(error),
    };
    trace_unix(|| {
        format!(
            "send fd={fd} handle={:#x} len={len} flags={flags:#x} nonblocking={non_blocking} result={result:?}",
            socket.handle
        )
    });
    result
}

/// Non-consuming message inspection, including the first message's complete
/// length. Win32 PeekNamedPipe hides NumberOfMessages, which is needed to
/// distinguish an empty record from a pipe with no records yet.
unsafe fn inspect_socket(
    handle: HANDLE,
    buffer: *mut u8,
    len: usize,
    non_blocking: bool,
    message: bool,
    full_length: bool,
    limit: Option<&dyn Fn(usize) -> Result<usize, i32>>,
) -> Result<Option<usize>, i32> {
    // A large caller buffer must not force an equally large temporary when
    // only a short payload is queued. Keep one query for the common <=64 KiB
    // path; larger peeks first bound storage by the observable payload.
    let len = if len > 64 * 1024 {
        let mut header = [std::mem::MaybeUninit::<u32>::uninit(); 4];
        let available = unsafe {
            inspect_socket_into(
                handle,
                std::ptr::null_mut(),
                0,
                non_blocking,
                message,
                true,
                header.as_mut_ptr().cast(),
                16,
                None,
            )
        }?;
        let Some(available) = available else {
            return Ok(None);
        };
        len.min(available)
    } else {
        len
    };
    // Length/readiness checks never need a payload buffer or its stack probe.
    if len == 0 {
        let mut header = [std::mem::MaybeUninit::<u32>::uninit(); 4];
        return unsafe {
            inspect_socket_into(
                handle,
                buffer,
                len,
                non_blocking,
                message,
                full_length,
                header.as_mut_ptr().cast(),
                16,
                limit,
            )
        };
    }
    if (513..=4096).contains(&len) {
        // Retain the allocation-free 4 KiB payload path without charging its
        // larger stack frame to header-only or short-message queries.
        unsafe {
            inspect_socket_payload::<1028>(
                handle,
                buffer,
                len,
                non_blocking,
                message,
                full_length,
                limit,
            )
        }
    } else {
        unsafe {
            inspect_socket_payload::<132>(
                handle,
                buffer,
                len,
                non_blocking,
                message,
                full_length,
                limit,
            )
        }
    }
}

// Keep the payload frame out of the header-only path, including under LTO.
#[inline(never)]
unsafe fn inspect_socket_payload<const WORDS: usize>(
    handle: HANDLE,
    buffer: *mut u8,
    len: usize,
    non_blocking: bool,
    message: bool,
    full_length: bool,
    limit: Option<&dyn Fn(usize) -> Result<usize, i32>>,
) -> Result<Option<usize>, i32> {
    let capacity = len.min(u32::MAX as usize - 16) + 16;
    // Each specialization has its own bounded stack frame. Larger peeks
    // reserve uninitialized storage: only the bytes returned by NPFS are read.
    let mut local = [std::mem::MaybeUninit::<u32>::uninit(); WORDS];
    let mut storage = Vec::new();
    let bytes: *mut u8 = if capacity <= std::mem::size_of_val(&local) {
        local.as_mut_ptr().cast()
    } else {
        storage
            .try_reserve_exact(capacity.div_ceil(4))
            .map_err(|_| crate::ENOMEM)?;
        let pointer: *mut std::mem::MaybeUninit<u32> = storage.as_mut_ptr();
        pointer.cast()
    };
    unsafe {
        inspect_socket_into(
            handle,
            buffer,
            len,
            non_blocking,
            message,
            full_length,
            bytes,
            capacity,
            limit,
        )
    }
}

#[inline(never)]
unsafe fn inspect_socket_into(
    handle: HANDLE,
    buffer: *mut u8,
    len: usize,
    non_blocking: bool,
    message: bool,
    full_length: bool,
    bytes: *mut u8,
    capacity: usize,
    limit: Option<&dyn Fn(usize) -> Result<usize, i32>>,
) -> Result<Option<usize>, i32> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtFsControlFile(
            file: HANDLE,
            event: HANDLE,
            apc: *const core::ffi::c_void,
            context: *const core::ffi::c_void,
            io: *mut IoStatusBlock,
            code: u32,
            input: *const core::ffi::c_void,
            input_len: u32,
            output: *mut core::ffi::c_void,
            output_len: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }
    let event = IoEvent::new().ok_or(EIO)?;
    loop {
        let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
        operation.hEvent = event.0;
        let mut status = unsafe {
            NtFsControlFile(
                handle,
                event.0,
                std::ptr::null(),
                std::ptr::null(),
                (&raw mut operation).cast(),
                0x0011400c,
                std::ptr::null(),
                0,
                bytes.cast(),
                capacity as u32,
            )
        };
        if status == 0x103 {
            match unsafe { wait_interruptible(handle, &raw mut operation, event.0) } {
                Ok(()) => {}
                Err(EAGAIN) => continue,
                Err(e) => return Err(e),
            }
            status = operation.Internal as i32;
        }
        if status < 0 && status as u32 != 0x80000005 {
            return match errno_from_pipe(unsafe { RtlNtStatusToDosError(status) }) {
                EPIPE => Ok(None),
                e => Err(e),
            };
        }
        if operation.InternalHigh < 16 || operation.InternalHigh > capacity {
            return Err(EIO);
        }
        let word = |offset| unsafe { bytes.add(offset).cast::<u32>().read_unaligned() };
        let present = if message { word(8) > 0 } else { word(4) > 0 };
        if present {
            let copied = operation.InternalHigh.saturating_sub(16).min(len);
            // Metadata is published before its payload. Limit this immutable
            // snapshot after inspection, before writing any caller bytes; new
            // sends cannot extend it past an ancillary or credential boundary.
            let copied = match limit {
                Some(limit) => copied.min(limit(copied)?),
                None => copied,
            };
            if copied != 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.add(16), buffer, copied);
                }
            }
            return Ok(Some(if message && full_length {
                word(12) as usize
            } else if full_length && len == 0 {
                // Header-only stream inspection also returns the byte count.
                // The ancillary reader holds the cross-process receive mutex
                // until it consumes this snapshot, so it can reuse the bound.
                word(4) as usize
            } else {
                copied
            }));
        }
        if word(0) == FILE_PIPE_CLOSING_STATE {
            return Ok(None);
        }
        if non_blocking {
            return Err(EAGAIN);
        }
        // A non-consuming NPFS peek has no wait-for-data operation. Recheck
        // with the same bounded interval as epoll while remaining interruptible.
        let interrupt = interrupt::current();
        if interrupt.is_null() {
            std::thread::sleep(std::time::Duration::from_millis(1));
            continue;
        }
        signal::register_waiter();
        let waited = unsafe { WaitForMultipleObjects(1, &interrupt, 0, 1) };
        signal::unregister_waiter();
        if waited == WAIT_OBJECT_0 {
            if signal::deliver_pending() != signal::Delivery::Restart {
                return Err(EINTR);
            }
        } else if waited != windows_sys::Win32::Foundation::WAIT_TIMEOUT {
            return Err(EIO);
        }
    }
}

/// `recv`/`read` on a Unix socket.
///
/// # Safety
///
/// `buffer` must be writable for `len` bytes.
pub unsafe fn recv(fd: i32, buffer: *mut u8, len: usize, flags: i32) -> Result<usize, i32> {
    unsafe { recv_rights(fd, buffer, len, flags, 0).map(|result| result.0) }
}
unsafe fn recv_payload(
    fd: i32,
    fd_entry: FdEntry,
    socket: &UnixSocket,
    buffer: *mut u8,
    len: usize,
    flags: i32,
    inspected_length: usize,
) -> Result<usize, i32> {
    if !matches!(socket.state, State::Connected | State::Disconnected) {
        return Err(ENOTCONN);
    }
    // `shutdown(SHUT_RD)` makes every later read report end of file.
    if socket.read_shut {
        return Ok(0);
    }
    if len == 0 && !socket.message_mode() {
        return Ok(0);
    }

    let handle = socket.handle as HANDLE;
    let non_blocking = fd_entry.flags.contains(FdFlags::NONBLOCK) || flags & MSG_DONTWAIT != 0;

    debug_assert_eq!(flags & MSG_PEEK, 0);
    // Ancillary::recv already inspected the pinned endpoint while holding its
    // shared receive mutex. No other reader can consume that record meanwhile.
    let record_length = socket.message_mode().then_some(inspected_length);
    if len == 0 {
        drain_message(handle);
        return Ok(if flags & 0x20 != 0 {
            record_length.unwrap_or(0)
        } else {
            0
        });
    }

    let observed_available = (!socket.message_mode()).then_some(inspected_length);

    let entry = transfer_entry(socket.handle);
    // SAFETY: the caller guarantees `len` writable bytes.
    let result = match unsafe { crate::platform_read(entry, buffer, len) } {
        Ok(read) => Ok(if flags & 0x20 != 0 {
            record_length.unwrap_or(read)
        } else {
            read
        }),
        Err(crate::EMSGSIZE) => {
            // A message longer than the caller's buffer. Linux truncates the
            // datagram and discards the remainder, whereas Windows leaves the
            // tail queued for the next read, which would surface it as a phantom
            // message. Draining it here restores the Linux behaviour.
            drain_message(socket.handle as HANDLE);
            Ok(if flags & 0x20 != 0 {
                record_length.unwrap_or(len)
            } else {
                len
            })
        }
        Err(EPIPE) => {
            // The peer closing is end of file for a stream read.
            Ok(0)
        }
        Err(ECONNRESET) => Ok(0),
        Err(error) => Err(error),
    };
    if let Ok(read) = result
        && read > 0
    {
        // If the read consumed every byte observed before it began, a later
        // byte belongs to a new readiness transition even when the peer wrote
        // too quickly for epoll's polling pass to sample the empty pipe. The
        // second check handles a partial record or a message-mode pipe.
        let drained_observed = observed_available.is_some_and(|amount| read >= amount);
        if drained_observed || matches!(available(handle), Ok(0)) {
            crate::epoll::readiness_consumed(
                fd_entry.description_id,
                crate::epoll::EPOLLIN | crate::epoll::EPOLLRDNORM,
            );
        }
    }
    trace_unix(|| {
        format!(
            "recv fd={fd} handle={:#x} len={len} flags={flags:#x} nonblocking={non_blocking} result={result:?}",
            socket.handle
        )
    });
    result
}

/// Writes without blocking, reporting `EAGAIN` if the pipe could not take it.
///
/// The stream caller bounds its request by a send-quota snapshot. That query
/// cannot reserve space against a concurrent writer, so an operation may still
/// pend. Retire such a request before releasing its OVERLAPPED and guest buffer;
/// report the completion's transferred count, or EAGAIN when none is reported.
/// Message callers use this without splitting their message into stream writes.
///
/// # Safety
///
/// `buffer` must be readable for `len` bytes.
unsafe fn write_without_blocking(
    handle: HANDLE,
    buffer: *const u8,
    len: usize,
) -> Result<usize, i32> {
    let io_event = IoEvent::new().ok_or(EIO)?;
    // SAFETY: OVERLAPPED is plain data; a pipe ignores the offset fields.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = io_event.0;
    let amount = len.min(u32::MAX as usize) as u32;

    // SAFETY: the caller guarantees `amount` readable bytes, and `overlapped`
    // outlives every wait below.
    let started = unsafe {
        WriteFile(
            handle,
            buffer,
            amount,
            std::ptr::null_mut(),
            &raw mut overlapped,
        )
    };

    if started == 0 {
        // SAFETY: GetLastError has no preconditions.
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(errno_from_pipe(error));
        }
        // The pipe could not take it all at once. Cancel and see how far it got.
        // SAFETY: the request is pending on this handle.
        unsafe { CancelIoEx(handle, &raw mut overlapped) };
        // Unbounded on purpose: returning while the request can still complete
        // would let the driver write into this dead stack frame.
        // SAFETY: the event stays live until the request retires.
        unsafe { WaitForMultipleObjects(1, &io_event.0, 0, u32::MAX) };
    }

    let mut transferred = 0u32;
    // SAFETY: the request has completed or been cancelled, so its result is ready.
    let ok = unsafe { GetOverlappedResult(handle, &raw const overlapped, &raw mut transferred, 0) };
    if ok == 0 {
        // SAFETY: GetLastError has no preconditions.
        return match unsafe { GetLastError() } {
            // Cancelled with nothing accepted is precisely "would block". POSIX
            // requires the partial count when bytes did move.
            ERROR_OPERATION_ABORTED if transferred == 0 => Err(EAGAIN),
            ERROR_OPERATION_ABORTED => Ok(transferred as usize),
            other => Err(errno_from_pipe(other)),
        };
    }
    if transferred == 0 && amount > 0 {
        return Err(EAGAIN);
    }
    Ok(transferred as usize)
}

/// Reads and throws away the rest of a partially consumed message.
///
/// Windows keeps the tail of an oversized message-mode read queued; Linux
/// discards it. Without this the leftover bytes would be delivered as if they
/// were a separate datagram.
fn drain_message(handle: HANDLE) {
    let entry = transfer_entry(handle as usize);
    let mut scratch = [0u8; 4096];
    loop {
        // Whatever remains of the current message is already buffered, so these
        // reads do not block. Only EMSGSIZE means the message is still not
        // finished; a plain Ok completed it, and continuing on that would block
        // waiting for a message that has not been sent.
        // SAFETY: the scratch buffer provides its own length in writable bytes.
        match unsafe { crate::platform_read(entry, scratch.as_mut_ptr(), scratch.len()) } {
            Err(crate::EMSGSIZE) => continue,
            // Anything else ends the message, errors included: leaving the socket
            // readable would be worse than losing the diagnosis.
            _ => return,
        }
    }
}

/// `sendto`. An address is only meaningful for an unconnected datagram socket,
/// which this emulation does not support, so a null address forwards to `send`.
///
/// # Safety
///
/// `buffer` must be readable for `len` bytes, and `address` must be null or
/// readable for `address_length` bytes.
pub unsafe fn sendto(
    fd: i32,
    buffer: *const u8,
    len: usize,
    flags: i32,
    address: *const u8,
    address_length: i32,
) -> Result<usize, i32> {
    unsafe {
        validate_destination(fd, address, address_length)?;
        send(fd, buffer, len, flags)
    }
}
/// Validate the destination before publishing any ancillary references.
/// Safety: address is null or readable for address_length bytes.
pub unsafe fn validate_destination(
    fd: i32,
    address: *const u8,
    address_length: i32,
) -> Result<(), i32> {
    if !address.is_null() && address_length > 0 {
        let socket = snapshot(fd)?;
        if socket.state == State::Connected {
            // Linux allows a destination on a connected socket only if it names
            // the same peer; comparing is cheaper than being wrong.
            // SAFETY: forwarded from this function's contract.
            let target = unsafe { parse_address(address, address_length) }?;
            if target != socket.peer && target.namespace != Namespace::Unnamed {
                return Err(EISCONN);
            }
        } else {
            // An unconnected datagram send would need a server multiplexing
            // recvfrom across pipe instances, which this layer does not build.
            return Err(EOPNOTSUPP);
        }
    }
    Ok(())
}

/// `recvfrom`. The peer address is always reported as unnamed, because a pipe
/// carries no sender identity.
///
/// # Safety
///
/// `buffer` must be writable for `len` bytes, and the address pair must be null
/// or writable.
pub unsafe fn recvfrom(
    fd: i32,
    buffer: *mut u8,
    len: usize,
    flags: i32,
    address: *mut u8,
    address_length: *mut i32,
) -> Result<usize, i32> {
    // SAFETY: forwarded from this function's contract.
    let read = unsafe { recv(fd, buffer, len, flags) }?;
    let peer = snapshot(fd)
        .map(|socket| socket.peer)
        .unwrap_or_else(|_| UnixAddress::unnamed());
    // SAFETY: forwarded from this function's contract.
    unsafe { write_address(&peer, address, address_length) }?;
    Ok(read)
}

/// `shutdown`.
///
/// Half-close state is shared by every inherited or transferred endpoint.
pub fn shutdown(fd: i32, how: i32) -> Result<(), i32> {
    let socket = snapshot(fd)?;
    if !matches!(socket.state, State::Connected | State::Disconnected) {
        return Err(ENOTCONN);
    }
    match how {
        SHUT_RD | SHUT_WR | SHUT_RDWR => {}
        _ => return Err(EINVAL),
    }

    ancillary::shutdown(&socket, how)?;
    modify(fd, |socket| {
        if how == SHUT_RD || how == SHUT_RDWR {
            socket.read_shut = true;
        }
        if how == SHUT_WR || how == SHUT_RDWR {
            socket.write_shut = true;
        }
        Ok(())
    })?;

    Ok(())
}

/// `getsockname`.
///
/// # Safety
///
/// The address pair must be null or writable.
pub unsafe fn getsockname(fd: i32, address: *mut u8, length: *mut i32) -> Result<(), i32> {
    let socket = snapshot(fd)?;
    // SAFETY: forwarded from this function's contract.
    unsafe { write_address(&socket.local, address, length) }
}

/// `getpeername`.
///
/// # Safety
///
/// The address pair must be null or writable.
pub unsafe fn getpeername(fd: i32, address: *mut u8, length: *mut i32) -> Result<(), i32> {
    let socket = snapshot(fd)?;
    if socket.state != State::Connected {
        return Err(ENOTCONN);
    }
    // SAFETY: forwarded from this function's contract.
    unsafe { write_address(&socket.peer, address, length) }
}

/// `setsockopt`. Unix sockets accept the options that have meaning and refuse the
/// rest rather than pretending to store them.
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
    // Confirm the descriptor is a Unix socket before reporting anything.
    let socket = snapshot(fd)?;
    if level != SOL_SOCKET {
        return Err(ENOPROTOOPT);
    }
    match name {
        16 => {
            // SO_PASSCRED
            if length < size_of::<i32>() as i32 {
                return Err(EINVAL);
            }
            if value.is_null() {
                return Err(EFAULT);
            }
            let enabled = unsafe { value.cast::<i32>().read_unaligned() } != 0;
            if let Some(state) = &socket.ancillary {
                state.set_passcred_with(socket.is_server_end, enabled, || {
                    socket.record.set_passcred(enabled)
                })
            } else {
                socket.record.set_passcred(enabled)
            }
        }
        // Buffer sizes are fixed at pipe creation. Accepting the request without
        // honouring it matches how Linux treats these as advisory.
        SO_SNDBUF | SO_RCVBUF => Ok(()),
        // Harmless on a pipe, and programs set them reflexively.
        crate::socket::SO_REUSEADDR | crate::socket::SO_KEEPALIVE | crate::socket::SO_LINGER => {
            Ok(())
        }
        _ => Err(ENOPROTOOPT),
    }
}

/// `getsockopt`.
///
/// # Safety
///
/// `value` must be writable for `*length` bytes, and `length` must be readable
/// and writable.
pub unsafe fn getsockopt(
    fd: i32,
    level: i32,
    name: i32,
    value: *mut u8,
    length: *mut i32,
) -> Result<(), i32> {
    let socket = snapshot(fd)?;
    if level != SOL_SOCKET {
        return Err(ENOPROTOOPT);
    }
    if value.is_null() || length.is_null() {
        return Err(EFAULT);
    }
    // SAFETY: the caller guarantees the length out-pointer is readable.
    let capacity = unsafe { *length };
    if capacity < 0 {
        return Err(EINVAL);
    }

    /// Copies an integer option out, honouring the caller's capacity.
    ///
    /// # Safety
    ///
    /// `value` must be writable for `capacity` bytes.
    unsafe fn write_int(
        value: *mut u8,
        length: *mut i32,
        capacity: i32,
        result: i32,
    ) -> Result<(), i32> {
        let bytes = result.to_le_bytes();
        let copied = bytes.len().min(capacity as usize);
        // SAFETY: bounded by the caller's stated capacity.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), value, copied);
            *length = copied as i32;
        }
        Ok(())
    }

    match name {
        // SAFETY: bounded by the checked capacity.
        SO_TYPE => unsafe { write_int(value, length, capacity, socket.socket_type) },
        16 => unsafe {
            write_int(
                value,
                length,
                capacity,
                i32::from(socket.record.passcred()?),
            )
        },
        // AF_UNIX has no selectable transport protocol. These read-only Linux
        // options let callers reconstruct a socket from a received descriptor.
        crate::socket::SO_DOMAIN => unsafe { write_int(value, length, capacity, AF_UNIX) },
        crate::socket::SO_PROTOCOL => unsafe { write_int(value, length, capacity, 0) },
        // No asynchronous error is ever pending: connect either completed or
        // reported its failure directly.
        // SAFETY: bounded by the checked capacity.
        SO_ERROR => unsafe { write_int(value, length, capacity, 0) },
        // SAFETY: bounded by the checked capacity.
        SO_SNDBUF | SO_RCVBUF => unsafe { write_int(value, length, capacity, PIPE_BUFFER as i32) },
        crate::socket::SO_ACCEPTCONN => {
            let listening = i32::from(socket.state == State::Listening);
            // SAFETY: bounded by the checked capacity.
            unsafe { write_int(value, length, capacity, listening) }
        }
        SO_PEERCRED => {
            let peer = if socket.state == State::Listening {
                socket
                    .listener
                    .as_ref()
                    .ok_or(EIO)?
                    .pool
                    .credentials()?
                    .visible()?
            } else if matches!(socket.state, State::Connected | State::Disconnected)
                && (socket.socket_type != SOCK_DGRAM
                    || (socket.local.namespace == Namespace::Unnamed
                        && socket.peer.namespace == Namespace::Unnamed))
            {
                ancillary::peer_credentials(socket.handle as HANDLE, socket.is_server_end)?
                    .visible()?
            } else {
                // Ordinary datagram connect has no credential exchange. Linux
                // exposes unset peer credentials, as for an unconnected socket.
                credentials::Ucred {
                    pid: 0,
                    uid: u32::MAX,
                    gid: u32::MAX,
                }
            };
            let mut credentials = [0u8; 12];
            credentials[..4].copy_from_slice(&peer.pid.to_le_bytes());
            credentials[4..8].copy_from_slice(&peer.uid.to_le_bytes());
            credentials[8..].copy_from_slice(&peer.gid.to_le_bytes());
            let copied = credentials.len().min(capacity as usize);
            // SAFETY: bounded by the caller's stated capacity.
            unsafe {
                std::ptr::copy_nonoverlapping(credentials.as_ptr(), value, copied);
                *length = copied as i32;
            }
            Ok(())
        }
        _ => Err(ENOPROTOOPT),
    }
}

/// `socketpair`.
///
/// Both ends are anonymous on Linux, so this binds a uniquely named pipe, connects
/// to it, and leaves neither end with a name the guest can observe.
pub fn socketpair(socket_type: i32) -> Result<(i32, i32), i32> {
    let base_type = socket_type & !(SOCK_NONBLOCK | crate::socket::SOCK_CLOEXEC);
    if !matches!(base_type, SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET) {
        return Err(EPROTOTYPE);
    }
    let message = base_type != SOCK_STREAM;

    // A private name in the abstract namespace: it needs to be unique against
    // every other pair in every process, so it carries the process id and a
    // counter.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ordinal = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let address = UnixAddress {
        namespace: Namespace::Abstract,
        name: format!("socketpair.{}.{ordinal}", std::process::id()).into_bytes(),
    };

    let (server, ancillary) = create_instance(&address.pipe_name(), message, true)?;
    let name = wide(&address.pipe_name());
    // SAFETY: `name` is NUL-terminated and the pipe was just created, so
    // OPEN_EXISTING finds it.
    let client = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            std::ptr::null_mut(),
        )
    };
    if client == INVALID_HANDLE_VALUE {
        // SAFETY: GetLastError has no preconditions.
        let error = unsafe { GetLastError() };
        // SAFETY: the server instance is still owned here.
        unsafe { CloseHandle(server) };
        return Err(errno_from_pipe(error));
    }

    // The connect above satisfies the server's pending connection, so no
    // ConnectNamedPipe is needed; the instance is already paired.
    if message {
        let mut mode = PIPE_READMODE_MESSAGE;
        // SAFETY: both handles are live and `mode` is a valid read-mode request.
        unsafe {
            SetNamedPipeHandleState(
                client,
                &raw mut mode,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
    }

    let mut flags = FdFlags::OVERLAPPED;
    if socket_type & SOCK_NONBLOCK != 0 {
        flags = flags.union(FdFlags::NONBLOCK);
    }
    if socket_type & crate::socket::SOCK_CLOEXEC != 0 {
        flags = flags.union(FdFlags::CLOSE_ON_EXEC);
    }

    let first = crate::install(server as usize, FdKind::UnixSocket, flags).inspect_err(|_| {
        // SAFETY: neither handle has been installed, so both are still owned.
        unsafe {
            CloseHandle(server);
            CloseHandle(client);
        }
    })?;
    let second = match crate::install(client as usize, FdKind::UnixSocket, flags) {
        Ok(second) => second,
        Err(error) => {
            let _ = crate::close(first);
            // SAFETY: the client end was never installed.
            unsafe { CloseHandle(client) };
            return Err(error);
        }
    };

    let mut table = sockets().lock().map_err(|_| EIO)?;
    for (fd, is_server) in [(first, true), (second, false)] {
        let mut socket = UnixSocket::new(base_type, crate::usernet::current()?)?;
        socket.ancillary = Some(ancillary.clone());
        socket.state = State::Connected;
        socket.handle = if is_server {
            server as usize
        } else {
            client as usize
        };
        socket.is_server_end = is_server;
        socket.record.publish(&socket)?;
        // A socketpair has no name at either end.
        table.insert(fd, socket);
    }
    Ok((first, second))
}

/// Reports what a Unix socket is ready for, without blocking.
///
/// `epoll` calls this because a pipe has no AFD equivalent: readiness is polled
/// rather than waited on. A listening socket is readable when a client is
/// waiting, and a connected one when bytes are buffered.
pub fn poll_readiness(fd: i32) -> Result<Readiness, i32> {
    let (_, socket, _pin) = ancillary::pin_socket(fd)?;
    let mut readiness = Readiness(0);

    match socket.state {
        State::Listening => {
            let listener = socket.listener.as_ref().ok_or(EIO)?;
            return Ok(if listener.pool.readable()? {
                Readiness::READABLE
            } else {
                Readiness(0)
            });
        }
        State::Connected => {
            let (read_closed, write_closed, all_closed) = ancillary::shutdown_state(&socket)?;
            match pipe_information(socket.handle as HANDLE) {
                Some(information) => {
                    if information.read_data_available > 0 {
                        trace_unix(|| {
                            format!(
                                "ready fd={fd} handle={:#x} available={} write-quota={} state={}",
                                socket.handle,
                                information.read_data_available,
                                information.write_quota_available,
                                information.named_pipe_state
                            )
                        });
                    }
                    if information.read_data_available > 0 {
                        readiness = readiness.union(Readiness::READABLE);
                    }
                    // A closing peer is readable at end of file and hung up.
                    if information.named_pipe_state == FILE_PIPE_CLOSING_STATE {
                        readiness = readiness
                            .union(Readiness::READABLE)
                            .union(Readiness::HANGUP);
                    }
                    // Real write space, rather than the usual assumption that a
                    // pipe is always writable. A full outbound buffer now reports
                    // not-writable, which is what an `epoll` caller needs to avoid
                    // spinning on a peer that has stopped reading.
                    if !socket.write_shut
                        && !write_closed
                        && information.named_pipe_state != FILE_PIPE_CLOSING_STATE
                        && information.write_quota_available > 0
                    {
                        readiness = readiness.union(Readiness::WRITABLE);
                    }
                }
                // The query failed, which for a connected pipe means the handle is
                // no longer usable.
                None => {
                    readiness = readiness
                        .union(Readiness::READABLE)
                        .union(Readiness::HANGUP);
                }
            }
            // A read-shutdown socket is readable at end of file forever.
            if socket.read_shut || read_closed {
                readiness = readiness.union(Readiness::READABLE);
            }
            if all_closed {
                readiness = readiness.union(Readiness::HANGUP);
            }
            if socket.message_mode()
                && unsafe {
                    inspect_socket(
                        socket.handle as HANDLE,
                        std::ptr::null_mut(),
                        0,
                        true,
                        true,
                        true,
                        None,
                    )
                }
                .is_ok_and(|n| n.is_some())
            {
                readiness = readiness.union(Readiness::READABLE);
            }
        }
        State::Disconnected => {
            readiness = readiness
                .union(Readiness::READABLE)
                .union(Readiness::HANGUP);
        }
        // An idle or merely bound socket is never ready.
        State::Idle | State::Bound => {}
    }
    Ok(readiness)
}

/// True when the descriptor is an emulated Unix socket.
pub fn is_unix_socket(fd: i32) -> bool {
    get(fd)
        .map(|entry| entry.kind == FdKind::UnixSocket)
        .unwrap_or(false)
}
