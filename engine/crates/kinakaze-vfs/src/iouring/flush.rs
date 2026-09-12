//! An operation-local asynchronous flush, without a guest descriptor or cache.
//! Copy-up must establish durability before publication, without blocking a
//! guest thread inside FlushFileBuffers or degrading to an uninterruptible wait.

use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingFlushFile, CloseIoRing, CreateIoRing, FILE_FLUSH_DEFAULT, HIORING, IORING_CQE,
    IORING_CREATE_FLAGS, IORING_HANDLE_REF, IORING_HANDLE_REF_0, IORING_OP_FLUSH, IORING_REF_RAW,
    IORING_VERSION_3, IsIoRingOpSupported, PopIoRingCompletion, SetIoRingCompletionEvent,
    SubmitIoRing,
};
use windows_sys::Win32::System::IO::CancelIoEx;
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForMultipleObjects};

use crate::{EINTR, EIO, EOPNOTSUPP, errno_from_win32, interrupt, signal};

pub(crate) struct Flush {
    ring: HIORING,
    event: HANDLE,
}

impl Drop for Flush {
    fn drop(&mut self) {
        unsafe {
            CloseIoRing(self.ring);
            if !self.event.is_null() {
                CloseHandle(self.event);
            }
        }
    }
}

fn check(result: i32) -> Result<(), i32> {
    if result >= 0 {
        return Ok(());
    }
    if result as u32 & 0xffff_0000 == 0x8007_0000 {
        Err(errno_from_win32(result as u32 & 0xffff))
    } else {
        Err(EIO)
    }
}

impl Flush {
    pub(crate) fn new() -> Result<Self, i32> {
        let mut ring = ptr::null_mut();
        let flags = IORING_CREATE_FLAGS {
            Required: 0,
            Advisory: 0,
        };
        check(unsafe { CreateIoRing(IORING_VERSION_3, flags, 2, 2, &mut ring) })?;
        let mut flush = Self {
            ring,
            event: ptr::null_mut(),
        };
        if unsafe { IsIoRingOpSupported(ring, IORING_OP_FLUSH) } == 0 {
            return Err(EOPNOTSUPP);
        }
        flush.event = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
        if flush.event.is_null() {
            return Err(EIO);
        }
        check(unsafe { SetIoRingCompletionEvent(ring, flush.event) })?;
        Ok(flush)
    }

    /// Flush a caller-owned handle with no other outstanding I/O on it.
    ///
    /// # Safety
    /// The handle must remain live and carry write access. Cancellation affects
    /// this private handle only, never another guest descriptor's operations.
    pub(crate) unsafe fn file(&mut self, file: HANDLE) -> Result<(), i32> {
        let interrupt = interrupt::current();
        if interrupt.is_null() {
            return Err(EIO);
        }
        let file_ref = IORING_HANDLE_REF {
            Kind: IORING_REF_RAW,
            Handle: IORING_HANDLE_REF_0 { Handle: file },
        };
        check(unsafe { BuildIoRingFlushFile(self.ring, file_ref, FILE_FLUSH_DEFAULT, 1, 0) })?;
        let mut submitted = 0;
        check(unsafe { SubmitIoRing(self.ring, 0, 0, &mut submitted) })?;
        if submitted != 1 {
            return Err(EIO);
        }
        let mut interrupted = false;
        loop {
            let mut completion: IORING_CQE = unsafe { std::mem::zeroed() };
            let result = unsafe { PopIoRingCompletion(self.ring, &mut completion) };
            if result == 0 {
                if completion.UserData != 1 {
                    return Err(EIO);
                }
                return if interrupted {
                    Err(EINTR)
                } else {
                    check(completion.ResultCode)
                };
            }
            if result != 1 {
                // S_FALSE is the documented empty-queue result.
                unsafe { CancelIoEx(file, ptr::null()) };
                // A flush owns no caller buffer. On a broken completion queue
                // the OS retains its file reference; no object is published.
                return Err(EIO);
            }
            let handles = [self.event, interrupt];
            signal::register_waiter();
            if !interrupted && signal::pending() & !signal::blocked_mask() != 0 {
                interrupted = true;
                unsafe { CancelIoEx(file, ptr::null()) };
            }
            let waited = unsafe {
                WaitForMultipleObjects(
                    if interrupted { 1 } else { 2 },
                    handles.as_ptr(),
                    0,
                    INFINITE,
                )
            };
            signal::unregister_waiter();
            if waited == WAIT_OBJECT_0 {
                continue;
            }
            if !interrupted && waited == WAIT_OBJECT_0 + 1 {
                // Retire the original flush, not merely its cancellation, before
                // releasing the stage. Signal delivery belongs to the syscall
                // boundary after the transaction has unwound.
                if signal::pending() & !signal::blocked_mask() != 0 {
                    interrupted = true;
                    unsafe { CancelIoEx(file, ptr::null()) };
                }
                continue;
            }
            unsafe { CancelIoEx(file, ptr::null()) };
            return Err(EIO);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    #[test]
    fn native_ring_flush_completes_and_reports_handle_errors() {
        let path = std::env::temp_dir().join(format!(
            "kinakaze-ring-flush-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED)
            .open(&path)
            .unwrap();
        let mut flush = Flush::new().unwrap();
        assert_eq!(unsafe { flush.file(file.as_raw_handle()) }, Ok(()));
        drop(file);
        let readonly = std::fs::File::open(&path).unwrap();
        assert!(unsafe { flush.file(readonly.as_raw_handle()) }.is_err());
        drop(readonly);
        std::fs::remove_file(path).unwrap();
    }
}
