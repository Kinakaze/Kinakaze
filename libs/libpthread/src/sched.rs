//! Guest thread scheduling backed by retained native thread handles.
//!
//! Linux ABI validation follows sched_setscheduler / sched_setparam:
//! https://codebrowser.dev/linux/linux/kernel/sched/syscalls.c.html
//! https://codebrowser.dev/glibc/glibc/nptl/pthread_setschedparam.c.html
//! Windows' normal class uses dynamic priority and time slicing:
//! https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setthreadpriority
//!
//! FIFO/RR require guarantees this native backend cannot supply. A legal request
//! for those policies returns EPERM without altering the native thread. BATCH,
//! IDLE and RESET_ON_FORK likewise need scheduler support before being enabled.

#![allow(clippy::missing_safety_doc)]

use super::{EAGAIN, EINVAL, EPERM, ESRCH};
use core::ptr;
use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, GetLastError,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, GetThreadPriority, SetThreadPriority, THREAD_PRIORITY_NORMAL,
    WaitForSingleObject,
};

pub const SCHED_OTHER: i32 = 0;
pub const SCHED_FIFO: i32 = 1;
pub const SCHED_RR: i32 = 2;
const SCHED_BATCH: i32 = 3;
const SCHED_IDLE: i32 = 5;
const SCHED_RESET_ON_FORK: i32 = 0x4000_0000;
const THREAD_PRIORITY_ERROR_RETURN: i32 = i32::MAX;

/// Linux sched_param contains one C int, including on x86-64.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SchedParam {
    pub sched_priority: i32,
}

// This registry also covers detached threads and threads claimed by a joiner;
// the joinability registry intentionally cannot represent either lifetime.
static LIVE: OnceLock<Mutex<HashMap<usize, Arc<OwnedHandle>>>> = OnceLock::new();

fn live() -> &'static Mutex<HashMap<usize, Arc<OwnedHandle>>> {
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn register_created(id: usize, handle: Arc<OwnedHandle>) {
    live()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id, handle);
}

pub(super) fn register_current(id: usize) -> Result<(), i32> {
    let mut registry = live().lock().unwrap_or_else(PoisonError::into_inner);
    if registry.contains_key(&id) {
        return Ok(());
    }
    // Duplicate the pseudo handle into a real owning handle before sharing it
    // with another thread. A raw native TID could be reused after thread exit.
    let borrowed = unsafe { BorrowedHandle::borrow_raw(GetCurrentThread()) };
    let handle = borrowed.try_clone_to_owned().map_err(|_| EAGAIN)?;
    registry.insert(id, Arc::new(handle));
    Ok(())
}

pub(super) fn unregister(id: usize) {
    live()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&id);
}

pub(super) fn reset_after_fork(id: usize) -> Result<(), i32> {
    // Parent native handles have no identity in the new worker. All permitted
    // policies are currently OTHER/0, which the new native thread inherits.
    live()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    register_current(id)
}

fn native_error() -> i32 {
    match unsafe { GetLastError() } {
        ERROR_ACCESS_DENIED => EPERM,
        ERROR_INVALID_HANDLE => ESRCH,
        ERROR_INVALID_PARAMETER => EINVAL,
        _ => EPERM,
    }
}

fn with_live_thread(id: usize, action: impl FnOnce(&OwnedHandle) -> i32) -> i32 {
    let current = super::pthread_self();
    if id == current {
        if let Err(error) = register_current(id) {
            return error;
        }
    }
    let mut registry = live().lock().unwrap_or_else(PoisonError::into_inner);
    let Some(handle) = registry.get(&id) else {
        return ESRCH;
    };
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => {
            registry.remove(&id);
            ESRCH
        }
        WAIT_TIMEOUT => action(handle),
        _ => native_error(),
    }
}

/// Clock IDs are opaque Linux-style per-thread clock encodings. Retain the
/// native handle while validating the ID so detached/exiting threads are safe.
pub fn cpu_clock(id: usize) -> Result<i32, i32> {
    let mut clock = 0;
    let error = with_live_thread(id, |handle| {
        let tid =
            unsafe { windows_sys::Win32::System::Threading::GetThreadId(handle.as_raw_handle()) };
        if tid == 0 {
            return ESRCH;
        }
        if tid > 0x0fff_ffff {
            return EAGAIN;
        }
        clock = (!(tid as i32) << 3) | 6;
        0
    });
    if error == 0 { Ok(clock) } else { Err(error) }
}

fn validate(policy: i32, priority: i32) -> Result<(), i32> {
    let base = policy & !SCHED_RESET_ON_FORK;
    let valid = match base {
        SCHED_OTHER | SCHED_BATCH | SCHED_IDLE => priority == 0,
        SCHED_FIFO | SCHED_RR => (1..=99).contains(&priority),
        _ => false,
    };
    if !valid {
        return Err(EINVAL);
    }
    if policy != SCHED_OTHER {
        return Err(EPERM);
    }
    Ok(())
}

fn apply(handle: &OwnedHandle, policy: i32, priority: i32) -> i32 {
    if let Err(error) = validate(policy, priority) {
        return error;
    }
    if unsafe { SetThreadPriority(handle.as_raw_handle(), THREAD_PRIORITY_NORMAL) } == 0 {
        return native_error();
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_setschedparam(
    thread: usize,
    policy: i32,
    param: *const SchedParam,
) -> i32 {
    if param.is_null() {
        return EINVAL;
    }
    let priority = unsafe { ptr::addr_of!((*param).sched_priority).read_unaligned() };
    with_live_thread(thread, |handle| apply(handle, policy, priority))
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_getschedparam(
    thread: usize,
    policy: *mut i32,
    param: *mut SchedParam,
) -> i32 {
    if policy.is_null() || param.is_null() {
        return EINVAL;
    }
    with_live_thread(thread, |handle| {
        // Query the host to validate the native scheduling object. Linux normal
        // policy's sched_priority remains zero irrespective of nice/boosts.
        if unsafe { GetThreadPriority(handle.as_raw_handle()) } == THREAD_PRIORITY_ERROR_RETURN {
            return native_error();
        }
        unsafe {
            policy.write_unaligned(SCHED_OTHER);
            param.write_unaligned(SchedParam { sched_priority: 0 });
        }
        0
    })
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_setschedprio(thread: usize, priority: i32) -> i32 {
    with_live_thread(thread, |handle| apply(handle, SCHED_OTHER, priority))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;
    use std::sync::mpsc;
    use windows_sys::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL;

    struct NativePriority(i32);
    impl NativePriority {
        fn save() -> Self {
            Self(unsafe { GetThreadPriority(GetCurrentThread()) })
        }
    }
    impl Drop for NativePriority {
        fn drop(&mut self) {
            unsafe { SetThreadPriority(GetCurrentThread(), self.0) };
        }
    }

    #[test]
    fn calling_thread_uses_real_host_priority_and_linux_query() {
        let _restore = NativePriority::save();
        let me = super::super::pthread_self();
        assert_ne!(
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) },
            0
        );
        kinakaze_tls::set_errno(123);
        assert_eq!(pthread_setschedprio(me, 0), 0);
        assert_eq!(
            unsafe { GetThreadPriority(GetCurrentThread()) },
            THREAD_PRIORITY_NORMAL
        );
        let mut policy = -1;
        let mut param = SchedParam { sched_priority: -1 };
        assert_eq!(
            unsafe { pthread_getschedparam(me, &mut policy, &mut param) },
            0
        );
        assert_eq!((policy, param.sched_priority), (SCHED_OTHER, 0));
        assert_eq!(size_of::<SchedParam>(), 4);
        assert_eq!(align_of::<SchedParam>(), 4);
        assert_eq!(kinakaze_tls::errno(), 123);
    }

    #[test]
    fn validation_realtime_denial_and_missing_threads_preserve_state() {
        let _restore = NativePriority::save();
        let me = super::super::pthread_self();
        assert_eq!(pthread_setschedprio(me, 0), 0);
        for (policy, priority, expected) in [
            (SCHED_OTHER, 1, EINVAL),
            (SCHED_OTHER, -1, EINVAL),
            (SCHED_FIFO, 0, EINVAL),
            (SCHED_FIFO, 100, EINVAL),
            (SCHED_RR, -1, EINVAL),
            (4, 0, EINVAL),
            (6, 0, EINVAL),
            (-1, 0, EINVAL),
            (SCHED_FIFO, 1, EPERM),
            (SCHED_FIFO, 99, EPERM),
            (SCHED_RR, 50, EPERM),
            (SCHED_BATCH, 0, EPERM),
            (SCHED_IDLE, 0, EPERM),
            (SCHED_RESET_ON_FORK, 0, EPERM),
        ] {
            assert_eq!(
                unsafe {
                    pthread_setschedparam(
                        me,
                        policy,
                        &SchedParam {
                            sched_priority: priority,
                        },
                    )
                },
                expected
            );
            assert_eq!(
                unsafe { GetThreadPriority(GetCurrentThread()) },
                THREAD_PRIORITY_NORMAL
            );
        }
        let mut policy = -1;
        let mut param = SchedParam { sched_priority: -1 };
        assert_eq!(
            unsafe { pthread_getschedparam(0, &mut policy, &mut param) },
            ESRCH
        );
        assert_eq!((policy, param.sched_priority), (-1, -1));
        assert_eq!(
            unsafe { pthread_setschedparam(usize::MAX, SCHED_OTHER, &SchedParam::default()) },
            ESRCH
        );
        assert_eq!(pthread_setschedprio(0, 0), ESRCH);
        assert_eq!(
            unsafe { pthread_setschedparam(me, SCHED_OTHER, ptr::null()) },
            EINVAL
        );
        assert_eq!(
            unsafe { pthread_getschedparam(me, ptr::null_mut(), &mut param) },
            EINVAL
        );
        assert_eq!(
            unsafe { pthread_getschedparam(me, &mut policy, ptr::null_mut()) },
            EINVAL
        );
    }

    struct LivePacket {
        main: usize,
        ready: mpsc::SyncSender<i32>,
        release: mpsc::Receiver<()>,
    }

    unsafe extern "sysv64" fn live_thread(argument: *mut c_void) -> *mut c_void {
        let packet = unsafe { Box::from_raw(argument.cast::<LivePacket>()) };
        let mut policy = -1;
        let mut param = SchedParam { sched_priority: -1 };
        let result = unsafe { pthread_getschedparam(packet.main, &mut policy, &mut param) };
        packet
            .ready
            .send(if result == 0 && (policy, param.sched_priority) == (0, 0) {
                0
            } else {
                -1
            })
            .unwrap();
        packet.release.recv().unwrap();
        ptr::null_mut()
    }

    #[test]
    fn created_threads_and_foreign_main_remain_addressable_until_exit() {
        let main = super::super::pthread_self();
        let (ready, ready_rx) = mpsc::sync_channel(0);
        let (release, release_rx) = mpsc::channel();
        let packet = Box::into_raw(Box::new(LivePacket {
            main,
            ready,
            release: release_rx,
        }));
        let mut thread = 0;
        assert_eq!(
            unsafe {
                super::super::pthread_create(
                    &mut thread,
                    ptr::null(),
                    Some(live_thread),
                    packet.cast(),
                )
            },
            0
        );
        assert_eq!(ready_rx.recv().unwrap(), 0);
        assert_eq!(
            unsafe { pthread_setschedparam(thread, 0, &SchedParam::default()) },
            0
        );
        let mut policy = -1;
        let mut param = SchedParam::default();
        assert_eq!(
            unsafe { pthread_getschedparam(thread, &mut policy, &mut param) },
            0
        );
        assert_eq!(policy, 0);
        release.send(()).unwrap();
        assert_eq!(
            unsafe { super::super::pthread_join(thread, ptr::null_mut()) },
            0
        );
        assert_eq!(
            unsafe { pthread_getschedparam(thread, &mut policy, &mut param) },
            ESRCH
        );
        assert_eq!(pthread_setschedprio(thread, 0), ESRCH);
    }

    #[test]
    fn detached_native_handle_is_kept_live_for_scheduling() {
        let main = super::super::pthread_self();
        let (ready, ready_rx) = mpsc::sync_channel(0);
        let (release, release_rx) = mpsc::channel();
        let packet = Box::into_raw(Box::new(LivePacket {
            main,
            ready,
            release: release_rx,
        }));
        let mut thread = 0;
        assert_eq!(
            unsafe {
                super::super::pthread_create(
                    &mut thread,
                    ptr::null(),
                    Some(live_thread),
                    packet.cast(),
                )
            },
            0
        );
        assert_eq!(ready_rx.recv().unwrap(), 0);
        assert_eq!(super::super::pthread_detach(thread), 0);
        assert_eq!(pthread_setschedprio(thread, 0), 0);
        let handle = live().lock().unwrap().get(&thread).unwrap().clone();
        release.send(()).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 5000) },
            WAIT_OBJECT_0
        );
        assert_eq!(pthread_setschedprio(thread, 0), ESRCH);
        assert!(!live().lock().unwrap().contains_key(&thread));
    }
}
