//! Completion of a named metadata query on a pinned, shared file object.
//!
//! A file object's event can be signalled by a different request. The request's
//! own status block is the only completion authority, and cancellation names
//! exactly that block instead of cancelling another thread's data transfer.

use super::NativeIoStatus;
use windows_sys::Win32::Foundation::HANDLE;

// IO_STATUS_BLOCK starts with a pointer-sized union whose status member is an
// NTSTATUS. Match the declaration already used by the native AFD backend.
#[repr(C)]
#[derive(Default)]
struct CancelStatus {
    status: i32,
    information: usize,
}

const _: () = assert!(size_of::<CancelStatus>() == size_of::<NativeIoStatus>());
const _: () = assert!(
    core::mem::offset_of!(CancelStatus, information)
        == core::mem::offset_of!(NativeIoStatus, information)
);

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCancelIoFileEx(
        file: HANDLE,
        request: *const CancelStatus,
        cancelled: *mut CancelStatus,
    ) -> i32;
}

/// The caller pins the handle and every request buffer through completion and
/// initializes `io.status` to STATUS_PENDING before submitting the request.
/// No guest signal handler may run until those resources have been released.
pub(super) unsafe fn complete(
    file: HANDLE,
    io: &mut NativeIoStatus,
    submitted: i32,
) -> Result<i32, i32> {
    use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        Sleep, WaitForMultipleObjects, WaitForSingleObject,
    };
    if submitted != 0x103 {
        return Ok(submitted);
    }
    let interrupt = crate::interrupt::current();
    let unavailable = interrupt.is_null();
    let mut cancelling = false;
    loop {
        let status = unsafe { core::ptr::read_volatile(&io.status) } as i32;
        if status != 0x103 {
            return if unavailable {
                Err(crate::EIO)
            } else {
                Ok(status)
            };
        }
        crate::signal::register_waiter();
        if !cancelling && (unavailable || crate::signal::interrupt_pending()) {
            cancelling = true;
            let mut cancelled = CancelStatus::default();
            // A racing completion may report STATUS_NOT_FOUND. Either way,
            // keep our buffers live until the original request has retired.
            unsafe { NtCancelIoFileEx(file, (io as *const NativeIoStatus).cast(), &mut cancelled) };
        }
        // The file event is only a wakeup hint: another request can signal it.
        // Waiting solely for the interrupt with a 1-ms timeout adds that delay
        // even when this metadata request completes a few microseconds later.
        let waited = if cancelling {
            unsafe { trace_native!("native.WaitForSingleObject", WaitForSingleObject(file, 1)) }
        } else {
            let handles = [file, interrupt];
            unsafe {
                trace_native!(
                    "native.WaitForMultipleObjects",
                    WaitForMultipleObjects(2, handles.as_ptr(), 0, 1)
                )
            }
        };
        if unsafe { core::ptr::read_volatile(&io.status) } as i32 == 0x103
            && matches!(waited, WAIT_OBJECT_0 | WAIT_FAILED)
        {
            // A stale event (or an imported handle without SYNCHRONIZE) must
            // not turn this into an unbounded busy wait. Cancellation still
            // names our exact status block, and buffers stay live until it
            // retires. The uncontended completion path pays no timed sleep.
            if cancelling {
                unsafe { Sleep(1) };
            } else {
                unsafe {
                    trace_native!(
                        "native.WaitForSingleObject",
                        WaitForSingleObject(interrupt, 1)
                    )
                };
            }
        }
        crate::signal::unregister_waiter();
    }
}

#[cfg(test)]
mod tests;
