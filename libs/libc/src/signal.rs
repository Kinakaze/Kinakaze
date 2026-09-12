//! Signal C ABI exports.

use core::ffi::c_int;
use core::ptr;

use kinakaze_vfs::signal::{
    self, Action, Disposition, Handler, SIG_BLOCK, SIG_DFL, SIG_ERR, SIG_IGN, SIG_SETMASK,
    SIG_UNBLOCK,
};

/// The Linux x86_64 `sigset_t`: 128 bytes, of which only the low word is used
/// for the 64 supported signals.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SigSet {
    pub bits: [u64; 16],
}

/// The Linux x86_64 `struct sigaction`.
///
/// Field order is ABI. `sa_restorer` is unused here but must occupy its slot so
/// guest code compiled against the real layout stays compatible.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SigAction {
    pub sa_handler: usize,
    pub sa_mask: SigSet,
    pub sa_flags: i32,
    pub _padding: i32,
    pub sa_restorer: usize,
}

/// Converts a guest handler word into a disposition.
pub(crate) fn disposition_of(handler: usize, flags: i32) -> Disposition {
    match handler {
        SIG_DFL => Disposition::Default,
        SIG_IGN => Disposition::Ignore,
        // SAFETY: the guest promises this address is a signal handler with the
        // System V signature; installing it is the documented contract of
        // `signal` and `sigaction`.
        address => Disposition::Handle(
            unsafe { core::mem::transmute::<usize, Handler>(address) },
            flags,
        ),
    }
}

/// Converts a disposition back into the guest's handler word.
pub(crate) fn handler_of(disposition: Disposition) -> usize {
    match disposition {
        Disposition::Default => SIG_DFL,
        Disposition::Ignore => SIG_IGN,
        Disposition::Handle(handler, _) => handler as usize,
    }
}

/// `signal`, which returns the previous handler or `SIG_ERR`.
///
/// # Safety
///
/// `handler` must be `SIG_DFL`, `SIG_IGN`, or a valid handler address.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_signal(signal: c_int, handler: usize) -> usize {
    ensure_terminate_hook();
    // BSD semantics: `signal` implies SA_RESTART, which is what glibc does and
    // what programs written against `signal` expect.
    let action = Action {
        disposition: disposition_of(handler, signal::SA_RESTART),
        flags: signal::SA_RESTART,
        mask: 0,
        restorer: 0,
    };
    match signal::sigaction(signal, Some(action)) {
        Ok(previous) => handler_of(previous.disposition),
        Err(error) => {
            crate::set_errno(error);
            SIG_ERR
        }
    }
}

/// System V signal semantics reset the handler and leave it unblocked while
/// executing. FFmpeg explicitly imports this interface instead of BSD signal.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___sysv_signal(number: c_int, handler: usize) -> usize {
    ensure_terminate_hook();
    let flags = signal::SA_RESETHAND | signal::SA_NODEFER;
    let action = Action {
        disposition: disposition_of(handler, flags),
        flags,
        mask: 0,
        restorer: 0,
    };
    match signal::sigaction(number, Some(action)) {
        Ok(previous) => handler_of(previous.disposition),
        Err(error) => {
            crate::set_errno(error);
            SIG_ERR
        }
    }
}

/// System V sigset preserves the handler while holding a signal and returns
/// SIG_HOLD when the previous thread mask blocked it.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigset(number: c_int, handler: usize) -> usize {
    const HOLD: usize = 2;
    if !(1..=64).contains(&number) || handler == SIG_ERR {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return SIG_ERR;
    }
    ensure_terminate_hook();
    let bit = 1u64 << (number - 1);
    let result = (|| {
        if handler == HOLD {
            let mask = signal::sigprocmask(SIG_BLOCK, bit)?;
            if mask & bit != 0 {
                return Ok(HOLD);
            }
            Ok(handler_of(signal::sigaction(number, None)?.disposition))
        } else {
            let previous = signal::sigaction(
                number,
                Some(Action {
                    disposition: disposition_of(handler, 0),
                    flags: 0,
                    mask: 0,
                    restorer: 0,
                }),
            )?;
            let mask = signal::sigprocmask(SIG_UNBLOCK, bit)?;
            Ok(if mask & bit != 0 {
                HOLD
            } else {
                handler_of(previous.disposition)
            })
        }
    })();
    match result {
        Ok(value) => value,
        Err(error) => {
            crate::set_errno(error);
            SIG_ERR
        }
    }
}

/// `sigaction`.
///
/// # Safety
///
/// `action` and `old_action` must be null or point to valid `struct sigaction`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigaction(
    signal_number: c_int,
    action: *const SigAction,
    old_action: *mut SigAction,
) -> c_int {
    ensure_terminate_hook();
    let requested = if action.is_null() {
        None
    } else {
        // SAFETY: the caller guarantees a readable struct sigaction.
        let action = unsafe { &*action };
        let flags = action.sa_flags;
        Some(Action {
            disposition: disposition_of(action.sa_handler, flags),
            flags,
            mask: action.sa_mask.bits[0],
            restorer: action.sa_restorer,
        })
    };

    match signal::sigaction(signal_number, requested) {
        Ok(previous) => {
            if !old_action.is_null() {
                let mut mask = SigSet { bits: [0; 16] };
                mask.bits[0] = previous.mask;
                // SAFETY: the caller guarantees a writable struct sigaction.
                unsafe {
                    ptr::write(
                        old_action,
                        SigAction {
                            sa_handler: handler_of(previous.disposition),
                            sa_mask: mask,
                            sa_flags: previous.flags,
                            _padding: 0,
                            sa_restorer: previous.restorer,
                        },
                    )
                };
            }
            0
        }
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// Terminates the process the way a shell reports a fatal signal.
///
/// A process killed by signal N exits with status `128 + N`, which is the
/// convention every shell and test harness reads.
///
/// The exit code alone cannot tell a parent's `waitpid` whether the process
/// chose that number or a signal imposed it — `exit(130)` and death by `SIGINT`
/// look identical from outside. The fact is therefore published to the shared
/// job registry first, where the parent reads it to build a `WIFSIGNALED`
/// status. That has to happen before the exit, because there is no "after".
fn terminate_from_signal(signal_number: i32) {
    crate::process::terminate_from_signal(signal_number);
}

/// Installs the default-action hook. Raw syscalls need it even when the guest
/// never calls a libc signal wrapper or installs a handler.
fn ensure_terminate_hook() {
    signal::set_terminate_hook(terminate_from_signal);
}

#[cfg(windows)]
extern "C" fn initialize_signal_termination() {
    ensure_terminate_hook();
}

#[cfg(windows)]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static SIGNAL_TERMINATION_INITIALIZER: extern "C" fn() = initialize_signal_termination;

/// `raise`, which sends a signal to the calling process.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_raise(signal_number: c_int) -> c_int {
    ensure_terminate_hook();
    match signal::raise_signal(signal_number) {
        Ok(()) => {
            // A signal raised by the thread itself is delivered before `raise`
            // returns, which is what POSIX requires.
            signal::deliver_pending();
            0
        }
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// Loader exception bridge for hardware-generated Linux signals.
///
/// `sigaction` state lives in this provider's VFS instance.  The executable
/// loader deliberately reaches it through this exported ABI instead of calling
/// its separately linked copy, so the disposition observed here is exactly the
/// one the guest installed through libc.
///
/// # Safety
///
/// `siginfo` and `context` must be live Linux-ABI records for the duration of
/// the synchronously invoked handler.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_deliver_synchronous_signal(
    signal_number: c_int,
    siginfo: *mut core::ffi::c_void,
    context: *mut core::ffi::c_void,
) -> c_int {
    ensure_terminate_hook();
    i32::from(unsafe { signal::deliver_synchronous(signal_number, siginfo, context) })
}

/// `kill`, in all four of its target forms.
///
/// `pid > 0` names one process, `pid == 0` the caller's own process group,
/// `pid < -1` the group `-pid`, and `pid == -1` every process the caller may
/// signal. Delivery goes through the shared job registry in
/// [`kinakaze_vfs::job`], so a signal really does cross into another Windows
/// process and run that process's Linux handler.
///
/// # Divergence
///
/// `pid == -1` reaches every *hosted* process rather than every process on the
/// machine. A native Windows process has no Linux signal disposition to run, so
/// including it would mean either lying about delivery or terminating it under
/// a rule its author never agreed to.
///
/// A signal of zero is the existence check and delivers nothing, which is what a
/// shell uses to ask whether a job is still alive.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_kill(pid: c_int, signal_number: c_int) -> c_int {
    ensure_terminate_hook();
    match kinakaze_vfs::job::kill(pid, signal_number) {
        Ok(()) => {
            // A signal the caller sent to itself is delivered before `kill`
            // returns, exactly as `raise` promises. Anything aimed elsewhere has
            // already been posted to that process's slot.
            signal::deliver_pending();
            0
        }
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `sigprocmask`.
///
/// # Safety
///
/// Both set pointers must be null or point to a valid `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigprocmask(
    how: c_int,
    set: *const SigSet,
    old_set: *mut SigSet,
) -> c_int {
    // A null `set` is a query, which SIG_SETMASK of the current mask performs
    // without changing anything.
    let (how, mask) = if set.is_null() {
        (SIG_SETMASK, signal::blocked_mask())
    } else {
        // SAFETY: the caller guarantees a readable sigset_t.
        (how, unsafe { (*set).bits[0] })
    };

    match signal::sigprocmask(how, mask) {
        Ok(previous) => {
            if !old_set.is_null() {
                let mut bits = [0u64; 16];
                bits[0] = previous;
                // SAFETY: the caller guarantees a writable sigset_t.
                unsafe { ptr::write(old_set, SigSet { bits }) };
            }
            // Unblocking may have made a signal deliverable.
            signal::deliver_pending();
            0
        }
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn sigprocmask(
    how: c_int,
    set: *const SigSet,
    old_set: *mut SigSet,
) -> c_int {
    unsafe { kinakaze_abi_sigprocmask(how, set, old_set) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn sigsetmask(mask: c_int) -> c_int {
    unsafe { kinakaze_abi_sigsetmask(mask) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigsetmask(mask: c_int) -> c_int {
    let mut bits = [0u64; 16];
    bits[0] = (mask as u32) as u64;
    let new_set = SigSet { bits };
    let mut old_set = SigSet { bits: [0u64; 16] };
    unsafe {
        kinakaze_abi_sigprocmask(SIG_SETMASK, &new_set, &mut old_set);
    }
    old_set.bits[0] as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn sigblock(mask: c_int) -> c_int {
    unsafe { kinakaze_abi_sigblock(mask) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigblock(mask: c_int) -> c_int {
    let mut bits = [0u64; 16];
    bits[0] = (mask as u32) as u64;
    let new_set = SigSet { bits };
    let mut old_set = SigSet { bits: [0u64; 16] };
    unsafe {
        kinakaze_abi_sigprocmask(SIG_BLOCK, &new_set, &mut old_set);
    }
    old_set.bits[0] as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn siggetmask() -> c_int {
    unsafe { kinakaze_abi_siggetmask() }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_siggetmask() -> c_int {
    let mut old_set = SigSet { bits: [0u64; 16] };
    unsafe {
        kinakaze_abi_sigprocmask(SIG_SETMASK, ptr::null(), &mut old_set);
    }
    old_set.bits[0] as c_int
}

/// `sigpending`.
///
/// # Safety
///
/// `set` must point to a writable `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigpending(set: *mut SigSet) -> c_int {
    if set.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let mut bits = [0u64; 16];
    bits[0] = signal::pending();
    // SAFETY: the caller guarantees a writable sigset_t.
    unsafe { ptr::write(set, SigSet { bits }) };
    0
}

/// Fills a set with no signals.
///
/// # Safety
///
/// `set` must point to a writable `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigemptyset(set: *mut SigSet) -> c_int {
    if set.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a writable sigset_t.
    unsafe { ptr::write(set, SigSet { bits: [0; 16] }) };
    0
}

/// Fills a set with every signal.
///
/// # Safety
///
/// `set` must point to a writable `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigfillset(set: *mut SigSet) -> c_int {
    if set.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a writable sigset_t.
    unsafe {
        ptr::write(
            set,
            SigSet {
                bits: [u64::MAX; 16],
            },
        )
    };
    0
}

/// Adds one signal to a set.
///
/// # Safety
///
/// `set` must point to a valid `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigaddset(
    set: *mut SigSet,
    signal_number: c_int,
) -> c_int {
    let Some(bit) = signal_bit(signal_number) else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    };
    if set.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a valid sigset_t.
    unsafe { (*set).bits[0] |= bit };
    0
}

/// Removes one signal from a set.
///
/// # Safety
///
/// `set` must point to a valid `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigdelset(
    set: *mut SigSet,
    signal_number: c_int,
) -> c_int {
    let Some(bit) = signal_bit(signal_number) else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    };
    if set.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a valid sigset_t.
    unsafe { (*set).bits[0] &= !bit };
    0
}

/// Tests whether a signal is in a set.
///
/// # Safety
///
/// `set` must point to a valid `sigset_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigismember(
    set: *const SigSet,
    signal_number: c_int,
) -> c_int {
    let Some(bit) = signal_bit(signal_number) else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    };
    if set.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable sigset_t.
    c_int::from(unsafe { (*set).bits[0] } & bit != 0)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sigisemptyset(set: *const SigSet) -> c_int {
    if set.is_null() {
        return 1;
    }
    let s = unsafe { &*set };
    if s.bits.iter().all(|&b| b == 0) { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn sigisemptyset(set: *const SigSet) -> c_int {
    unsafe { kinakaze_abi_sigisemptyset(set) }
}

/// Returns the mask bit for a signal number.
fn signal_bit(signal_number: c_int) -> Option<u64> {
    if signal_number <= 0 || signal_number as usize >= signal::NSIG {
        return None;
    }
    Some(1u64 << (signal_number as u64 - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigset_manipulation_operations() {
        let mut set: SigSet = unsafe { core::mem::zeroed() };
        assert_eq!(unsafe { kinakaze_abi_sigemptyset(&mut set) }, 0);
        assert_eq!(set.bits[0], 0);

        // Add SIGINT (2) and SIGTERM (15)
        assert_eq!(unsafe { kinakaze_abi_sigaddset(&mut set, 2) }, 0);
        assert_eq!(unsafe { kinakaze_abi_sigaddset(&mut set, 15) }, 0);

        assert_eq!(unsafe { kinakaze_abi_sigismember(&set, 2) }, 1);
        assert_eq!(unsafe { kinakaze_abi_sigismember(&set, 15) }, 1);
        assert_eq!(unsafe { kinakaze_abi_sigismember(&set, 9) }, 0);

        // Delete SIGINT
        assert_eq!(unsafe { kinakaze_abi_sigdelset(&mut set, 2) }, 0);
        assert_eq!(unsafe { kinakaze_abi_sigismember(&set, 2) }, 0);
        assert_eq!(unsafe { kinakaze_abi_sigismember(&set, 15) }, 1);

        // Fill set
        assert_eq!(unsafe { kinakaze_abi_sigfillset(&mut set) }, 0);
        assert_eq!(unsafe { kinakaze_abi_sigismember(&set, 2) }, 1);
        assert_eq!(unsafe { kinakaze_abi_sigismember(&set, 64) }, 1);

        // Invalid signals (< 1 or > 64) return EINVAL
        assert_eq!(unsafe { kinakaze_abi_sigaddset(&mut set, 0) }, -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EINVAL);
        assert_eq!(unsafe { kinakaze_abi_sigaddset(&mut set, 65) }, -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EINVAL);
    }

    #[test]
    fn sigaction_disposition_roundtrip_and_masks() {
        unsafe extern "sysv64" fn dummy_handler(_sig: c_int) {}

        let act = SigAction {
            sa_handler: dummy_handler as usize,
            sa_mask: SigSet { bits: [0; 16] },
            sa_flags: kinakaze_vfs::signal::SA_RESTART,
            _padding: 0,
            sa_restorer: 0x1234_5678,
        };
        let mut old_act: SigAction = unsafe { core::mem::zeroed() };

        // Install dummy_handler for SIGUSR1 (10)
        assert_eq!(unsafe { kinakaze_abi_sigaction(10, &act, &mut old_act) }, 0);

        // Query again and verify old_act matches dummy_handler
        let mut cur_act: SigAction = unsafe { core::mem::zeroed() };
        assert_eq!(
            unsafe { kinakaze_abi_sigaction(10, ptr::null(), &mut cur_act) },
            0
        );
        assert_eq!(cur_act.sa_handler, dummy_handler as usize);
        assert_eq!(cur_act.sa_flags, kinakaze_vfs::signal::SA_RESTART);
        assert_eq!(cur_act.sa_restorer, 0x1234_5678);

        // Restore SIG_DFL
        let dfl_act = SigAction {
            sa_handler: SIG_DFL,
            sa_mask: SigSet { bits: [0; 16] },
            sa_flags: 0,
            _padding: 0,
            sa_restorer: 0,
        };
        assert_eq!(
            unsafe { kinakaze_abi_sigaction(10, &dfl_act, ptr::null_mut()) },
            0
        );
    }

    #[test]
    fn sigprocmask_block_unblock_and_query() {
        let mut block_set: SigSet = unsafe { core::mem::zeroed() };
        unsafe { kinakaze_abi_sigaddset(&mut block_set, 10) }; // SIGUSR1

        let mut prev_set: SigSet = unsafe { core::mem::zeroed() };
        assert_eq!(
            unsafe { kinakaze_abi_sigprocmask(SIG_BLOCK, &block_set, &mut prev_set) },
            0
        );

        let mut cur_mask: SigSet = unsafe { core::mem::zeroed() };
        assert_eq!(
            unsafe { kinakaze_abi_sigprocmask(SIG_BLOCK, ptr::null(), &mut cur_mask) },
            0
        );
        assert_eq!(unsafe { kinakaze_abi_sigismember(&cur_mask, 10) }, 1);

        // Unblock SIGUSR1
        assert_eq!(
            unsafe { kinakaze_abi_sigprocmask(SIG_UNBLOCK, &block_set, ptr::null_mut()) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_sigprocmask(SIG_BLOCK, ptr::null(), &mut cur_mask) },
            0
        );
        assert_eq!(unsafe { kinakaze_abi_sigismember(&cur_mask, 10) }, 0);
    }
}
