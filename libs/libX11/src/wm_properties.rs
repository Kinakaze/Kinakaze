//! ICCCM properties used by toolkit-created windows.
use crate::wm::{XClassHint, XWMHints};
use crate::{Display, Window, XSizeHints};
use core::ffi::{c_char, c_int};
use std::ffi::CStr;
pub(crate) mod query;

unsafe fn bytes(d: *mut Display, w: Window, atom: usize, kind: usize, data: &[u8]) {
    if let Ok(length) = c_int::try_from(data.len()) {
        unsafe {
            crate::XChangeProperty(d, w, atom, kind, 8, 0, data.as_ptr(), length);
        }
    }
}
pub(crate) unsafe fn set_names(
    d: *mut Display,
    w: Window,
    name: *const c_char,
    icon: *const c_char,
    argv: *mut *mut c_char,
    argc: c_int,
    normal: *mut XSizeHints,
    hints: *mut XWMHints,
    class: *mut XClassHint,
) {
    unsafe {
        for (value, atom) in [(name, 39), (icon, 37)] {
            if !value.is_null() {
                let mut value = value.cast_mut();
                let mut property = core::mem::zeroed();
                // UTF-8 is an explicit encoding, never mislabeled as STRING.
                if crate::Xutf8TextListToTextProperty(d, &mut value, 1, 4, &mut property) == 0 {
                    crate::property::XSetTextProperty(d, w, &property, atom);
                    crate::XFree(property.value.cast());
                }
            }
        }
        set_hints(d, w, argv, argc, normal, hints, class);
    }
}
pub(crate) unsafe fn set_hints(
    d: *mut Display,
    w: Window,
    argv: *mut *mut c_char,
    argc: c_int,
    normal: *mut XSizeHints,
    hints: *mut XWMHints,
    class: *mut XClassHint,
) {
    unsafe {
        if argc >= 0 && !argv.is_null() {
            let mut command = Vec::new();
            for index in 0..argc as usize {
                let arg = *argv.add(index);
                if !arg.is_null() {
                    command.extend_from_slice(CStr::from_ptr(arg).to_bytes());
                }
                command.push(0);
            }
            bytes(d, w, 34, 31, &command);
        }
        if !normal.is_null() {
            XSetWMSizeHints(d, w, normal, 40);
        }
        if !hints.is_null() {
            crate::property::XSetWMHints(d, w, hints);
        }
        if !class.is_null() {
            let mut value = Vec::new();
            for string in [(*class).res_name, (*class).res_class] {
                if !string.is_null() {
                    value.extend_from_slice(CStr::from_ptr(string).to_bytes());
                }
                value.push(0);
            }
            bytes(d, w, 67, 31, &value);
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XReconfigureWMWindow")]
pub unsafe extern "sysv64" fn XReconfigureWMWindow(
    d: *mut Display,
    w: Window,
    _screen: c_int,
    mask: u32,
    changes: *mut crate::wm::XWindowChanges,
) -> c_int {
    unsafe { crate::wm::XConfigureWindow(d, w, mask, changes) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWMSizeHints")]
pub unsafe extern "sysv64" fn XSetWMSizeHints(
    d: *mut Display,
    w: Window,
    hints: *mut XSizeHints,
    property: crate::Atom,
) {
    if hints.is_null() {
        return;
    }
    unsafe {
        let h = &*hints;
        let values = [
            h.flags as u64,
            h.x as u64,
            h.y as u64,
            h.width as u64,
            h.height as u64,
            h.min_width as u64,
            h.min_height as u64,
            h.max_width as u64,
            h.max_height as u64,
            h.width_inc as u64,
            h.height_inc as u64,
            h.min_aspect.0 as u64,
            h.min_aspect.1 as u64,
            h.max_aspect.0 as u64,
            h.max_aspect.1 as u64,
            h.base_width as u64,
            h.base_height as u64,
            h.win_gravity as u64,
        ];
        crate::XChangeProperty(d, w, property, 41, 32, 0, values.as_ptr().cast(), 18);
    }
}
