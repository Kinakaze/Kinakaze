//! Fork rebuilds native handles without displaying a second copy of the parent.
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(
        window: super::Handle,
        attribute: u32,
        value: *const core::ffi::c_void,
        size: u32,
    ) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetPropW(window: super::Handle, name: *const u16) -> super::Handle;
    fn SetPropW(window: super::Handle, name: *const u16, value: super::Handle) -> i32;
    fn RemovePropW(window: super::Handle, name: *const u16) -> super::Handle;
}
fn first_frame_key() -> *const u16 {
    static KEY: OnceLock<Vec<u16>> = OnceLock::new();
    KEY.get_or_init(|| super::wide("KinakazeFirstFrame"))
        .as_ptr()
}
fn frame_protocol_key() -> *const u16 {
    static KEY: OnceLock<Vec<u16>> = OnceLock::new();
    KEY.get_or_init(|| super::wide("KinakazeFrameProtocol"))
        .as_ptr()
}
pub fn frame_protocol(window: usize, enabled: bool) {
    unsafe {
        if enabled {
            SetPropW(window as _, frame_protocol_key(), 1usize as _);
            RemovePropW(window as _, first_frame_key());
        } else {
            RemovePropW(window as _, frame_protocol_key());
        }
    }
}
/// Mapping still delivers Expose while DWM holds back an unpainted surface.
/// The HWND property also covers maps requested by a foreign window manager.
pub(super) fn visibility(window: usize, visible: bool) {
    // WM frame containers and legacy X windows may never draw themselves.
    // Only clients that publish explicit frame boundaries can be held back.
    if unsafe { GetPropW(window as _, frame_protocol_key()) }.is_null() {
        return;
    }
    if super::desktop::is_desktop(window)
        || super::input_only::is_input_only(window)
        || unsafe { super::GetWindowLongPtrW(window as _, super::GWL_STYLE) }
            & super::WS_CHILD as isize
            != 0
    {
        return;
    }
    let hwnd = window as super::Handle;
    unsafe {
        if !visible {
            // Menus reuse a native window with entirely different content.
            if super::is_override_redirect(window) {
                RemovePropW(hwnd, first_frame_key());
            }
        } else if GetPropW(hwnd, first_frame_key()).is_null() {
            let cloak: i32 = 1;
            if DwmSetWindowAttribute(hwnd, 13, (&raw const cloak).cast(), 4) >= 0 {
                SetPropW(hwnd, first_frame_key(), 2usize as _);
            }
        }
    }
}
pub type PaintHandler = unsafe extern "system" fn(usize, usize, i32, i32, i32, i32) -> bool;
static PAINT: AtomicUsize = AtomicUsize::new(0);
pub fn set_paint_handler(handler: Option<PaintHandler>) {
    PAINT.store(handler.map_or(0, |h| h as usize), Ordering::Release);
}
pub(super) unsafe fn paint(window: usize, dc: usize, area: super::Rect) -> bool {
    let handler = PAINT.load(Ordering::Acquire);
    if handler == 0 {
        return false;
    }
    let handler: PaintHandler = unsafe { core::mem::transmute(handler) };
    unsafe {
        handler(
            window,
            dc,
            area.left,
            area.top,
            area.right - area.left,
            area.bottom - area.top,
        )
    }
}

fn deferred() -> &'static Mutex<HashSet<usize>> {
    static WINDOWS: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    WINDOWS.get_or_init(|| Mutex::new(HashSet::new()))
}
fn small_windows() -> &'static Mutex<HashMap<usize, isize>> {
    static WINDOWS: OnceLock<Mutex<HashMap<usize, isize>>> = OnceLock::new();
    WINDOWS.get_or_init(|| Mutex::new(HashMap::new()))
}
/// A 1x1 InputOutput window may be a GTK application's initial placeholder.
pub fn small_window(window: usize, original_extended: isize) {
    small_windows()
        .lock()
        .unwrap()
        .insert(window, original_extended);
}
pub(super) fn resized(window: usize, width: i32, height: i32) {
    if width <= 32
        || height <= 32
        || super::is_override_redirect(window)
        || super::input_only::is_input_only(window)
    {
        return;
    }
    let Some(original) = small_windows().lock().unwrap().remove(&window) else {
        return;
    };
    // Restore only the taskbar flags we changed; preserve other later hints.
    const TASKBAR_FLAGS: isize = 0x80 | 0x40000;
    unsafe {
        let hwnd = window as super::Handle;
        let extended = super::GetWindowLongPtrW(hwnd, super::GWL_EXSTYLE);
        super::SetWindowLongPtrW(
            hwnd,
            super::GWL_EXSTYLE,
            (extended & !TASKBAR_FLAGS) | (original & TASKBAR_FLAGS),
        );
    }
}

pub fn defer_until_frame(window: usize) {
    deferred()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(window);
}

pub fn forget(window: usize) {
    deferred()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&window);
    small_windows().lock().unwrap().remove(&window);
}

pub fn frame_ready(window: usize) {
    if !unsafe { GetPropW(window as _, frame_protocol_key()) }.is_null() {
        return;
    }
    client_frame_ready(window);
}
pub fn client_frame_ready(window: usize) {
    let hwnd = window as super::Handle;
    unsafe {
        let state = GetPropW(hwnd, first_frame_key()) as usize;
        if state != 1 {
            SetPropW(hwnd, first_frame_key(), 1usize as _);
            if state == 2 {
                let cloak: i32 = 0;
                DwmSetWindowAttribute(hwnd, 13, (&raw const cloak).cast(), 4);
            }
        }
    }
    let show = deferred()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&window);
    if show {
        super::set_visible(window, true);
    }
}
