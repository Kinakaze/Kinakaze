//! Change notifications for the hosted primary output. Mode changes unsupported
//! by the host remain failures; subscriptions observe actual screen changes.
use super::*;
use std::{collections::BTreeMap, sync::Mutex};
pub const EVENT: i32 = 112;
static SUBSCRIPTIONS: Mutex<BTreeMap<(usize, usize), (i32, Screen)>> = Mutex::new(BTreeMap::new());
#[repr(C)]
struct ScreenEvent {
    kind: i32,
    serial: u64,
    send: i32,
    display: *mut Display,
    window: Window,
    root: Window,
    timestamp: Time,
    config_timestamp: Time,
    size_index: u16,
    subpixel_order: u16,
    rotation: u16,
    width: i32,
    height: i32,
    mwidth: i32,
    mheight: i32,
}
unsafe extern "sysv64" fn decode(d: *mut Display, out: *mut XEvent, wire: *mut u8) -> i32 {
    if out.is_null() || wire.is_null() {
        return 0;
    }
    let b = unsafe { core::slice::from_raw_parts(wire, 32) };
    if b[0] & 127 != EVENT as u8 {
        return 0;
    }
    let u16_at = |n| u16::from_le_bytes([b[n], b[n + 1]]);
    let u32_at = |n| u32::from_le_bytes(b[n..n + 4].try_into().unwrap());
    unsafe {
        out.cast::<ScreenEvent>().write(ScreenEvent {
            kind: EVENT,
            serial: u16_at(2) as u64,
            send: (b[0] & 128 != 0) as i32,
            display: d,
            window: u32_at(16) as usize,
            root: u32_at(12) as usize,
            timestamp: u32_at(4) as u64,
            config_timestamp: u32_at(8) as u64,
            size_index: u16_at(20),
            subpixel_order: u16_at(22),
            rotation: b[1] as u16,
            width: u16_at(24) as i32,
            height: u16_at(26) as i32,
            mwidth: u16_at(28) as i32,
            mheight: u16_at(30) as i32,
        });
    }
    1
}
pub fn initialize(d: *mut Display) {
    kinakaze_libX11::xext::protocol::register_poll(poll);
    unsafe {
        kinakaze_libX11::event_wire::XESetWireToEvent(d, EVENT, Some(decode));
    }
}
pub fn select(d: *mut Display, w: Window, mask: i32) {
    initialize(d);
    if mask & !15 != 0 {
        unsafe {
            kinakaze_libX11::errors::report(d, 2, 140, 4, mask as usize);
        }
        return;
    }
    let mut attributes = unsafe { mem::zeroed() };
    if unsafe { kinakaze_libX11::XGetWindowAttributes(d, w, &mut attributes) } == 0 {
        return;
    }
    let mut subscriptions = SUBSCRIPTIONS.lock().unwrap_or_else(|e| e.into_inner());
    if mask == 0 {
        subscriptions.remove(&(d as usize, w));
    } else if let Some(screen) = current() {
        subscriptions.insert((d as usize, w), (mask, screen));
    }
}
fn poll(d: *mut Display, close: bool) {
    let mut subscriptions = SUBSCRIPTIONS.lock().unwrap_or_else(|e| e.into_inner());
    if close {
        subscriptions.retain(|(display, _), _| *display != d as usize);
        return;
    }
    let Some(screen) = current() else {
        return;
    };
    for (&(display, window), (mask, old)) in subscriptions.iter_mut() {
        if display != d as usize || *old == screen {
            continue;
        }
        *old = screen;
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u32;
        let mut b = [0u8; 32];
        use kinakaze_libX11::xkb::server::{put16, put32};
        if *mask & 1 != 0 {
            b[0] = EVENT as u8;
            b[1] = 1;
            put32(&mut b, 4, time);
            put32(&mut b, 8, time);
            put32(&mut b, 12, 1);
            put32(&mut b, 16, window as u32);
            put16(&mut b, 24, screen.width as u16);
            put16(&mut b, 26, screen.height as u16);
            put16(&mut b, 28, screen.mm_width as u16);
            put16(&mut b, 30, screen.mm_height as u16);
            kinakaze_libX11::xext::protocol::queue_event(b);
        }
        if *mask & 2 != 0 {
            let mut b = [0; 32];
            b[0] = EVENT as u8 + 1;
            put32(&mut b, 4, time);
            put32(&mut b, 8, window as u32);
            put32(&mut b, 12, 1);
            put32(&mut b, 16, 1);
            put16(&mut b, 20, 1);
            put16(&mut b, 28, screen.width as u16);
            put16(&mut b, 30, screen.height as u16);
            kinakaze_libX11::xext::protocol::queue_event(b);
        }
        if *mask & 4 != 0 {
            let mut b = [0; 32];
            b[0] = EVENT as u8 + 1;
            b[1] = 1;
            put32(&mut b, 4, time);
            put32(&mut b, 8, time);
            put32(&mut b, 12, window as u32);
            put32(&mut b, 16, 1);
            put32(&mut b, 20, 1);
            put32(&mut b, 24, 1);
            put16(&mut b, 28, 1);
            kinakaze_libX11::xext::protocol::queue_event(b);
        }
    }
}
pub unsafe fn update(event: *mut XEvent) -> i32 {
    if event.is_null() || unsafe { (*event).r#type } != EVENT {
        return 0;
    }
    let e = unsafe { &*event.cast::<ScreenEvent>() };
    if !e.display.is_null() {
        let screen = unsafe { (*e.display).screens };
        if !screen.is_null() {
            unsafe {
                (*screen).width = e.width;
                (*screen).height = e.height;
                (*screen).mwidth = e.mwidth;
                (*screen).mheight = e.mheight;
            }
        }
    }
    1
}
