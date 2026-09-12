//! Queue selection preserves unrelated events. Blocking selection waits on the
//! consumer's eventfd rather than repeatedly sleeping.
use crate::{Bool, Display, Window, XEvent};
use core::ffi::c_char;
use core::ffi::c_int;
mod wait;
pub use wait::Waiter;
pub(crate) use wait::notify;

type Predicate = Option<unsafe extern "sysv64" fn(*mut Display, *mut XEvent, *mut c_char) -> Bool>;
pub(crate) unsafe fn predicate(
    display: *mut Display,
    output: *mut XEvent,
    callback: Predicate,
    argument: *mut c_char,
    block: bool,
    consume: bool,
) -> Bool {
    if block && crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] predicate display={display:p} callback={:?} argument={argument:p} xcb={}",
            callback.map(|p| p as usize),
            crate::connection::xcb_events_for(display)
        );
    }
    unsafe {
        select_mode(display, output, block, consume, |event| {
            let mut candidate = *event;
            callback.is_none_or(|callback| callback(display, &mut candidate, argument) != 0)
        })
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XPeekIfEvent")]
pub unsafe extern "sysv64" fn XPeekIfEvent(
    display: *mut Display,
    output: *mut XEvent,
    callback: Predicate,
    argument: *mut c_char,
) -> c_int {
    if unsafe { predicate(display, output, callback, argument, true, false) } != 0 {
        0
    } else {
        -1
    }
}

fn event_mask(event: &XEvent) -> i64 {
    match unsafe { event.r#type } {
        2..=5 => 1 << (unsafe { event.r#type } - 2),
        6 => (1 << 6) | (1 << 7) | (1 << 13) | (unsafe { event.xmotion.state } as i64 & 0x1f00),
        7 => 1 << 4,
        8 => 1 << 5,
        9 | 10 => 1 << 21,
        11 => 1 << 14,
        12..=14 => 1 << 15,
        15 => 1 << 16,
        16 => 1 << 19,
        17..=19 | 21 | 22 | 24 | 26 => (1 << 17) | (1 << 19),
        20 | 23 | 27 => 1 << 20,
        25 => 1 << 18,
        28 => 1 << 22,
        32 => 1 << 23,
        _ => 0,
    }
}

unsafe fn select_mode(
    display: *mut Display,
    output: *mut XEvent,
    block: bool,
    consume: bool,
    matches: impl Fn(&XEvent) -> bool,
) -> Bool {
    if display.is_null() || output.is_null() {
        return 0;
    }
    crate::graphics::flush_all();
    let mut waiter: Option<wait::Waiter> = None;
    loop {
        if let Some(waiter) = &waiter {
            waiter.drain();
        }
        crate::drain_native_events(display);
        {
            let mut state = crate::state()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(index) = state
                .pending_events
                .iter()
                .position(|event| unsafe { event.xany.display == display } && matches(event))
            {
                let event = if consume {
                    state.pending_events.remove(index).unwrap()
                } else {
                    let Some(copy) =
                        crate::input_bridge::duplicate_event(state.pending_events[index])
                    else {
                        return 0;
                    };
                    copy
                };
                crate::set_display_queue_len(display, state.count_for(display));
                unsafe {
                    output.write(event);
                }
                if crate::trace_enabled()
                    && matches!(unsafe { event.r#type }, 12 | 17 | 19 | 20 | 21 | 22 | 23)
                {
                    crate::diagnostic!(
                        "[x11-shared] dequeue pid={} kind={} window={}",
                        crate::shared::pid(),
                        unsafe { event.r#type },
                        unsafe { event.xmap.window }
                    );
                }
                return 1;
            }
        }
        if !block {
            return 0;
        }
        if let Some(waiter) = &waiter {
            if !waiter.wait() {
                return 0;
            }
        } else {
            waiter = wait::Waiter::new();
            if waiter.is_none() {
                return 0;
            }
            // Rescan after registering, closing the check/register race.
        }
    }
}

unsafe fn select(
    display: *mut Display,
    output: *mut XEvent,
    block: bool,
    matches: impl Fn(&XEvent) -> bool,
) -> Bool {
    unsafe { select_mode(display, output, block, true, matches) }
}

pub(crate) unsafe fn next(display: *mut Display, output: *mut XEvent, consume: bool) -> c_int {
    if unsafe { select_mode(display, output, true, consume, |_| true) } != 0 {
        0
    } else {
        -1
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XWindowEvent")]
pub unsafe extern "sysv64" fn XWindowEvent(
    display: *mut Display,
    window: Window,
    mask: i64,
    output: *mut XEvent,
) -> c_int {
    if unsafe {
        select(display, output, true, |event| {
            event.xany.window == window && event_mask(event) & mask != 0
        })
    } != 0
    {
        0
    } else {
        -1
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XMaskEvent")]
pub unsafe extern "sysv64" fn XMaskEvent(
    display: *mut Display,
    mask: i64,
    output: *mut XEvent,
) -> c_int {
    if unsafe { select(display, output, true, |event| event_mask(event) & mask != 0) } != 0 {
        0
    } else {
        -1
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XCheckWindowEvent")]
pub unsafe extern "sysv64" fn XCheckWindowEvent(
    display: *mut Display,
    window: Window,
    mask: i64,
    output: *mut XEvent,
) -> Bool {
    unsafe {
        select(display, output, false, |event| {
            event.xany.window == window && event_mask(event) & mask != 0
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XCheckMaskEvent")]
pub unsafe extern "sysv64" fn XCheckMaskEvent(
    display: *mut Display,
    mask: i64,
    output: *mut XEvent,
) -> Bool {
    unsafe {
        select(display, output, false, |event| {
            event_mask(event) & mask != 0
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XCheckTypedWindowEvent")]
pub unsafe extern "sysv64" fn XCheckTypedWindowEvent(
    display: *mut Display,
    window: Window,
    event_type: c_int,
    output: *mut XEvent,
) -> Bool {
    unsafe {
        select(display, output, false, |event| {
            event.xany.window == window && event.r#type == event_type
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XCheckTypedEvent")]
pub unsafe extern "sysv64" fn XCheckTypedEvent(
    display: *mut Display,
    event_type: c_int,
    output: *mut XEvent,
) -> Bool {
    unsafe { select(display, output, false, |event| event.r#type == event_type) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XPutBackEvent")]
pub unsafe extern "sysv64" fn XPutBackEvent(display: *mut Display, event: *mut XEvent) -> c_int {
    if event.is_null() || display.is_null() {
        return 0;
    }
    {
        let mut state = crate::state()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(mut event) = crate::input_bridge::duplicate_event(unsafe { *event }) else {
            return 0;
        };
        event.xany.display = display;
        state.pending_events.push_front(event);
        crate::set_display_queue_len(display, state.count_for(display));
    }
    unsafe {
        crate::notify_x11_event();
    }
    0
}
