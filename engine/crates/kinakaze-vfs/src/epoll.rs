//! `epoll` built on the Windows AFD poll interface.
//!
//! Winsock's own `WSAPoll` cannot be cancelled and ignores some conditions, so
//! readiness comes from `\Device\Afd` directly: `NtDeviceIoControlFile` with
//! `IOCTL_AFD_POLL` is the same primitive Windows' own socket layer uses, and it
//! reports readiness as an event rather than as a completion. That avoids
//! translating between the readiness model epoll promises and the completion
//! model IOCP provides.
//!
//! The poll is issued against a socket's *base* handle. A socket may be layered
//! by LSPs, and AFD only understands the bottom of that stack, so every
//! registration resolves the base handle once with `SIO_BASE_HANDLE`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

#[cfg(all(test, windows))]
mod fifo_tests;
pub(crate) mod native_wait;
pub(crate) mod packet_wait;
#[cfg(all(test, windows))]
mod socket_edge_tests;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_NOT_CONNECTED, GetLastError,
    HANDLE, STATUS_PENDING, STATUS_SUCCESS, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Networking::WinSock::{SIO_BASE_HANDLE, SOCKET, SOCKET_ERROR, WSAIoctl};
use windows_sys::Win32::Storage::FileSystem::WriteFile;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::socket::Readiness;
use crate::{
    EBADF, EEXIST, EINTR, EINVAL, EIO, ENOENT, EPERM, FdFlags, FdKind, install, interrupt, signal,
};

// Linux epoll event bits.
pub const EPOLLIN: u32 = 0x001;
pub const EPOLLPRI: u32 = 0x002;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDNORM: u32 = 0x040;
pub const EPOLLRDBAND: u32 = 0x080;
pub const EPOLLWRNORM: u32 = 0x100;
pub const EPOLLWRBAND: u32 = 0x200;
pub const EPOLLRDHUP: u32 = 0x2000;
/// Request one-shot delivery: the registration is disarmed after it fires.
pub const EPOLLONESHOT: u32 = 1 << 30;
/// Request edge-triggered delivery.
pub const EPOLLET: u32 = 1 << 31;

// `epoll_ctl` operations.
pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;

/// `EPOLL_CLOEXEC`, the only flag `epoll_create1` accepts.
pub const EPOLL_CLOEXEC: i32 = 0o2000000;

/// The Linux x86_64 `struct epoll_event`.
///
/// This is packed on x86_64: 4 bytes of events followed by an 8-byte union with
/// no padding between them, so the struct is 12 bytes rather than 16. Guest code
/// indexes arrays of these, so the layout is ABI.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

// AFD poll event bits, from the Windows socket driver's interface.
const AFD_POLL_RECEIVE: u32 = 0x0001;
const AFD_POLL_RECEIVE_EXPEDITED: u32 = 0x0002;
const AFD_POLL_SEND: u32 = 0x0004;
const AFD_POLL_DISCONNECT: u32 = 0x0008;
const AFD_POLL_ABORT: u32 = 0x0010;
const AFD_POLL_LOCAL_CLOSE: u32 = 0x0020;
const AFD_POLL_ACCEPT: u32 = 0x0080;
const AFD_POLL_CONNECT_FAIL: u32 = 0x0100;

/// `IOCTL_AFD_POLL`.
const IOCTL_AFD_POLL: u32 = 0x0001_2024;

// NT statuses AFD reports when a descriptor from an older epoll snapshot was
// closed before or during submission. These are control-flow races, not an I/O
// failure of the epoll descriptor itself.
const STATUS_INVALID_HANDLE: i32 = 0xc000_0008u32 as i32;
// A closed socket's numeric handle can already name an event (or another
// non-file object) when a concurrent epoll waiter submits its old snapshot.
const STATUS_OBJECT_TYPE_MISMATCH: i32 = 0xc000_0024u32 as i32;
// A reused handle may still be a file object, but no longer an AFD device
// (for example NUL or a regular file). Such an object rejects the AFD ioctl.
const STATUS_INVALID_DEVICE_REQUEST: i32 = 0xc000_0010u32 as i32;
const STATUS_CANCELLED: i32 = 0xc000_0120u32 as i32;
const STATUS_FILE_CLOSED: i32 = 0xc000_0128u32 as i32;

#[inline]
fn stale_afd_status(status: i32) -> bool {
    matches!(
        status,
        STATUS_INVALID_HANDLE
            | STATUS_OBJECT_TYPE_MISMATCH
            | STATUS_INVALID_DEVICE_REQUEST
            | STATUS_CANCELLED
            | STATUS_FILE_CLOSED
    )
}

/// One socket's poll request or result.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AfdPollHandleInfo {
    handle: usize,
    events: u32,
    status: i32,
}

/// The `AFD_POLL_INFO` header, followed by `handle_count` handle entries.
#[repr(C)]
struct AfdPollInfo {
    /// Relative timeout in 100ns units; negative means relative to now.
    timeout: i64,
    handle_count: u32,
    /// Non-zero requests level-triggered semantics from the driver.
    exclusive: u32,
    handles: [AfdPollHandleInfo; MAX_POLL_HANDLES],
}

/// Sockets polled per `NtDeviceIoControlFile` call.
///
/// The request is a single contiguous structure, so this bounds one syscall's
/// batch rather than the number of registrations an epoll set may hold.
const MAX_POLL_HANDLES: usize = 512;

/// The NT `IO_STATUS_BLOCK`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IoStatusBlock {
    status: i32,
    information: usize,
}

// `NtDeviceIoControlFile` and `NtCreateFile` are not in windows-sys, so they are
// declared here against ntdll directly.
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtDeviceIoControlFile(
        file: HANDLE,
        event: HANDLE,
        apc_routine: *const core::ffi::c_void,
        apc_context: *const core::ffi::c_void,
        io_status_block: *mut IoStatusBlock,
        io_control_code: u32,
        input_buffer: *const core::ffi::c_void,
        input_buffer_length: u32,
        output_buffer: *mut core::ffi::c_void,
        output_buffer_length: u32,
    ) -> i32;

    fn NtCancelIoFileEx(
        file: HANDLE,
        io_request_to_cancel: *const IoStatusBlock,
        io_status_block: *mut IoStatusBlock,
    ) -> i32;

}

/// One registered descriptor.
#[derive(Clone, Copy)]
struct Registration {
    /// The AFD base handle, resolved once at registration time.
    base_handle: usize,
    /// A currently live descriptor naming the registered open description.
    /// This may differ from the descriptor used at ADD after that descriptor is
    /// closed while a `dup` remains open.
    poll_fd: i32,
    /// Events the guest asked for.
    interest: u32,
    /// The guest's opaque cookie, returned verbatim on readiness.
    data: u64,
    /// True once a one-shot registration has fired.
    disarmed: bool,
    /// Readiness already reported, used for edge-triggered suppression.
    reported: u32,
    /// Local in-flight samples must not restore an edge retired by concurrent
    /// I/O. No outstanding epoll call crosses provider fork/exec restoration.
    readiness_revision: u64,
    /// Shared eventfd read/write wake generations consumed by this registration.
    eventfd_edges: [u64; 2],
    /// Whether this descriptor is a socket; non-sockets take the always-ready
    /// path because AFD only understands sockets.
    is_socket: bool,
    is_packet: bool,
    packet_output_blocked: bool,
    /// Whether this is an emulated `AF_UNIX` socket. Those are named pipes, which
    /// AFD cannot poll, so their readiness is queried each pass instead.
    is_unix: bool,
    /// Whether this is an in-memory eventfd. Its readiness follows the counter;
    /// treating it like an always-ready regular file loses EPOLLET wakeups.
    is_eventfd: bool,
    /// Netlink readiness follows the emulated kernel response queue. It is a
    /// pollable socket on Linux even though it has no Winsock/AFD handle here.
    is_netlink: bool,
    /// Inotify readiness follows its queued event records.
    is_inotify: bool,
    /// Whether this is a pipe. A pipe is readable only when data or EOF is
    /// present; reporting an empty pipe as readable makes the following read
    /// block inside a callback and starves every other epoll registration.
    is_pipe: bool,
    is_fifo: bool,
    /// Access belongs to the Linux open description and is captured at pipe
    /// creation.  It is never rediscovered by querying a synchronous host pipe.
    pipe_readable: bool,
    pipe_writable: bool,
    pipe_overlapped: bool,
    /// Whether this is one end of a pseudo-terminal. Its readiness comes from
    /// the line discipline — a terminal with half a line typed is *not*
    /// readable in canonical mode — so it must not fall into the always-ready
    /// bucket that regular files use.
    is_pty: bool,
}

/// Linux keys an epoll registration by the pair (descriptor number, open file
/// description).  Including both permits a closed numeric fd to be reused and
/// added again while a duplicate still keeps the older registration alive.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct RegistrationKey {
    fd: i32,
    description_id: u64,
}

/// Fallback for listener/write/half-close and other emulated readiness without
/// native notifications yet. Connected Unix stream data and EOF additionally
/// wake through a non-consuming overlapped read, without waiting for this tick.
const UNIX_POLL_INTERVAL_MS: u32 = 10;

/// An epoll set.
struct EpollSet {
    registrations: HashMap<RegistrationKey, Registration>,
    /// Manual-reset event signalled whenever the interest list changes.
    ///
    /// Linux wakes a blocked epoll waiter when a watched open description is
    /// removed.  AFD only sees sockets, so this event is the control-plane half
    /// of the wait and prevents a concurrent close from turning into EIO.
    wake_handle: usize,
}

/// Whether this descriptor type has an epoll readiness implementation.
///
/// Linux rejects regular files and other objects without a `poll` operation
/// with EPERM at `epoll_ctl` time. Accepting them and fabricating permanent
/// readiness makes event loops spin and prevents their documented blocking-I/O
/// path from being selected.
fn supports_epoll(kind: FdKind) -> bool {
    matches!(
        kind,
        FdKind::Socket
            | FdKind::UnixSocket
            | FdKind::NetlinkSocket
            | FdKind::EventFd
            | FdKind::TimerFd
            | FdKind::MessageQueue
            | FdKind::SysfsFile
            | FdKind::Inotify
            | FdKind::Pipe
            | FdKind::Fifo
            | FdKind::PtyMaster
            | FdKind::PtySlave
            | FdKind::Null
            | FdKind::Zero
            | FdKind::Random
            | FdKind::Full
    )
}

static SETS: OnceLock<Mutex<HashMap<i32, EpollSet>>> = OnceLock::new();
static SETS_LOCK_OWNER: AtomicUsize = AtomicUsize::new(0);
static PIPE_STATE_TRACE_ACTIVE: AtomicBool = AtomicBool::new(false);
static PIPE_STATE_TRACE_LINES: AtomicUsize = AtomicUsize::new(0);

fn epoll_trace_enabled() -> bool {
    crate::diagnostic_target_enabled("KINAKAZE_EPOLL_TRACE")
}

pub fn enable_pipe_state_trace() {
    if std::env::var_os("KINAKAZE_PIPE_TRACE").is_some() {
        PIPE_STATE_TRACE_ACTIVE.store(true, Ordering::Release);
    }
}

fn sets() -> &'static Mutex<HashMap<i32, EpollSet>> {
    SETS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct SetsGuard(MutexGuard<'static, HashMap<i32, EpollSet>>);

impl std::ops::Deref for SetsGuard {
    type Target = HashMap<i32, EpollSet>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for SetsGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for SetsGuard {
    fn drop(&mut self) {
        SETS_LOCK_OWNER.store(0, Ordering::Release);
    }
}

fn lock_sets(owner: usize) -> Result<SetsGuard, ()> {
    let guard = sets().lock().map_err(|_| ())?;
    SETS_LOCK_OWNER.store(owner, Ordering::Release);
    Ok(SetsGuard(guard))
}

/// Serializes epoll interest state.  The transient AFD poll request/event is not
/// included; `epoll_wait` creates those per call in the child as on the parent.
pub(crate) fn serialize_fork_state() -> Result<Vec<u8>, i32> {
    let sets = lock_sets(1).map_err(|_| EIO)?;
    let mut payload = Vec::new();
    payload.extend_from_slice(&(sets.keys().filter(|&&fd| fd >= 0).count() as u32).to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    for (epoll_fd, set) in sets.iter() {
        if *epoll_fd < 0 {
            continue;
        }
        payload.extend_from_slice(&epoll_fd.to_le_bytes());
        payload.extend_from_slice(&(set.registrations.len() as u32).to_le_bytes());
        for (key, registration) in set.registrations.iter() {
            let flags = u32::from(registration.disarmed)
                | (u32::from(registration.is_socket) << 1)
                | (u32::from(registration.is_unix) << 2);
            payload.extend_from_slice(&key.fd.to_le_bytes());
            payload.extend_from_slice(&key.description_id.to_le_bytes());
            payload.extend_from_slice(&registration.interest.to_le_bytes());
            payload.extend_from_slice(&registration.data.to_le_bytes());
            payload.extend_from_slice(&registration.reported.to_le_bytes());
            payload.extend_from_slice(&flags.to_le_bytes());
            for edge in registration.eventfd_edges {
                payload.extend_from_slice(&edge.to_le_bytes());
            }
        }
    }
    Ok(payload)
}

pub(crate) fn restore_fork_state(payload: &[u8]) -> bool {
    if payload.len() < 8 {
        return false;
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default()) as usize;
    let mut cursor = 8usize;
    let mut restored = HashMap::with_capacity(count);
    for _ in 0..count {
        if cursor + 8 > payload.len() {
            return false;
        }
        let epoll_fd =
            i32::from_le_bytes(payload[cursor..cursor + 4].try_into().unwrap_or_default());
        let registrations = u32::from_le_bytes(
            payload[cursor + 4..cursor + 8]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        let epoll_handle = crate::get(epoll_fd).ok().map(|entry| entry.raw);
        cursor += 8;
        let mut set = EpollSet {
            registrations: HashMap::with_capacity(registrations),
            wake_handle: epoll_handle.unwrap_or_default(),
        };
        for _ in 0..registrations {
            if cursor + 48 > payload.len() {
                return false;
            }
            let fd = i32::from_le_bytes(payload[cursor..cursor + 4].try_into().unwrap_or_default());
            let description_id = u64::from_le_bytes(
                payload[cursor + 4..cursor + 12]
                    .try_into()
                    .unwrap_or_default(),
            );
            let interest = u32::from_le_bytes(
                payload[cursor + 12..cursor + 16]
                    .try_into()
                    .unwrap_or_default(),
            );
            let data = u64::from_le_bytes(
                payload[cursor + 16..cursor + 24]
                    .try_into()
                    .unwrap_or_default(),
            );
            let reported = u32::from_le_bytes(
                payload[cursor + 24..cursor + 28]
                    .try_into()
                    .unwrap_or_default(),
            );
            let flags = u32::from_le_bytes(
                payload[cursor + 28..cursor + 32]
                    .try_into()
                    .unwrap_or_default(),
            );
            let eventfd_edges = [
                u64::from_le_bytes(payload[cursor + 32..cursor + 40].try_into().unwrap()),
                u64::from_le_bytes(payload[cursor + 40..cursor + 48].try_into().unwrap()),
            ];
            cursor += 48;
            let Some((poll_fd, entry)) = crate::get_by_description_id(description_id) else {
                continue;
            };
            if entry.kind == FdKind::Pipe && !entry.flags.has_pipe_access() {
                return false;
            }
            if !supports_epoll(entry.kind) {
                return false;
            }
            let is_socket = flags & 2 != 0;
            let is_unix = flags & 4 != 0;
            let resolved = if is_socket {
                match base_handle(entry.raw as SOCKET) {
                    Ok(value) => value,
                    Err(_) => continue,
                }
            } else {
                entry.raw
            };
            if epoll_handle.is_some() {
                set.registrations.insert(
                    RegistrationKey { fd, description_id },
                    Registration {
                        readiness_revision: 0,
                        eventfd_edges,
                        base_handle: resolved,
                        poll_fd,
                        interest,
                        data,
                        disarmed: flags & 1 != 0,
                        reported,
                        is_socket,
                        is_packet: entry.flags.contains(FdFlags::PACKET_SOCKET),
                        packet_output_blocked: false,
                        is_unix,
                        is_eventfd: matches!(
                            entry.kind,
                            FdKind::EventFd
                                | FdKind::TimerFd
                                | FdKind::MessageQueue
                                | FdKind::SysfsFile
                        ),
                        is_netlink: entry.kind == FdKind::NetlinkSocket,
                        is_inotify: entry.kind == FdKind::Inotify,
                        is_pipe: entry.kind == FdKind::Pipe,
                        is_fifo: entry.kind == FdKind::Fifo,
                        pipe_readable: entry.flags.contains(FdFlags::PIPE_READ_END),
                        pipe_writable: entry.flags.contains(FdFlags::PIPE_WRITE_END),
                        pipe_overlapped: entry.flags.contains(FdFlags::OVERLAPPED),
                        is_pty: matches!(entry.kind, FdKind::PtyMaster | FdKind::PtySlave),
                    },
                );
            }
        }
        if epoll_handle.is_some() {
            restored.insert(epoll_fd, set);
        }
    }
    if cursor != payload.len() {
        return false;
    }
    let Ok(mut sets) = lock_sets(2) else {
        return false;
    };
    *sets = restored;
    true
}

/// Resolves a socket's base handle for AFD.
///
/// Layered service providers can wrap a socket, and AFD only understands the
/// bottom of that stack. Skipping this makes the poll fail with an obscure NT
/// status on machines that have an LSP installed.
fn base_handle(socket: SOCKET) -> Result<usize, i32> {
    let mut base: SOCKET = 0;
    let mut returned = 0u32;
    // SAFETY: SIO_BASE_HANDLE writes one SOCKET into the output buffer.
    let result = unsafe {
        WSAIoctl(
            socket,
            SIO_BASE_HANDLE,
            std::ptr::null(),
            0,
            (&raw mut base).cast(),
            size_of::<SOCKET>() as u32,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    };
    if result == SOCKET_ERROR {
        // Without an LSP the socket is its own base handle, so a failure here is
        // recoverable rather than fatal.
        return Ok(socket);
    }
    Ok(base as usize)
}

/// Translates epoll interest into AFD poll bits.
fn interest_to_afd(interest: u32) -> u32 {
    let mut events = 0;
    if interest & (EPOLLIN | EPOLLRDNORM) != 0 {
        // Accept is the readable condition for a listening socket, and
        // disconnect is how a peer close surfaces as readable-at-EOF.
        events |= AFD_POLL_RECEIVE | AFD_POLL_ACCEPT | AFD_POLL_DISCONNECT;
    }
    if interest & (EPOLLPRI | EPOLLRDBAND) != 0 {
        events |= AFD_POLL_RECEIVE_EXPEDITED;
    }
    if interest & (EPOLLOUT | EPOLLWRNORM) != 0 {
        events |= AFD_POLL_SEND;
    }
    // Error conditions are always requested: epoll reports EPOLLERR and
    // EPOLLHUP whether or not the caller asked for them.
    events | AFD_POLL_ABORT | AFD_POLL_CONNECT_FAIL | AFD_POLL_LOCAL_CLOSE
}

/// Translates AFD poll results into epoll event bits.
fn afd_to_events(afd: u32, interest: u32) -> u32 {
    let mut events = 0;
    if afd & (AFD_POLL_RECEIVE | AFD_POLL_ACCEPT) != 0 {
        events |= EPOLLIN | EPOLLRDNORM;
    }
    if afd & AFD_POLL_RECEIVE_EXPEDITED != 0 {
        events |= EPOLLPRI | EPOLLRDBAND;
    }
    if afd & AFD_POLL_SEND != 0 {
        events |= EPOLLOUT | EPOLLWRNORM;
    }
    if afd & AFD_POLL_DISCONNECT != 0 {
        // A graceful peer shutdown is both readable and a read-hangup, which is
        // what lets a reader see the remaining data before the EOF.
        events |= EPOLLIN | EPOLLRDNORM | EPOLLRDHUP;
    }
    if afd & AFD_POLL_ABORT != 0 {
        events |= EPOLLHUP;
    }
    if afd & (AFD_POLL_CONNECT_FAIL | AFD_POLL_LOCAL_CLOSE) != 0 {
        events |= EPOLLERR;
    }
    // Only the requested conditions are delivered, except the error ones, which
    // epoll always reports.
    events & (interest | EPOLLERR | EPOLLHUP | EPOLLRDHUP)
}

/// AFD polls levels. An edge already delivered must be removed from its
/// request, or a writable socket completes every poll immediately forever.
/// Socket I/O retires the corresponding history and wakes this set to rebuild.
fn pending_afd_events(registration: &Registration) -> u32 {
    if registration.is_packet {
        return AFD_POLL_RECEIVE
            | AFD_POLL_ABORT
            | AFD_POLL_LOCAL_CLOSE
            | if registration.packet_output_blocked {
                AFD_POLL_SEND
            } else {
                0
            };
    }
    let mut events = interest_to_afd(registration.interest);
    if registration.interest & EPOLLRDHUP != 0 {
        events |= AFD_POLL_DISCONNECT;
    }
    if registration.interest & EPOLLET != 0 {
        for bit in [
            AFD_POLL_RECEIVE,
            AFD_POLL_RECEIVE_EXPEDITED,
            AFD_POLL_SEND,
            AFD_POLL_DISCONNECT,
            AFD_POLL_ABORT,
            AFD_POLL_LOCAL_CLOSE,
            AFD_POLL_ACCEPT,
            AFD_POLL_CONNECT_FAIL,
        ] {
            if afd_to_events(bit, registration.interest) & !registration.reported == 0 {
                events &= !bit;
            }
        }
    }
    events
}

/// Creates an epoll set and returns its descriptor.
/// An operation-owned set uses the same readiness engine without consuming a
/// Linux fd or quota. Its registrations never enter fork/exec snapshots.
pub struct PollSet {
    key: i32,
    handle: usize,
}

impl PollSet {
    pub fn new() -> Result<Self, i32> {
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if handle.is_null() {
            return Err(EIO);
        }
        let result = (|| {
            let mut sets = lock_sets(3).map_err(|_| EIO)?;
            let key = (i32::MIN..0)
                .rev()
                .find(|key| !sets.contains_key(key))
                .ok_or(crate::ENOMEM)?;
            sets.insert(
                key,
                EpollSet {
                    registrations: HashMap::new(),
                    wake_handle: handle as usize,
                },
            );
            Ok(Self {
                key,
                handle: handle as usize,
            })
        })();
        if result.is_err() {
            unsafe { CloseHandle(handle) };
        }
        result
    }

    pub fn add(&self, fd: i32, event: EpollEvent) -> Result<(), i32> {
        control_set(self.key, EPOLL_CTL_ADD, fd, Some(event))
    }

    /// Return EINTR with all wait resources released. The caller drops this
    /// operation before delivering a guest handler which could fork or exec.
    pub fn wait(&self, events: &mut [EpollEvent], timeout: i32) -> Result<usize, i32> {
        wait_set(self.key, events, timeout, false, true)
    }
}

impl Drop for PollSet {
    fn drop(&mut self) {
        forget_set(self.key, self.handle);
        unsafe { CloseHandle(self.handle as HANDLE) };
    }
}

pub fn epoll_create1(flags: i32) -> Result<i32, i32> {
    if flags & !EPOLL_CLOEXEC != 0 {
        return Err(EINVAL);
    }
    // An epoll set is not a kernel object here, but it still needs a descriptor
    // number. A manual-reset event is the placeholder object: it is cheap and
    // gives the table a real handle to own and close.
    // SAFETY: null security descriptor and name request an unnamed event.
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if handle.is_null() {
        return Err(EIO);
    }
    let mut fd_flags = FdFlags::NONE;
    if flags & EPOLL_CLOEXEC != 0 {
        fd_flags = fd_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    let fd = install(handle as usize, FdKind::Event, fd_flags).inspect_err(|_| {
        // SAFETY: installation failed, so this function still owns the event.
        unsafe { CloseHandle(handle) };
    })?;
    if let Ok(mut sets) = lock_sets(3) {
        sets.insert(
            fd,
            EpollSet {
                registrations: HashMap::new(),
                wake_handle: handle as usize,
            },
        );
    }
    Ok(fd)
}

/// `epoll_create`, whose size argument Linux ignores.
pub fn epoll_create(size: i32) -> Result<i32, i32> {
    if size <= 0 {
        return Err(EINVAL);
    }
    epoll_create1(0)
}

/// Releases an epoll set's registrations. Called when its descriptor closes.
pub fn forget_set(fd: i32, raw: usize) {
    if let Ok(mut sets) = lock_sets(4) {
        // The old native event stays owned until close returns. A reused fd
        // with a different event is a different set, never ours to tear down.
        if !sets.get(&fd).is_some_and(|set| set.wake_handle == raw) {
            return;
        }
        if let Some(set) = sets.remove(&fd) {
            // SAFETY: the descriptor still owns this event until close returns.
            unsafe { SetEvent(set.wake_handle as HANDLE) };
        }
    }
}

/// Rebinds or removes registrations after one descriptor is closed.
///
/// Linux keeps an epoll registration while any duplicate still names the open
/// file description, and removes it after the final descriptor closes.  The
/// table lookup and any Winsock provider query happen before taking the epoll
/// lock, preserving the table-before-epoll lock order used by `epoll_ctl`.
pub fn descriptor_closed(description_id: u64, survivor: Option<(i32, crate::FdEntry)>) {
    let replacement = survivor.and_then(|(fd, entry)| {
        let handle = if entry.kind == FdKind::Socket {
            base_handle(entry.raw as SOCKET).ok()?
        } else {
            entry.raw
        };
        Some((fd, handle))
    });
    let Ok(mut sets) = lock_sets(11) else {
        return;
    };
    if epoll_trace_enabled() {
        eprintln!(
            "kinakaze epoll: descriptor-close description={description_id} survivor={:?}",
            survivor.map(|(fd, entry)| (fd, entry.raw))
        );
    }
    for set in sets.values_mut() {
        let mut changed = false;
        if let Some((poll_fd, base_handle)) = replacement {
            for (key, registration) in &mut set.registrations {
                if key.description_id == description_id {
                    registration.poll_fd = poll_fd;
                    registration.base_handle = base_handle;
                    changed = true;
                }
            }
        } else {
            let before = set.registrations.len();
            set.registrations
                .retain(|key, _| key.description_id != description_id);
            changed = set.registrations.len() != before;
        }
        if changed {
            // SAFETY: every live epoll set owns this manual-reset event.
            unsafe { SetEvent(set.wake_handle as HANDLE) };
        }
    }
}

/// Refreshes registrations after one descriptor keeps its Linux identity but
/// starts using a different host handle.
///
/// The replacement is an implementation detail, not a Linux close/open pair,
/// so the interest and user data remain registered. Its readiness state is a
/// new snapshot, however: retaining an `EPOLLET` report from the old host object
/// can suppress the first edge produced by the replacement.
pub fn descriptor_rebound(fd: i32, entry: crate::FdEntry) {
    let handle = if entry.kind == FdKind::Socket {
        let Ok(handle) = base_handle(entry.raw as SOCKET) else {
            return;
        };
        handle
    } else {
        entry.raw
    };
    let Ok(mut sets) = lock_sets(12) else {
        return;
    };
    for set in sets.values_mut() {
        let mut changed = false;
        for (key, registration) in &mut set.registrations {
            if key.description_id == entry.description_id {
                registration.poll_fd = fd;
                registration.base_handle = handle;
                registration.reported = 0;
                changed = true;
            }
        }
        if changed {
            // SAFETY: every live epoll set owns this manual-reset event. A wait
            // using the old host handle must rebuild its readiness snapshot.
            unsafe { SetEvent(set.wake_handle as HANDLE) };
        }
    }
}

/// Clears readiness history which an I/O operation proved it consumed.
///
/// AFD reports socket edges directly, but emulated Unix sockets are sampled.
/// When a reader drains a named pipe and the peer writes again before the next
/// sample, polling never observes the empty interval. Clearing only the bits the
/// successful operation consumed preserves edge-triggered liveness without
/// changing the registration or rearming `EPOLLONESHOT`.
pub fn readiness_consumed(description_id: u64, events: u32) {
    if description_id == 0 || events == 0 {
        return;
    }
    let Ok(mut sets) = lock_sets(13) else {
        return;
    };
    for set in sets.values_mut() {
        let mut changed = false;
        for (key, registration) in &mut set.registrations {
            if key.description_id == description_id && registration.interest & events != 0 {
                registration.readiness_revision = registration.readiness_revision.wrapping_add(1);
                registration.reported &= !events;
                changed = true;
            }
        }
        if changed {
            // SAFETY: every live epoll set owns this manual-reset event. A peer
            // may already have produced the next edge, so re-sample promptly.
            unsafe { SetEvent(set.wake_handle as HANDLE) };
        }
    }
}

/// `epoll_ctl`.
pub fn epoll_ctl(
    epoll_fd: i32,
    operation: i32,
    fd: i32,
    event: Option<EpollEvent>,
) -> Result<(), i32> {
    if epoll_fd < 0 {
        return Err(EBADF);
    }
    control_set(epoll_fd, operation, fd, event)
}

fn control_set(
    epoll_fd: i32,
    operation: i32,
    fd: i32,
    event: Option<EpollEvent>,
) -> Result<(), i32> {
    // A set cannot watch itself; Linux reports EINVAL for that.
    if epoll_fd == fd {
        return Err(EINVAL);
    }
    let entry = crate::get(fd)?;
    let key = RegistrationKey {
        fd,
        description_id: entry.description_id,
    };
    if entry.kind == FdKind::Pipe && !entry.flags.has_pipe_access() {
        return Err(EINVAL);
    }
    if operation != EPOLL_CTL_DEL && !supports_epoll(entry.kind) {
        return Err(EPERM);
    }
    // Resolve every property that may enter Winsock, another subsystem, or the
    // diagnostic path before taking the registration-table lock. In
    // particular, SIO_BASE_HANDLE is a synchronous provider call and must not
    // serialize unrelated epoll_ctl/epoll_wait operations process-wide.
    let addition = if operation == EPOLL_CTL_ADD {
        let event = event.ok_or(EINVAL)?;
        let interest = event.events;
        let data = event.data;
        let is_socket = entry.kind == FdKind::Socket;
        let is_eventfd = matches!(
            entry.kind,
            FdKind::EventFd | FdKind::TimerFd | FdKind::MessageQueue | FdKind::SysfsFile
        );
        if is_eventfd && crate::eventfd_trace_enabled() {
            eprintln!(
                "kinakaze eventfd: epoll add epfd={epoll_fd} fd={fd} interest={:#x}",
                interest
            );
        }
        let base_handle = if is_socket {
            base_handle(entry.raw as SOCKET)?
        } else {
            entry.raw
        };
        Some(Registration {
            readiness_revision: 0,
            eventfd_edges: [0; 2],
            base_handle,
            poll_fd: fd,
            interest,
            data,
            disarmed: false,
            reported: 0,
            is_socket,
            is_packet: entry.flags.contains(FdFlags::PACKET_SOCKET),
            packet_output_blocked: false,
            is_unix: entry.kind == FdKind::UnixSocket,
            is_eventfd,
            is_netlink: entry.kind == FdKind::NetlinkSocket,
            is_inotify: entry.kind == FdKind::Inotify,
            is_pipe: entry.kind == FdKind::Pipe,
            is_fifo: entry.kind == FdKind::Fifo,
            pipe_readable: entry.flags.contains(FdFlags::PIPE_READ_END),
            pipe_writable: entry.flags.contains(FdFlags::PIPE_WRITE_END),
            pipe_overlapped: entry.flags.contains(FdFlags::OVERLAPPED),
            is_pty: matches!(entry.kind, FdKind::PtyMaster | FdKind::PtySlave),
        })
    } else {
        None
    };
    let mut sets = lock_sets(5).map_err(|_| EIO)?;
    let set = sets.get_mut(&epoll_fd).ok_or(EBADF)?;

    if epoll_trace_enabled() {
        let requested = event.map(|value| (value.events, value.data));
        let existing = set.registrations.get(&key).map(|registration| {
            (
                registration.interest,
                registration.reported,
                registration.disarmed,
                registration.poll_fd,
            )
        });
        eprintln!(
            "kinakaze epoll: ctl epfd={epoll_fd} op={operation} fd={fd} description={} kind={:?} request={requested:?} existing={existing:?}",
            key.description_id, entry.kind
        );
    }

    let result = match operation {
        EPOLL_CTL_ADD => {
            if set.registrations.contains_key(&key) {
                return Err(EEXIST);
            }
            set.registrations.insert(
                key,
                addition.expect("EPOLL_CTL_ADD prepared its registration"),
            );
            Ok(())
        }
        EPOLL_CTL_MOD => {
            let event = event.ok_or(EINVAL)?;
            let registration = set.registrations.get_mut(&key).ok_or(ENOENT)?;
            registration.interest = event.events;
            registration.data = event.data;
            // A modification rearms a one-shot registration and clears the
            // edge-triggered history, matching Linux.
            registration.disarmed = false;
            registration.reported = 0;
            Ok(())
        }
        EPOLL_CTL_DEL => set.registrations.remove(&key).map(|_| ()).ok_or(ENOENT),
        _ => Err(EINVAL),
    };
    if result.is_ok() {
        // SAFETY: the epoll set owns a live manual-reset event. A blocked wait
        // must rebuild its AFD snapshot after every interest-list mutation.
        unsafe { SetEvent(set.wake_handle as HANDLE) };
    }
    result
}

enum AfdPollResult {
    Ready(u32),
    InterestChanged,
}

/// Polls one batch of sockets through AFD.
///
/// Returns the number of handles whose `events` field was filled in. The device
/// handle used is the first socket's own base handle, which is already an open
/// handle on `\Device\Afd`.
fn afd_poll_batch(
    info: &mut AfdPollInfo,
    timeout_ms: Option<u32>,
    event: HANDLE,
    interest_changed: HANDLE,
    other_ready: HANDLE,
) -> Result<AfdPollResult, i32> {
    afd_poll_batch_policy(info, timeout_ms, event, interest_changed, other_ready, true)
}
fn afd_poll_batch_policy(
    info: &mut AfdPollInfo,
    timeout_ms: Option<u32>,
    event: HANDLE,
    interest_changed: HANDLE,
    other_ready: HANDLE,
    interruptible: bool,
) -> Result<AfdPollResult, i32> {
    #[cfg(test)]
    if let Some(result) = socket_edge_tests::AFD_RESULT.take() {
        return result;
    }
    if info.handle_count == 0 {
        return Ok(AfdPollResult::Ready(0));
    }
    // The event is manual-reset and reused for every batch in this epoll_wait.
    // A prior completed AFD request must never make a new pending request look
    // complete: the driver still owns `info` until it actually signals.
    // SAFETY: the caller owns the live event for the whole epoll_wait call.
    unsafe { ResetEvent(event) };
    // A negative timeout is relative, in 100ns units. `None` means wait forever,
    // which AFD spells as the maximum negative value.
    info.timeout = match timeout_ms {
        Some(milliseconds) => -((milliseconds as i64) * 10_000),
        None => i64::MIN,
    };
    // Level-triggered semantics: report the current state, not just changes.
    info.exclusive = 0;

    let device = info.handles[0].handle as HANDLE;
    let mut status_block = IoStatusBlock::default();
    let size = size_of::<i64>()
        + size_of::<u32>() * 2
        + size_of::<AfdPollHandleInfo>() * info.handle_count as usize;

    // SAFETY: `info` is a correctly shaped AFD_POLL_INFO of `size` bytes used as
    // both input and output, and `event` is signalled on completion.
    let status = unsafe {
        NtDeviceIoControlFile(
            device,
            event,
            std::ptr::null(),
            std::ptr::null(),
            &raw mut status_block,
            IOCTL_AFD_POLL,
            (&raw const *info).cast(),
            size as u32,
            (&raw mut *info).cast(),
            size as u32,
        )
    };

    if status == STATUS_SUCCESS {
        return Ok(AfdPollResult::Ready(info.handle_count));
    }
    if status != STATUS_PENDING {
        // A close can remove an interest after the snapshot but before AFD sees
        // it. The control event makes that ordinary Linux close race a request
        // to rebuild, not an EIO from epoll_wait.
        // SAFETY: the epoll set owns this event while the wait is in progress.
        if unsafe { WaitForSingleObject(interest_changed, 0) } == WAIT_OBJECT_0 {
            return Ok(AfdPollResult::InterestChanged);
        }
        if stale_afd_status(status) {
            return Ok(AfdPollResult::InterestChanged);
        }
        report_afd_failure("submission", status);
        return Err(EIO);
    }

    // The request is pending, so wait on the event alongside the thread's
    // interrupt event to keep epoll_wait interruptible.
    let interrupt = if interruptible {
        interrupt::current()
    } else {
        std::ptr::null_mut()
    };
    let timeout = timeout_ms.unwrap_or(u32::MAX);
    let mut handles = vec![event];
    let interest_index = if interest_changed.is_null() {
        None
    } else {
        handles.push(interest_changed);
        Some(handles.len() as u32 - 1)
    };
    let interrupt_index = if interrupt.is_null() {
        None
    } else {
        handles.push(interrupt);
        Some(handles.len() as u32 - 1)
    };
    let other_index = if other_ready.is_null() {
        None
    } else {
        handles.push(other_ready);
        Some(handles.len() as u32 - 1)
    };
    let waited = if interrupt.is_null() {
        // SAFETY: both events are live for the duration of the wait.
        unsafe { crate::deadline_wait::any(&handles, timeout) }
    } else {
        signal::register_waiter();
        // SAFETY: all three handles are live for the duration of the wait.
        let result = unsafe { crate::deadline_wait::any(&handles, timeout) };
        signal::unregister_waiter();
        result
    };

    if waited == WAIT_OBJECT_0 {
        if status_block.status >= 0 {
            return Ok(AfdPollResult::Ready(info.handle_count));
        }
        // If an interest mutation raced the I/O completion, discard this old
        // snapshot. Otherwise surface a real AFD failure for diagnosis.
        // SAFETY: the epoll set owns this event while the wait is in progress.
        if unsafe { WaitForSingleObject(interest_changed, 0) } == WAIT_OBJECT_0 {
            return Ok(AfdPollResult::InterestChanged);
        }
        if stale_afd_status(status_block.status) {
            return Ok(AfdPollResult::InterestChanged);
        }
        report_afd_failure("completion", status_block.status);
        return Err(EIO);
    }

    // Either the timeout expired or a signal arrived. The request is still
    // pending against `info`, which lives in the caller's stack frame, so it has
    // to be retired before returning.
    let mut cancel_status = IoStatusBlock::default();
    // SAFETY: the request was issued against this handle and is still pending.
    unsafe { NtCancelIoFileEx(device, &raw const status_block, &raw mut cancel_status) };
    // Deliberately unbounded. A bounded wait that expires would return while the
    // driver can still write the poll result into `info`, corrupting a stack
    // frame that no longer belongs to this request. The kernel always signals the
    // event once the request completes or is cancelled, and a cancel that reports
    // failure means it had already completed and signalled, so this cannot hang.
    // SAFETY: the event is live and manual-reset.
    unsafe { WaitForMultipleObjects(1, &event, 0, INFINITE) };
    // SAFETY: the event belongs to this call.
    unsafe { ResetEvent(event) };

    if waited == WAIT_TIMEOUT {
        return Ok(AfdPollResult::Ready(0));
    }
    if interest_index.is_some_and(|index| waited == WAIT_OBJECT_0 + index) {
        return Ok(AfdPollResult::InterestChanged);
    }
    if other_index.is_some_and(|index| waited == WAIT_OBJECT_0 + index) {
        return Ok(AfdPollResult::InterestChanged);
    }
    if interrupt_index.is_some_and(|index| waited == WAIT_OBJECT_0 + index) {
        // The outer boundary delivers only after all sources and callback waits
        // have retired. A handler is allowed to close descriptors or longjmp.
        return Err(EINTR);
    }
    Err(EIO)
}

fn report_afd_failure(phase: &str, status: i32) {
    use std::io::Write;
    // A service may close or redirect its inherited native stderr. Diagnostics
    // must not turn an errno return into a panic across the guest syscall ABI.
    let _ = writeln!(
        std::io::stderr().lock(),
        "kinakaze epoll: AFD poll {phase} failed with NTSTATUS {status:#x}"
    );
}

/// `epoll_wait` / `epoll_pwait`.
///
/// `timeout_ms` of `-1` waits indefinitely, `0` polls without blocking.
pub fn epoll_wait(epoll_fd: i32, events: &mut [EpollEvent], timeout_ms: i32) -> Result<usize, i32> {
    if epoll_fd < 0 {
        return Err(EBADF);
    }
    wait_set(epoll_fd, events, timeout_ms, true, true)
}

/// Observe ready registrations without disarming one-shots or consuming edges.
/// poll/select use this when another event loop exposes its epoll descriptor.
pub fn poll_readable(epoll_fd: i32) -> Result<bool, i32> {
    let mut event = [EpollEvent { events: 0, data: 0 }];
    wait_set(epoll_fd, &mut event, 0, false, false).map(|count| count != 0)
}

fn wait_set(
    epoll_fd: i32,
    events: &mut [EpollEvent],
    timeout_ms: i32,
    deliver_signals: bool,
    consume: bool,
) -> Result<usize, i32> {
    enable_pipe_state_trace();
    if events.is_empty() {
        return Err(EINVAL);
    }
    let deadline = (timeout_ms > 0)
        .then(|| std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64));

    // SAFETY: null security descriptor and name request an unnamed event.
    let poll_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if poll_event.is_null() {
        return Err(EIO);
    }
    let mut interrupted = false;
    let result = epoll_wait_inner(
        epoll_fd,
        events,
        timeout_ms,
        deadline,
        poll_event,
        &mut interrupted,
        consume,
    );
    // SAFETY: this function created and owns the event.
    unsafe { CloseHandle(poll_event) };
    if deliver_signals && (interrupted || result == Err(EINTR)) {
        // Linux multiplexing waits are never restarted by SA_RESTART. No
        // operation-local handles, pins, or callback contexts survive here.
        signal::deliver_pending();
    }
    result
}

/// A descriptor can close or be reused after the interest snapshot. Readiness
/// belongs to its open-file description; a stale snapshot must neither fail the
/// epoll fd nor report readiness from an unrelated replacement descriptor.
fn query_registration<T>(
    fd: i32,
    description: u64,
    query: impl FnOnce(crate::FdEntry) -> Result<T, i32>,
) -> Result<Option<T>, i32> {
    let entry = match crate::get(fd) {
        Ok(entry) if entry.description_id == description => entry,
        Ok(_) | Err(EBADF) => return Ok(None),
        Err(error) => return Err(error),
    };
    let result = query(entry);
    match crate::get(fd) {
        Ok(current) if current.description_id == description => result.map(Some),
        Ok(_) | Err(EBADF) => Ok(None),
        Err(error) => Err(error),
    }
}

/// The body of [`epoll_wait`], separated so the event is always closed.
fn epoll_wait_inner(
    epoll_fd: i32,
    events: &mut [EpollEvent],
    timeout_ms: i32,
    deadline: Option<std::time::Instant>,
    poll_event: HANDLE,
    interrupted: &mut bool,
    consume: bool,
) -> Result<usize, i32> {
    'wait: loop {
        // Snapshot the registrations so the set is not locked across the wait.
        let (
            mut socket_entries,
            packet_pending,
            ready_now,
            unix_pending,
            eventfd_pending,
            netlink_pending,
            inotify_pending,
            pipe_pending,
            fifo_pending,
            pty_pending,
            wake_source,
        ) = {
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                eprintln!(
                    "kinakaze pipe epoll snapshot: before-lock owner={}",
                    SETS_LOCK_OWNER.load(Ordering::Acquire)
                );
            }
            let sets = lock_sets(6).map_err(|_| EIO)?;
            let set = sets.get(&epoll_fd).ok_or(EBADF)?;
            // The snapshot below includes every mutation which happened before
            // this lock was acquired. Reset while holding the same lock so any
            // later ctl/close reliably wakes the wait built from this snapshot.
            // SAFETY: the epoll set owns this live manual-reset event.
            if consume {
                unsafe { ResetEvent(set.wake_handle as HANDLE) };
            }
            let mut sockets = Vec::new();
            let mut packets = Vec::new();
            let mut immediate = Vec::new();
            let mut unix_pending = Vec::new();
            let mut eventfd_pending = Vec::new();
            let mut netlink_pending = Vec::new();
            let mut inotify_pending = Vec::new();
            let mut pipe_pending = Vec::new();
            let mut fifo_pending = Vec::new();
            let mut pty_pending = Vec::new();
            for (key, registration) in &set.registrations {
                if registration.disarmed {
                    continue;
                }
                if registration.is_packet {
                    packets.push((*key, *registration));
                } else if registration.is_fifo {
                    fifo_pending.push((*key, *registration));
                } else if registration.is_unix {
                    // Only collected here. Polling a Unix socket needs the
                    // descriptor table and the Unix side table, and this runs
                    // under the epoll set lock: `epoll_ctl` acquires those in the
                    // opposite order, so querying here would invert the lock
                    // order and can deadlock.
                    unix_pending.push((*key, *registration));
                } else if registration.is_eventfd {
                    // eventfd readiness depends on its counter. Querying its
                    // side table under the epoll-set lock would invert the lock
                    // order used by close/epoll_ctl, so collect it for polling
                    // after this snapshot just like AF_UNIX.
                    eventfd_pending.push((*key, *registration));
                } else if registration.is_netlink {
                    // Netlink's receive queue is maintained by its side table,
                    // so preserve the same no-lock-inversion rule as eventfd.
                    netlink_pending.push((*key, *registration));
                } else if registration.is_inotify {
                    inotify_pending.push((*key, *registration));
                } else if registration.is_pty {
                    // A terminal's readiness lives behind its own lock, and a
                    // half-typed canonical line is not readable. Same lock-order
                    // discipline as the two above.
                    pty_pending.push((*key, *registration));
                } else if registration.is_pipe {
                    // AFD cannot wait on pipes. Snapshot them here and perform
                    // only non-consuming/non-blocking endpoint probes after the
                    // set lock has been released.
                    pipe_pending.push((*key, *registration));
                } else if registration.is_socket {
                    if pending_afd_events(registration) != 0 {
                        sockets.push((*key, *registration));
                    }
                } else {
                    // AFD only understands sockets. A regular file is always
                    // ready in Linux too, so reporting the requested read and
                    // write bits matches; a pipe is reported ready and left for
                    // the following read to block, which is a real limitation
                    // recorded in the crate documentation.
                    immediate.push((*key, *registration));
                }
            }
            (
                sockets,
                packets,
                immediate,
                unix_pending,
                eventfd_pending,
                netlink_pending,
                inotify_pending,
                pipe_pending,
                fifo_pending,
                pty_pending,
                // close removes the set under this same guard before releasing
                // its handle; pin while that native lifetime is still stable.
                unsafe { native_wait::Source::duplicate(set.wake_handle as HANDLE)? },
            )
        };
        let wake_handle = wake_source.raw();

        let polled_watched = unix_pending.len()
            + eventfd_pending.len()
            + inotify_pending.len()
            + pipe_pending.len()
            + pty_pending.len();
        if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
            let list = |entries: &[(RegistrationKey, Registration)]| {
                entries
                    .iter()
                    .map(|(_, registration)| registration.poll_fd.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            eprintln!(
                "kinakaze pipe epoll categories: unix=[{}] eventfd=[{}] inotify=[{}] pipe=[{}] pty=[{}] sockets={} immediate={}",
                list(&unix_pending),
                list(&eventfd_pending),
                list(&inotify_pending),
                list(&pipe_pending),
                list(&pty_pending),
                socket_entries.len(),
                ready_now.len()
            );
        }
        // Polled with no lock held, for the ordering reason noted above.
        let mut unix_ready = Vec::new();
        let mut packet_waits = Vec::new();
        let mut packet_deadline: Option<i64> = None;
        for (key, registration) in &packet_pending {
            let Some(wait) = query_registration(registration.poll_fd, key.description_id, |_| {
                crate::usernet::packet::prepare(registration.poll_fd)
            })?
            else {
                continue;
            };
            let mut reported = wait.ready & (registration.interest | EPOLLERR | EPOLLHUP);
            if wait.ready & EPOLLIN != 0 {
                reported |= registration.interest & EPOLLRDNORM;
            }
            if wait.ready & EPOLLOUT != 0 {
                reported |= registration.interest & EPOLLWRNORM;
            }
            clear_absent_readiness(epoll_fd, *key, reported);
            if reported != 0 {
                unix_ready.push((*key, *registration, reported));
            }
            packet_deadline = match (packet_deadline, wait.deadline) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let mut transport = *registration;
            transport.base_handle = base_handle(wait.socket() as SOCKET)?;
            transport.packet_output_blocked = wait.output_blocked;
            socket_entries.push((*key, transport));
            packet_waits.push(wait);
        }
        let mut fifo_waits = Vec::new();
        for (key, registration) in &fifo_pending {
            let Ok(entry) = crate::get(registration.poll_fd) else {
                continue;
            };
            if entry.description_id != key.description_id || entry.kind != FdKind::Fifo {
                continue;
            }
            let (ready, wait) = match crate::fifo::prepare_wait(registration.poll_fd, entry) {
                Ok(value) => value,
                Err(EBADF) => continue, // concurrent close/reuse changed this snapshot
                Err(error) => return Err(error),
            };
            let mut reported = 0;
            if ready.readable {
                reported |= (EPOLLIN | EPOLLRDNORM) & registration.interest;
            }
            if ready.writable {
                reported |= (EPOLLOUT | EPOLLWRNORM) & registration.interest;
            }
            if ready.hangup {
                reported |= EPOLLHUP;
            }
            if ready.error {
                reported |= EPOLLERR;
            }
            clear_absent_readiness(epoll_fd, *key, reported);
            if reported != 0 {
                unix_ready.push((*key, *registration, reported));
            }
            // Sampling readiness and enrolling the epoch happen under one
            // inode lock; retaining the registration closes the sleep race.
            fifo_waits.push(wait);
        }
        let mut unix_waits = Vec::new();
        let mut unix_write_waits = Vec::new();
        for (key, registration) in &unix_pending {
            let fd = registration.poll_fd;
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                eprintln!("kinakaze pipe epoll unix: fd={fd} before-readiness");
            }
            let Some(readiness) =
                query_registration(fd, key.description_id, |_| crate::unix::poll_readiness(fd))?
            else {
                continue;
            };
            if timeout_ms != 0
                && registration.interest & (EPOLLIN | EPOLLRDNORM) != 0
                && !readiness.contains(Readiness::READABLE)
                && !readiness.contains(Readiness::HANGUP)
                && !readiness.contains(Readiness::ERROR)
            {
                if let Some(Some(wait)) = query_registration(fd, key.description_id, |_| {
                    crate::unix::readiness::prepare(fd)
                })? {
                    unix_waits.push(wait);
                }
            }
            if timeout_ms != 0
                && registration.interest & (EPOLLOUT | EPOLLWRNORM) != 0
                && !readiness.contains(Readiness::WRITABLE)
                && !readiness.contains(Readiness::HANGUP)
                && !readiness.contains(Readiness::ERROR)
            {
                if let Some(Some(wait)) = query_registration(fd, key.description_id, |_| {
                    crate::unix::readiness::prepare_write(fd)
                })? {
                    unix_write_waits.push(wait);
                }
            }
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                eprintln!("kinakaze pipe epoll unix: fd={fd} after-readiness");
            }
            let mut reported = 0;
            if readiness.contains(Readiness::READABLE) {
                reported |= (EPOLLIN | EPOLLRDNORM) & registration.interest;
            }
            if readiness.contains(Readiness::WRITABLE) {
                reported |= (EPOLLOUT | EPOLLWRNORM) & registration.interest;
            }
            // Hangup and error are delivered whether or not they were requested,
            // exactly as Linux does.
            if readiness.contains(Readiness::HANGUP) {
                reported |= EPOLLHUP;
            }
            if readiness.contains(Readiness::ERROR) {
                reported |= EPOLLERR;
            }
            clear_absent_readiness(epoll_fd, *key, reported);
            if reported != 0 {
                unix_ready.push((*key, *registration, reported));
            }
        }

        let mut eventfd_ready = Vec::new();
        let mut eventfd_waits = Vec::new();
        let mut native_eventfds = 0;
        for (key, registration) in &eventfd_pending {
            let mut sampled = *registration;
            let mut sampled_edges = registration.eventfd_edges;
            let fd = registration.poll_fd;
            let previous_waits = eventfd_waits.len();
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                eprintln!("kinakaze pipe epoll eventfd: fd={fd} before-readiness");
            }
            let Some(reported) = query_registration(fd, key.description_id, |entry| {
                // poll and level-triggered epoll sleep on the shared counter's
                // native levels. An already-delivered ET level must not become
                // a permanently signaled wait source; retain its edge path.
                if timeout_ms != 0
                    && entry.kind == FdKind::EventFd
                    && registration.interest & EPOLLET == 0
                    && registration.interest & (EPOLLIN | EPOLLRDNORM | EPOLLOUT | EPOLLWRNORM) != 0
                {
                    eventfd_waits.push(crate::eventfd::prepare_wait(
                        fd,
                        registration.interest & (EPOLLIN | EPOLLRDNORM) != 0,
                        registration.interest & (EPOLLOUT | EPOLLWRNORM) != 0,
                    )?);
                    native_eventfds += 1;
                }
                if entry.kind == FdKind::SysfsFile {
                    return Ok(
                        crate::sysfs::poll(fd)? & (registration.interest | EPOLLERR | EPOLLHUP)
                    );
                }
                let (readable, writable, overflow) = match entry.kind {
                    FdKind::TimerFd => {
                        let (r, w) = crate::timerfd::poll(fd)?;
                        (r, w, false)
                    }
                    FdKind::MessageQueue => {
                        let (r, w) = crate::mqueue::poll(fd)?;
                        (r, w, false)
                    }
                    _ => {
                        let (r, w, overflow, edges) = crate::eventfd::poll_status_with_edges(fd)?;
                        if edges[0] != registration.eventfd_edges[0] {
                            sampled.reported &= !(EPOLLIN | EPOLLRDNORM | EPOLLERR);
                        }
                        if edges[1] != registration.eventfd_edges[1] {
                            sampled.reported &= !(EPOLLOUT | EPOLLWRNORM);
                        }
                        sampled_edges = edges;
                        (r, w, overflow)
                    }
                };
                Ok((if overflow { EPOLLERR } else { 0 })
                    | (if readable {
                        (EPOLLIN | EPOLLRDNORM) & registration.interest
                    } else {
                        0
                    })
                    | (if writable {
                        (EPOLLOUT | EPOLLWRNORM) & registration.interest
                    } else {
                        0
                    }))
            })?
            else {
                // Reuse after the first descriptor check invalidates both the
                // readiness result and any native source prepared by it.
                native_eventfds -= eventfd_waits.len() - previous_waits;
                eventfd_waits.truncate(previous_waits);
                continue;
            };
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                eprintln!("kinakaze pipe epoll eventfd: fd={fd} after-readiness");
            }
            if reported != 0 && crate::eventfd_trace_enabled() {
                eprintln!(
                    "kinakaze eventfd: epoll ready epfd={epoll_fd} fd={fd} events={reported:#x} previous={:#x}",
                    registration.reported
                );
            }
            clear_absent_readiness(epoll_fd, *key, reported);
            if reported != 0 {
                eventfd_ready.push((*key, sampled, reported, sampled_edges));
            }
        }

        let polled_watched = polled_watched - native_eventfds;
        let mut netlink_ready = Vec::new();
        let mut netlink_waits = Vec::new();
        for (key, registration) in &netlink_pending {
            let Some((readable, sources)) =
                query_registration(registration.poll_fd, key.description_id, |_| {
                    crate::netlink::multicast::prepare_wait(registration.poll_fd)
                })?
            else {
                continue;
            };
            netlink_waits.extend(sources);
            let mut reported = 0;
            if readable {
                reported |= (EPOLLIN | EPOLLRDNORM) & registration.interest;
            }
            reported |= (EPOLLOUT | EPOLLWRNORM) & registration.interest;
            clear_absent_readiness(epoll_fd, *key, reported);
            if reported != 0 {
                netlink_ready.push((*key, *registration, reported));
            }
        }

        let mut inotify_ready = Vec::new();
        for (key, registration) in &inotify_pending {
            let Some(readable) =
                query_registration(registration.poll_fd, key.description_id, |_| {
                    crate::inotify::poll_inotify(registration.poll_fd)
                })?
            else {
                continue;
            };
            let reported = if readable {
                (EPOLLIN | EPOLLRDNORM) & registration.interest
            } else {
                0
            };
            clear_absent_readiness(epoll_fd, *key, reported);
            if reported != 0 {
                inotify_ready.push((*key, *registration, reported));
            }
        }

        let mut pipe_ready = Vec::new();
        for (key, registration) in &pipe_pending {
            let fd = registration.poll_fd;
            let reported = pipe_events(
                registration.base_handle,
                registration.interest,
                registration.pipe_readable,
                registration.pipe_writable,
                registration.pipe_overlapped,
            );
            if epoll_trace_enabled()
                && (reported != registration.reported || reported & (EPOLLERR | EPOLLHUP) != 0)
            {
                eprintln!(
                    "kinakaze epoll: pipe-state epfd={epoll_fd} fd={} description={} handle={:#x} interest={:#x} previous={:#x} current={reported:#x}",
                    registration.poll_fd,
                    key.description_id,
                    registration.base_handle,
                    registration.interest,
                    registration.reported
                );
            }
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire)
                && PIPE_STATE_TRACE_LINES.fetch_add(1, Ordering::Relaxed) < 64
            {
                eprintln!(
                    "kinakaze pipe epoll registration: epfd={epoll_fd} fd={fd} previous={:#x} current={reported:#x}",
                    registration.reported
                );
            }
            clear_absent_readiness(epoll_fd, *key, reported);
            if reported != 0 {
                pipe_ready.push((*key, *registration, reported));
            }
        }

        let mut pty_ready = Vec::new();
        if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
            let fds = pty_pending
                .iter()
                .map(|(_, registration)| registration.poll_fd.to_string())
                .collect::<Vec<_>>()
                .join(",");
            eprintln!(
                "kinakaze pipe epoll pty: count={} fds=[{fds}]",
                pty_pending.len()
            );
        }
        for (key, registration) in &pty_pending {
            let fd = registration.poll_fd;
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                eprintln!("kinakaze pipe epoll pty: fd={fd} before-readiness");
            }
            let readiness = crate::tty::readiness(fd).unwrap_or_default();
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                eprintln!("kinakaze pipe epoll pty: fd={fd} after-readiness");
            }
            let mut reported = 0;
            if readiness.readable {
                reported |= (EPOLLIN | EPOLLRDNORM) & registration.interest;
            }
            if readiness.writable {
                reported |= (EPOLLOUT | EPOLLWRNORM) & registration.interest;
            }
            // A hangup is delivered whether or not it was requested, exactly as
            // Linux does: it is how a terminal emulator learns its child exited.
            if readiness.hangup {
                reported |= EPOLLHUP;
            }
            clear_absent_readiness(epoll_fd, *key, reported);
            if reported != 0 {
                pty_ready.push((*key, *registration, reported));
            }
        }

        let mut filled = 0usize;

        for (key, registration, reported) in &unix_ready {
            if filled >= events.len() {
                break;
            }
            // Edge-triggered registrations suppress unchanged readiness.
            if registration.interest & EPOLLET != 0 && reported & !registration.reported == 0 {
                continue;
            }
            events[filled] = EpollEvent {
                events: *reported,
                data: registration.data,
            };
            filled += 1;
            if consume {
                finish_delivery(epoll_fd, *key, *reported, registration.readiness_revision);
            }
        }

        for (key, registration, reported, edges) in &eventfd_ready {
            if filled >= events.len() {
                break;
            }
            if registration.interest & EPOLLET != 0 && reported & !registration.reported == 0 {
                continue;
            }
            events[filled] = EpollEvent {
                events: *reported,
                data: registration.data,
            };
            filled += 1;
            if consume {
                finish_delivery_with_edges(
                    epoll_fd,
                    *key,
                    *reported,
                    registration.readiness_revision,
                    Some(*edges),
                );
            }
        }

        for (key, registration, reported) in &netlink_ready {
            if filled >= events.len() {
                break;
            }
            if registration.interest & EPOLLET != 0 && reported & !registration.reported == 0 {
                continue;
            }
            events[filled] = EpollEvent {
                events: *reported,
                data: registration.data,
            };
            filled += 1;
            if consume {
                finish_delivery(epoll_fd, *key, *reported, registration.readiness_revision);
            }
        }

        for (key, registration, reported) in &inotify_ready {
            if filled >= events.len() {
                break;
            }
            if registration.interest & EPOLLET != 0 && reported & !registration.reported == 0 {
                continue;
            }
            events[filled] = EpollEvent {
                events: *reported,
                data: registration.data,
            };
            filled += 1;
            if consume {
                finish_delivery(epoll_fd, *key, *reported, registration.readiness_revision);
            }
        }

        for (key, registration, reported) in &pipe_ready {
            if filled >= events.len() {
                break;
            }
            if registration.interest & EPOLLET != 0 && reported & !registration.reported == 0 {
                continue;
            }
            events[filled] = EpollEvent {
                events: *reported,
                data: registration.data,
            };
            filled += 1;
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                let fd = registration.poll_fd;
                eprintln!(
                    "kinakaze pipe epoll delivery: epfd={epoll_fd} fd={fd} filled={filled} before-finish"
                );
            }
            if consume {
                finish_delivery(epoll_fd, *key, *reported, registration.readiness_revision);
            }
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                let fd = registration.poll_fd;
                eprintln!(
                    "kinakaze pipe epoll delivery: epfd={epoll_fd} fd={fd} filled={filled} after-finish"
                );
            }
        }

        for (key, registration, reported) in &pty_ready {
            if filled >= events.len() {
                break;
            }
            if registration.interest & EPOLLET != 0 && reported & !registration.reported == 0 {
                continue;
            }
            events[filled] = EpollEvent {
                events: *reported,
                data: registration.data,
            };
            filled += 1;
            if consume {
                finish_delivery(epoll_fd, *key, *reported, registration.readiness_revision);
            }
        }

        // Non-socket descriptors report their requested readiness directly.
        for (key, registration) in &ready_now {
            if filled >= events.len() {
                break;
            }
            let reported = registration.interest & (EPOLLIN | EPOLLOUT | EPOLLRDNORM | EPOLLWRNORM);
            if reported == 0 {
                continue;
            }
            if registration.interest & EPOLLET != 0 && reported & !registration.reported == 0 {
                continue;
            }
            events[filled] = EpollEvent {
                events: reported,
                data: registration.data,
            };
            filled += 1;
            if consume {
                finish_delivery(epoll_fd, *key, reported, registration.readiness_revision);
            }
        }

        if !socket_entries.is_empty() {
            // AFD cannot wait on a shared FIFO. Its epoch and last-token-close
            // notifications join the same cancellable native wait, so mixed
            // socket/FIFO sets do not need a periodic re-poll for FIFO traffic.
            let fifo_fan_in = if filled == 0
                && (!packet_waits.is_empty()
                    || !fifo_waits.is_empty()
                    || !netlink_waits.is_empty()
                    || !eventfd_waits.is_empty()
                    || !unix_waits.is_empty()
                    || !unix_write_waits.is_empty())
            {
                let mut group = native_wait::FanIn::new()?;
                for wait in &packet_waits {
                    for event in wait.events() {
                        unsafe {
                            group.add(event)?;
                        }
                    }
                }
                for wait in &fifo_waits {
                    for handle in wait.handles() {
                        unsafe { group.add(handle as HANDLE)? };
                    }
                }
                for source in &netlink_waits {
                    unsafe { group.add(source.raw())? };
                }
                for wait in &eventfd_waits {
                    for &handle in wait.handles() {
                        unsafe { group.add(handle)? };
                    }
                }
                for wait in &unix_waits {
                    unsafe { group.add(wait.raw())? };
                }
                for wait in &unix_write_waits {
                    unsafe { group.add(wait.raw())? };
                }
                Some(group)
            } else {
                None
            };
            let fifo_event = fifo_fan_in
                .as_ref()
                .map_or(std::ptr::null_mut(), native_wait::FanIn::raw);
            // Poll sockets in batches bounded by the request structure.
            for chunk in socket_entries.chunks(MAX_POLL_HANDLES) {
                if filled >= events.len() {
                    break;
                }
                // SAFETY: AfdPollInfo is a plain data structure the driver fills.
                let mut info: AfdPollInfo = unsafe { std::mem::zeroed() };
                info.handle_count = chunk.len() as u32;
                for (slot, (_, registration)) in info.handles.iter_mut().zip(chunk) {
                    slot.handle = registration.base_handle;
                    slot.events = pending_afd_events(registration);
                    slot.status = 0;
                }

                // Only the first batch may block; later batches poll so a ready
                // socket in an earlier batch is not delayed behind a quiet one.
                let mut batch_timeout = if filled == 0 {
                    let remaining = remaining_timeout(timeout_ms, deadline);
                    // A watched Unix socket has to be re-polled, so the AFD wait
                    // cannot be allowed to sleep past the poll interval or a
                    // pipe becoming readable would go unnoticed until a socket
                    // happened to wake this up.
                    if polled_watched > 0 {
                        Some(
                            remaining
                                .unwrap_or(UNIX_POLL_INTERVAL_MS)
                                .min(UNIX_POLL_INTERVAL_MS),
                        )
                    } else {
                        remaining
                    }
                } else {
                    Some(0)
                };
                if let Some(deadline) = packet_deadline {
                    let remaining = deadline
                        .saturating_sub(crate::usernet::stack::shared::now_ms())
                        .clamp(0, (u32::MAX - 1) as i64) as u32;
                    batch_timeout =
                        Some(batch_timeout.map_or(remaining, |timeout| timeout.min(remaining)));
                }
                let reported = match afd_poll_batch(
                    &mut info,
                    batch_timeout,
                    poll_event,
                    if consume {
                        wake_handle
                    } else {
                        core::ptr::null_mut()
                    },
                    fifo_event,
                ) {
                    Ok(AfdPollResult::Ready(reported)) => reported,
                    Err(EINTR) if filled != 0 => {
                        // The auto-reset interrupt has been consumed. Dispatch
                        // the signal at the outer boundary after every native
                        // request/pin is retired, while preserving the events.
                        *interrupted = true;
                        return Ok(filled);
                    }
                    // Earlier descriptors have already committed edge/one-shot
                    // delivery. A later cancel, close, ctl or signal must not
                    // discard their events while leaving that state consumed.
                    Ok(AfdPollResult::InterestChanged) | Err(_) if filled != 0 => {
                        return Ok(filled);
                    }
                    Ok(AfdPollResult::InterestChanged) => continue 'wait,
                    Err(error) => return Err(error),
                };

                // The driver compacts its output: it returns only the handles that
                // became ready, so entry N of the result is not entry N of the
                // request. Results are therefore matched back to registrations by
                // handle value. Pairing them by position reports a ready socket
                // under some other descriptor's cookie, which was verified by a
                // test that registers three sockets and writes to the last.
                let reported = (reported as usize).min(chunk.len());
                for (key, registration) in chunk {
                    if registration.is_packet {
                        continue;
                    }
                    let is_ready =
                        info.handles.iter().take(reported).any(|slot| {
                            slot.handle == registration.base_handle && slot.events != 0
                        });
                    // Masked edges were not queried. Only socket I/O or MOD
                    // rearms them; treating the omission as "not ready" would
                    // restore the permanently writable poll on the next pass.
                    if !is_ready && registration.interest & EPOLLET == 0 {
                        clear_absent_readiness(epoll_fd, *key, 0);
                    }
                }
                for slot in info.handles.iter().take(reported) {
                    if filled >= events.len() {
                        break;
                    }
                    if slot.events == 0 {
                        continue;
                    }
                    let Some((key, registration)) = chunk
                        .iter()
                        .find(|(_, registration)| registration.base_handle == slot.handle)
                    else {
                        // A handle the driver reported that this batch never
                        // asked about. Skipping is the only safe response.
                        continue;
                    };
                    // A transport packet is only a reason to run the protocol
                    // engine again. It is never itself guest TCP/UDP readiness.
                    if registration.is_packet {
                        continue;
                    }
                    let translated = afd_to_events(slot.events, registration.interest);
                    if translated == 0 {
                        continue;
                    }
                    // Edge-triggered registrations suppress readiness that was
                    // already reported and has not changed since.
                    if registration.interest & EPOLLET != 0
                        && translated & !registration.reported == 0
                    {
                        continue;
                    }
                    events[filled] = EpollEvent {
                        events: translated,
                        data: registration.data,
                    };
                    filled += 1;
                    if consume {
                        finish_delivery(
                            epoll_fd,
                            *key,
                            translated,
                            registration.readiness_revision,
                        );
                    }
                }
            }
        }

        if filled > 0 {
            if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
                eprintln!("kinakaze pipe epoll return: epfd={epoll_fd} filled={filled}");
            }
            return Ok(filled);
        }
        // Nothing ready. A zero timeout returns immediately; an expired deadline
        // does too. Otherwise rebuild the readiness snapshot and keep waiting.
        if timeout_ms == 0 {
            return Ok(0);
        }
        if let Some(deadline) = deadline
            && std::time::Instant::now() >= deadline
        {
            return Ok(0);
        }
        if socket_entries.is_empty() && ready_now.is_empty() {
            // An empty set with an infinite timeout would spin, so wait on the
            // interrupt event alone and let a signal or the deadline end it.
            let mut wait = remaining_timeout(timeout_ms, deadline).unwrap_or(u32::MAX);
            // A terminal publishes readiness on a real event, so it can be
            // waited on rather than polled — which is what makes a keystroke
            // wake `epoll_wait` immediately instead of on the next sweep.
            let mut waitables: Vec<HANDLE> = vec![wake_handle];
            waitables.extend(netlink_waits.iter().map(native_wait::Source::raw));
            waitables.extend(
                eventfd_waits
                    .iter()
                    .flat_map(|wait| wait.handles())
                    .copied(),
            );
            waitables.extend(unix_waits.iter().map(crate::unix::readiness::ReadWait::raw));
            waitables.extend(unix_write_waits.iter().map(crate::fs::object::Object::raw));
            waitables.extend(
                fifo_waits
                    .iter()
                    .flat_map(|wait| wait.handles())
                    .map(|handle| handle as HANDLE),
            );
            let pty_events: Vec<_> = pty_pending
                .iter()
                .filter(|(_, registration)| registration.interest & (EPOLLIN | EPOLLRDNORM) != 0)
                .filter_map(|(key, registration)| {
                    native_wait::Source::for_fd(registration.poll_fd, key.description_id).ok()
                })
                .collect();
            waitables.extend(pty_events.iter().map(native_wait::Source::raw));
            // A set holding only Unix sockets, eventfds or pipes must come back
            // to re-poll them rather than sleeping out the whole timeout.
            let repolled = unix_pending.len()
                + (eventfd_pending.len() - native_eventfds)
                + inotify_pending.len()
                + pipe_pending.len();
            if repolled > 0 || (polled_watched > 0 && waitables.is_empty()) {
                wait = wait.min(UNIX_POLL_INTERVAL_MS);
            }
            let interrupt = interrupt::current();
            if interrupt.is_null() && waitables.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(wait.min(50) as u64));
            } else {
                if !interrupt.is_null() {
                    signal::register_waiter();
                }
                // MAX_POLL_HANDLES sizes AFD batches, not a Windows wait (64).
                // Large event sets use transient native callback fan-in without
                // dropping readiness sources or the interrupt at the end.
                let waited = unsafe { native_wait::wait(&waitables, interrupt, wait) };
                if !interrupt.is_null() {
                    signal::unregister_waiter();
                }
                match waited? {
                    native_wait::Outcome::Ready => continue 'wait,
                    native_wait::Outcome::Interrupted => {
                        return Err(EINTR);
                    }
                    native_wait::Outcome::Timeout => {}
                }
            }
            // A watched descriptor may have become ready during the sleep, so
            // the loop has to run again; only a set with nothing to re-examine
            // can conclude that the timeout is spent.
            if timeout_ms > 0 && polled_watched == 0 {
                return Ok(0);
            }
        }
    }
}

/// Milliseconds left before `deadline`, or `None` for an infinite wait.
fn remaining_timeout(timeout_ms: i32, deadline: Option<std::time::Instant>) -> Option<u32> {
    match deadline {
        Some(deadline) => {
            let now = std::time::Instant::now();
            Some(if now >= deadline {
                0
            } else {
                (deadline - now)
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min(u32::MAX as u128) as u32
            })
        }
        // A negative timeout is Linux's infinite wait.
        None if timeout_ms < 0 => None,
        None => Some(0),
    }
}

/// Records a delivery, disarming one-shot registrations.
fn finish_delivery(epoll_fd: i32, key: RegistrationKey, reported: u32, revision: u64) {
    finish_delivery_with_edges(epoll_fd, key, reported, revision, None);
}

fn finish_delivery_with_edges(
    epoll_fd: i32,
    key: RegistrationKey,
    reported: u32,
    revision: u64,
    edges: Option<[u64; 2]>,
) {
    let Ok(mut sets) = lock_sets(7) else {
        return;
    };
    let Some(set) = sets.get_mut(&epoll_fd) else {
        return;
    };
    let Some(registration) = set.registrations.get_mut(&key) else {
        return;
    };
    if epoll_trace_enabled() {
        eprintln!(
            "kinakaze epoll: deliver epfd={epoll_fd} fd={} description={} interest={:#x} previous={:#x} events={reported:#x}",
            registration.poll_fd, key.description_id, registration.interest, registration.reported
        );
    }
    if registration.readiness_revision == revision {
        registration.reported |= reported;
        if let Some(edges) = edges {
            registration.eventfd_edges = edges;
        }
    }
    if registration.interest & EPOLLONESHOT != 0 {
        // One-shot stays silent until EPOLL_CTL_MOD rearms it.
        registration.disarmed = true;
    }
}

/// Clears edge-trigger history for conditions that are no longer true.
///
/// EPOLLET suppresses a condition only until it becomes false. Keeping a stale
/// `EPOLLIN` bit after an eventfd was drained makes the next zero-to-one counter
/// transition invisible, which is exactly the transition libuv uses to stop its
/// delayed-task thread.
fn clear_absent_readiness(epoll_fd: i32, key: RegistrationKey, current: u32) {
    if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
        let fd = key.fd;
        eprintln!(
            "kinakaze pipe epoll clear: epfd={epoll_fd} fd={fd} current={current:#x} before-lock"
        );
    }
    let Ok(mut sets) = lock_sets(8) else {
        return;
    };
    if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire) {
        let fd = key.fd;
        eprintln!(
            "kinakaze pipe epoll clear: epfd={epoll_fd} fd={fd} current={current:#x} after-lock"
        );
    }
    let Some(registration) = sets
        .get_mut(&epoll_fd)
        .and_then(|set| set.registrations.get_mut(&key))
    else {
        return;
    };
    registration.reported &= current;
}

pub fn notify_read(fd: i32) {
    if let Ok(entry) = crate::get(fd) {
        readiness_consumed(
            entry.description_id,
            EPOLLIN | EPOLLRDNORM | EPOLLPRI | EPOLLRDBAND,
        );
    }
}

pub fn notify_write(fd: i32) {
    if let Ok(entry) = crate::get(fd) {
        readiness_consumed(entry.description_id, EPOLLOUT | EPOLLWRNORM);
    }
}

/// Returns the current Linux epoll readiness of one Windows pipe endpoint.
///
/// Both probes are guaranteed not to wait for pipe data: `PeekNamedPipe` is a
/// non-consuming read-side query and a zero-length `WriteFile` validates the
/// write endpoint without occupying buffer space.  In particular, this never
/// calls `NtQueryInformationFile(FilePipeLocalInformation)`: that query may wait
/// on a synchronous anonymous-pipe handle and used to freeze the whole epoll
/// set depending on hash iteration order.
fn pipe_events(
    handle: usize,
    interest: u32,
    readable: bool,
    writable: bool,
    overlapped: bool,
) -> u32 {
    let mut reported = 0;

    if readable {
        let mut available = 0u32;
        let peeked = unsafe {
            PeekNamedPipe(
                handle as HANDLE,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        let peek_error = (peeked == 0).then(|| unsafe { GetLastError() });
        let peer_closed = matches!(
            peek_error,
            Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED)
        );
        if PIPE_STATE_TRACE_ACTIVE.load(Ordering::Acquire)
            && PIPE_STATE_TRACE_LINES.fetch_add(1, Ordering::Relaxed) < 64
        {
            eprintln!(
                "kinakaze pipe epoll: handle={handle:#x} interest={interest:#x} peeked={peeked} peek_available={available} peek_error={peek_error:?}",
            );
        }
        if available > 0 {
            reported |= (EPOLLIN | EPOLLRDNORM) & interest;
        }
        if peer_closed {
            reported |= (EPOLLIN | EPOLLRDNORM) & interest;
            reported |= EPOLLHUP;
        } else if peek_error.is_some() {
            reported |= EPOLLERR;
        }
    }

    if writable {
        if pipe_writer_is_live(handle as HANDLE, overlapped) {
            reported |= (EPOLLOUT | EPOLLWRNORM) & interest;
        } else {
            reported |= EPOLLERR;
        }
    }
    reported
}

/// Validates the write endpoint with a zero-byte operation.
///
/// Handles created with `FILE_FLAG_OVERLAPPED` require an OVERLAPPED record even
/// for a zero-length write. Passing null made every healthy pipe look broken and
/// caused level-triggered epoll users to spin on EPOLLERR. The operation cannot
/// consume quota or alter the stream; waiting for this operation's own event is
/// therefore independent of reader progress and yields an authoritative peer
/// state rather than a guessed readiness value.
fn pipe_writer_is_live(handle: HANDLE, overlapped: bool) -> bool {
    if !overlapped {
        let mut written = 0u32;
        return unsafe {
            WriteFile(
                handle,
                std::ptr::null(),
                0,
                &mut written,
                std::ptr::null_mut(),
            )
        } != 0;
    }

    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return false;
    }
    let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
    operation.hEvent = event;
    let started = unsafe {
        WriteFile(
            handle,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            &raw mut operation,
        )
    };
    let completed = if started != 0 {
        true
    } else if unsafe { GetLastError() } == ERROR_IO_PENDING {
        let waited = unsafe { WaitForMultipleObjects(1, &event, 0, INFINITE) };
        let mut written = 0u32;
        waited == WAIT_OBJECT_0
            && unsafe { GetOverlappedResult(handle, &raw const operation, &mut written, 0) } != 0
    } else {
        false
    };
    if !completed {
        // Cancellation is harmless if the operation already failed or retired;
        // it also guarantees the stack OVERLAPPED is no longer kernel-owned.
        unsafe { CancelIoEx(handle, &raw mut operation) };
    }
    unsafe { CloseHandle(event) };
    completed
}

/// Shared pipe readiness used by libc's `poll` path.
pub fn poll_pipe(entry: crate::FdEntry, interest: u32) -> Result<u32, i32> {
    if entry.kind != FdKind::Pipe || !entry.flags.has_pipe_access() {
        return Err(EINVAL);
    }
    Ok(pipe_events(
        entry.raw,
        interest,
        entry.flags.contains(FdFlags::PIPE_READ_END),
        entry.flags.contains(FdFlags::PIPE_WRITE_END),
        entry.flags.contains(FdFlags::OVERLAPPED),
    ))
}

#[cfg(all(test, windows))]
mod pipe_tests {
    use super::*;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    #[test]
    fn readiness_snapshot_ignores_close_during_query() {
        let fd = crate::eventfd::create_eventfd(0, 0).unwrap();
        let description = crate::get(fd).unwrap().description_id;
        // Deterministically put close between the snapshot and side-table read.
        let result = query_registration(fd, description, |_| {
            crate::close(fd).unwrap();
            crate::eventfd::poll_eventfd(fd)
        });
        assert_eq!(result, Ok(None));
        assert_eq!(
            query_registration(fd, description, |_| panic!("closed fd queried")),
            Ok(None::<()>)
        );
    }

    #[test]
    fn readiness_snapshot_rejects_reused_descriptor() {
        let fd = crate::install_handleless(FdKind::Null, FdFlags::NONE).unwrap();
        let description = crate::get(fd).unwrap().description_id;
        let result = query_registration(fd, description, |_| {
            // Replace atomically under the real descriptor table lock so other
            // tests cannot claim this integer in the simulated close/reuse gap.
            crate::table()
                .write()
                .unwrap()
                .insert_at(fd, 0, FdKind::Null, FdFlags::NONE);
            Ok(EPOLLIN)
        });
        assert_ne!(crate::get(fd).unwrap().description_id, description);
        assert_eq!(result, Ok(None));
        assert_eq!(
            query_registration(fd, description, |_| panic!("replacement queried")),
            Ok(None::<()>)
        );
        crate::close(fd).unwrap();
    }

    #[test]
    fn readiness_snapshot_preserves_live_description_errors() {
        let fd = crate::eventfd::create_eventfd(1, 0).unwrap();
        let description = crate::get(fd).unwrap().description_id;
        assert_eq!(
            query_registration(fd, description, |_| crate::eventfd::poll_eventfd(fd)),
            Ok(Some((true, true)))
        );
        assert_eq!(
            query_registration::<()>(fd, description, |_| Err(EIO)),
            Err(EIO)
        );
        assert_eq!(
            query_registration::<()>(fd, description, |_| Err(EBADF)),
            Err(EBADF)
        );
        crate::close(fd).unwrap();
    }

    #[test]
    fn afd_snapshot_rebuilds_when_closed_socket_handle_is_reused_by_event() {
        // Reproduce the state after a socket in a copied interest list closes
        // and Windows reuses its numeric handle for a different object type.
        // No interest-change event need remain signalled: another waiter may
        // already have reset that shared notification for its newer snapshot.
        let reused = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        let complete = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        let changed = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        assert!(!reused.is_null() && !complete.is_null() && !changed.is_null());
        let mut info: AfdPollInfo = unsafe { std::mem::zeroed() };
        info.handle_count = 1;
        info.handles[0].handle = reused as usize;
        info.handles[0].events = AFD_POLL_RECEIVE;
        let result = afd_poll_batch(&mut info, Some(0), complete, changed, std::ptr::null_mut());
        for handle in [reused, complete, changed] {
            unsafe { CloseHandle(handle) };
        }
        assert!(matches!(result, Ok(AfdPollResult::InterestChanged)));
    }

    #[test]
    fn afd_snapshot_rebuilds_when_closed_socket_handle_is_reused_by_file() {
        use std::os::windows::io::AsRawHandle;
        let reused = std::fs::File::open("NUL").unwrap();
        let complete = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        let changed = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        assert!(!complete.is_null() && !changed.is_null());
        let mut info: AfdPollInfo = unsafe { std::mem::zeroed() };
        info.handle_count = 1;
        info.handles[0].handle = reused.as_raw_handle() as usize;
        info.handles[0].events = AFD_POLL_RECEIVE;
        // Another waiter may have reset the shared interest-change event.
        let result = afd_poll_batch(&mut info, Some(0), complete, changed, std::ptr::null_mut());
        for handle in [complete, changed] {
            unsafe { CloseHandle(handle) };
        }
        assert!(matches!(result, Ok(AfdPollResult::InterestChanged)));
    }

    #[test]
    fn afd_diagnostic_unwritable_stderr_child() {
        if std::env::var_os("KINAKAZE_EPOLL_STDERR_CHILD").is_none() {
            return;
        }
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, SetStdHandle};
        let readonly = std::fs::File::open("NUL").unwrap();
        let original = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        assert_ne!(
            unsafe { SetStdHandle(STD_ERROR_HANDLE, readonly.as_raw_handle()) },
            0
        );
        report_afd_failure("test", 0xc000_000du32 as i32);
        assert_ne!(unsafe { SetStdHandle(STD_ERROR_HANDLE, original) }, 0);
    }

    #[test]
    fn afd_diagnostic_survives_unwritable_native_stderr() {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "epoll::pipe_tests::afd_diagnostic_unwritable_stderr_child",
                "--nocapture",
            ])
            .env("KINAKAZE_EPOLL_STDERR_CHILD", "1")
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "diagnostic write killed the subprocess: {:?}",
            output.status
        );
    }

    #[test]
    fn pipe_edge_survives_drain_and_refill_between_polls() {
        let (reader, writer) = crate::create_pipe(FdFlags::NONBLOCK, 4096).unwrap();
        let poller = epoll_create1(0).unwrap();
        epoll_ctl(
            poller,
            EPOLL_CTL_ADD,
            reader,
            Some(EpollEvent {
                events: EPOLLIN | EPOLLET,
                data: 37,
            }),
        )
        .unwrap();
        let mut events = [EpollEvent::default(); 1];
        crate::write(writer, b"first").unwrap();
        assert_eq!(epoll_wait(poller, &mut events, 0).unwrap(), 1);
        let mut bytes = [0; 64];
        assert_eq!(crate::read(reader, &mut bytes).unwrap(), 5);
        assert_eq!(crate::read(reader, &mut bytes), Err(crate::EAGAIN));
        // No intervening epoll call observes the empty queue. Closing the
        // writer while bytes remain must still let the reader drain to EOF.
        crate::write(writer, b"last").unwrap();
        crate::close(writer).unwrap();
        assert_eq!(epoll_wait(poller, &mut events, 100).unwrap(), 1);
        assert_eq!(crate::read(reader, &mut bytes).unwrap(), 4);
        assert_eq!(&bytes[..4], b"last");
        assert_eq!(crate::read(reader, &mut bytes).unwrap(), 0);
        for fd in [poller, reader] {
            crate::close(fd).unwrap();
        }
    }

    #[test]
    fn empty_pipe_is_not_readable_and_write_wakes_epoll() {
        let mut read = core::ptr::null_mut();
        let mut write = core::ptr::null_mut();
        // SAFETY: both outputs are writable and null security selects defaults.
        assert_ne!(
            unsafe { CreatePipe(&raw mut read, &raw mut write, core::ptr::null(), 4096) },
            0
        );
        let read_fd = crate::install(read as usize, FdKind::Pipe, FdFlags::PIPE_READ_END).unwrap();
        let write_fd =
            crate::install(write as usize, FdKind::Pipe, FdFlags::PIPE_WRITE_END).unwrap();
        let epoll_fd = epoll_create1(0).unwrap();
        epoll_ctl(
            epoll_fd,
            EPOLL_CTL_ADD,
            read_fd,
            Some(EpollEvent {
                events: EPOLLIN | EPOLLET,
                data: 0x5151,
            }),
        )
        .unwrap();

        let mut events = [EpollEvent::default(); 2];
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);
        assert_eq!(crate::write(write_fd, b"x").unwrap(), 1);
        assert_eq!(epoll_wait(epoll_fd, &mut events, 100).unwrap(), 1);
        let cookie = events[0].data;
        assert_eq!(cookie, 0x5151);

        let mut byte = [0u8; 1];
        assert_eq!(crate::read(read_fd, &mut byte).unwrap(), 1);
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        crate::close(epoll_fd).unwrap();
        crate::close(read_fd).unwrap();
        crate::close(write_fd).unwrap();
    }

    #[test]
    fn closing_the_last_pipe_writer_wakes_epoll_with_eof() {
        let mut read = core::ptr::null_mut();
        let mut write = core::ptr::null_mut();
        // SAFETY: both outputs are writable and null security selects defaults.
        assert_ne!(
            unsafe { CreatePipe(&raw mut read, &raw mut write, core::ptr::null(), 4096) },
            0
        );
        let read_fd = crate::install(read as usize, FdKind::Pipe, FdFlags::PIPE_READ_END).unwrap();
        let write_fd =
            crate::install(write as usize, FdKind::Pipe, FdFlags::PIPE_WRITE_END).unwrap();
        let epoll_fd = epoll_create1(0).unwrap();
        epoll_ctl(
            epoll_fd,
            EPOLL_CTL_ADD,
            read_fd,
            Some(EpollEvent {
                events: EPOLLIN | EPOLLET,
                data: 0x454f46,
            }),
        )
        .unwrap();

        crate::close(write_fd).unwrap();
        let mut events = [EpollEvent::default(); 1];
        assert_eq!(epoll_wait(epoll_fd, &mut events, 100).unwrap(), 1);
        let reported = events[0].events;
        assert_ne!(reported & (EPOLLIN | EPOLLHUP), 0);
        let mut byte = [0u8; 1];
        assert_eq!(crate::read(read_fd, &mut byte).unwrap(), 0);

        crate::close(epoll_fd).unwrap();
        crate::close(read_fd).unwrap();
    }

    #[test]
    fn closing_the_pipe_reader_changes_write_readiness_to_error() {
        let (read_fd, write_fd) = crate::create_pipe(FdFlags::NONE, 4096).unwrap();
        let epoll_fd = epoll_create1(0).unwrap();
        epoll_ctl(
            epoll_fd,
            EPOLL_CTL_ADD,
            write_fd,
            Some(EpollEvent {
                events: EPOLLOUT | EPOLLET,
                data: 0x5752_4954_45,
            }),
        )
        .unwrap();

        let mut events = [EpollEvent::default(); 1];
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 1);
        assert_ne!(events[0].events & EPOLLOUT, 0);

        crate::close(read_fd).unwrap();
        assert_eq!(epoll_wait(epoll_fd, &mut events, 100).unwrap(), 1);
        assert_ne!(events[0].events & EPOLLERR, 0);

        crate::close(epoll_fd).unwrap();
        crate::close(write_fd).unwrap();
    }

    #[test]
    fn pipe_install_rejects_missing_endpoint_access() {
        let mut read = core::ptr::null_mut();
        let mut write = core::ptr::null_mut();
        assert_ne!(
            unsafe { CreatePipe(&raw mut read, &raw mut write, core::ptr::null(), 4096) },
            0
        );
        assert_eq!(
            crate::install(read as usize, FdKind::Pipe, FdFlags::NONE),
            Err(EINVAL)
        );
        unsafe {
            CloseHandle(read);
            CloseHandle(write);
        }
    }

    #[test]
    fn overlapped_pipe_probe_never_queues_behind_a_pending_read() {
        let (read_fd, write_fd) = crate::create_pipe(FdFlags::NONE, 4096).unwrap();
        let read_entry = crate::get(read_fd).unwrap();
        assert!(read_entry.flags.contains(FdFlags::OVERLAPPED));

        let epoll_fd = epoll_create1(0).unwrap();
        epoll_ctl(
            epoll_fd,
            EPOLL_CTL_ADD,
            read_fd,
            Some(EpollEvent {
                events: EPOLLIN,
                data: 0x4153_594e_43,
            }),
        )
        .unwrap();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let mut byte = [0u8; 1];
            assert_eq!(crate::read(read_fd, &mut byte).unwrap(), 1);
            assert_eq!(byte, [b'x']);
            crate::close(read_fd).unwrap();
        });
        started_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        let started = std::time::Instant::now();
        let mut events = [EpollEvent::default(); 1];
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));

        assert_eq!(crate::write(write_fd, b"x").unwrap(), 1);
        reader.join().unwrap();
        crate::close(epoll_fd).unwrap();
        crate::close(write_fd).unwrap();
    }

    #[test]
    fn a_pty_wakes_epoll_only_once_a_whole_line_has_been_typed() {
        use crate::fs::{O_NOCTTY, O_RDWR};

        let master = crate::tty::open_master(O_RDWR | O_NOCTTY).unwrap();
        crate::tty::set_slave_lock(master, false).unwrap();
        let slave =
            crate::tty::open_slave(crate::tty::pty_number(master).unwrap(), O_RDWR | O_NOCTTY)
                .unwrap();

        let epoll_fd = epoll_create1(0).unwrap();
        epoll_ctl(
            epoll_fd,
            EPOLL_CTL_ADD,
            slave,
            Some(EpollEvent {
                events: EPOLLIN,
                data: 0x7079,
            }),
        )
        .unwrap();

        let mut events = [EpollEvent::default(); 2];
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        // Half a line is queued but not readable in canonical mode, and epoll
        // must agree with the read that follows it — reporting ready here would
        // make the callback block inside a supposedly non-blocking loop.
        crate::write(master, b"half").unwrap();
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        // The terminator makes the line readable, and the wait must end without
        // burning its whole timeout: the terminal's readiness event is a real
        // waitable object, not something polled for.
        crate::write(master, b"\r").unwrap();
        let started = std::time::Instant::now();
        assert_eq!(epoll_wait(epoll_fd, &mut events, 5000).unwrap(), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        // `EpollEvent` is packed, so the cookie is copied out before comparing:
        // a reference to a field of a packed struct would be unaligned.
        let cookie = events[0].data;
        let reported = events[0].events;
        assert_eq!(cookie, 0x7079);
        assert_ne!(reported & EPOLLIN, 0);

        // Draining it makes the set quiet again.
        let mut buffer = [0u8; 16];
        assert_eq!(crate::read(slave, &mut buffer).unwrap(), 5);
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        // Closing the master is a hangup, which epoll reports whether or not it
        // was asked for.
        crate::close(master).unwrap();
        assert_eq!(epoll_wait(epoll_fd, &mut events, 100).unwrap(), 1);
        let hangup = events[0].events;
        assert_ne!(hangup & (EPOLLIN | EPOLLHUP), 0);

        crate::close(epoll_fd).unwrap();
        crate::close(slave).unwrap();
    }

    #[test]
    fn an_inflight_sample_cannot_restore_a_consumed_edge() {
        let (reader, writer) = crate::unix::socketpair(crate::socket::SOCK_STREAM).unwrap();
        let poller = epoll_create1(0).unwrap();
        epoll_ctl(
            poller,
            EPOLL_CTL_ADD,
            writer,
            Some(EpollEvent {
                events: EPOLLOUT | EPOLLET,
                data: 91,
            }),
        )
        .unwrap();
        let key = RegistrationKey {
            fd: writer,
            description_id: crate::get(writer).unwrap().description_id,
        };
        let snapshot = lock_sets(0).unwrap()[&poller].registrations[&key];
        // The old writable edge is still zero. A concurrent full write must
        // nevertheless invalidate this ready sample before it is committed.
        readiness_consumed(key.description_id, EPOLLOUT);
        finish_delivery(poller, key, EPOLLOUT, snapshot.readiness_revision);
        let mut events = [EpollEvent::default(); 1];
        assert_eq!(epoll_wait(poller, &mut events, 0).unwrap(), 1);
        assert_eq!(epoll_wait(poller, &mut events, 0).unwrap(), 0);
        for fd in [poller, reader, writer] {
            crate::close(fd).unwrap();
        }
    }
}
