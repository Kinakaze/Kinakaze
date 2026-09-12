//! XInput2 virtual seat: Windows raw input is exposed through the core Xlib event bridge.
#![allow(non_snake_case, non_camel_case_types)]
use core::ffi::{c_double, c_int, c_uchar, c_uint, c_void};
use kinakaze_libX11::{Display, GenericEvent, Window, XEvent, XGenericEventCookie};
use std::sync::atomic::{AtomicU32, Ordering};
type c_ulong = u64;
type Bool = c_int;
type Status = c_int;
mod barriers;
mod device_events;
mod devices;
mod grabs;
mod legacy;
mod object_layout;
mod query;
mod selection;
pub use selection::{XIGetSelectedEvents, XISelectEvents};
mod lifecycle;
pub const XI_RAW_KEY_PRESS: c_int = 13;
pub const XI_RAW_KEY_RELEASE: c_int = 14;
pub const XI_RAW_BUTTON_PRESS: c_int = 15;
pub const XI_RAW_BUTTON_RELEASE: c_int = 16;
pub const XI_RAW_MOTION: c_int = 17;

const XI_EXTENSION_OPCODE: c_int = 128;
const XI_MASTER_POINTER_ID: c_int = 2;
const XI_POINTER_SOURCE_ID: c_int = 4;

static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XIValuatorState {
    pub mask_len: c_int,
    pub mask: *mut c_uchar,
    pub values: *mut c_double,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XIRawEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub extension: c_int,
    pub evtype: c_int,
    pub time: c_ulong,
    pub deviceid: c_int,
    pub sourceid: c_int,
    pub detail: c_int,
    pub flags: c_int,
    pub valuators: XIValuatorState,
    pub raw_values: *mut c_double,
}

#[repr(C)]
struct RawEventPayload {
    // First so `XIRawEvent*` can be cast back to the owning allocation.
    event: XIRawEvent,
    mask: [c_uchar; 1],
    values: [c_double; 2],
    raw_values: [c_double; 2],
}

fn x11_button(button: u16) -> c_int {
    match button {
        kinakaze_libdisplay::event::BTN_LEFT => 1,
        kinakaze_libdisplay::event::BTN_MIDDLE => 2,
        kinakaze_libdisplay::event::BTN_RIGHT => 3,
        kinakaze_libdisplay::event::BTN_SIDE => 8,
        kinakaze_libdisplay::event::BTN_EXTRA => 9,
        _ => 0,
    }
}

/// Converts a Raw Input bridge record into an XI2 generic-event cookie.
pub fn raw_event_cookie(
    dpy: *mut Display,
    source: &kinakaze_libdisplay::event::Event,
) -> Option<XEvent> {
    let (evtype, detail, dx, dy) = match source.kind {
        kinakaze_libdisplay::event::EVENT_RAW_KEY => (
            if source.state == kinakaze_libdisplay::event::STATE_RELEASED {
                XI_RAW_KEY_RELEASE
            } else {
                XI_RAW_KEY_PRESS
            },
            c_int::from(source.keycode) + 8,
            0.0,
            0.0,
        ),
        kinakaze_libdisplay::event::EVENT_RAW_BUTTON => {
            let detail = x11_button(source.button);
            if detail == 0 {
                return None;
            }
            (
                if source.state == kinakaze_libdisplay::event::STATE_RELEASED {
                    XI_RAW_BUTTON_RELEASE
                } else {
                    XI_RAW_BUTTON_PRESS
                },
                detail,
                0.0,
                0.0,
            )
        }
        kinakaze_libdisplay::event::EVENT_RAW_MOTION => {
            (XI_RAW_MOTION, 0, f64::from(source.x), f64::from(source.y))
        }
        _ => return None,
    };

    if !selection::selected(dpy, evtype) {
        return None;
    }

    let motion = evtype == XI_RAW_MOTION;
    let allocation =
        unsafe { kinakaze_alloc::guest::malloc(core::mem::size_of::<RawEventPayload>()) }
            .cast::<RawEventPayload>();
    if allocation.is_null() {
        return None;
    }
    unsafe {
        allocation.write(RawEventPayload {
            event: XIRawEvent {
                r#type: GenericEvent,
                serial: 0,
                send_event: 0,
                display: dpy,
                extension: XI_EXTENSION_OPCODE,
                evtype,
                time: source.time_ms,
                deviceid: if evtype <= XI_RAW_KEY_RELEASE {
                    3
                } else {
                    XI_MASTER_POINTER_ID
                },
                sourceid: if evtype <= XI_RAW_KEY_RELEASE {
                    5
                } else {
                    XI_POINTER_SOURCE_ID
                },
                detail,
                flags: 0,
                valuators: XIValuatorState {
                    mask_len: c_int::from(motion),
                    mask: core::ptr::null_mut(),
                    values: core::ptr::null_mut(),
                },
                raw_values: core::ptr::null_mut(),
            },
            mask: [if motion { 0b11 } else { 0 }],
            values: [dx, dy],
            raw_values: [dx, dy],
        });
    }
    let payload = unsafe { &mut *allocation };
    if motion {
        payload.event.valuators.mask = payload.mask.as_mut_ptr();
        payload.event.valuators.values = payload.values.as_mut_ptr();
        payload.event.raw_values = payload.raw_values.as_mut_ptr();
    }
    let data = (&raw mut payload.event).cast::<c_void>();

    let cookie = XGenericEventCookie {
        r#type: GenericEvent,
        serial: 0,
        send_event: 0,
        display: dpy,
        extension: XI_EXTENSION_OPCODE,
        evtype,
        cookie: NEXT_COOKIE.fetch_add(1, Ordering::Relaxed),
        data,
    };
    Some(XEvent { xcookie: cookie })
}

/// Releases the decoded payload owned by one generic-event cookie.
pub unsafe fn free_event_data(data: *mut c_void) {
    if !data.is_null() {
        unsafe { kinakaze_alloc::guest::free(data.cast()) };
    }
}

unsafe fn copy_payload<T>(data: *mut c_void) -> *mut T {
    let copy = unsafe { kinakaze_alloc::guest::malloc(core::mem::size_of::<T>()) }.cast::<T>();
    if !copy.is_null() {
        unsafe { core::ptr::copy_nonoverlapping(data.cast::<T>(), copy, 1) };
    }
    copy
}

unsafe extern "system" fn duplicate(data: *mut c_void) -> *mut c_void {
    if data.is_null() {
        return core::ptr::null_mut();
    }
    let kind = unsafe { (*data.cast::<XGenericEventCookie>()).evtype };
    match kind {
        2..=10 | 18..=20 => unsafe { device_events::duplicate(data, kind) },
        13..=17 => {
            let copy = unsafe { copy_payload::<RawEventPayload>(data) };
            if let Some(p) = unsafe { copy.as_mut() } {
                if !p.event.valuators.mask.is_null() {
                    p.event.valuators.mask = p.mask.as_mut_ptr();
                }
                if !p.event.valuators.values.is_null() {
                    p.event.valuators.values = p.values.as_mut_ptr();
                }
                if !p.event.raw_values.is_null() {
                    p.event.raw_values = p.raw_values.as_mut_ptr();
                }
            }
            copy.cast()
        }
        25 | 26 => unsafe { copy_payload::<barriers::BarrierEvent>(data) }.cast(),
        _ => core::ptr::null_mut(),
    }
}

#[unsafe(export_name = "kinakaze_engine_libXi_XIQueryVersion")]
pub unsafe extern "sysv64" fn XIQueryVersion(
    _dpy: *mut Display,
    major_return: *mut c_int,
    minor_return: *mut c_int,
) -> Status {
    let requested_major = if major_return.is_null() {
        2
    } else {
        unsafe { *major_return }
    };
    if requested_major < 2 {
        return 1;
    }
    if !major_return.is_null() {
        unsafe { *major_return = 2 };
    }
    if !minor_return.is_null() {
        // XI 2.3 adds barrier hit/leave cookies and pointer release sequences.
        unsafe {
            *minor_return = if requested_major > 2 {
                3
            } else {
                (*minor_return).min(3).max(0)
            }
        };
    }
    0
}

#[repr(C)]
pub struct XIEventMask {
    pub deviceid: c_int,
    pub mask_len: c_int,
    pub mask: *mut c_uchar,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_connections_do_not_steal_events_or_close_each_others_grabs() {
        use kinakaze_libX11::*;
        use kinakaze_libdisplay::event::{EVENT_POINTER_MOTION, EVENT_RAW_MOTION, Event, publish};
        unsafe {
            let a = XOpenDisplay(core::ptr::null());
            let b = XOpenDisplay(core::ptr::null());
            let w = XCreateSimpleWindow(a, 1, 20, 20, 240, 160, 0, 0, 0);
            let logical = kinakaze_libdisplay::window::logical_for_native(w).unwrap();
            let mut bits = [0x40u8];
            let mut mask = XIEventMask {
                deviceid: 2,
                mask_len: 1,
                mask: bits.as_mut_ptr(),
            };
            XISelectEvents(b, w, &mut mask, 1);
            let mut count = -1;
            assert!(XIGetSelectedEvents(a, w, &mut count).is_null());
            assert_eq!(count, 0);
            let mut event = core::mem::zeroed::<XEvent>();
            for display in [a, b] {
                while XPending(display) > 0 {
                    XNextEvent(display, &mut event);
                }
            }
            publish(Event {
                kind: EVENT_POINTER_MOTION,
                window: logical,
                x: 17,
                y: 29,
                ..Default::default()
            });
            assert_eq!(XPending(a), 0, "the first polling Display stole XI motion");
            assert_eq!(XPending(b), 1);
            let mut peek = core::mem::zeroed::<XEvent>();
            XPeekEvent(b, &mut peek);
            let peek_data = peek.xcookie.data;
            XNextEvent(b, &mut event);
            assert_ne!(
                peek_data, event.xcookie.data,
                "peek must own a separate XI cookie"
            );
            // CEF frees peeked cookies while coalescing motion, then later
            // dispatches the queued event. Put-back has the same ownership rule.
            wm::XFreeEventData(b, (&raw mut peek.xcookie).cast());
            XPutBackEvent(b, &mut event);
            wm::XFreeEventData(b, (&raw mut event.xcookie).cast());
            XNextEvent(b, &mut event);
            assert_eq!(event.xcookie.display, b);
            assert_eq!(event.xcookie.evtype, 6);
            let payload = &*event.xcookie.data.cast::<device_events::DeviceEvent>();
            // Nested valuator pointers must refer to the cloned allocation too.
            assert_eq!((payload.event_x, payload.event_y), (17.0, 29.0));
            assert_eq!(*payload.valuators.mask, 3);
            let base = event.xcookie.data as usize;
            assert!((base..base + 300).contains(&(payload.valuators.values as usize)));
            assert!(wm::XGetEventData(b, (&raw mut event.xcookie).cast()) != 0);
            free_event_data(event.xcookie.data);
            let mut raw_bits = [0u8, 0, 2];
            mask.mask_len = 3;
            mask.mask = raw_bits.as_mut_ptr();
            XISelectEvents(b, 1, &mut mask, 1);
            publish(Event {
                kind: EVENT_RAW_MOTION,
                window: logical,
                x: 3,
                y: 4,
                ..Default::default()
            });
            assert_eq!(XPending(a), 0);
            assert_eq!(XPending(b), 1);
            XNextEvent(b, &mut event);
            assert_eq!(event.xcookie.evtype, 17);
            free_event_data(event.xcookie.data);
            mask.mask_len = 1;
            mask.mask = bits.as_mut_ptr();
            assert_eq!(grabs::XIGrabDevice(b, 2, w, 0, 0, 1, 1, 0, &mut mask), 0);
            XCloseDisplay(a);
            publish(Event {
                kind: EVENT_POINTER_MOTION,
                window: logical,
                x: 27,
                y: 39,
                ..Default::default()
            });
            assert_eq!(XPending(b), 1, "closing another Display cleared the grab");
            XNextEvent(b, &mut event);
            assert_eq!(event.xcookie.evtype, 6);
            free_event_data(event.xcookie.data);
            grabs::XIUngrabDevice(b, 2, 0);
            XDestroyWindow(b, w);
            XCloseDisplay(b);
        }
    }
    #[test]
    fn xi2_raw_motion_has_a_real_cookie_and_two_valuators() {
        let mut mask = [0u8; 3];
        mask[crate::XI_RAW_MOTION as usize / 8] |= 1 << (crate::XI_RAW_MOTION as usize % 8);
        let mut selection = crate::XIEventMask {
            deviceid: 1,
            mask_len: mask.len() as c_int,
            mask: mask.as_mut_ptr(),
        };
        assert_eq!(
            unsafe { crate::XISelectEvents(core::ptr::null_mut(), 1, &raw mut selection, 1) },
            0
        );

        let source = kinakaze_libdisplay::event::Event {
            kind: kinakaze_libdisplay::event::EVENT_RAW_MOTION,
            x: 7,
            y: -3,
            time_ms: 42,
            ..Default::default()
        };
        let mut event =
            crate::raw_event_cookie(core::ptr::null_mut(), &source).expect("raw motion selected");
        let cookie = unsafe { &mut event.xcookie };
        assert_eq!(
            (cookie.r#type, cookie.extension, cookie.evtype),
            (35, 128, 17)
        );
        assert_eq!(
            unsafe {
                kinakaze_libX11::wm::XGetEventData(core::ptr::null_mut(), cookie as *mut _ as _)
            },
            1
        );
        let raw = unsafe { &*cookie.data.cast::<crate::XIRawEvent>() };
        assert_eq!(raw.valuators.mask_len, 1);
        assert_eq!(unsafe { *raw.valuators.mask }, 0b11);
        assert_eq!(
            unsafe { core::slice::from_raw_parts(raw.raw_values, 2) },
            &[7.0, -3.0]
        );
        unsafe {
            kinakaze_libX11::wm::XFreeEventData(core::ptr::null_mut(), cookie as *mut _ as _)
        };
        assert!(cookie.data.is_null());
    }
}

unsafe extern "system" fn translate(
    display: *mut Display,
    source: *const kinakaze_libdisplay::event::Event,
    output: *mut XEvent,
) -> i32 {
    if matches!(
        unsafe { (*source).kind },
        kinakaze_libdisplay::event::EVENT_KEY
            | kinakaze_libdisplay::event::EVENT_FOCUS
            | kinakaze_libdisplay::event::EVENT_SCROLL
            | kinakaze_libdisplay::event::EVENT_POINTER_ENTER
            | kinakaze_libdisplay::event::EVENT_POINTER_LEAVE
            | kinakaze_libdisplay::event::EVENT_POINTER_BUTTON
            | kinakaze_libdisplay::event::EVENT_POINTER_MOTION
            | kinakaze_libdisplay::event::EVENT_TOUCH_BEGIN
            | kinakaze_libdisplay::event::EVENT_TOUCH_UPDATE
            | kinakaze_libdisplay::event::EVENT_TOUCH_END
    ) {
        return unsafe { device_events::translate(display, &*source, output) };
    }
    let Some(event) = raw_event_cookie(display, unsafe { &*source }) else {
        return 0;
    };
    unsafe { output.write(event) };
    1
}
unsafe extern "system" fn release(data: *mut c_void) {
    unsafe { free_event_data(data) };
}
extern "C" fn register() {
    kinakaze_libX11::xext::protocol::register_poll(barriers::poll);
    unsafe fn close(d: *mut Display) {
        grabs::close_display(d);
        let mut masks = selection::masks().lock().unwrap_or_else(|e| e.into_inner());
        masks.retain(|(display, _, _), _| *display != d as usize);
    }
    kinakaze_libX11::register_display_close(close);
    fn destroyed(window: usize) {
        grabs::window_destroyed(window);
        let mut masks = selection::masks().lock().unwrap_or_else(|e| e.into_inner());
        masks.retain(|(_, w, _), _| *w != window);
    }
    kinakaze_libX11::register_window_destroy(destroyed);
    unsafe {
        kinakaze_libX11::input_bridge::kinakaze_x11_raw_input_hooks(translate, release, duplicate)
    };
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
