//! Socket, `epoll` and `io_uring` C ABI exports.

use core::ffi::{c_int, c_void};

use kinakaze_vfs::epoll::{self, EpollEvent};
use kinakaze_vfs::iouring::{self, CompletionEntry, SubmissionEntry};
use kinakaze_vfs::socket;
use kinakaze_vfs::unix;

use crate::set_errno;

/// Applies the POSIX `-1`/`errno` convention.
fn posix<T>(result: Result<T, i32>, success: impl FnOnce(T) -> i64) -> i64 {
    match result {
        Ok(value) => success(value),
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Reports success as 0 and failure as -1.
fn posix_unit(result: Result<(), i32>) -> c_int {
    posix(result, |()| 0) as c_int
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_socket(
    family: c_int,
    socket_type: c_int,
    protocol: c_int,
) -> c_int {
    posix(socket::socket(family, socket_type, protocol), i64::from) as c_int
}

/// `bind`.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_bind(
    fd: c_int,
    address: *const u8,
    length: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    posix_unit(unsafe { socket::bind(fd, address, length) })
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_listen(fd: c_int, backlog: c_int) -> c_int {
    posix_unit(socket::listen(fd, backlog))
}

/// `socketpair`. Only `AF_UNIX` has a pair to create.
///
/// # Safety
///
/// `pair` must be writable for two `int`s.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_socketpair(
    domain: c_int,
    socket_type: c_int,
    protocol: c_int,
    pair: *mut c_int,
) -> c_int {
    if pair.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    if domain != socket::AF_UNIX {
        set_errno(kinakaze_vfs::EOPNOTSUPP);
        return -1;
    }
    if protocol != 0 {
        set_errno(kinakaze_vfs::EPROTOTYPE);
        return -1;
    }
    match unix::socketpair(socket_type) {
        Ok((first, second)) => {
            // SAFETY: the caller guarantees room for two ints.
            unsafe {
                pair.write(first);
                pair.add(1).write(second);
            }
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `accept`.
///
/// # Safety
///
/// `address` and `length` must be null or a writable pair.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_accept(
    fd: c_int,
    address: *mut u8,
    length: *mut c_int,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    posix(unsafe { socket::accept(fd, address, length, 0) }, i64::from) as c_int
}

/// `accept4`.
///
/// # Safety
///
/// `address` and `length` must be null or a writable pair.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_accept4(
    fd: c_int,
    address: *mut u8,
    length: *mut c_int,
    flags: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    posix(
        unsafe { socket::accept(fd, address, length, flags) },
        i64::from,
    ) as c_int
}

/// `connect`.
///
/// # Safety
///
/// `address` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_connect(
    fd: c_int,
    address: *const u8,
    length: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    posix_unit(unsafe { socket::connect(fd, address, length) })
}

/// `send`.
///
/// # Safety
///
/// `buffer` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_send(
    fd: c_int,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: forwarded from this function's contract.
    posix(
        unsafe { socket::send(fd, buffer.cast(), length, flags) },
        |sent| sent as i64,
    ) as isize
}

/// `recv`.
///
/// # Safety
///
/// `buffer` must be writable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_recv(
    fd: c_int,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
) -> isize {
    // SAFETY: forwarded from this function's contract.
    posix(
        unsafe { socket::recv(fd, buffer.cast(), length, flags) },
        |received| received as i64,
    ) as isize
}

/// `sendto`.
///
/// # Safety
///
/// `buffer` must be readable for `length` bytes and `address` readable for
/// `address_length` bytes when non-null.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sendto(
    fd: c_int,
    buffer: *const c_void,
    length: usize,
    flags: c_int,
    address: *const u8,
    address_length: c_int,
) -> isize {
    // SAFETY: forwarded from this function's contract.
    posix(
        unsafe { socket::sendto(fd, buffer.cast(), length, flags, address, address_length) },
        |sent| sent as i64,
    ) as isize
}

/// `recvfrom`.
///
/// # Safety
///
/// `buffer` must be writable for `length` bytes; `address` and `address_length`
/// must be null or a writable pair.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_recvfrom(
    fd: c_int,
    buffer: *mut c_void,
    length: usize,
    flags: c_int,
    address: *mut u8,
    address_length: *mut c_int,
) -> isize {
    // SAFETY: forwarded from this function's contract.
    posix(
        unsafe { socket::recvfrom(fd, buffer.cast(), length, flags, address, address_length) },
        |received| received as i64,
    ) as isize
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_shutdown(fd: c_int, how: c_int) -> c_int {
    posix_unit(socket::shutdown(fd, how))
}

/// `setsockopt`.
///
/// # Safety
///
/// `value` must be readable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setsockopt(
    fd: c_int,
    level: c_int,
    name: c_int,
    value: *const c_void,
    length: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    posix_unit(unsafe { socket::setsockopt(fd, level, name, value.cast(), length) })
}

/// `getsockopt`.
///
/// # Safety
///
/// `value` must be writable for `*length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getsockopt(
    fd: c_int,
    level: c_int,
    name: c_int,
    value: *mut c_void,
    length: *mut c_int,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    posix_unit(unsafe { socket::getsockopt(fd, level, name, value.cast(), length) })
}

/// `getsockname`.
///
/// # Safety
///
/// `address` and `length` must be a writable pair.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getsockname(
    fd: c_int,
    address: *mut u8,
    length: *mut c_int,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    posix_unit(unsafe { socket::getsockname(fd, address, length) })
}

/// `getpeername`.
///
/// # Safety
///
/// `address` and `length` must be a writable pair.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getpeername(
    fd: c_int,
    address: *mut u8,
    length: *mut c_int,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    posix_unit(unsafe { socket::getpeername(fd, address, length) })
}

// Byte-order helpers. These are pure computation and belong here rather than in
// the socket layer.

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_htons(value: u16) -> u16 {
    value.to_be()
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ntohs(value: u16) -> u16 {
    u16::from_be(value)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_htonl(value: u32) -> u32 {
    value.to_be()
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ntohl(value: u32) -> u32 {
    u32::from_be(value)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_epoll_create(size: c_int) -> c_int {
    posix(epoll::epoll_create(size), i64::from) as c_int
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_epoll_create1(flags: c_int) -> c_int {
    posix(epoll::epoll_create1(flags), i64::from) as c_int
}

/// `epoll_ctl`.
///
/// # Safety
///
/// `event` must be null or point to a readable `struct epoll_event`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_epoll_ctl(
    epoll_fd: c_int,
    operation: c_int,
    fd: c_int,
    event: *const EpollEvent,
) -> c_int {
    let requested = if event.is_null() {
        None
    } else {
        // SAFETY: the caller guarantees a readable epoll_event.
        Some(unsafe { event.read_unaligned() })
    };
    posix_unit(epoll::epoll_ctl(epoll_fd, operation, fd, requested))
}

/// `epoll_wait`.
///
/// # Safety
///
/// `events` must be writable for `max_events` entries.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_epoll_wait(
    epoll_fd: c_int,
    events: *mut EpollEvent,
    max_events: c_int,
    timeout: c_int,
) -> c_int {
    if events.is_null() || max_events <= 0 {
        set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    // SAFETY: the caller guarantees `max_events` writable entries.
    let out = unsafe { core::slice::from_raw_parts_mut(events, max_events as usize) };
    posix(epoll::epoll_wait(epoll_fd, out, timeout), |ready| {
        ready as i64
    }) as c_int
}

/// `epoll_pwait`.
///
/// The signal mask is ignored: this layer delivers signals at wait boundaries
/// rather than atomically swapping the mask, so honouring it would imply a
/// guarantee that is not provided.
///
/// # Safety
///
/// `events` must be writable for `max_events` entries.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_epoll_pwait(
    epoll_fd: c_int,
    events: *mut EpollEvent,
    max_events: c_int,
    timeout: c_int,
    _mask: *const c_void,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    unsafe { kinakaze_abi_epoll_wait(epoll_fd, events, max_events, timeout) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_io_uring_setup(entries: u32, flags: u32) -> c_int {
    posix(iouring::setup(entries, flags), i64::from) as c_int
}

/// Queues one submission entry.
///
/// # Safety
///
/// Guest memory must obey the ABI ownership contract. Invalid SQE and data
/// pointers are fault-contained; no Rust reference is made to the guest SQE.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_io_uring_push(
    ring_fd: c_int,
    entry: *const SubmissionEntry,
) -> c_int {
    if entry.is_null() {
        set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the VFS imports the SQE through a kernel-probed guest copy.
    posix_unit(unsafe { iouring::push_guest(ring_fd, entry as usize) })
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_io_uring_enter(
    ring_fd: c_int,
    to_submit: u32,
    min_complete: u32,
    flags: u32,
) -> c_int {
    posix(
        iouring::enter_guest(ring_fd, to_submit, min_complete, flags),
        |submitted| submitted as i64,
    ) as c_int
}

/// Reaps completions.
///
/// # Safety
///
/// Guest output must not alias the runtime's private queue state. Invalid
/// destinations are fault-contained and leave unpublished CQEs available.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_io_uring_reap(
    ring_fd: c_int,
    out: *mut CompletionEntry,
    count: u32,
) -> c_int {
    if out.is_null() || count == 0 {
        set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    // SAFETY: guest publication is kernel-probed before CQEs are consumed.
    posix(
        unsafe { iouring::reap_guest(ring_fd, out as usize, count) },
        |reaped| reaped as i64,
    ) as c_int
}

#[cfg(test)]
mod io_uring_tests;

#[repr(C, align(4))]
pub struct In6Addr(pub [u8; 16]);

#[unsafe(no_mangle)]
pub static in6addr_any: In6Addr = In6Addr([0u8; 16]);

#[unsafe(no_mangle)]
pub static kinakaze_abi_in6addr_any: In6Addr = In6Addr([0u8; 16]);

#[unsafe(no_mangle)]
pub static in6addr_loopback: In6Addr = In6Addr([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

#[unsafe(no_mangle)]
pub static kinakaze_abi_in6addr_loopback: In6Addr =
    In6Addr([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
