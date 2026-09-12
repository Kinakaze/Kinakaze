//! Shared level notifications for eventfd pollers. The counter remains authoritative.
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

pub(crate) struct ReadyEvents {
    pub(super) readable: HANDLE,
    pub(super) writable: HANDLE,
}
// Named kernel events are usable by any thread; Store owns their lifetime.
unsafe impl Send for ReadyEvents {}
unsafe impl Sync for ReadyEvents {}

impl ReadyEvents {
    pub(super) fn new(id: u64, count: u64) -> Result<Self, i32> {
        let domain = kinakaze_runtime::authority::domain_id();
        let create = |suffix: &str, set: bool| {
            let name = format!(r"Local\kinakaze.eventfd-ready.v1.{domain}.{id}.{suffix}");
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            let handle = unsafe { CreateEventW(ptr::null(), 1, i32::from(set), wide.as_ptr()) };
            if handle.is_null() {
                Err(crate::errno_from_win32(unsafe { GetLastError() }))
            } else {
                Ok(handle)
            }
        };
        let readable = create("read", count != 0)?;
        let writable = match create("write", count < u64::MAX - 1) {
            Ok(handle) => handle,
            Err(error) => {
                unsafe { CloseHandle(readable) };
                return Err(error);
            }
        };
        Ok(Self { readable, writable })
    }

    /// Called after counter publication while still holding its writer mutex.
    pub(super) fn publish(&self, count: u64) {
        for (handle, set) in [
            (self.readable, count != 0),
            (self.writable, count < u64::MAX - 1),
        ] {
            unsafe {
                if set {
                    SetEvent(handle)
                } else {
                    ResetEvent(handle)
                }
            };
        }
    }
}
impl Drop for ReadyEvents {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.readable);
            CloseHandle(self.writable);
        }
    }
}

/// Retain both the counter and its native events across descriptor close/reuse.
pub(crate) struct Wait {
    pub(super) _store: std::sync::Arc<crate::mount::shared::Store>,
    pub(super) handles: Vec<HANDLE>,
}
impl Wait {
    pub(crate) fn handles(&self) -> &[HANDLE] {
        &self.handles
    }
}
