//! select/pselect bitmap ABI and signal-mask handling.
use super::{
    EINVAL, POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, POLLPRI, PollFd, poll_wait, set_errno,
};
use core::{ffi::c_int, ptr};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FdSet {
    pub fds_bits: [u64; 16],
}

impl FdSet {
    pub fn is_set(&self, fd: usize) -> bool {
        if fd >= 1024 {
            return false;
        }
        let idx = fd / 64;
        let bit = fd % 64;
        (self.fds_bits[idx] & (1u64 << bit)) != 0
    }

    pub fn set(&mut self, fd: usize) {
        if fd < 1024 {
            let idx = fd / 64;
            let bit = fd % 64;
            self.fds_bits[idx] |= 1u64 << bit;
        }
    }

    pub fn zero(&mut self) {
        self.fds_bits = [0u64; 16];
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_select(
    nfds: c_int,
    readfds: *mut FdSet,
    writefds: *mut FdSet,
    exceptfds: *mut FdSet,
    timeout: *mut Timeval,
) -> c_int {
    if nfds < 0 || nfds as usize > kinakaze_vfs::MAX_FDS {
        set_errno(EINVAL);
        return -1;
    }
    let timeout_ms = if timeout.is_null() {
        -1
    } else {
        let tv = unsafe { &*timeout };
        if tv.tv_sec < 0 || tv.tv_usec < 0 {
            set_errno(EINVAL);
            return -1;
        }
        tv.tv_sec
            .saturating_mul(1000)
            .saturating_add(tv.tv_usec.saturating_add(999) / 1000)
            .min(c_int::MAX as i64) as c_int
    };
    let words = (nfds as usize).div_ceil(64);
    // The public fd_set remains 1024 bits. The Linux syscall consumes the
    // caller's nfds-sized bitmap, which may be larger than that libc typedef.
    let sets = [readfds, writefds, exceptfds];
    let word = |set: *mut FdSet, index: usize| {
        if set.is_null() {
            0
        } else {
            unsafe { set.cast::<u64>().add(index).read_unaligned() }
        }
    };
    let mut entries = Vec::new();
    for index in 0..words {
        let bits = sets.map(|set| word(set, index));
        let mut selected = bits[0] | bits[1] | bits[2];
        if index + 1 == words && nfds % 64 != 0 {
            selected &= (1u64 << (nfds % 64)) - 1;
        }
        while selected != 0 {
            let bit = selected.trailing_zeros() as usize;
            let mask = 1u64 << bit;
            selected &= !mask;
            let events = (if bits[0] & mask != 0 { POLLIN } else { 0 })
                | (if bits[1] & mask != 0 { POLLOUT } else { 0 })
                | (if bits[2] & mask != 0 { POLLPRI } else { 0 });
            entries.push(PollFd {
                fd: (index * 64 + bit) as c_int,
                events,
                revents: 0,
            });
        }
    }
    let started = std::time::Instant::now();
    let result = poll_wait(&mut entries, timeout_ms);
    if !timeout.is_null() {
        let elapsed = started.elapsed().as_micros().min(i64::MAX as u128) as i64;
        let tv = unsafe { &mut *timeout };
        let remaining = tv
            .tv_sec
            .saturating_mul(1_000_000)
            .saturating_add(tv.tv_usec)
            .saturating_sub(elapsed)
            .max(0);
        tv.tv_sec = remaining / 1_000_000;
        tv.tv_usec = remaining % 1_000_000;
    }
    if let Err(error) = result {
        if error == kinakaze_vfs::EINTR {
            kinakaze_vfs::signal::deliver_pending();
        }
        set_errno(error);
        return -1;
    }
    if entries.iter().any(|entry| entry.revents & POLLNVAL != 0) {
        set_errno(kinakaze_vfs::EBADF);
        return -1;
    }
    for set in sets {
        if !set.is_null() {
            unsafe { ptr::write_bytes(set.cast::<u64>(), 0, words) };
        }
    }
    let mut ready = 0;
    for entry in entries {
        // select returns the number of set bits, including a descriptor ready
        // in both read and write sets. Only originally selected sets are filled.
        let returned = [
            entry.revents & (POLLIN | POLLHUP | POLLERR),
            entry.revents & (POLLOUT | POLLERR),
            entry.revents & POLLPRI,
        ];
        for (index, interest) in [POLLIN, POLLOUT, POLLPRI].into_iter().enumerate() {
            if entry.events & interest != 0 && returned[index] != 0 {
                let address = unsafe { sets[index].cast::<u64>().add(entry.fd as usize / 64) };
                unsafe {
                    address
                        .write_unaligned(address.read_unaligned() | (1 << (entry.fd as usize % 64)))
                };
                ready += 1;
            }
        }
    }
    ready
}

/// Linux's `struct timespec`, as `pselect` takes it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// `pselect`.
///
/// The signal mask is the whole reason this exists rather than `select`: the
/// classic race is to check a flag a handler sets, then call `select`, and have
/// the signal arrive in between — the handler runs, the flag is set, and the
/// wait blocks anyway. `pselect` closes it by installing `sigmask` *and*
/// waiting under one operation, so a signal that was blocked before the call
/// becomes deliverable only while the wait is in progress.
///
/// The mask really is swapped here and really is restored afterwards, including
/// on the error paths. The atomicity is as good as this layer's signals are:
/// delivery happens when a thread checks for pending signals rather than
/// asynchronously, and unblocking immediately before the wait means the wait is
/// the next such check — which is the property the race needs.
///
/// # Safety
///
/// The three descriptor sets must be null or writable, `timeout` must be null or
/// a readable `struct timespec`, and `sigmask` null or a readable `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pselect(
    nfds: c_int,
    readfds: *mut FdSet,
    writefds: *mut FdSet,
    exceptfds: *mut FdSet,
    timeout: *const Timespec,
    sigmask: *const u64,
) -> c_int {
    // pselect promises not to modify the timeout, and takes nanoseconds rather
    // than microseconds. Converting into a local keeps both promises.
    let mut converted = Timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut timeout_argument = ptr::null_mut();
    if !timeout.is_null() {
        // SAFETY: the caller guarantees a readable struct timespec.
        let requested = unsafe { *timeout };
        if requested.tv_sec < 0 || !(0..1_000_000_000).contains(&requested.tv_nsec) {
            set_errno(EINVAL);
            return -1;
        }
        converted.tv_sec = requested.tv_sec;
        // Rounded up rather than truncated: a caller asking for 500ns of
        // patience must not get a zero-timeout poll.
        converted.tv_usec = (requested.tv_nsec + 999) / 1000;
        timeout_argument = &raw mut converted;
    }

    let restore = if sigmask.is_null() {
        None
    } else {
        // SAFETY: the caller guarantees a readable sigset_t, which on Linux
        // x86_64 is a 64-bit word for the signals this layer implements.
        let requested = unsafe { *sigmask };
        // One operation rather than a read followed by a write: the swap is what
        // makes changing the mask and capturing the old one indivisible.
        Some(kinakaze_vfs::signal::swap_blocked_mask(requested))
    };

    // SAFETY: the descriptor sets are forwarded unchanged, and the timeout now
    // points at a local this function owns.
    let result =
        unsafe { kinakaze_abi_select(nfds, readfds, writefds, exceptfds, timeout_argument) };

    if let Some(previous) = restore {
        // Restored on every path, including the error one: leaving the caller's
        // mask replaced would silently change the disposition of every later
        // wait on this thread. The errno the wait produced is carried across the
        // restore, because the caller reads it after this returns.
        let saved = kinakaze_tls::errno();
        kinakaze_vfs::signal::swap_blocked_mask(previous);
        set_errno(saved);
    }
    result
}
