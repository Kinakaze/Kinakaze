//! XKB client compatibility when the native display exposes core keyboard events.

use crate::{Bool, Display, Status, c_ulong};
use core::ffi::{c_char, c_int, c_uchar, c_uint, c_ushort, c_void};
pub(crate) mod client;
pub(crate) mod data;
mod geometry;
mod requests;
pub mod server;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct XkbStateRec {
    pub group: c_uchar,
    pub locked_group: c_uchar,
    pub base_group: c_ushort,
    pub latched_group: c_ushort,
    pub mods: c_uchar,
    pub base_mods: c_uchar,
    pub latched_mods: c_uchar,
    pub locked_mods: c_uchar,
    pub compat_state: c_uchar,
    pub grab_mods: c_uchar,
    pub compat_grab_mods: c_uchar,
    pub lookup_mods: c_uchar,
    pub compat_lookup_mods: c_uchar,
    pub ptr_buttons: c_ushort,
}

#[repr(C)]
pub struct XkbDescRec {
    pub dpy: *mut Display,
    pub flags: c_ushort,
    pub device_spec: c_ushort,
    pub min_key_code: u8,
    pub max_key_code: u8,
    pub ctrls: *mut c_void,
    pub server: *mut c_void,
    pub map: *mut c_void,
    pub indicators: *mut c_void,
    pub names: *mut c_void,
    pub compat: *mut c_void,
    pub geom: *mut c_void,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbSelectEventDetails")]
pub unsafe extern "sysv64" fn XkbSelectEventDetails(
    _dpy: *mut Display,
    _device_spec: c_uint,
    _event_type: c_uint,
    _bits_to_change: c_ulong,
    _values_for_bits: c_ulong,
) -> Bool {
    if _device_spec > 65535 || !server::valid_device(_device_spec as u16) || _event_type >= 12 {
        return 0;
    }
    server::select(
        1 << _event_type,
        if _values_for_bits != 0 {
            1 << _event_type
        } else {
            0
        },
    );
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbSetDetectableAutoRepeat")]
pub unsafe extern "sysv64" fn XkbSetDetectableAutoRepeat(
    _dpy: *mut Display,
    _detectable: Bool,
    supported_rtrn: *mut Bool,
) -> Bool {
    if !supported_rtrn.is_null() {
        unsafe { *supported_rtrn = 1 };
    }
    server::detectable(_detectable != 0);
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbGetState")]
pub unsafe extern "sysv64" fn XkbGetState(
    _dpy: *mut Display,
    _device_spec: c_uint,
    state_return: *mut XkbStateRec,
) -> Status {
    if state_return.is_null() {
        return 2;
    }
    if _device_spec > 65535 || !server::valid_device(_device_spec as u16) {
        return server::ERROR as i32;
    }
    unsafe { state_return.write(server::state()) };
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbGetNames")]
pub unsafe extern "sysv64" fn XkbGetNames(
    _dpy: *mut Display,
    _which: c_uint,
    _xkb: *mut XkbDescRec,
) -> Status {
    unsafe { client::names(_which, _xkb) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbKeycodeToKeysym")]
pub unsafe extern "sysv64" fn XkbKeycodeToKeysym(
    dpy: *mut Display,
    kc: u8,
    group: c_uint,
    level: c_uint,
) -> usize {
    if group != 0 {
        return 0;
    }
    unsafe { crate::XKeycodeToKeysym(dpy, kc, level as c_int) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbFreeNames")]
pub unsafe extern "sysv64" fn XkbFreeNames(xkb: *mut XkbDescRec, which: c_uint, free_map: Bool) {
    unsafe { data::free_names(xkb, which, free_map) };
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbGetUpdatedMap")]
pub unsafe extern "sysv64" fn XkbGetUpdatedMap(
    _display: *mut Display,
    _which: c_uint,
    _map: *mut XkbDescRec,
) -> Status {
    unsafe { client::load_map(_display, _map, _which) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbLibraryVersion")]
pub unsafe extern "sysv64" fn XkbLibraryVersion(major: *mut c_int, minor: *mut c_int) -> Bool {
    if major.is_null() || minor.is_null() {
        return 0;
    }
    let compatible = unsafe { *major == 1 && *minor <= 0 };
    unsafe {
        *major = 1;
        *minor = 0;
    }
    compatible as Bool
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbGetControls")]
pub unsafe extern "sysv64" fn XkbGetControls(
    _display: *mut Display,
    _which: c_ulong,
    _map: *mut XkbDescRec,
) -> Status {
    unsafe { data::load_controls(_display, _map) }
}

// Xlib's setters first negotiate XKB. They return False without issuing a
// request when the server lacks the extension, as this native seat currently
// does. In particular, do not claim to have changed the host keyboard map.
#[unsafe(export_name = "kinakaze_engine_libX11_XkbSetControls")]
pub unsafe extern "sysv64" fn XkbSetControls(
    _display: *mut Display,
    _which: c_ulong,
    _map: *mut XkbDescRec,
) -> Bool {
    0
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbSetMap")]
pub unsafe extern "sysv64" fn XkbSetMap(
    _display: *mut Display,
    _which: c_uint,
    _map: *mut XkbDescRec,
) -> Bool {
    0
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbLockModifiers")]
pub unsafe extern "sysv64" fn XkbLockModifiers(
    _display: *mut Display,
    _device: c_uint,
    _affect: c_uint,
    _values: c_uint,
) -> Bool {
    if _device > 65535 || !server::valid_device(_device as u16) || _affect > 255 || _values > 255 {
        return 0;
    }
    server::lock_mods(_affect as u8, _values as u8, false);
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbLatchModifiers")]
pub unsafe extern "sysv64" fn XkbLatchModifiers(
    _display: *mut Display,
    _device: c_uint,
    _affect: c_uint,
    _values: c_uint,
) -> Bool {
    if _device > 65535 || !server::valid_device(_device as u16) || _affect > 255 || _values > 255 {
        return 0;
    }
    server::lock_mods(_affect as u8, _values as u8, true);
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbSelectEvents")]
pub unsafe extern "sysv64" fn XkbSelectEvents(
    _display: *mut Display,
    _device: c_uint,
    _affect: c_uint,
    _values: c_uint,
) -> Bool {
    if _device > 65535 || !server::valid_device(_device as u16) || _affect & !0xfff != 0 {
        return 0;
    }
    server::select(_affect as u16, _values as u16);
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbBell")]
pub unsafe extern "sysv64" fn XkbBell(
    display: *mut Display,
    _window: usize,
    percent: c_int,
    _name: usize,
) -> Bool {
    // Xlib specifies the core Bell request when XKB is unavailable.
    unsafe {
        crate::XBell(display, percent);
    }
    0
}
#[repr(C)]
pub struct XTimeCoord {
    pub time: u64,
    pub x: i16,
    pub y: i16,
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetMotionEvents")]
pub unsafe extern "sysv64" fn XGetMotionEvents(
    _display: *mut Display,
    _window: usize,
    _start: u64,
    _stop: u64,
    count: *mut c_int,
) -> *mut XTimeCoord {
    // Motion history is not retained; live motion remains in the event queue.
    if !count.is_null() {
        unsafe {
            *count = 0;
        }
    }
    core::ptr::null_mut()
}
