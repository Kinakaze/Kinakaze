//! POSIX semaphore tokens live in guest memory; contended waits share futex queues.
use std::sync::atomic::{AtomicU64, Ordering};

use kinakaze_vfs::{EAGAIN, EINTR, EINVAL, EOVERFLOW, ETIMEDOUT};
use libpthread::Timespec;

mod named;
pub use named::{kinakaze_abi_sem_close, kinakaze_abi_sem_open, kinakaze_abi_sem_unlink};

const WAITER: u64 = 1 << 32;
const VALUE_MAX: u32 = i32::MAX as u32;

/// Linux x86_64 sem_t. Combining tokens and waiters lets sem_post stop touching
/// guest storage once it publishes the last token: its consumer may destroy it.
#[repr(C, align(8))]
pub struct SemT {
    data: AtomicU64,
    private: u32,
    reserved: [u32; 5],
}

impl SemT {
    pub(crate) const fn new(value: u32, shared: bool) -> Self {
        Self {
            data: AtomicU64::new(value as u64),
            private: if shared { 0 } else { 128 },
            reserved: [0; 5],
        }
    }

    fn take(&self, waiter: bool) -> bool {
        self.data
            .fetch_update(Ordering::Acquire, Ordering::Relaxed, |data| {
                (data as u32 != 0).then(|| data - 1 - if waiter { WAITER } else { 0 })
            })
            .is_ok()
    }

    fn key(&self) -> Result<crate::futex::Key, i32> {
        if self.private != 0 {
            crate::futex::Key::private(self as *const Self as usize)
        } else {
            crate::fdio::futex_key(self as *const Self as usize)
        }
    }
}

fn failed(error: i32) -> i32 {
    crate::set_errno(error);
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_init(
    sem: *mut SemT,
    pshared: i32,
    value: u32,
) -> i32 {
    if sem.is_null() || value > VALUE_MAX {
        return failed(EINVAL);
    }
    unsafe { sem.write(SemT::new(value, pshared != 0)) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_destroy(sem: *mut SemT) -> i32 {
    if sem.is_null() { failed(EINVAL) } else { 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_post(sem: *mut SemT) -> i32 {
    let Some(sem) = (unsafe { sem.as_ref() }) else {
        return failed(EINVAL);
    };
    let mut data = sem.data.load(Ordering::Relaxed);
    loop {
        if data as u32 == VALUE_MAX {
            return failed(EOVERFLOW);
        }
        // Resolve before publishing, while the object is still guaranteed live.
        // The uncontended path needs neither a kernel object nor an IPC call.
        let wake = if data >> 32 != 0 {
            match sem.key() {
                Ok(key) => Some(key),
                Err(error) => return failed(error),
            }
        } else {
            None
        };
        match sem
            .data
            .compare_exchange_weak(data, data + 1, Ordering::Release, Ordering::Relaxed)
        {
            Ok(_) => {
                if let Some(key) = wake {
                    // A concurrent final consumer can remove its wait record.
                    // Token publication succeeds even when there is nobody left.
                    if let Err(error) = crate::futex::wake(key, 1, u32::MAX) {
                        return failed(error);
                    }
                }
                return 0;
            }
            Err(current) => data = current,
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_trywait(sem: *mut SemT) -> i32 {
    let Some(sem) = (unsafe { sem.as_ref() }) else {
        return failed(EINVAL);
    };
    if sem.take(false) { 0 } else { failed(EAGAIN) }
}

struct Registered<'a>(&'a SemT, bool);
impl Drop for Registered<'_> {
    fn drop(&mut self) {
        if self.1 {
            self.0.data.fetch_sub(WAITER, Ordering::Relaxed);
        }
    }
}

unsafe fn wait(sem: *mut SemT, deadline: Option<(i32, Timespec)>) -> i32 {
    libpthread::pthread_testcancel();
    let Some(sem) = (unsafe { sem.as_ref() }) else {
        return failed(EINVAL);
    };
    if sem.take(false) {
        return 0;
    }
    sem.data.fetch_add(WAITER, Ordering::Relaxed);
    let mut registered = Registered(sem, true);
    loop {
        if sem.take(true) {
            registered.1 = false;
            return 0;
        }
        let duration = if let Some((clock, deadline)) = deadline {
            let mut now = crate::time::TimeSpec::default();
            if unsafe { crate::time::kinakaze_abi_clock_gettime(clock, &mut now) } != 0 {
                return -1;
            }
            let remaining = (i128::from(deadline.tv_sec) - i128::from(now.tv_sec)) * 1_000_000_000
                + i128::from(deadline.tv_nsec)
                - i128::from(now.tv_nsec);
            if remaining <= 0 {
                return failed(ETIMEDOUT);
            }
            Some(std::time::Duration::new(
                (remaining / 1_000_000_000) as u64,
                (remaining % 1_000_000_000) as u32,
            ))
        } else {
            None
        };
        // Value comparison and enqueue use the same lock as wake. Shared keys
        // describe the backing object and offset, including mmap aliases/fork.
        match crate::futex::wait(0, duration, u32::MAX, || {
            Ok((sem.key()?, sem.data.load(Ordering::Relaxed) as i32))
        }) {
            Ok(()) | Err(EAGAIN) => (),
            Err(EINTR) => return failed(EINTR),
            Err(ETIMEDOUT) => (), // Recheck the absolute guest clock.
            Err(error) => return failed(error),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_wait(sem: *mut SemT) -> i32 {
    unsafe { wait(sem, None) }
}

pub(crate) unsafe fn sem_wait_until(sem: *mut SemT, clock: i32, timeout: *const Timespec) -> i32 {
    if timeout.is_null() || !matches!(clock, 0 | 1) {
        return failed(EINVAL);
    }
    let timeout = unsafe { *timeout };
    if !(0..1_000_000_000).contains(&timeout.tv_nsec) {
        return failed(EINVAL);
    }
    unsafe { wait(sem, Some((clock, timeout))) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_timedwait(
    sem: *mut SemT,
    timeout: *const Timespec,
) -> i32 {
    unsafe { sem_wait_until(sem, 0, timeout) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_getvalue(sem: *mut SemT, value: *mut i32) -> i32 {
    let Some(sem) = (unsafe { sem.as_ref() }) else {
        return failed(EINVAL);
    };
    if value.is_null() {
        return failed(EINVAL);
    }
    unsafe { value.write(sem.data.load(Ordering::Relaxed) as u32 as i32) };
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semaphore_layout_limits_and_overflow() {
        assert_eq!(size_of::<SemT>(), 32);
        assert_eq!(align_of::<SemT>(), 8);
        let mut sem = SemT::new(0, false);
        assert_eq!(unsafe { kinakaze_abi_sem_init(&mut sem, 1, u32::MAX) }, -1);
        assert_eq!(crate::kinakaze_errno(), EINVAL);
        assert_eq!(unsafe { kinakaze_abi_sem_init(&mut sem, 0, VALUE_MAX) }, 0);
        assert_eq!(unsafe { kinakaze_abi_sem_post(&mut sem) }, -1);
        assert_eq!(crate::kinakaze_errno(), EOVERFLOW);
        assert_eq!(sem.data.load(Ordering::Relaxed), u64::from(VALUE_MAX));
        assert_eq!(unsafe { kinakaze_abi_sem_trywait(&mut sem) }, 0);
        assert_eq!(unsafe { kinakaze_abi_sem_post(&mut sem) }, 0);
    }
}
