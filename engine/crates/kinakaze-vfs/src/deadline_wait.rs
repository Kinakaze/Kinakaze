//! Precise, interruptible native deadlines without changing the system timer rate.
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, HANDLE, SetLastError, WAIT_FAILED,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
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
    unsafe { wait_timer(handles, timer) }
}

/// Wait until an absolute Unix CLOCK_REALTIME deadline or a source event.
/// Windows adjusts outstanding absolute timers when the wall clock changes.
/// Failure returns WAIT_FAILED rather than silently losing that property.
///
/// # Safety
/// All source handles must remain valid for the entire wait.
pub unsafe fn any_realtime(handles: &[HANDLE], unix_ns: i128) -> u32 {
    if handles.len() >= 64 {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return WAIT_FAILED;
    }
    let mut timer = unsafe {
        CreateWaitableTimerExW(
            ptr::null(),
            ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        )
    };
    if timer.is_null() {
        // Older hosts may lack high-resolution timers; ordinary absolute
        // timers retain the same clock-change semantics.
        timer = unsafe { CreateWaitableTimerExW(ptr::null(), ptr::null(), 0, TIMER_ALL_ACCESS) };
    }
    if timer.is_null() {
        return WAIT_FAILED;
    }
    let due = realtime_due(unix_ns);
    if unsafe { SetWaitableTimer(timer, &due, 0, None, ptr::null(), 0) } == 0 {
        let error = unsafe { GetLastError() };
        unsafe {
            CloseHandle(timer);
            SetLastError(error);
        }
        return WAIT_FAILED;
    }
    unsafe { wait_timer(handles, timer) }
}

fn realtime_due(unix_ns: i128) -> i64 {
    // FILETIME starts in 1601 and uses 100 ns ticks. Round toward the future;
    // expiry is checked against the full copied deadline again after waking.
    const UNIX_EPOCH: i128 = 116_444_736_000_000_000;
    unix_ns
        .saturating_add(99)
        .div_euclid(100)
        .saturating_add(UNIX_EPOCH)
        .clamp(1, i128::from(i64::MAX)) as i64
}

unsafe fn wait_timer(handles: &[HANDLE], timer: HANDLE) -> u32 {
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

#[cfg(test)]
mod tests;
