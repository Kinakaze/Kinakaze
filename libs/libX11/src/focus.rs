//! Virtual focus and its native UI-thread operation; no host HWND is serialized.
use crate::{Display, Time, Window, errors};
use std::sync::Mutex;
use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
mod lifecycle;
const FOREIGN: u64 = 1 << 63;
fn logical(window: usize) -> Option<u64> {
    kinakaze_libdisplay::window::logical_for_native(window)
        .or_else(|| crate::shared::valid(window).then_some(window as u64 | FOREIGN))
}
fn native(window: u64) -> Option<usize> {
    if window & FOREIGN == 0 {
        kinakaze_libdisplay::window::native_handle(window)
    } else {
        let value = (window & !FOREIGN) as usize;
        crate::shared::valid(value).then_some(value)
    }
}
#[derive(Clone, Copy)]
struct Focus {
    mode: u8,
    window: u64,
    revert: u8,
    time: u32,
    grab: Option<Grab>,
}
#[derive(Clone, Copy)]
struct Grab {
    window: u64,
    owner_events: bool,
    time: u32,
}
static FOCUS: Mutex<Focus> = Mutex::new(Focus {
    mode: 1,
    window: 0,
    revert: 0,
    time: 0,
    grab: None,
});
pub(crate) fn now() -> u32 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetTickCount64() -> u64;
    }
    unsafe { GetTickCount64() as u32 }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetInputFocus")]
pub unsafe extern "sysv64" fn XSetInputFocus(
    d: *mut Display,
    target: Window,
    revert: i32,
    time: Time,
) -> i32 {
    unsafe { crate::issue_request(d) };
    if !(0..=2).contains(&revert) {
        return unsafe { errors::report(d, 2, 42, 0, revert as usize) };
    }
    let window = if target > 1 {
        let Some(id) = logical(target) else {
            return unsafe { errors::report(d, 3, 42, 0, target) };
        };
        if unsafe { IsWindowVisible(target as _) } == 0 {
            return unsafe { errors::report(d, 8, 42, 0, target) };
        }
        id
    } else {
        0
    };
    let current = now();
    let time = if time == 0 { current } else { time as u32 };
    let mut s = FOCUS.lock().unwrap();
    if (s.time != 0 && (time.wrapping_sub(s.time) as i32) < 0)
        || (time.wrapping_sub(current) as i32) > 0
    {
        return 1;
    }
    if target != 1 && !kinakaze_libdisplay::ui::set_focus(target) {
        drop(s);
        return unsafe { errors::report(d, 17, 42, 0, target) };
    }
    *s = Focus {
        mode: if target > 1 { 2 } else { target as u8 },
        window,
        revert: revert as u8,
        time,
        grab: s.grab,
    };
    let mut record = (target as u64).to_le_bytes().to_vec();
    record.extend_from_slice(&revert.to_le_bytes());
    record.extend_from_slice(&time.to_le_bytes());
    let _ = crate::shared::set("focus".into(), record);
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetInputFocus")]
pub unsafe extern "sysv64" fn XGetInputFocus(
    _d: *mut Display,
    target: *mut Window,
    revert: *mut i32,
) -> i32 {
    if let Some(record) = crate::shared::get("focus")
        && record.len() == 16
    {
        let window = u64::from_le_bytes(record[..8].try_into().unwrap()) as usize;
        if window < 2 || crate::shared::valid(window) {
            if !target.is_null() {
                unsafe {
                    *target = window;
                }
            }
            if !revert.is_null() {
                unsafe {
                    *revert = i32::from_le_bytes(record[8..12].try_into().unwrap());
                }
            }
            return 1;
        }
    }
    let s = *FOCUS.lock().unwrap();
    if !target.is_null() {
        unsafe {
            target.write(if s.mode < 2 {
                s.mode as usize
            } else {
                native(s.window).unwrap_or(0)
            })
        };
    }
    if !revert.is_null() {
        unsafe { revert.write(s.revert as i32) };
    }
    1
}
pub fn key_target(window: usize) -> Option<usize> {
    let s = *FOCUS.lock().unwrap();
    if let Some(grab) = s.grab {
        if !grab.owner_events || s.mode == 0 {
            return native(grab.window);
        }
    }
    match s.mode {
        0 => None,
        1 => Some(window),
        _ => native(s.window),
    }
}
pub(crate) fn destroying(window: usize) {
    let mut s = FOCUS.lock().unwrap();
    if s.grab.is_some_and(|g| native(g.window) == Some(window)) {
        s.grab = None;
    }
    if s.mode != 2 || native(s.window) != Some(window) {
        return;
    }
    if s.revert == 2 {
        let mut parent = kinakaze_libdisplay::window::hierarchy(Some(window)).and_then(|(p, _)| p);
        while let Some(native) = parent {
            if unsafe { IsWindowVisible(native as _) } != 0 {
                if let Some(id) = logical(native) {
                    if kinakaze_libdisplay::ui::set_focus(native) {
                        s.window = id;
                        s.revert = 0;
                        return;
                    }
                }
            }
            parent = kinakaze_libdisplay::window::hierarchy(Some(native)).and_then(|(p, _)| p);
        }
        s.mode = 1;
    } else {
        s.mode = s.revert;
    }
    s.window = 0;
    s.revert = 0;
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGrabKeyboard")]
pub unsafe extern "sysv64" fn XGrabKeyboard(
    d: *mut Display,
    window: Window,
    owner_events: i32,
    pointer_mode: i32,
    keyboard_mode: i32,
    time: Time,
) -> i32 {
    unsafe { crate::issue_request(d) };
    if !(0..=1).contains(&owner_events)
        || !(0..=1).contains(&pointer_mode)
        || !(0..=1).contains(&keyboard_mode)
    {
        return unsafe { errors::report(d, 2, 31, 0, window) };
    }
    // Synchronous freezing/replay requires the corresponding AllowEvents path.
    if pointer_mode == 0 || keyboard_mode == 0 {
        return unsafe { errors::report(d, 17, 31, 0, window) };
    }
    let Some(id) = logical(window) else {
        return unsafe { errors::report(d, 3, 31, 0, window) };
    };
    if unsafe { IsWindowVisible(window as _) } == 0 {
        return 3;
    }
    let current = now();
    let time = if time == 0 { current } else { time as u32 };
    let mut s = FOCUS.lock().unwrap();
    if (time.wrapping_sub(current) as i32) > 0
        || s.grab
            .is_some_and(|g| (time.wrapping_sub(g.time) as i32) < 0)
    {
        return 2;
    }
    s.grab = Some(Grab {
        window: id,
        owner_events: owner_events != 0,
        time,
    });
    0
}
#[unsafe(export_name = "kinakaze_engine_libX11_XUngrabKeyboard")]
pub unsafe extern "sysv64" fn XUngrabKeyboard(_d: *mut Display, time: Time) -> i32 {
    unsafe { crate::issue_request(_d) };
    let current = now();
    let time = if time == 0 { current } else { time as u32 };
    let mut s = FOCUS.lock().unwrap();
    if (time.wrapping_sub(current) as i32) <= 0
        && s.grab
            .is_some_and(|g| (time.wrapping_sub(g.time) as i32) >= 0)
    {
        s.grab = None;
    }
    1
}
