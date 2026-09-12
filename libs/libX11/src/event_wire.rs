//! Xlib event conversion hooks and 32-byte core wire events. Callbacks execute
//! outside the registry lock, so a converter may replace itself or call Xlib.
use crate::{Display, XEvent};
use core::ffi::c_int;
use std::collections::BTreeMap;
use std::sync::Mutex;
mod lifecycle;
pub type Converter = Option<unsafe extern "sysv64" fn(*mut Display, *mut XEvent, *mut u8) -> c_int>;
#[derive(Default)]
struct State {
    callbacks: BTreeMap<(usize, u8, bool), usize>,
    active: usize,
}
static STATE: Mutex<State> = Mutex::new(State {
    callbacks: BTreeMap::new(),
    active: 0,
});
unsafe extern "sysv64" fn unknown(
    _display: *mut Display,
    _event: *mut XEvent,
    _wire: *mut u8,
) -> c_int {
    0
}
fn replace(display: *mut Display, kind: c_int, outgoing: bool, callback: Converter) -> Converter {
    if !(0..128).contains(&kind) {
        return None;
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let old = state.callbacks.insert(
        (display as usize, kind as u8, outgoing),
        callback.unwrap_or(unknown) as usize,
    );
    old.map(|old| unsafe { core::mem::transmute(old) })
        .or(if outgoing { None } else { Some(_XWireToEvent) })
}
#[unsafe(export_name = "kinakaze_engine_libX11_XESetEventToWire")]
pub unsafe extern "sysv64" fn XESetEventToWire(
    display: *mut Display,
    kind: c_int,
    callback: Converter,
) -> Converter {
    replace(display, kind, true, callback)
}
#[unsafe(export_name = "kinakaze_engine_libX11_XESetWireToEvent")]
pub unsafe extern "sysv64" fn XESetWireToEvent(
    display: *mut Display,
    kind: c_int,
    callback: Converter,
) -> Converter {
    replace(display, kind, false, callback)
}
struct Active;
impl Drop for Active {
    fn drop(&mut self) {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).active -= 1;
    }
}
unsafe fn convert(
    display: *mut Display,
    event: *mut XEvent,
    wire: *mut u8,
    kind: u8,
    outgoing: bool,
) -> bool {
    let (address, _active) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let address = state
            .callbacks
            .get(&(display as usize, kind, outgoing))
            .copied();
        state.active += 1;
        (address, Active)
    };
    let callback = address
        .map(|p| unsafe {
            core::mem::transmute::<
                usize,
                unsafe extern "sysv64" fn(*mut Display, *mut XEvent, *mut u8) -> c_int,
            >(p)
        })
        .unwrap_or(if outgoing {
            _XEventToWire
        } else {
            _XWireToEvent
        });
    unsafe { callback(display, event, wire) != 0 }
}
/// Encode and decode through the same hook chain used for wire-originated events.
pub unsafe fn sent(display: *mut Display, event: *mut XEvent) -> Result<Option<XEvent>, ()> {
    if event.is_null() {
        return Err(());
    }
    let kind = unsafe { (*event).r#type };
    if !(2..128).contains(&kind) {
        return Err(());
    }
    let mut wire = [0u8; 32];
    if !unsafe { convert(display, event, wire.as_mut_ptr(), kind as u8, true) } {
        return Err(());
    }
    wire[0] |= 0x80;
    let mut decoded: XEvent = unsafe { core::mem::zeroed() };
    if unsafe {
        convert(
            display,
            &mut decoded,
            wire.as_mut_ptr(),
            wire[0] & 0x7f,
            false,
        )
    } {
        Ok(Some(decoded))
    } else {
        Ok(None)
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11__XEnq")]
pub unsafe extern "sysv64" fn _XEnq(display: *mut Display, wire: *mut u8) {
    if wire.is_null() {
        return;
    }
    let mut decoded: XEvent = unsafe { core::mem::zeroed() };
    if unsafe { convert(display, &mut decoded, wire, *wire & 0x7f, false) } {
        if crate::trace_enabled() && matches!(unsafe { decoded.r#type }, 17 | 20 | 23) {
            crate::diagnostic!(
                "[x11-shared] enqueue pid={} kind={} window={}",
                crate::shared::pid(),
                unsafe { decoded.r#type },
                unsafe { decoded.xmap.window }
            );
        }
        crate::queue_extension_event(display, decoded);
    }
}
fn put16(wire: &mut [u8; 32], offset: usize, value: c_int) {
    wire[offset..offset + 2].copy_from_slice(&(value as u16).to_le_bytes());
}
fn put32(wire: &mut [u8; 32], offset: usize, value: u64) {
    wire[offset..offset + 4].copy_from_slice(&(value as u32).to_le_bytes());
}
fn get16(wire: &[u8; 32], offset: usize) -> u16 {
    u16::from_le_bytes(wire[offset..offset + 2].try_into().unwrap())
}
fn get32(wire: &[u8; 32], offset: usize) -> u32 {
    u32::from_le_bytes(wire[offset..offset + 4].try_into().unwrap())
}

#[unsafe(export_name = "kinakaze_engine_libX11__XEventToWire")]
pub unsafe extern "sysv64" fn _XEventToWire(
    _display: *mut Display,
    event: *mut XEvent,
    wire: *mut u8,
) -> c_int {
    if event.is_null() || wire.is_null() {
        return 0;
    }
    let event = unsafe { &*event };
    let wire = unsafe { &mut *wire.cast::<[u8; 32]>() };
    wire.fill(0);
    wire[0] = unsafe { event.r#type } as u8;
    put16(wire, 2, unsafe { event.xany.serial } as c_int);
    match wire[0] {
        2..=6 => {
            let e = unsafe { event.xkey };
            wire[1] = if wire[0] == 6 {
                (unsafe { event.xmotion.is_hint }) as u8
            } else {
                e.keycode as u8
            };
            put32(wire, 4, e.time as u64);
            put32(wire, 8, e.root as u64);
            put32(wire, 12, e.window as u64);
            put32(wire, 16, e.subwindow as u64);
            for (offset, value) in [
                (20, e.x_root),
                (22, e.y_root),
                (24, e.x),
                (26, e.y),
                (28, e.state as c_int),
            ] {
                put16(wire, offset, value);
            }
            wire[30] = u8::from(e.same_screen != 0);
        }
        7 | 8 => {
            let e = unsafe { event.xcrossing };
            wire[1] = e.detail as u8;
            put32(wire, 4, e.time as u64);
            put32(wire, 8, e.root as u64);
            put32(wire, 12, e.window as u64);
            put32(wire, 16, e.subwindow as u64);
            for (offset, value) in [
                (20, e.x_root),
                (22, e.y_root),
                (24, e.x),
                (26, e.y),
                (28, e.state as c_int),
            ] {
                put16(wire, offset, value);
            }
            wire[30] = e.mode as u8;
            wire[31] = u8::from(e.same_screen != 0) | (u8::from(e.focus != 0) << 1);
        }
        9 | 10 => {
            let e = unsafe { event.xfocus };
            wire[1] = e.detail as u8;
            put32(wire, 4, e.window as u64);
            wire[8] = e.mode as u8;
        }
        12 => {
            let e = unsafe { event.xexpose };
            put32(wire, 4, e.window as u64);
            for (offset, value) in [
                (8, e.x),
                (10, e.y),
                (12, e.width),
                (14, e.height),
                (16, e.count),
            ] {
                put16(wire, offset, value);
            }
        }
        15 => {
            let e = unsafe { event.xvisibility };
            put32(wire, 4, e.window as u64);
            wire[8] = e.state as u8;
        }
        16 => {
            let e = unsafe { &*(event as *const XEvent).cast::<crate::shared::CreateEvent>() };
            put32(wire, 4, e.parent as u64);
            put32(wire, 8, e.window as u64);
            for (offset, value) in [
                (12, e.x),
                (14, e.y),
                (16, e.width),
                (18, e.height),
                (20, e.border),
            ] {
                put16(wire, offset, value);
            }
            wire[22] = e.override_redirect as u8;
        }
        17 | 18 | 19 | 20 => {
            let e = unsafe { event.xmap };
            put32(wire, 4, e.event as u64);
            put32(wire, 8, e.window as u64);
            wire[12] = e.override_redirect as u8;
        }
        21 => {
            let e =
                unsafe { &*(event as *const XEvent).cast::<crate::window_tree::ReparentEvent>() };
            put32(wire, 4, e.event as u64);
            put32(wire, 8, e.window as u64);
            put32(wire, 12, e.parent as u64);
            put16(wire, 16, e.x);
            put16(wire, 18, e.y);
            wire[20] = e.override_redirect as u8;
        }
        28 => {
            let e = unsafe {
                &*(event as *const XEvent).cast::<crate::property::notify::PropertyEvent>()
            };
            put32(wire, 4, e.window as u64);
            put32(wire, 8, e.atom as u64);
            put32(wire, 12, e.time);
            wire[16] = e.state as u8;
        }
        29 => {
            let e = unsafe {
                &*(event as *const XEvent).cast::<crate::property::selection::ClearEvent>()
            };
            put32(wire, 4, e.time);
            put32(wire, 8, e.window as u64);
            put32(wire, 12, e.selection as u64);
        }
        30 => {
            let e = unsafe {
                &*(event as *const XEvent).cast::<crate::property::selection::RequestEvent>()
            };
            for (offset, value) in [
                (4, e.time),
                (8, e.owner as u64),
                (12, e.requestor as u64),
                (16, e.selection as u64),
                (20, e.target as u64),
                (24, e.property as u64),
            ] {
                put32(wire, offset, value);
            }
        }
        31 => {
            let e = unsafe {
                &*(event as *const XEvent).cast::<crate::property::selection::NotifyEvent>()
            };
            for (offset, value) in [
                (4, e.time),
                (8, e.requestor as u64),
                (12, e.selection as u64),
                (16, e.target as u64),
                (20, e.property as u64),
            ] {
                put32(wire, offset, value);
            }
        }
        22 => {
            let e = unsafe { event.xconfigure };
            put32(wire, 4, e.event as u64);
            put32(wire, 8, e.window as u64);
            put32(wire, 12, e.above as u64);
            for (offset, value) in [
                (16, e.x),
                (18, e.y),
                (20, e.width),
                (22, e.height),
                (24, e.border_width),
            ] {
                put16(wire, offset, value);
            }
            wire[26] = u8::from(e.override_redirect != 0);
        }
        33 => {
            let e = unsafe { event.xclient };
            wire[1] = e.format as u8;
            put32(wire, 4, e.window as u64);
            put32(wire, 8, e.message_type as u64);
            match e.format {
                8 | 16 => unsafe {
                    core::ptr::copy_nonoverlapping(
                        e.data.as_ptr().cast::<u8>(),
                        wire.as_mut_ptr().add(12),
                        20,
                    );
                },
                32 => {
                    for (index, value) in e.data.into_iter().enumerate() {
                        put32(wire, 12 + index * 4, value as u64);
                    }
                }
                _ => return 0,
            }
        }
        _ => return 0,
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11__XWireToEvent")]
pub unsafe extern "sysv64" fn _XWireToEvent(
    display: *mut Display,
    event: *mut XEvent,
    wire: *mut u8,
) -> c_int {
    if event.is_null() || wire.is_null() {
        return 0;
    }
    let event = unsafe { &mut *event };
    let wire = unsafe { &*wire.cast::<[u8; 32]>() };
    *event = unsafe { core::mem::zeroed() };
    event.xany = crate::XAnyEvent {
        r#type: (wire[0] & 0x7f) as c_int,
        serial: get16(wire, 2) as u64,
        send_event: c_int::from(wire[0] & 0x80 != 0),
        display,
        window: get32(wire, 4) as usize,
    };
    match wire[0] & 0x7f {
        28 => {
            let e = unsafe {
                &mut *(event as *mut XEvent).cast::<crate::property::notify::PropertyEvent>()
            };
            e.atom = get32(wire, 8) as usize;
            e.time = get32(wire, 12) as u64;
            e.state = wire[16] as c_int;
        }
        2..=6 => {
            let e = unsafe { &mut event.xkey };
            e.time = get32(wire, 4) as u64;
            e.root = get32(wire, 8) as usize;
            e.window = get32(wire, 12) as usize;
            e.subwindow = get32(wire, 16) as usize;
            e.x_root = get16(wire, 20) as i16 as c_int;
            e.y_root = get16(wire, 22) as i16 as c_int;
            e.x = get16(wire, 24) as i16 as c_int;
            e.y = get16(wire, 26) as i16 as c_int;
            e.state = get16(wire, 28) as u32;
            e.keycode = wire[1] as u32;
            e.same_screen = c_int::from(wire[30] != 0);
        }
        7 | 8 => {
            let e = unsafe { &mut event.xcrossing };
            e.detail = wire[1] as c_int;
            e.time = get32(wire, 4) as u64;
            e.root = get32(wire, 8) as usize;
            e.window = get32(wire, 12) as usize;
            e.subwindow = get32(wire, 16) as usize;
            e.x_root = get16(wire, 20) as i16 as c_int;
            e.y_root = get16(wire, 22) as i16 as c_int;
            e.x = get16(wire, 24) as i16 as c_int;
            e.y = get16(wire, 26) as i16 as c_int;
            e.state = get16(wire, 28) as u32;
            e.mode = wire[30] as c_int;
            e.same_screen = c_int::from(wire[31] & 1 != 0);
            e.focus = c_int::from(wire[31] & 2 != 0);
        }
        9 | 10 => {
            let e = unsafe { &mut event.xfocus };
            e.detail = wire[1] as c_int;
            e.mode = wire[8] as c_int;
        }
        12 => {
            let e = unsafe { &mut event.xexpose };
            e.x = get16(wire, 8) as c_int;
            e.y = get16(wire, 10) as c_int;
            e.width = get16(wire, 12) as c_int;
            e.height = get16(wire, 14) as c_int;
            e.count = get16(wire, 16) as c_int;
        }
        15 => {
            event.xvisibility.state = wire[8] as c_int;
        }
        16 => {
            let e = unsafe { &mut *(event as *mut XEvent).cast::<crate::shared::CreateEvent>() };
            e.parent = get32(wire, 4) as usize;
            e.window = get32(wire, 8) as usize;
            e.x = get16(wire, 12) as i16 as i32;
            e.y = get16(wire, 14) as i16 as i32;
            e.width = get16(wire, 16) as i32;
            e.height = get16(wire, 18) as i32;
            e.border = get16(wire, 20) as i32;
            e.override_redirect = wire[22] as i32;
        }
        17 | 18 | 19 | 20 => {
            let e = unsafe { &mut event.xmap };
            e.event = get32(wire, 4) as usize;
            e.window = get32(wire, 8) as usize;
            e.override_redirect = wire[12] as i32;
        }
        23 => {
            let e =
                unsafe { &mut *(event as *mut XEvent).cast::<crate::shared::ConfigureRequest>() };
            e.parent = get32(wire, 4) as usize;
            e.window = get32(wire, 8) as usize;
            e.above = get32(wire, 12) as usize;
            e.x = get16(wire, 16) as i16 as i32;
            e.y = get16(wire, 18) as i16 as i32;
            e.width = get16(wire, 20) as i32;
            e.height = get16(wire, 22) as i32;
            e.border = get16(wire, 24) as i32;
            e.mask = get16(wire, 26) as u64;
            e.detail = wire[1] as i32;
        }
        21 => {
            let e =
                unsafe { &mut *(event as *mut XEvent).cast::<crate::window_tree::ReparentEvent>() };
            e.event = get32(wire, 4) as usize;
            e.window = get32(wire, 8) as usize;
            e.parent = get32(wire, 12) as usize;
            e.x = get16(wire, 16) as i16 as i32;
            e.y = get16(wire, 18) as i16 as i32;
            e.override_redirect = wire[20] as i32;
        }
        29 => {
            let e = unsafe {
                &mut *(event as *mut XEvent).cast::<crate::property::selection::ClearEvent>()
            };
            e.time = get32(wire, 4) as u64;
            e.window = get32(wire, 8) as usize;
            e.selection = get32(wire, 12) as usize;
        }
        30 => {
            let e = unsafe {
                &mut *(event as *mut XEvent).cast::<crate::property::selection::RequestEvent>()
            };
            e.time = get32(wire, 4) as u64;
            e.owner = get32(wire, 8) as usize;
            e.requestor = get32(wire, 12) as usize;
            e.selection = get32(wire, 16) as usize;
            e.target = get32(wire, 20) as usize;
            e.property = get32(wire, 24) as usize;
        }
        31 => {
            let e = unsafe {
                &mut *(event as *mut XEvent).cast::<crate::property::selection::NotifyEvent>()
            };
            e.time = get32(wire, 4) as u64;
            e.requestor = get32(wire, 8) as usize;
            e.selection = get32(wire, 12) as usize;
            e.target = get32(wire, 16) as usize;
            e.property = get32(wire, 20) as usize;
        }
        22 => {
            let e = unsafe { &mut event.xconfigure };
            e.window = get32(wire, 8) as usize;
            e.above = get32(wire, 12) as usize;
            e.x = get16(wire, 16) as i16 as c_int;
            e.y = get16(wire, 18) as i16 as c_int;
            e.width = get16(wire, 20) as c_int;
            e.height = get16(wire, 22) as c_int;
            e.border_width = get16(wire, 24) as c_int;
            e.override_redirect = c_int::from(wire[26] != 0);
        }
        33 => {
            let e = unsafe { &mut event.xclient };
            e.format = wire[1] as c_int;
            e.message_type = get32(wire, 8) as usize;
            match e.format {
                8 | 16 => unsafe {
                    core::ptr::copy_nonoverlapping(
                        wire.as_ptr().add(12),
                        e.data.as_mut_ptr().cast::<u8>(),
                        20,
                    );
                },
                32 => {
                    for (index, value) in e.data.iter_mut().enumerate() {
                        *value = get32(wire, 12 + index * 4) as i32 as i64;
                    }
                }
                _ => return 0,
            }
        }
        _ => return 0,
    }
    1
}
