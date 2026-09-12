//! Per-thread interrupt events used to break blocking I/O waits.
//!
//! Every hosted thread owns one auto-reset Windows event. A blocking operation
//! waits on both its own I/O completion event and this interrupt event, which
//! is what allows a signal raised from another thread to cancel an in-flight
//! request and surface `EINTR`. The registry maps host thread IDs to handles so
//! the raising thread can find the target without touching its TLS.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, GetCurrentThreadId, GetExitCodeThread, GetProcessIdOfThread,
    OpenThread, SetEvent, THREAD_QUERY_LIMITED_INFORMATION,
};

use std::ptr;

/// Owns one thread's interrupt event and deregisters it on thread exit.
struct ThreadInterrupt {
    handle: HANDLE,
    thread_id: u32,
}

impl Drop for ThreadInterrupt {
    fn drop(&mut self) {
        if let Some(registry) = REGISTRY.get()
            && let Ok(mut registry) = registry.lock()
        {
            registry.remove(&self.thread_id);
        }
        // SAFETY: this type owns the event created in `with_current`.
        unsafe { CloseHandle(self.handle) };
    }
}

// The handle is only ever passed to Win32 wait and signal calls, which accept
// use from any thread.
unsafe impl Send for ThreadInterrupt {}

static REGISTRY: OnceLock<Mutex<HashMap<u32, usize>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<u32, usize>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    static INTERRUPT: Option<ThreadInterrupt> = create_current();
}

fn create_current() -> Option<ThreadInterrupt> {
    // An auto-reset event: waking exactly one waiter and clearing itself
    // matches one signal delivery consuming one interrupt.
    // SAFETY: a null security descriptor and name request the documented
    // default unnamed event object.
    let handle = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
    if handle.is_null() {
        return None;
    }
    // SAFETY: GetCurrentThreadId has no preconditions.
    let thread_id = unsafe { GetCurrentThreadId() };
    if let Ok(mut registry) = registry().lock() {
        registry.insert(thread_id, handle as usize);
    }
    Some(ThreadInterrupt { handle, thread_id })
}

/// Returns the calling thread's interrupt event, or null when unavailable.
///
/// The handle is borrowed from this thread's TLS and must not be closed. An
/// interruptible operation must report an error if event creation failed;
/// silently using an uninterruptible wait changes the syscall's semantics.
pub fn current() -> HANDLE {
    INTERRUPT
        .with(|slot| slot.as_ref().map(|interrupt| interrupt.handle as usize))
        .unwrap_or(0) as HANDLE
}

/// Wakes one blocking operation on `thread_id`, reporting whether a thread was
/// registered under that ID.
pub fn interrupt_thread(thread_id: u32) -> bool {
    let Ok(registry) = registry().lock() else {
        return false;
    };
    let Some(&handle) = registry.get(&thread_id) else {
        return false;
    };
    // SAFETY: the registry only holds live handles; the owning thread removes
    // its entry before closing the event.
    unsafe { SetEvent(handle as HANDLE) != 0 }
}

/// Returns the calling thread's host ID for use with [`interrupt_thread`].
pub fn current_thread_id() -> u32 {
    // SAFETY: GetCurrentThreadId has no preconditions.
    unsafe { GetCurrentThreadId() }
}

/// Whether a Windows thread ID still names a live thread in this process.
///
/// A thread need not currently be blocked in VFS, so absence from the interrupt
/// event registry is not an existence test. `tgkill` uses this query to
/// distinguish a runnable target from `ESRCH` before queuing its signal.
pub fn thread_exists(thread_id: u32) -> bool {
    thread_exists_in_process(thread_id, unsafe { GetCurrentProcessId() })
}

/// Query a task directory in another guest process without accepting a thread
/// ID belonging to an unrelated Windows process.
pub(crate) fn thread_exists_in_process(thread_id: u32, process_id: u32) -> bool {
    if thread_id == 0 {
        return false;
    }
    // SAFETY: the ID is an integer supplied by the guest and the returned
    // query-only handle is closed immediately.
    let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, thread_id) };
    if handle.is_null() {
        return false;
    }
    let mut exit_code = 0;
    let live = unsafe {
        GetProcessIdOfThread(handle) == process_id
            && GetExitCodeThread(handle, &mut exit_code) != 0
            && exit_code == 259 // STILL_ACTIVE
    };
    unsafe { CloseHandle(handle) };
    live
}
