//! Signal waiting, process-group signalling and signal naming.
//!
//! The signal machinery itself lives in [`kinakaze_vfs::signal`]: dispositions,
//! the per-thread blocked mask, the pending set, and the interrupt events that
//! let a raised signal break a blocking wait. Everything here is built on that
//! one mechanism. A second one would be worse than useless, because a signal
//! raised through `kill` would then be invisible to a thread parked in
//! `sigsuspend`.
//!
//! Two things about that foundation shape this file. Handlers run when a thread
//! looks for them rather than asynchronously, so a wait here is a loop that
//! checks the pending set. And the pending set is process-wide, so a
//! process-directed signal is claimed by whichever thread notices it first —
//! which is what Linux does too, among the threads that do not block it.

use core::ffi::{c_char, c_int};
use core::ptr;
use std::cell::UnsafeCell;
use std::time::{Duration, Instant};

use kinakaze_vfs::signal::{self, Delivery, NSIG, SIG_SETMASK};
use kinakaze_vfs::{EAGAIN, EFAULT, EINTR, EINVAL};

use crate::fdio::TimeSpec;
use crate::signal::SigSet;

/// How long a wait sleeps between checks of the pending set.
///
/// The waits here poll rather than blocking on the thread's interrupt event,
/// because that event is private to [`kinakaze_vfs::interrupt`] and is owned by
/// the I/O paths that wait on it alongside a completion handle. A millisecond
/// is short enough that no program can tell the difference — signal delivery on
/// a real kernel is not instantaneous either — and it keeps this file from
/// reaching into another crate's internals to build a second wakeup path.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Installs the process-termination hook without changing any disposition.
///
/// A signal whose default action is to terminate can only do so if
/// [`crate::signal`]'s hook is installed, and that module installs it from its
/// own entry points. Querying one disposition is the cheapest way to reach it:
/// `sigaction` with both pointers null reads nothing and writes nothing, but
/// runs the installation on the way in. Without this, a program whose very first
/// signal call is `sigsuspend` would survive a `SIGTERM` it never handled.
fn ensure_terminate_hook() {
    // SAFETY: a null action is a pure query and a null old-action writes nothing.
    unsafe { crate::signal::kinakaze_abi_sigaction(signal::SIGHUP, ptr::null(), ptr::null_mut()) };
}

/// Reads the low word of a guest `sigset_t`, which holds all 64 signals.
///
/// # Safety
///
/// `set` must be null or point to a readable `sigset_t`.
unsafe fn mask_of(set: *const SigSet) -> Option<u64> {
    if set.is_null() {
        return None;
    }
    // SAFETY: forwarded from this function's contract.
    Some(unsafe { (*set).bits[0] })
}

/// Returns the mask bit for a signal number, or `None` if out of range.
fn bit(signal_number: c_int) -> Option<u64> {
    if signal_number <= 0 || signal_number as usize >= NSIG {
        return None;
    }
    Some(1u64 << (signal_number as u64 - 1))
}

// ---------------------------------------------------------------------------
// Waiting for signals.
// ---------------------------------------------------------------------------

/// `sigsuspend`: replaces the blocked mask and waits for a signal to be handled.
///
/// Returns -1 with `EINTR` always. There is no success case, which surprises
/// people reading the signature: the call exists precisely to be interrupted, so
/// the only other thing it can do is fail with `EFAULT`.
///
/// The old mask is restored before returning, which is what makes the
/// block-check-suspend idiom race-free: a caller blocks a signal, tests a flag,
/// and calls `sigsuspend` with the signal unblocked, so there is no window in
/// which the signal could arrive unnoticed.
///
/// A signal that is already pending when the call is made is delivered
/// immediately rather than waited for. Checking before sleeping is the whole
/// difference between this working and hanging: the signal may have arrived
/// between the caller's flag test and this call, and there will be no second
/// notification for it.
///
/// # Safety
///
/// `mask` must be null or point to a readable `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigsuspend(mask: *const SigSet) -> c_int {
    ensure_terminate_hook();
    // SAFETY: forwarded from this function's contract.
    let Some(requested) = (unsafe { mask_of(mask) }) else {
        crate::set_errno(EFAULT);
        return -1;
    };

    let Ok(previous) = signal::sigprocmask(SIG_SETMASK, requested) else {
        crate::set_errno(EINVAL);
        return -1;
    };

    // Registering as a waiter means a signal raised on another thread wakes this
    // one's interrupt event. The poll loop does not depend on that wake, but a
    // registered waiter is also what stops `raise_signal` from having nobody to
    // notify, and it keeps this wait visible to the rest of the machinery.
    signal::register_waiter();
    loop {
        // Pending signals are checked before every sleep, so one that arrived
        // before the mask changed is handled rather than waited for.
        if signal::deliver_pending() != Delivery::None {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    signal::unregister_waiter();

    // Restoring the mask may itself make a signal deliverable, but that delivery
    // belongs to whatever the caller does next, not to this call.
    let _ = signal::sigprocmask(SIG_SETMASK, previous);
    crate::set_errno(EINTR);
    -1
}

/// `sigtimedwait`: accepts one signal from `set` without running its handler.
///
/// Returns the signal number, or -1 with `EAGAIN` if the timeout expires and
/// `EINTR` if a signal outside `set` was handled first. Callers depend on that
/// distinction: `EAGAIN` means the timeout is the answer, `EINTR` means retry.
///
/// The caller is expected to have blocked every signal in `set` beforehand,
/// which is what stops a handler from claiming the signal before this call sees
/// it. `sigwaitinfo` is this call with a null timeout.
///
/// Acceptance removes one occurrence atomically from the canonical signal queue,
/// retaining its sender/timer payload without changing the process disposition.
///
/// # Safety
///
/// `set` must point to a readable `sigset_t`. `info` must be null or point to a
/// writable `siginfo_t`, and `timeout` null or a readable `struct timespec`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigtimedwait(
    set: *const SigSet,
    info: *mut SigInfo,
    timeout: *const TimeSpec,
) -> c_int {
    ensure_terminate_hook();
    // SAFETY: forwarded from this function's contract.
    let Some(wanted) = (unsafe { mask_of(set) }) else {
        crate::set_errno(EFAULT);
        return -1;
    };
    if wanted == 0 {
        // No signal can ever satisfy an empty set, so waiting would be a hang.
        crate::set_errno(EINVAL);
        return -1;
    }

    let deadline = if timeout.is_null() {
        None
    } else {
        // SAFETY: forwarded from this function's contract.
        let requested = unsafe { *timeout };
        if requested.tv_sec < 0 || !(0..1_000_000_000).contains(&requested.tv_nsec) {
            crate::set_errno(EINVAL);
            return -1;
        }
        Some(Instant::now() + Duration::new(requested.tv_sec as u64, requested.tv_nsec as u32))
    };

    signal::register_waiter();
    let outcome = loop {
        // A wanted signal that is already pending is taken without sleeping.
        if let Some(pending) = signal::take_pending(wanted) {
            break Ok(pending);
        }

        // A signal outside the set whose handler can run interrupts the wait,
        // which is the EINTR the caller distinguishes from a timeout.
        if signal::deliver_pending() != Delivery::None {
            break Err(EINTR);
        }

        // A zero timeout makes this a poll, which is a documented use: it tests
        // for a pending signal without waiting for one.
        if let Some(deadline) = deadline {
            let now = Instant::now();
            if now >= deadline {
                break Err(EAGAIN);
            }
            std::thread::sleep(POLL_INTERVAL.min(deadline - now));
        } else {
            std::thread::sleep(POLL_INTERVAL);
        }
    };
    signal::unregister_waiter();

    match outcome {
        Ok(pending) => {
            let value = SigInfo {
                si_signo: pending.signal,
                si_errno: 0,
                si_code: pending.code,
                padding: pending.payload,
            };
            if !info.is_null() {
                // SAFETY: the caller guarantees a writable siginfo_t.
                unsafe { ptr::write(info, value) };
            }
            pending.signal
        }
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `sigwaitinfo`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigwaitinfo(
    set: *const SigSet,
    info: *mut SigInfo,
) -> c_int {
    unsafe { kinakaze_abi_sigtimedwait(set, info, ptr::null()) }
}

/// `sigwait`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigwait(set: *const SigSet, sig: *mut c_int) -> c_int {
    if sig.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    let res = unsafe { kinakaze_abi_sigtimedwait(set, ptr::null_mut(), ptr::null()) };
    if res > 0 {
        unsafe { *sig = res };
        0
    } else {
        crate::kinakaze_errno()
    }
}

/// The Linux x86_64 `siginfo_t`.
///
/// 128 bytes, of which the first three `int`s are `si_signo`, `si_errno` and
/// `si_code` in that order. Everything after them is a union whose
/// interpretation depends on `si_code`; the canonical queue supplies the exact
/// union payload for sender, timer, queued value, and other supported sources.
///
/// The alignment is 8: the union contains pointers and a 64-bit clock value.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub struct SigInfo {
    pub si_signo: c_int,
    pub si_errno: c_int,
    pub si_code: c_int,
    /// The remainder of the 128-byte structure.
    pub padding: [u8; 116],
}

/// `SI_USER`: sent by `kill`, `raise` or `sigqueue` from user space.
const SI_USER: c_int = 0;

impl SigInfo {
    /// Describes a signal this layer accepted.
    ///
    /// `SI_USER` is the truthful code: every signal in this implementation
    /// originates from a `kill` or `raise` call, never from a hardware fault the
    /// way a real `SIGSEGV` would.
    fn from_signal(signal_number: c_int) -> Self {
        Self {
            si_signo: signal_number,
            si_errno: 0,
            si_code: SI_USER,
            padding: [0; 116],
        }
    }
}

// ---------------------------------------------------------------------------
// Process groups.
//
// `crate::userdb` establishes the shape of this: process groups and sessions
// live in the shared registry `kinakaze_vfs::job` keeps, so a group really can
// span the separate Windows processes that `fork` produces. `killpg` is the
// signalling half of that and shares its delivery path with `kill(-pgid)`.
// ---------------------------------------------------------------------------

/// `killpg`: sends a signal to every process in a process group.
///
/// A group argument of 0 means the caller's own group. Every member of the named
/// group receives the signal through the registry, which is what makes a shell's
/// `kill %1` reach a pipeline rather than only the process that asked.
///
/// `ESRCH` means the group has no live members — the honest answer, and the one
/// a shell reads to decide a job has finished. A signal of 0 performs that
/// existence check without delivering anything.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_killpg(pgrp: c_int, signal_number: c_int) -> c_int {
    // A negative group id is not a group.
    if pgrp < 0 {
        crate::set_errno(EINVAL);
        return -1;
    }
    ensure_terminate_hook();
    let group = if pgrp == 0 {
        kinakaze_vfs::job::current_pgid()
    } else {
        pgrp
    };
    match kinakaze_vfs::job::signal_process_group(group, signal_number) {
        Ok(()) => {
            // A signal that landed on the caller is delivered before returning,
            // which is what `raise` promises and what a shell signalling its own
            // group depends on.
            signal::deliver_pending();
            0
        }
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `setpgrp`: equivalent to `setpgid(0, 0)`.
///
/// This is the POSIX no-argument form, which is what glibc exports by default.
/// It is defined in terms of `setpgid` rather than duplicating its reasoning, so
/// the two can never disagree about which moves are legal.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setpgrp() -> c_int {
    crate::userdb::kinakaze_abi_setpgid(0, 0)
}

// ---------------------------------------------------------------------------
// Signal names.
// ---------------------------------------------------------------------------

/// glibc's signal descriptions, indexed by signal number.
///
/// These are the exact strings glibc's `sys_siglist` holds, because they are
/// user-visible: BusyBox's `kill -l`, and every shell that reports why a child
/// died, prints them. "Segmentation fault" is what a user recognises where
/// "Signal 11" would leave them looking it up.
///
/// Index 0 is unused, so the table is read by signal number directly.
const DESCRIPTIONS: [&str; 32] = [
    "Unknown signal 0\0",
    "Hangup\0",
    "Interrupt\0",
    "Quit\0",
    "Illegal instruction\0",
    "Trace/breakpoint trap\0",
    "Aborted\0",
    "Bus error\0",
    "Floating point exception\0",
    "Killed\0",
    "User defined signal 1\0",
    "Segmentation fault\0",
    "User defined signal 2\0",
    "Broken pipe\0",
    "Alarm clock\0",
    "Terminated\0",
    "Stack fault\0",
    "Child exited\0",
    "Continued\0",
    "Stopped (signal)\0",
    "Stopped\0",
    "Stopped (tty input)\0",
    "Stopped (tty output)\0",
    "Urgent I/O condition\0",
    "CPU time limit exceeded\0",
    "File size limit exceeded\0",
    "Virtual timer expired\0",
    "Profiling timer expired\0",
    "Window changed\0",
    "I/O possible\0",
    "Power failure\0",
    "Bad system call\0",
];

/// The lowest real-time signal.
const SIGRTMIN: c_int = 34;

/// glibc's signal abbreviations, indexed by signal number.
///
/// These match glibc's `_sys_sigabbrev_internal`, returned by `sigabbrev_np`.
const ABBREVIATIONS: [&str; 32] = [
    "\0", "HUP\0", "INT\0", "QUIT\0", "ILL\0", "TRAP\0", "ABRT\0", "BUS\0", "FPE\0", "KILL\0",
    "USR1\0", "SEGV\0", "USR2\0", "PIPE\0", "ALRM\0", "TERM\0", "STKFLT\0", "CHLD\0", "CONT\0",
    "STOP\0", "TSTP\0", "TTIN\0", "TTOU\0", "URG\0", "XCPU\0", "XFSZ\0", "VTALRM\0", "PROF\0",
    "WINCH\0", "IO\0", "PWR\0", "SYS\0",
];

/// A per-thread buffer for the descriptions that must be formatted.
///
/// `strsignal` returns a pointer the caller may read until its next call, so the
/// storage has to outlive the return. glibc uses thread-local storage for
/// exactly this, which also means two threads naming unknown signals do not
/// overwrite each other's answer.
struct NameBuffer(UnsafeCell<[u8; NAME_CAPACITY]>);

/// Size of the formatting buffer. Ample: the longest string formatted into it is
/// "Unknown signal -2147483648".
const NAME_CAPACITY: usize = 64;

thread_local! {
    static FORMATTED: NameBuffer = const { NameBuffer(UnsafeCell::new([0; NAME_CAPACITY])) };
}

/// Copies `text` into this thread's buffer and returns its address.
fn formatted(text: &str) -> *mut c_char {
    FORMATTED.with(|buffer| {
        // Written through raw pointers throughout. Taking a `&mut [u8]` to the
        // cell's contents would be a reference to the interior of an
        // `UnsafeCell` held across a call, which is exactly what the pointer
        // arithmetic below avoids.
        let base = buffer.0.get().cast::<u8>();
        let bytes = text.as_bytes();
        // One byte is reserved so the result is terminated even when truncated.
        let length = bytes.len().min(NAME_CAPACITY - 1);
        // SAFETY: `base` addresses this thread's own `NAME_CAPACITY`-byte buffer,
        // `length` is at most one less than that, and the source is a live slice
        // of at least `length` bytes that cannot overlap the thread-local.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), base, length);
            *base.add(length) = 0;
            base.cast::<c_char>()
        }
    })
}

/// `strsignal`: a human-readable description of a signal number.
///
/// The returned string must not be freed or modified. For a known signal it is a
/// static constant; for anything else it is this thread's formatted buffer,
/// valid until the same thread calls `strsignal` again. glibc has the same
/// contract.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_strsignal(signal_number: c_int) -> *mut c_char {
    if signal_number > 0 && (signal_number as usize) < DESCRIPTIONS.len() {
        // A static description needs no buffer, so the pointer stays valid
        // indefinitely and survives a later call on the same thread.
        return DESCRIPTIONS[signal_number as usize]
            .as_ptr()
            .cast_mut()
            .cast();
    }
    if (SIGRTMIN..NSIG as c_int).contains(&signal_number) {
        // glibc numbers these from SIGRTMIN rather than absolutely, since the
        // first few real-time signals are reserved by the C library.
        return formatted(&format!("Real-time signal {}", signal_number - SIGRTMIN));
    }
    formatted(&format!("Unknown signal {signal_number}"))
}

/// `sigdescr_np`: a human-readable description of a signal number.
///
/// Unlike `strsignal`, this returns an immutable, non-localized string. If the
/// signal number is not valid or out of range, it returns `NULL`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sigdescr_np(signal_number: c_int) -> *const c_char {
    if signal_number > 0 && (signal_number as usize) < DESCRIPTIONS.len() {
        DESCRIPTIONS[signal_number as usize].as_ptr().cast()
    } else {
        ptr::null()
    }
}

/// `sigabbrev_np`: the abbreviated name of a signal number.
///
/// Returns the abbreviation (e.g. "HUP", "INT") without the "SIG" prefix, or
/// `NULL` if the signal number is not valid or out of range.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sigabbrev_np(signal_number: c_int) -> *const c_char {
    if signal_number > 0 && (signal_number as usize) < ABBREVIATIONS.len() {
        ABBREVIATIONS[signal_number as usize].as_ptr().cast()
    } else {
        ptr::null()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::CStr;
    /// `ESRCH`, taken from the process-group registry that produces it rather
    /// than restated here, so the two cannot drift apart.
    use kinakaze_vfs::job::ESRCH;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::mpsc;
    use std::sync::{Mutex, MutexGuard};

    /// Serializes every test that raises a signal.
    ///
    /// Dispositions and the pending set are process-wide, so two tests raising at
    /// once would claim each other's signals. This is the same reasoning
    /// [`kinakaze_vfs::signal`] applies to its own tests; the lock cannot be
    /// shared across the crate boundary, so this is a second one covering this
    /// file's tests.
    fn serialized() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The signal the recording handler last saw.
    static OBSERVED: AtomicI32 = AtomicI32::new(0);

    unsafe extern "sysv64" fn record(signal_number: c_int) {
        OBSERVED.store(signal_number, Ordering::SeqCst);
    }

    /// Installs the recording handler, returning a guard that restores the
    /// default on the way out.
    ///
    /// A handler is not optional in these tests. With the default disposition,
    /// delivering SIGUSR1 would run the terminate hook and `_exit` the test
    /// process, taking every other test with it.
    fn handled(signal_number: c_int) -> Restore {
        signal::sigaction(
            signal_number,
            Some(signal::Action {
                disposition: signal::Disposition::Handle(record, 0),
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .expect("installing a test handler");
        OBSERVED.store(0, Ordering::SeqCst);
        Restore(signal_number)
    }

    /// Restores a signal's default disposition when dropped.
    struct Restore(c_int);

    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = signal::sigaction(self.0, Some(signal::Action::default()));
        }
    }

    /// Builds a `sigset_t` containing the listed signals.
    fn set_of(signals: &[c_int]) -> SigSet {
        let mut bits = [0u64; 16];
        for signal_number in signals {
            bits[0] |= bit(*signal_number).expect("a valid signal number");
        }
        SigSet { bits }
    }

    /// Runs `body` on another thread, failing the test if it does not finish.
    ///
    /// Every wait in this file is a loop, so a bug turns a test into a hang that
    /// blocks the whole run. Bounding it converts that into a readable failure.
    fn with_watchdog<T: Send + 'static>(
        what: &str,
        body: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = sender.send(body());
        });
        match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(value) => {
                let _ = worker.join();
                value
            }
            // The sender was dropped without a value, which means the body
            // panicked. Re-raising its payload reports the real assertion rather
            // than blaming a timeout that did not happen.
            Err(mpsc::RecvTimeoutError::Disconnected) => match worker.join() {
                Err(payload) => std::panic::resume_unwind(payload),
                Ok(()) => panic!("{what}: the worker finished without a result"),
            },
            // A genuine hang. The worker is left running; the process is about to
            // fail anyway, and detaching it is better than blocking on a join
            // that would never return.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("{what} did not return within ten seconds")
            }
        }
    }

    #[test]
    fn sigsuspend_returns_eintr_after_a_handler_runs() {
        let _lock = serialized();
        let _restore = handled(signal::SIGUSR1);

        let result = with_watchdog("sigsuspend with a pending signal", || {
            signal::sigprocmask(signal::SIG_BLOCK, bit(signal::SIGUSR1).unwrap())
                .expect("blocking SIGUSR1");
            signal::raise_signal(signal::SIGUSR1).expect("raising SIGUSR1");
            let empty = set_of(&[]);
            // SAFETY: `empty` is a live, readable sigset_t.
            let result = unsafe { kinakaze_abi_sigsuspend(&raw const empty) };
            (result, crate::kinakaze_errno())
        });

        assert_eq!(result.0, -1, "sigsuspend always fails");
        assert_eq!(result.1, EINTR, "and always with EINTR");
        assert_eq!(
            OBSERVED.load(Ordering::SeqCst),
            signal::SIGUSR1,
            "the handler must have run"
        );
    }

    #[test]
    fn sigsuspend_restores_the_previous_mask() {
        let _lock = serialized();
        let _restore = handled(signal::SIGUSR2);

        with_watchdog("sigsuspend mask restoration", || {
            // Block SIGUSR2 on this thread, then suspend with an empty mask. The
            // block-test-suspend idiom depends on the original mask coming back.
            signal::sigprocmask(signal::SIG_BLOCK, bit(signal::SIGUSR2).unwrap())
                .expect("blocking SIGUSR2");
            let before = signal::blocked_mask();
            assert_ne!(before & bit(signal::SIGUSR2).unwrap(), 0);

            signal::raise_signal(signal::SIGUSR2).expect("raising SIGUSR2");
            let empty = set_of(&[]);
            // SAFETY: `empty` is a live, readable sigset_t.
            assert_eq!(unsafe { kinakaze_abi_sigsuspend(&raw const empty) }, -1);

            assert_eq!(
                signal::blocked_mask(),
                before,
                "sigsuspend did not restore the blocked mask"
            );
            // Clean up this thread's mask for anything that runs after.
            signal::sigprocmask(SIG_SETMASK, 0).expect("clearing the mask");
        });
    }

    #[test]
    fn sigsuspend_rejects_a_null_mask() {
        let _lock = serialized();
        // SAFETY: passing null is the case under test.
        assert_eq!(unsafe { kinakaze_abi_sigsuspend(ptr::null()) }, -1);
        assert_eq!(crate::kinakaze_errno(), EFAULT);
    }

    #[test]
    fn sigtimedwait_accepts_a_pending_signal_without_running_its_handler() {
        let _lock = serialized();
        let _restore = handled(signal::SIGUSR1);

        let outcome = with_watchdog("sigtimedwait accepting a pending signal", || {
            // The caller is expected to block the set it waits on, which is what
            // stops a handler from claiming the signal first.
            signal::sigprocmask(signal::SIG_BLOCK, bit(signal::SIGUSR1).unwrap())
                .expect("blocking SIGUSR1");
            signal::raise_signal(signal::SIGUSR1).expect("raising SIGUSR1");

            let wanted = set_of(&[signal::SIGUSR1]);
            let mut info = SigInfo::from_signal(0);
            let timeout = TimeSpec {
                tv_sec: 5,
                tv_nsec: 0,
            };
            // SAFETY: all three arguments are live locals of the right types.
            let result = unsafe {
                kinakaze_abi_sigtimedwait(&raw const wanted, &raw mut info, &raw const timeout)
            };
            signal::sigprocmask(SIG_SETMASK, 0).expect("clearing the mask");
            (result, info)
        });

        assert_eq!(
            outcome.0,
            signal::SIGUSR1,
            "sigtimedwait must return the accepted signal number"
        );
        assert_eq!(
            OBSERVED.load(Ordering::SeqCst),
            0,
            "an accepted signal must not run its handler"
        );
        // The siginfo_t is filled in with the signal and a user-origin code.
        assert_eq!(outcome.1.si_signo, signal::SIGUSR1);
        assert_eq!(outcome.1.si_errno, 0);
        assert_eq!(outcome.1.si_code, SI_USER);
        assert_eq!(
            signal::pending() & bit(signal::SIGUSR1).unwrap(),
            0,
            "the accepted signal must no longer be pending"
        );
    }

    #[test]
    fn sigtimedwait_preserves_timer_payload_and_disposition() {
        let _lock = serialized();
        let _restore = handled(signal::SIGUSR1);
        with_watchdog("sigtimedwait timer payload", || {
            let mask = bit(signal::SIGUSR1).unwrap();
            let previous_mask = signal::sigprocmask(signal::SIG_BLOCK, mask).unwrap();
            let before = signal::current_action(signal::SIGUSR1).unwrap();
            let overrun = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(-1));
            signal::queue_timer_signal(
                None,
                signal::SIGUSR1,
                419,
                0x1234_abcd_5678_9012,
                7,
                std::sync::Arc::clone(&overrun),
            )
            .unwrap();
            let wanted = set_of(&[signal::SIGUSR1]);
            let mut info = SigInfo::from_signal(0);
            let timeout = TimeSpec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            assert_eq!(
                unsafe { kinakaze_abi_sigtimedwait(&wanted, &mut info, &timeout) },
                signal::SIGUSR1
            );
            assert_eq!(info.si_code, -2);
            assert_eq!(
                i32::from_le_bytes(info.padding[4..8].try_into().unwrap()),
                419
            );
            assert_eq!(
                i32::from_le_bytes(info.padding[8..12].try_into().unwrap()),
                7
            );
            assert_eq!(
                u64::from_le_bytes(info.padding[12..20].try_into().unwrap()),
                0x1234_abcd_5678_9012
            );
            assert_eq!(overrun.load(Ordering::Acquire), 7);
            assert_eq!(
                signal::current_action(signal::SIGUSR1).unwrap().flags,
                before.flags
            );
            assert_eq!(OBSERVED.load(Ordering::SeqCst), 0);
            signal::sigprocmask(SIG_SETMASK, previous_mask).unwrap();
        });
    }

    #[test]
    fn sigtimedwait_reports_eagain_when_the_timeout_expires() {
        let _lock = serialized();
        let outcome = with_watchdog("sigtimedwait timing out", || {
            let wanted = set_of(&[signal::SIGUSR2]);
            // A short but nonzero timeout, so the deadline arithmetic is
            // exercised rather than the immediate-poll shortcut.
            let timeout = TimeSpec {
                tv_sec: 0,
                tv_nsec: 20_000_000,
            };
            let started = Instant::now();
            // SAFETY: both arguments are live locals; a null `info` is allowed.
            let result = unsafe {
                kinakaze_abi_sigtimedwait(&raw const wanted, ptr::null_mut(), &raw const timeout)
            };
            (result, crate::kinakaze_errno(), started.elapsed())
        });

        assert_eq!(outcome.0, -1);
        assert_eq!(
            outcome.1, EAGAIN,
            "a timeout is EAGAIN, which is how a caller tells it from EINTR"
        );
        assert!(
            outcome.2 >= Duration::from_millis(15),
            "sigtimedwait returned before its timeout elapsed"
        );
    }

    #[test]
    fn sigtimedwait_rejects_an_empty_set_and_a_bad_timeout() {
        let _lock = serialized();
        // An empty set can never be satisfied, so waiting on one would hang.
        let empty = set_of(&[]);
        // SAFETY: `empty` is a live, readable sigset_t.
        assert_eq!(
            unsafe { kinakaze_abi_sigtimedwait(&raw const empty, ptr::null_mut(), ptr::null()) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), EINVAL);

        // SAFETY: passing null for the set is the case under test.
        assert_eq!(
            unsafe { kinakaze_abi_sigtimedwait(ptr::null(), ptr::null_mut(), ptr::null()) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), EFAULT);

        // An out-of-range nanosecond field is invalid.
        let wanted = set_of(&[signal::SIGUSR2]);
        let bad = TimeSpec {
            tv_sec: 0,
            tv_nsec: 1_000_000_000,
        };
        // SAFETY: both arguments are live locals.
        assert_eq!(
            unsafe {
                kinakaze_abi_sigtimedwait(&raw const wanted, ptr::null_mut(), &raw const bad)
            },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), EINVAL);
    }

    #[test]
    fn killpg_delivers_to_this_processs_own_group_and_reports_empty_ones() {
        let _lock = serialized();
        let _restore = handled(signal::SIGUSR1);
        let own_group = kinakaze_vfs::job::current_pgid();

        // Group 0 is the caller's own group, of which this process is a member,
        // so the signal really is delivered.
        assert_eq!(kinakaze_abi_killpg(0, signal::SIGUSR1), 0);
        assert_eq!(
            OBSERVED.load(Ordering::SeqCst),
            signal::SIGUSR1,
            "killpg(0, sig) must reach this process"
        );

        // Naming the group explicitly reaches the same members.
        OBSERVED.store(0, Ordering::SeqCst);
        assert_eq!(kinakaze_abi_killpg(own_group, signal::SIGUSR1), 0);
        assert_eq!(OBSERVED.load(Ordering::SeqCst), signal::SIGUSR1);

        // Signal 0 is an existence check: the group exists, nothing is sent.
        OBSERVED.store(0, Ordering::SeqCst);
        assert_eq!(kinakaze_abi_killpg(0, 0), 0);
        assert_eq!(OBSERVED.load(Ordering::SeqCst), 0);

        // A group nobody is in has no members, and saying so is more useful
        // than returning 0 for a signal that reached nothing.
        assert_eq!(kinakaze_abi_killpg(0x7fff_0010, signal::SIGUSR1), -1);
        assert_eq!(crate::kinakaze_errno(), ESRCH);

        // A negative group id is not a group.
        assert_eq!(kinakaze_abi_killpg(-1, signal::SIGUSR1), -1);
        assert_eq!(crate::kinakaze_errno(), EINVAL);

        // Nor is an out-of-range signal number.
        assert_eq!(kinakaze_abi_killpg(0, 9999), -1);
        assert_eq!(crate::kinakaze_errno(), EINVAL);
    }

    #[test]
    fn setpgrp_agrees_with_setpgid() {
        // `setpgrp` is defined as `setpgid(0, 0)`, so the two must agree on
        // every host state — including the ones where the request is refused.
        let _lock = serialized();
        assert_eq!(
            kinakaze_abi_setpgrp(),
            crate::userdb::kinakaze_abi_setpgid(0, 0),
            "setpgrp must be setpgid(0, 0)"
        );
    }

    /// Reads a `strsignal` result as a string.
    fn describe(signal_number: c_int) -> String {
        let text = kinakaze_abi_strsignal(signal_number);
        assert!(!text.is_null());
        // SAFETY: `strsignal` returns a NUL-terminated string.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn strsignal_gives_the_real_glibc_text() {
        // The exact strings glibc uses, because these are what a user reads when
        // a shell reports how a process died.
        assert_eq!(describe(signal::SIGHUP), "Hangup");
        assert_eq!(describe(signal::SIGINT), "Interrupt");
        assert_eq!(describe(signal::SIGQUIT), "Quit");
        assert_eq!(describe(signal::SIGILL), "Illegal instruction");
        assert_eq!(describe(5), "Trace/breakpoint trap");
        assert_eq!(describe(signal::SIGABRT), "Aborted");
        assert_eq!(describe(7), "Bus error");
        assert_eq!(describe(signal::SIGFPE), "Floating point exception");
        assert_eq!(describe(signal::SIGKILL), "Killed");
        assert_eq!(describe(signal::SIGUSR1), "User defined signal 1");
        assert_eq!(describe(signal::SIGSEGV), "Segmentation fault");
        assert_eq!(describe(signal::SIGUSR2), "User defined signal 2");
        assert_eq!(describe(signal::SIGPIPE), "Broken pipe");
        assert_eq!(describe(signal::SIGALRM), "Alarm clock");
        assert_eq!(describe(signal::SIGTERM), "Terminated");
        assert_eq!(describe(16), "Stack fault");
        assert_eq!(describe(signal::SIGCHLD), "Child exited");
        assert_eq!(describe(signal::SIGCONT), "Continued");
        assert_eq!(describe(signal::SIGSTOP), "Stopped (signal)");
        assert_eq!(describe(signal::SIGTSTP), "Stopped");
        assert_eq!(describe(21), "Stopped (tty input)");
        assert_eq!(describe(22), "Stopped (tty output)");
        assert_eq!(describe(signal::SIGWINCH), "Window changed");
        assert_eq!(describe(31), "Bad system call");
    }

    #[test]
    fn strsignal_formats_the_numbers_it_has_no_name_for() {
        // glibc numbers real-time signals from SIGRTMIN rather than absolutely.
        assert_eq!(describe(34), "Real-time signal 0");
        assert_eq!(describe(40), "Real-time signal 6");

        // Anything else gets the "Unknown signal" form, including zero and
        // negatives, which are not signals at all.
        assert_eq!(describe(0), "Unknown signal 0");
        assert_eq!(describe(999), "Unknown signal 999");
        assert_eq!(describe(-1), "Unknown signal -1");

        // The formatted result stays readable until the next call on this thread,
        // which is the contract glibc documents.
        let first = kinakaze_abi_strsignal(500);
        // SAFETY: the pointer is this thread's buffer, valid until the next call.
        assert_eq!(
            unsafe { CStr::from_ptr(first) }.to_bytes(),
            b"Unknown signal 500"
        );
    }

    #[test]
    fn sigdescr_np_returns_static_description_or_null() {
        unsafe {
            let hup = kinakaze_abi_sigdescr_np(signal::SIGHUP);
            assert!(!hup.is_null());
            assert_eq!(CStr::from_ptr(hup).to_bytes(), b"Hangup");

            let segv = kinakaze_abi_sigdescr_np(signal::SIGSEGV);
            assert!(!segv.is_null());
            assert_eq!(CStr::from_ptr(segv).to_bytes(), b"Segmentation fault");

            let sys = kinakaze_abi_sigdescr_np(31);
            assert!(!sys.is_null());
            assert_eq!(CStr::from_ptr(sys).to_bytes(), b"Bad system call");

            // Invalid or out-of-range signals return NULL
            assert!(kinakaze_abi_sigdescr_np(0).is_null());
            assert!(kinakaze_abi_sigdescr_np(-1).is_null());
            assert!(kinakaze_abi_sigdescr_np(32).is_null());
            assert!(kinakaze_abi_sigdescr_np(34).is_null());
            assert!(kinakaze_abi_sigdescr_np(999).is_null());
        }
    }

    #[test]
    fn sigabbrev_np_returns_abbreviation_or_null() {
        unsafe {
            let hup = kinakaze_abi_sigabbrev_np(signal::SIGHUP);
            assert!(!hup.is_null());
            assert_eq!(CStr::from_ptr(hup).to_bytes(), b"HUP");

            let int_sig = kinakaze_abi_sigabbrev_np(signal::SIGINT);
            assert!(!int_sig.is_null());
            assert_eq!(CStr::from_ptr(int_sig).to_bytes(), b"INT");

            let segv = kinakaze_abi_sigabbrev_np(signal::SIGSEGV);
            assert!(!segv.is_null());
            assert_eq!(CStr::from_ptr(segv).to_bytes(), b"SEGV");

            let sys = kinakaze_abi_sigabbrev_np(31);
            assert!(!sys.is_null());
            assert_eq!(CStr::from_ptr(sys).to_bytes(), b"SYS");

            // Invalid or out-of-range signals return NULL
            assert!(kinakaze_abi_sigabbrev_np(0).is_null());
            assert!(kinakaze_abi_sigabbrev_np(-1).is_null());
            assert!(kinakaze_abi_sigabbrev_np(32).is_null());
            assert!(kinakaze_abi_sigabbrev_np(34).is_null());
            assert!(kinakaze_abi_sigabbrev_np(999).is_null());
        }
    }

    #[test]
    fn sigaltstack_install_query_and_disable() {
        use super::{SS_DISABLE, Stack, kinakaze_abi_sigaltstack};

        let mut old_stack = Stack::default();
        assert_eq!(
            unsafe { kinakaze_abi_sigaltstack(ptr::null(), &mut old_stack) },
            0
        );
        assert_eq!(old_stack.ss_flags, SS_DISABLE);

        // Allocate a dedicated alternate signal stack buffer
        let mut buffer = vec![0u8; 8192];
        let new_stack = Stack {
            ss_sp: buffer.as_mut_ptr().cast(),
            ss_flags: 0,
            ss_size: buffer.len(),
        };

        let mut prev = Stack::default();
        assert_eq!(
            unsafe { kinakaze_abi_sigaltstack(&new_stack, &mut prev) },
            0
        );
        assert_eq!(prev.ss_flags, SS_DISABLE);

        // Query current state
        let mut cur = Stack::default();
        assert_eq!(
            unsafe { kinakaze_abi_sigaltstack(ptr::null(), &mut cur) },
            0
        );
        assert_eq!(cur.ss_sp, new_stack.ss_sp);
        assert_eq!(cur.ss_flags, 0);
        assert_eq!(cur.ss_size, 8192);

        // Disable stack
        let disable_stack = Stack {
            ss_sp: ptr::null_mut(),
            ss_flags: SS_DISABLE,
            ss_size: 0,
        };
        assert_eq!(
            unsafe { kinakaze_abi_sigaltstack(&disable_stack, ptr::null_mut()) },
            0
        );

        let mut final_check = Stack::default();
        assert_eq!(
            unsafe { kinakaze_abi_sigaltstack(ptr::null(), &mut final_check) },
            0
        );
        assert_eq!(final_check.ss_flags, SS_DISABLE);
    }

    #[test]
    fn sigaltstack_rejects_sub_minimum_size_or_null() {
        use super::{Stack, kinakaze_abi_sigaltstack};
        use kinakaze_vfs::EINVAL;

        let mut buf = [0u8; 1024];
        // Too small (< 2048 MINSIGSTKSZ)
        let too_small = Stack {
            ss_sp: buf.as_mut_ptr().cast(),
            ss_flags: 0,
            ss_size: 1024,
        };
        assert_eq!(
            unsafe { kinakaze_abi_sigaltstack(&too_small, ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        // Null pointer with size
        let null_sp = Stack {
            ss_sp: ptr::null_mut(),
            ss_flags: 0,
            ss_size: 4096,
        };
        assert_eq!(
            unsafe { kinakaze_abi_sigaltstack(&null_sp, ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }
}

// ---------------------------------------------------------------------------
// Alternate signal stack (sigaltstack).
// ---------------------------------------------------------------------------

pub use kinakaze_vfs::signal::SignalStack as Stack;

/// `SS_DISABLE`: flag indicating the alternate stack is disabled.
const SS_DISABLE: c_int = kinakaze_vfs::signal::SS_DISABLE;

/// `sigaltstack`: installs or queries the alternate signal stack for this thread.
///
/// Both `ss` and `old_ss` may be null. If `ss` is non-null it installs a new
/// alternate stack (or disables the existing one). If `old_ss` is non-null the
/// current configuration is written there before any change.
///
/// # Safety
///
/// `ss` must be null or point at a readable `stack_t`.
/// `old_ss` must be null or point at a writable `stack_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigaltstack(
    ss: *const Stack,
    old_ss: *mut Stack,
) -> c_int {
    let requested = if ss.is_null() {
        None
    } else {
        // SAFETY: caller guarantees readable storage.
        Some(unsafe { *ss })
    };
    match signal::sigaltstack(requested) {
        Ok(previous) => {
            if !old_ss.is_null() {
                // SAFETY: caller guarantees writable storage.
                unsafe { old_ss.write(previous) };
            }
            0
        }
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn sigaltstack(ss: *const Stack, old_ss: *mut Stack) -> c_int {
    unsafe { kinakaze_abi_sigaltstack(ss, old_ss) }
}
