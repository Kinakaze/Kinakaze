//! Normal XI2 events share the native input queue with core X11 and raw input.
use super::*;
use kinakaze_libdisplay::event::*;
static BUTTONS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(test)]
pub(super) fn reset_buttons() {
    BUTTONS.store(0, std::sync::atomic::Ordering::Relaxed);
}
fn buttons_before(e: &Event, detail: i32) -> u32 {
    use std::sync::atomic::Ordering;
    if e.kind == EVENT_POINTER_BUTTON && (1..32).contains(&detail) {
        if e.state == STATE_RELEASED {
            BUTTONS.fetch_and(!(1 << detail), Ordering::Relaxed) | (1 << detail)
        } else {
            BUTTONS.fetch_or(1 << detail, Ordering::Relaxed) & !(1 << detail)
        }
    } else {
        BUTTONS.load(Ordering::Relaxed)
    }
}
#[repr(C)]
#[derive(Default)]
struct Mods {
    base: i32,
    latched: i32,
    locked: i32,
    effective: i32,
}
#[repr(C)]
struct Buttons {
    len: i32,
    mask: *mut u8,
}
#[repr(C)]
pub(crate) struct DeviceEvent {
    kind: i32,
    serial: u64,
    send: i32,
    display: *mut Display,
    extension: i32,
    evtype: i32,
    time: u64,
    device: i32,
    source: i32,
    detail: i32,
    root: Window,
    event: Window,
    child: Window,
    root_x: f64,
    root_y: f64,
    pub(super) event_x: f64,
    pub(super) event_y: f64,
    flags: i32,
    buttons: Buttons,
    pub(super) valuators: XIValuatorState,
    mods: Mods,
    group: Mods,
}
#[repr(C)]
struct Payload {
    event: DeviceEvent,
    buttons: [u8; 4],
    mask: [u8; 1],
    values: [f64; 2],
}
/// XIEnterEvent, which XI_Enter/XI_Leave (and focus) events use in place of
/// XIDeviceEvent: mode, focus and same_screen sit where the device event has
/// its flags, and there are no valuators.
#[repr(C)]
pub(crate) struct EnterEvent {
    kind: i32,
    serial: u64,
    send: i32,
    display: *mut Display,
    extension: i32,
    evtype: i32,
    time: u64,
    device: i32,
    source: i32,
    detail: i32,
    root: Window,
    event: Window,
    child: Window,
    root_x: f64,
    root_y: f64,
    event_x: f64,
    event_y: f64,
    mode: i32,
    focus: i32,
    same_screen: i32,
    buttons: Buttons,
    mods: Mods,
    group: Mods,
}
#[repr(C)]
struct EnterPayload {
    event: EnterEvent,
    buttons: [u8; 4],
}

pub(super) unsafe fn duplicate(data: *mut c_void, kind: i32) -> *mut c_void {
    if (7..=10).contains(&kind) {
        let copy = unsafe { super::copy_payload::<EnterPayload>(data) };
        if let Some(p) = unsafe { copy.as_mut() } {
            p.event.buttons.mask = p.buttons.as_mut_ptr();
        }
        copy.cast()
    } else {
        let copy = unsafe { super::copy_payload::<Payload>(data) };
        if let Some(p) = unsafe { copy.as_mut() } {
            p.event.buttons.mask = p.buttons.as_mut_ptr();
            p.event.valuators.mask = p.mask.as_mut_ptr();
            p.event.valuators.values = p.values.as_mut_ptr();
        }
        copy.cast()
    }
}
/// Pointer crossing and keyboard focus use XIEnterEvent. A grab does not
/// redirect these native notifications to another window.
fn translate_crossing(d: *mut Display, e: &Event, out: *mut XEvent) -> i32 {
    let focus = e.kind == EVENT_FOCUS;
    let mode = if focus { 0 } else { e.state.min(2) as i32 };
    if mode != 0 {
        // A different process owned the release during the grab. Crossing back
        // carries the server's current buttons, not this client's stale cache.
        BUTTONS.store(e.button as u32, Ordering::Relaxed);
    }
    let entered = if focus {
        e.state != 0
    } else {
        e.kind == EVENT_POINTER_ENTER
    };
    let kind = if focus {
        if entered { 9 } else { 10 }
    } else if entered {
        7
    } else {
        8
    };
    let device = if focus { 3 } else { 2 };
    let Some(w) = kinakaze_libdisplay::window::native_handle(e.window) else {
        return 2;
    };
    let masks = selection::masks().lock().unwrap_or_else(|e| e.into_inner());
    let selected = [0, 1, device, device + 2].into_iter().any(|id| {
        masks
            .get(&(d as usize, w, id))
            .is_some_and(|v| v & (1 << kind) != 0)
    });
    drop(masks);
    if !selected {
        return 0;
    }
    let (mut root_x, mut root_y, mut child) = (e.x, e.y, 0);
    unsafe {
        kinakaze_libX11::XTranslateCoordinates(
            d,
            w,
            1,
            e.x,
            e.y,
            &mut root_x,
            &mut root_y,
            &mut child,
        );
    }
    let (mut event_x, mut event_y) = (e.x, e.y);
    if focus {
        let (mut root, mut mask) = (0, 0);
        unsafe {
            kinakaze_libX11::graphics::XQueryPointer(
                d,
                w,
                &mut root,
                &mut child,
                &mut root_x,
                &mut root_y,
                &mut event_x,
                &mut event_y,
                &mut mask,
            );
        }
    }
    let key = kinakaze_libX11::xkb::server::state();
    let payload =
        unsafe { kinakaze_alloc::guest::malloc(size_of::<EnterPayload>()) }.cast::<EnterPayload>();
    if payload.is_null() {
        return 2;
    }
    unsafe {
        payload.write(EnterPayload {
            event: EnterEvent {
                kind: 35,
                serial: 0,
                send: 0,
                display: d,
                extension: 128,
                evtype: kind,
                time: e.time_ms,
                device,
                source: device + 2,
                // NotifyNonlinear: the other window is not in this hierarchy.
                detail: 3,
                root: 1,
                event: w,
                child: 0,
                root_x: f64::from(root_x),
                root_y: f64::from(root_y),
                event_x: f64::from(event_x),
                event_y: f64::from(event_y),
                mode,
                focus: i32::from(if focus {
                    entered
                } else {
                    kinakaze_libX11::focus::key_target(w) == Some(w)
                }),
                same_screen: 1,
                buttons: Buttons {
                    len: 4,
                    mask: core::ptr::null_mut(),
                },
                mods: Mods {
                    base: key.base_mods as i32,
                    latched: key.latched_mods as i32,
                    locked: key.locked_mods as i32,
                    effective: key.mods as i32,
                },
                group: Mods {
                    effective: key.group as i32,
                    ..Default::default()
                },
            },
            buttons: BUTTONS
                .load(std::sync::atomic::Ordering::Relaxed)
                .to_le_bytes(),
        });
        let p = &mut *payload;
        p.event.buttons.mask = p.buttons.as_mut_ptr();
        out.write(XEvent {
            xcookie: XGenericEventCookie {
                r#type: 35,
                serial: 0,
                send_event: 0,
                display: d,
                extension: 128,
                evtype: kind,
                cookie: NEXT_COOKIE.fetch_add(1, Ordering::Relaxed),
                data: payload.cast(),
            },
        });
    }
    1
}
fn target(d: *mut Display, mut w: Window, device: i32, kind: i32) -> Option<Window> {
    let masks = selection::masks().lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if [0, 1, device, device + 2].into_iter().any(|id| {
            masks
                .get(&(d as usize, w, id))
                .is_some_and(|v| v & (1 << kind) != 0)
        }) {
            return Some(w);
        }
        if w == 1 {
            return None;
        }
        w = kinakaze_libdisplay::window::hierarchy(Some(w))
            .and_then(|(parent, _)| parent)
            .unwrap_or(1);
    }
}
fn translate_inner(d: *mut Display, e: &Event, out: *mut XEvent, replay: bool) -> i32 {
    if matches!(
        e.kind,
        EVENT_POINTER_ENTER | EVENT_POINTER_LEAVE | EVENT_FOCUS
    ) {
        return translate_crossing(d, e, out);
    }
    let (device, detail, kind) = match e.kind {
        EVENT_KEY => (
            3,
            e.keycode as i32 + 8,
            if e.state == STATE_RELEASED { 3 } else { 2 },
        ),
        EVENT_POINTER_BUTTON => (
            2,
            super::x11_button(e.button),
            if e.state == STATE_RELEASED { 5 } else { 4 },
        ),
        EVENT_POINTER_MOTION => (2, 0, 6),
        // X11 translates wheel notches to button 4..7 without treating them
        // as physically held mouse buttons or valuator motion.
        EVENT_SCROLL => (
            2,
            e.button as i32,
            if e.state == STATE_RELEASED { 5 } else { 4 },
        ),
        EVENT_TOUCH_BEGIN | EVENT_TOUCH_UPDATE | EVENT_TOUCH_END => (
            2,
            e.touch_id as i32,
            18 + (e.kind - EVENT_TOUCH_BEGIN) as i32,
        ),
        _ => return 0,
    };
    // The window may have been destroyed after the native message was queued.
    let Some(w) = kinakaze_libdisplay::window::native_handle(e.window) else {
        return 2;
    };
    let grab = match grabs::route(
        d,
        e,
        device,
        detail,
        w,
        matches!(kind, 2 | 4),
        matches!(kind, 3 | 5),
        replay,
    ) {
        Ok(g) => g,
        Err(()) => return 2,
    };
    let buttons = buttons_before(e, detail);
    let selected = target(d, w, device, kind);
    let dest = if let Some(g) = grab {
        if g.mask & (1 << kind) == 0 {
            return 2;
        }
        if g.owner {
            selected.unwrap_or(g.window)
        } else {
            g.window
        }
    } else {
        let Some(w) = selected else {
            return 0;
        };
        w
    };
    let mut root_x = e.x;
    let mut root_y = e.y;
    let mut child = 0;
    if w != 1 {
        unsafe {
            kinakaze_libX11::XTranslateCoordinates(
                d,
                w,
                1,
                e.x,
                e.y,
                &mut root_x,
                &mut root_y,
                &mut child,
            );
        }
    }
    let mut x = root_x;
    let mut y = root_y;
    if dest != 1 {
        unsafe {
            kinakaze_libX11::XTranslateCoordinates(
                d, 1, dest, root_x, root_y, &mut x, &mut y, &mut child,
            );
        }
    }
    let key = kinakaze_libX11::xkb::server::state();
    let payload = unsafe { kinakaze_alloc::guest::malloc(size_of::<Payload>()) }.cast::<Payload>();
    if payload.is_null() {
        return 2;
    }
    unsafe {
        payload.write(Payload {
            event: DeviceEvent {
                kind: 35,
                serial: 0,
                send: 0,
                display: d,
                extension: 128,
                evtype: kind,
                time: e.time_ms,
                device,
                source: device + 2,
                detail,
                root: 1,
                event: dest,
                child: 0,
                root_x: root_x as f64,
                root_y: root_y as f64,
                event_x: x as f64,
                event_y: y as f64,
                flags: if e.state == STATE_REPEAT { 1 << 16 } else { 0 },
                buttons: Buttons {
                    len: 4,
                    mask: core::ptr::null_mut(),
                },
                valuators: XIValuatorState {
                    mask_len: 1,
                    mask: core::ptr::null_mut(),
                    values: core::ptr::null_mut(),
                },
                mods: Mods {
                    base: key.base_mods as i32,
                    latched: key.latched_mods as i32,
                    locked: key.locked_mods as i32,
                    effective: key.mods as i32,
                },
                group: Mods {
                    effective: key.group as i32,
                    ..Default::default()
                },
            },
            buttons: buttons.to_le_bytes(),
            mask: [if kind == 6 || (18..=20).contains(&kind) {
                3
            } else {
                0
            }],
            values: [root_x as f64, root_y as f64],
        });
        let p = &mut *payload;
        p.event.buttons.mask = p.buttons.as_mut_ptr();
        p.event.valuators.mask = p.mask.as_mut_ptr();
        p.event.valuators.values = p.values.as_mut_ptr();
        out.write(XEvent {
            xcookie: XGenericEventCookie {
                r#type: 35,
                serial: 0,
                send_event: 0,
                display: d,
                extension: 128,
                evtype: kind,
                cookie: NEXT_COOKIE.fetch_add(1, Ordering::Relaxed),
                data: payload.cast(),
            },
        });
    }
    1
}
pub unsafe fn translate(d: *mut Display, e: &Event, out: *mut XEvent) -> i32 {
    translate_inner(d, e, out, false)
}
pub fn queue(d: *mut Display, e: &Event, replay: bool) {
    let mut out = core::mem::MaybeUninit::uninit();
    if translate_inner(d, e, out.as_mut_ptr(), replay) == 1 {
        kinakaze_libX11::queue_extension_event(d, unsafe { out.assume_init() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drag_button_mask_survives_motion_and_reports_state_before_release() {
        reset_buttons();
        let mut event = Event {
            kind: EVENT_POINTER_BUTTON,
            state: STATE_PRESSED,
            ..Event::default()
        };
        assert_eq!(buttons_before(&event, 1), 0);
        event.kind = EVENT_POINTER_MOTION;
        assert_eq!(buttons_before(&event, 0), 2);
        event.kind = EVENT_POINTER_BUTTON;
        event.state = STATE_RELEASED;
        assert_eq!(buttons_before(&event, 1), 2);
        event.kind = EVENT_POINTER_MOTION;
        assert_eq!(buttons_before(&event, 0), 0);
    }
    #[test]
    fn touch_grab_routes_contacts_and_rejects_until_end() {
        unsafe {
            grabs::close();
            selection::masks().lock().unwrap().clear();
            let d = kinakaze_libX11::XOpenDisplay(core::ptr::null());
            let mut bits = [0u8, 0, 0x1c, 1];
            let mut mask = XIEventMask {
                deviceid: 2,
                mask_len: 4,
                mask: bits.as_mut_ptr(),
            };
            assert_eq!(selection::XISelectEvents(d, 1, &mut mask, 1), 0);
            let mut count = 0;
            let selected = selection::XIGetSelectedEvents(d, 1, &mut count);
            assert_eq!(count, 1);
            assert_eq!((*selected).mask_len, 4);
            assert_eq!(core::slice::from_raw_parts((*selected).mask, 4), &bits);
            kinakaze_libX11::XFree(selected.cast());
            selection::masks().lock().unwrap().clear();
            let mut mods = grabs::Modifiers {
                modifiers: i32::MIN,
                status: -1,
            };
            assert_eq!(
                grabs::touch_select(d, 2, 1, 0, &mask, 1, &mut mods, true),
                0
            );
            // Native events name a real window; the root has no native handle.
            let w = kinakaze_libX11::XCreateSimpleWindow(d, 1, 0, 0, 40, 30, 0, 0, 0);
            let logical = kinakaze_libdisplay::window::logical_for_native(w).unwrap();
            let mut source = Event {
                window: logical,
                kind: EVENT_TOUCH_BEGIN,
                touch_id: 42,
                x: 18,
                y: 31,
                ..Default::default()
            };
            let mut event = core::mem::zeroed::<XEvent>();
            for (native, xi) in [(EVENT_TOUCH_BEGIN, 18), (EVENT_TOUCH_UPDATE, 19)] {
                source.kind = native;
                assert_eq!(translate(d, &source, &mut event), 1);
                let payload = &*event.xcookie.data.cast::<DeviceEvent>();
                assert_eq!((payload.evtype, payload.detail, payload.event), (xi, 42, 1));
                let (mut root_x, mut root_y, mut child) = (0, 0, 0);
                kinakaze_libX11::XTranslateCoordinates(
                    d,
                    w,
                    1,
                    18,
                    31,
                    &mut root_x,
                    &mut root_y,
                    &mut child,
                );
                assert_eq!(
                    (payload.root_x, payload.root_y),
                    (f64::from(root_x), f64::from(root_y))
                );
                assert_eq!(*payload.valuators.mask, 3);
                free_event_data(event.xcookie.data);
            }
            assert_eq!(grabs::XIAllowTouchEvents(d, 2, 42, 1, 1), 0);
            assert_eq!(translate(d, &source, &mut event), 2);
            source.kind = EVENT_TOUCH_END;
            assert_eq!(translate(d, &source, &mut event), 2);
            source.kind = EVENT_TOUCH_BEGIN;
            assert_eq!(translate(d, &source, &mut event), 1);
            free_event_data(event.xcookie.data);
            source.kind = EVENT_TOUCH_END;
            assert_eq!(translate(d, &source, &mut event), 1);
            free_event_data(event.xcookie.data);
            mods.modifiers = 0;
            assert_eq!(
                grabs::touch_select(d, 2, 1, 0, &mask, 1, &mut mods, true),
                0
            );
            mods.modifiers = i32::MIN;
            assert_eq!(
                grabs::touch_select(d, 2, 1, 0, core::ptr::null(), 1, &mut mods, false),
                0
            );
            source.kind = EVENT_TOUCH_BEGIN;
            assert_eq!(translate(d, &source, &mut event), 0);
            kinakaze_libX11::XDestroyWindow(d, w);
            kinakaze_libX11::XCloseDisplay(d);
        }
    }
    #[test]
    fn passive_button_grab_freezes_then_releases_queued_motion() {
        unsafe {
            grabs::close();
            selection::masks().lock().unwrap().clear();
            let d = kinakaze_libX11::XOpenDisplay(core::ptr::null());
            let mut bits = [0x70u8];
            let mut mask = XIEventMask {
                deviceid: 2,
                mask_len: 1,
                mask: bits.as_mut_ptr(),
            };
            let mut mods = grabs::Modifiers {
                modifiers: i32::MIN,
                status: -1,
            };
            assert_eq!(
                grabs::XIGrabButton(d, 2, 1, 1, 0, 0, 1, 0, &mut mask, 1, &mut mods),
                0
            );
            assert_eq!(mods.status, 0);
            let w = kinakaze_libX11::XCreateSimpleWindow(d, 1, 0, 0, 40, 30, 0, 0, 0);
            let logical = kinakaze_libdisplay::window::logical_for_native(w).unwrap();
            let source = Event {
                window: logical,
                kind: EVENT_POINTER_BUTTON,
                button: BTN_LEFT,
                state: STATE_PRESSED,
                x: 4,
                y: 9,
                ..Default::default()
            };
            let mut event = core::mem::zeroed::<XEvent>();
            assert_eq!(translate(d, &source, &mut event), 1);
            let payload = &*event.xcookie.data.cast::<DeviceEvent>();
            assert_eq!((payload.evtype, payload.event, payload.detail), (4, 1, 1));
            free_event_data(event.xcookie.data);
            let motion = Event {
                kind: EVENT_POINTER_MOTION,
                x: 12,
                y: 15,
                ..source
            };
            assert_eq!(translate(d, &motion, &mut event), 2);
            assert_eq!(grabs::XIAllowEvents(d, 2, 0, 0), 0);
            assert!(kinakaze_libX11::XPending(d) > 0);
            kinakaze_libX11::XNextEvent(d, &mut event);
            assert_eq!(event.xcookie.evtype, 6);
            free_event_data(event.xcookie.data);
            grabs::XIUngrabDevice(d, 2, 0);
            grabs::XIUngrabButton(d, 2, 1, 1, 1, &mods);
            kinakaze_libX11::XDestroyWindow(d, w);
            kinakaze_libX11::XCloseDisplay(d);
        }
    }
}
