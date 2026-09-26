//! `xf86vmode.rs`: XF86VidMode extension stubs.

use crate::{Bool, Display};
use core::ffi::c_int;

#[unsafe(export_name = "kinakaze_engine_libX11_XF86VidModeQueryExtension")]
pub unsafe extern "sysv64" fn XF86VidModeQueryExtension(
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

#[unsafe(export_name = "kinakaze_engine_libX11_XF86VidModeGetGammaRampSize")]
pub unsafe extern "sysv64" fn XF86VidModeGetGammaRampSize(
    _dpy: *mut Display,
    _screen: c_int,
    size_return: *mut c_int,
) -> Bool {
    if !size_return.is_null() {
        unsafe { *size_return = 256 };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XF86VidModeGetGammaRamp")]
pub unsafe extern "sysv64" fn XF86VidModeGetGammaRamp(
    _dpy: *mut Display,
    _screen: c_int,
    size: c_int,
    red_array: *mut u16,
    green_array: *mut u16,
    blue_array: *mut u16,
) -> Bool {
    if !red_array.is_null() && size > 0 {
        unsafe { core::ptr::write_bytes(red_array, 0, size as usize) };
    }
    if !green_array.is_null() && size > 0 {
        unsafe { core::ptr::write_bytes(green_array, 0, size as usize) };
    }
    if !blue_array.is_null() && size > 0 {
        unsafe { core::ptr::write_bytes(blue_array, 0, size as usize) };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XF86VidModeSetGammaRamp")]
pub unsafe extern "sysv64" fn XF86VidModeSetGammaRamp(
    _dpy: *mut Display,
    _screen: c_int,
    _size: c_int,
    _red_array: *mut u16,
    _green_array: *mut u16,
    _blue_array: *mut u16,
) -> Bool {
    1
}
