//! `xinerama.rs`: Xinerama stubs.

use crate::{Bool, Display, Status, XEvent};
use core::ffi::{c_char, c_int, c_short, c_void};

#[repr(C)]
pub struct XineramaScreenInfo {
    pub screen_number: c_int,
    pub x_org: c_short,
    pub y_org: c_short,
    pub width: c_short,
    pub height: c_short,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XineramaQueryExtension")]
pub unsafe extern "sysv64" fn XineramaQueryExtension(
    _dpy: *mut Display,
    event_base_return: *mut c_int,
    error_base_return: *mut c_int,
) -> Bool {
    if !event_base_return.is_null() {
        unsafe { *event_base_return = 0 };
    }
    if !error_base_return.is_null() {
        unsafe { *error_base_return = 0 };
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XineramaIsActive")]
pub unsafe extern "sysv64" fn XineramaIsActive(_dpy: *mut Display) -> Bool {
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XineramaQueryScreens")]
pub unsafe extern "sysv64" fn XineramaQueryScreens(
    _dpy: *mut Display,
    number: *mut c_int,
) -> *mut XineramaScreenInfo {
    if !number.is_null() {
        unsafe { *number = 0 };
    }
    core::ptr::null_mut()
}
