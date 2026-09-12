//! Precise, interruptible native deadlines without changing the system timer rate.
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, SetLastError, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForMultipleObjects,
};

/// Wait-any semantics, preserving source indices and Win32 failure diagnostics.
///
/// A finite deadline is a high-resolution kernel timer in the same wait set as
/// I/O and signal events. Ordinary wait timeouts can oversleep a GTK frame by a
/// system clock tick. No polling thread, spin, or process-global timer setting is
/// needed. The unnamed, non-inherited timer belongs only to this active wait.
///
/// # Safety
/// All source handles must remain valid for the entire wait.
pub unsafe fn any(handles: &[HANDLE], timeout: u32) -> u32 {
    if timeout == 0 || timeout == INFINITE || handles.len() >= 64 {
        return unsafe {
            WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout)
        };
    }
    let timer = unsafe {
        CreateWaitableTimerExW(
            ptr::null(),
            ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        )
    };
    if timer.is_null() {
        return unsafe {
            WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout)
        };
    }
    let due = -(i64::from(timeout) * 10_000);
    if unsafe { SetWaitableTimer(timer, &due, 0, None, ptr::null(), 0) } == 0 {
        unsafe { CloseHandle(timer) };
        return unsafe {
            WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout)
        };
    }
    let mut sources = [ptr::null_mut(); 64];
    sources[..handles.len()].copy_from_slice(handles);
    sources[handles.len()] = timer;
    let result =
        unsafe { WaitForMultipleObjects(handles.len() as u32 + 1, sources.as_ptr(), 0, INFINITE) };
    let error = unsafe { GetLastError() };
    unsafe { CloseHandle(timer) };
    if result == WAIT_FAILED {
        unsafe { SetLastError(error) };
    }
    if result == WAIT_OBJECT_0 + handles.len() as u32 {
        WAIT_TIMEOUT
    } else {
        result
    }
}
