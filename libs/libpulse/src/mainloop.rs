//! Pulse's poll-based loop. Events and native synchronization belong to this
//! library; the runtime only coordinates their fork boundary.

use crate::lock;
use core::ffi::{c_int, c_void};
use libc::fdio::{self, PollFd};
use std::collections::BTreeMap;
use std::sync::Mutex;

mod abi;
mod dispatch;
mod events;
mod lifecycle;
pub use abi::*;
pub use dispatch::*;
pub use events::pa_mainloop_api_once;

pub struct pa_mainloop {
    api: pa_mainloop_api,
    state: Mutex<State>,
    wake_fd: c_int,
    _lease: lifecycle::Lease,
}

// All mutable event state is behind the mutex. API pointers are stable and
// immutable; userdata and callbacks are touched only by the dispatching thread.
unsafe impl Send for pa_mainloop {}
unsafe impl Sync for pa_mainloop {}

#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Prepared,
    Polled,
}

#[derive(Default)]
struct State {
    events: BTreeMap<u64, Box<Event>>,
    next_id: u64,
    phase: Phase,
    timeout_ms: c_int,
    quitting: bool,
    retval: c_int,
    closing: bool,
    poll_func: Option<PollCallback>,
    poll_userdata: usize,
    fds: Vec<PollFd>,
    ids: Vec<u64>,
    ready: Vec<(u64, Trigger)>,
}

struct Event {
    owner: usize,
    id: u64,
    userdata: usize,
    destroy: Option<DestroyCallback>,
    kind: Kind,
}

enum Kind {
    Io {
        fd: c_int,
        flags: c_int,
        callback: IoCallback,
    },
    Time {
        deadline: Option<Timeval>,
        callback: TimeCallback,
    },
    Defer {
        enabled: bool,
        callback: DeferCallback,
    },
}

#[derive(Clone, Copy)]
enum Trigger {
    Io(c_int),
    Time,
    Defer,
}

impl pa_mainloop {
    pub(crate) fn reset_iteration(&self) {
        lock(&self.state).phase = Phase::Idle;
    }

    fn wake(&self) {
        // Nonblocking eventfd coalesces changes and interrupts poll without an
        // idle thread, repeated timeout, or per-notification allocation.
        unsafe {
            fdio::kinakaze_abi_eventfd_write(self.wake_fd, 1);
        }
    }

    fn drain_wakeup(&self) {
        let mut value = 0;
        unsafe {
            fdio::kinakaze_abi_eventfd_read(self.wake_fd, &raw mut value);
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_new")]
pub unsafe extern "sysv64" fn pa_mainloop_new() -> *mut pa_mainloop {
    let lease = lifecycle::Lease::new();
    let fd = unsafe { fdio::kinakaze_abi_eventfd(0, fdio::EFD_CLOEXEC | fdio::EFD_NONBLOCK) };
    if fd < 0 {
        return core::ptr::null_mut();
    }
    let pointer = Box::into_raw(Box::new(pa_mainloop {
        api: events::api(),
        state: Mutex::new(State::default()),
        wake_fd: fd,
        _lease: lease,
    }));
    unsafe {
        (*pointer).api.userdata = pointer.cast();
    }
    pointer
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_free")]
pub unsafe extern "sysv64" fn pa_mainloop_free(pointer: *mut pa_mainloop) {
    let Some(loop_) = (unsafe { pointer.as_ref() }) else {
        return;
    };
    lock(&loop_.state).closing = true;
    // Destroy callbacks may delete other events. Pop one at a time without
    // holding a lock over foreign code.
    loop {
        let next = lock(&loop_.state).events.pop_first();
        let Some((_, event)) = next else {
            break;
        };
        unsafe {
            events::destroy(event);
        }
    }
    libc::kinakaze_abi_close(loop_.wake_fd);
    unsafe {
        drop(Box::from_raw(pointer));
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_get_api")]
pub unsafe extern "sysv64" fn pa_mainloop_get_api(
    pointer: *mut pa_mainloop,
) -> *mut pa_mainloop_api {
    if pointer.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { &raw mut (*pointer).api }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_wakeup")]
pub unsafe extern "sysv64" fn pa_mainloop_wakeup(pointer: *mut pa_mainloop) {
    if let Some(loop_) = unsafe { pointer.as_ref() } {
        loop_.wake();
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_quit")]
pub unsafe extern "sysv64" fn pa_mainloop_quit(pointer: *mut pa_mainloop, retval: c_int) {
    if let Some(loop_) = unsafe { pointer.as_ref() } {
        let mut state = lock(&loop_.state);
        state.quitting = true;
        state.retval = retval;
        drop(state);
        loop_.wake();
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_get_retval")]
pub unsafe extern "sysv64" fn pa_mainloop_get_retval(pointer: *const pa_mainloop) -> c_int {
    unsafe { pointer.as_ref() }.map_or(-1, |loop_| lock(&loop_.state).retval)
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_mainloop_set_poll_func")]
pub unsafe extern "sysv64" fn pa_mainloop_set_poll_func(
    pointer: *mut pa_mainloop,
    callback: Option<PollCallback>,
    userdata: *mut c_void,
) {
    if let Some(loop_) = unsafe { pointer.as_ref() } {
        let mut state = lock(&loop_.state);
        state.poll_func = callback;
        state.poll_userdata = userdata as usize;
    }
}
