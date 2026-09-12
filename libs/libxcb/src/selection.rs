//! Shared ownership with Xlib; wire events preserve XCB resource IDs.
use super::*;
macro_rules! owner_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            owner: u32,
            selection: u32,
            time: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let owner = if owner == 0 {
                        0
                    } else {
                        window::native(owner, 22)?
                    };
                    x11::errors::capture(|| unsafe {
                        x11::XSetSelectionOwner(
                            x11::connection::xcb_display(),
                            selection as usize,
                            owner,
                            time as u64,
                        )
                    })?;
                    Ok(None)
                }),
            }
        }
    };
}
owner_export!(xcb_set_selection_owner, false);
owner_export!(xcb_set_selection_owner_checked, true);
macro_rules! query_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            selection: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, true, || {
                    let owner = x11::errors::capture(|| unsafe {
                        x11::XGetSelectionOwner(x11::connection::xcb_display(), selection as usize)
                    })?;
                    let mut r = request::reply(0);
                    request::put32(&mut r, 8, wire_window(owner));
                    Ok(Some(r))
                }),
            }
        }
    };
}
query_export!(xcb_get_selection_owner, true);
query_export!(xcb_get_selection_owner_unchecked, false);
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_selection_owner_reply")]
pub unsafe extern "sysv64" fn xcb_get_selection_owner_reply(
    _c: *mut xcb_connection_t,
    k: xcb_void_cookie_t,
    e: *mut *mut xcb_generic_error_t,
) -> *mut c_void {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { request::take(k.sequence, e) }
}
macro_rules! convert_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            requestor: u32,
            selection: u32,
            target: u32,
            property: u32,
            time: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let requestor = window::native(requestor, 24)?;
                    x11::errors::capture(|| unsafe {
                        x11::XConvertSelection(
                            x11::connection::xcb_display(),
                            selection as usize,
                            target as usize,
                            property as usize,
                            requestor,
                            time as u64,
                        )
                    })?;
                    Ok(None)
                }),
            }
        }
    };
}
convert_export!(xcb_convert_selection, false);
convert_export!(xcb_convert_selection_checked, true);
fn wire_window(native: usize) -> u32 {
    if native < 2 {
        native as u32
    } else {
        xcb_state()
            .lock()
            .unwrap()
            .hwnd_to_wid
            .get(&native)
            .copied()
            .unwrap_or(native as u32)
    }
}
fn remap(event: &mut x11::XEvent, state: &XcbState) {
    let wire_window = |native: usize| {
        if native < 2 {
            native as u32
        } else {
            state
                .hwnd_to_wid
                .get(&native)
                .copied()
                .unwrap_or(native as u32)
        }
    };
    use x11::property::selection::*;
    let pointer = event as *mut x11::XEvent;
    unsafe {
        match event.r#type {
            2..=8 => {
                let e = &mut event.xkey;
                e.root = wire_window(e.root) as usize;
                e.window = wire_window(e.window) as usize;
                e.subwindow = wire_window(e.subwindow) as usize;
            }
            18 | 19 => {
                let e = &mut event.xmap;
                e.event = wire_window(e.event) as usize;
                e.window = wire_window(e.window) as usize;
            }
            21 => {
                let e = &mut *pointer.cast::<x11::window_tree::ReparentEvent>();
                e.event = wire_window(e.event) as usize;
                e.window = wire_window(e.window) as usize;
                e.parent = wire_window(e.parent) as usize;
            }
            22 => {
                let e = &mut event.xconfigure;
                e.event = wire_window(e.event) as usize;
                e.window = wire_window(e.window) as usize;
                e.above = wire_window(e.above) as usize;
            }
            29 => {
                let e = &mut *pointer.cast::<ClearEvent>();
                e.window = wire_window(e.window) as usize;
            }
            30 => {
                let e = &mut *pointer.cast::<RequestEvent>();
                e.owner = wire_window(e.owner) as usize;
                e.requestor = wire_window(e.requestor) as usize;
            }
            31 => {
                let e = &mut *pointer.cast::<NotifyEvent>();
                e.requestor = wire_window(e.requestor) as usize;
            }
            _ => {
                event.xany.window = wire_window(event.xany.window) as usize;
            }
        }
    }
}
fn append_shared(state: &mut XcbState, mut event: x11::XEvent) {
    remap(&mut event, state);
    let mut bytes = [0u8; 36];
    if unsafe {
        x11::event_wire::_XEventToWire(
            x11::connection::xcb_display(),
            &mut event,
            bytes.as_mut_ptr(),
        )
    } != 0
    {
        if unsafe { event.xany.send_event } != 0 {
            bytes[0] |= 0x80;
        }
        state.pending_events.push_back(unsafe {
            bytes
                .as_ptr()
                .cast::<xcb_generic_event_t>()
                .read_unaligned()
        });
    }
}
pub(super) fn wire_event(wire: [u8; 32], state: &XcbState) -> xcb_generic_event_t {
    let mut bytes = [0u8; 36];
    bytes[..32].copy_from_slice(&wire);
    let offsets: &[usize] = match wire[0] & 127 {
        2..=8 => &[8, 12, 16],
        9 | 10 | 12..=15 | 24 | 25 | 28 | 33 => &[4],
        16..=20 => &[4, 8],
        21..=23 => &[4, 8, 12],
        29 | 31 => &[8],
        30 => &[8, 12],
        x11::damage::EVENT => &[4],
        _ => &[],
    };
    for &offset in offsets {
        let native = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        if let Some(wid) = state.hwnd_to_wid.get(&native) {
            bytes[offset..offset + 4].copy_from_slice(&wid.to_le_bytes());
        }
    }
    unsafe {
        bytes
            .as_ptr()
            .cast::<xcb_generic_event_t>()
            .read_unaligned()
    }
}
pub(super) fn drain_shared_events() {
    if !x11::connection::xcb_events() {
        return;
    }
    let mut state = xcb_state().lock().unwrap();
    while let Some(event) = x11::take_wire_event() {
        append_shared(&mut state, event);
    }
}
// Prepare runs in descending priority: drain before any connection/resource
// freeze. Never wait on a mutex which a suspended guest thread might own.
unsafe extern "system" fn prepare_events() -> i32 {
    match x11::connection::try_xcb_events() {
        Ok(false) => return 0,
        Err(e) => return e,
        _ => {}
    }
    let Ok(mut state) = xcb_state().try_lock() else {
        return 11;
    };
    loop {
        match x11::xext::protocol::try_next_event() {
            Ok(Some(wire)) => {
                let event = wire_event(wire, &state);
                state.pending_events.push_back(event);
            }
            Ok(None) => break,
            Err(e) => return e,
        }
    }
    loop {
        match x11::try_take_wire_event() {
            Ok(Some(e)) => append_shared(&mut state, e),
            Ok(None) => return 0,
            Err(e) => return e,
        }
    }
}
extern "C" fn register() {
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 453,
        key: u64::from_le_bytes(*b"CYXCBQE1"),
        prepare: Some(prepare_events),
        snapshot: None,
        parent: None,
        child: None,
    });
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
macro_rules! send_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            propagate: u8,
            destination: u32,
            mask: u32,
            p: *const c_char,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            let mut event = None;
            let sequence = request::submit($checked, false, || {
                let target = window::native(destination, 25)?;
                if propagate > 1 || p.is_null() {
                    return Err(request::error(2, 25, propagate as u32));
                }
                if mask & !0x01ff_ffff != 0 {
                    return Err(request::error(2, 25, mask));
                }
                if propagate != 0 {
                    return Err(request::error(17, 25, destination));
                }
                let mut b = [0u8; 36];
                unsafe { core::ptr::copy_nonoverlapping(p.cast::<u8>(), b.as_mut_ptr(), 32) };
                if !(2..=34).contains(&(b[0] & 127)) {
                    return Err(request::error(2, 25, b[0] as u32));
                }
                if b[0] & 127 == 33 && !matches!(b[1], 8 | 16 | 32) {
                    return Err(request::error(2, 25, b[1] as u32));
                }
                // With no mask, deliver to the destination owner. Otherwise
                // deliver only when its client selected at least one mask bit.
                // A valid request without subscribers succeeds without an event.
                if mask == 0 || x11::property::notify::selected(target) & mask != 0 {
                    b[0] |= 128;
                    event = Some(b);
                }
                Ok(None)
            });
            if let Some(mut b) = event {
                request::put16(&mut b, 2, sequence as u16);
                request::put32(&mut b, 32, sequence);
                xcb_state()
                    .lock()
                    .unwrap()
                    .pending_events
                    .push_back(unsafe {
                        b.as_ptr().cast::<xcb_generic_event_t>().read_unaligned()
                    });
                x11::notify_event();
            }
            xcb_void_cookie_t { sequence }
        }
    };
}
send_export!(xcb_send_event, false);
send_export!(xcb_send_event_checked, true);
