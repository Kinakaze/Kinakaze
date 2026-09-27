//! Xlib type filters used by XIM servers, including IBus's IMdkit.
//!
//! New filters precede old ones. XFilterEvent returns the first matching
//! callback's result, even False, and callbacks run without the registry lock.
use crate::{Bool, Display, Window, XEvent};
use core::ffi::{c_int, c_void};
use std::sync::Mutex;

mod lifecycle;

pub type Filter = unsafe extern "sysv64" fn(*mut Display, Window, *mut XEvent, *mut c_void) -> Bool;

#[derive(Clone, Copy)]
struct Entry {
    display: usize,
    window: u64,
    first: c_int,
    last: c_int,
    callback: Filter,
    data: usize,
}

static FILTERS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

fn display_key(display: *mut Display) -> usize {
    if display == crate::shared_display() {
        0
    } else {
        display as usize
    }
}

fn window_key(window: Window) -> u64 {
    // Locally recreated HWNDs change across fork; foreign windows and the root
    // retain their identity. The callback receives the current caller's XID.
    kinakaze_libdisplay::window::logical_for_native(window).unwrap_or(window as u64 | (1 << 63))
}

#[unsafe(export_name = "kinakaze_engine_libX11__XRegisterFilterByType")]
pub unsafe extern "sysv64" fn _XRegisterFilterByType(
    display: *mut Display,
    window: Window,
    first: c_int,
    last: c_int,
    callback: Option<Filter>,
    data: *mut c_void,
) {
    let Some(callback) = callback else { return };
    if display.is_null() {
        return;
    }
    let entry = Entry {
        display: display_key(display),
        window: window_key(window),
        first,
        last,
        callback,
        data: data as usize,
    };
    let mut filters = FILTERS.lock().unwrap_or_else(|e| e.into_inner());
    if filters.try_reserve(1).is_ok() {
        filters.push(entry);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XUnregisterFilter")]
pub unsafe extern "sysv64" fn _XUnregisterFilter(
    display: *mut Display,
    window: Window,
    callback: Option<Filter>,
    data: *mut c_void,
) {
    let Some(callback) = callback else { return };
    if display.is_null() {
        return;
    }
    let display = display_key(display);
    let window = window_key(window);
    FILTERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|entry| {
            entry.display != display
                || entry.window != window
                || entry.callback as usize != callback as usize
                || entry.data != data as usize
        });
}

pub(crate) fn close(display: *mut Display) {
    let display = display_key(display);
    FILTERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|e| e.display != display);
}

pub(crate) unsafe fn filter(event: *mut XEvent, window: Window) -> Bool {
    if event.is_null() {
        return 0;
    }
    let any = unsafe { (*event).xany };
    if any.display.is_null() {
        return 0;
    }
    let window = if window == 0 { any.window } else { window };
    let key = window_key(window);
    let display = display_key(any.display);
    let entry = FILTERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .find(|e| {
            e.display == display && e.window == key && (e.first..=e.last).contains(&any.r#type)
        })
        .copied();
    // In particular, a filter may unregister itself, register another filter,
    // or call XFilterEvent recursively. No registry borrow survives the call.
    entry.map_or(0, |e| unsafe {
        (e.callback)(any.display, window, event, e.data as *mut c_void)
    })
}

#[cfg(test)]
mod tests;
