//! Exclusive overlapped-I/O completion events, with one idle event per thread.
//! The operation must retire before its lease is dropped, just as it must
//! retire before releasing the OVERLAPPED and transfer buffer on the stack.
use core::cell::Cell;
use core::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent};

struct Cache(Cell<HANDLE>);
impl Drop for Cache {
    fn drop(&mut self) {
        let handle = self.0.replace(ptr::null_mut());
        if !handle.is_null() {
            unsafe { CloseHandle(handle) };
        }
    }
}
thread_local! {
    // Native TLS starts empty in a fork child; cached host handles are never
    // serialized or inherited. No guest allocator or process-wide lock here.
    static CACHE: Cache = const { Cache(Cell::new(ptr::null_mut())) };
}

pub(crate) struct IoEvent(pub(crate) HANDLE);
impl IoEvent {
    pub(crate) fn new() -> Option<Self> {
        let handle = CACHE
            .try_with(|cache| cache.0.replace(ptr::null_mut()))
            .unwrap_or(ptr::null_mut());
        if !handle.is_null() {
            if unsafe { ResetEvent(handle) } != 0 {
                return Some(Self(handle));
            }
            unsafe { CloseHandle(handle) };
        }
        let handle = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        (!handle.is_null()).then(|| Self(handle))
    }
}
impl Drop for IoEvent {
    fn drop(&mut self) {
        // Nested/reentrant I/O owns a different event while the outer lease is
        // active. Retain at most one; TLS teardown always falls back to close.
        let retained = CACHE
            .try_with(|cache| {
                if cache.0.get().is_null() {
                    cache.0.set(self.0);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if !retained {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::{GetHandleInformation, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{SetEvent, WaitForSingleObject};

    #[test]
    fn sequential_requests_reuse_an_unsignalled_event() {
        let first = IoEvent::new().unwrap();
        let handle = first.0;
        assert_ne!(unsafe { SetEvent(handle) }, 0);
        drop(first);
        for _ in 0..1024 {
            let event = IoEvent::new().unwrap();
            assert_eq!(event.0, handle);
            assert_eq!(unsafe { WaitForSingleObject(event.0, 0) }, WAIT_TIMEOUT);
            assert_ne!(unsafe { SetEvent(event.0) }, 0);
        }
    }

    #[test]
    fn nested_requests_are_exclusive_and_thread_exit_closes_the_cache() {
        let handle = std::thread::spawn(|| {
            let outer = IoEvent::new().unwrap();
            let inner = IoEvent::new().unwrap();
            assert_ne!(outer.0, inner.0);
            assert_ne!(unsafe { SetEvent(outer.0) }, 0);
            assert_eq!(unsafe { WaitForSingleObject(inner.0, 0) }, WAIT_TIMEOUT);
            let retained = inner.0;
            drop(inner);
            drop(outer);
            let again = IoEvent::new().unwrap();
            assert_eq!(again.0, retained);
            retained as usize
        })
        .join()
        .unwrap() as HANDLE;
        let mut flags = 0;
        assert_eq!(unsafe { GetHandleInformation(handle, &mut flags) }, 0);
    }
}
