//! XKB requests on the virtual keyboard. Unsupported map/geometry mutations
//! report failure; state and controls queries use the server's current data.
use super::*;
macro_rules! absent {
    ($name:ident($($arg:ident:$ty:ty),*) -> $result:ty = $value:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libX11_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name($($arg:$ty),*) -> $result { $value }
    };
}
#[unsafe(export_name = "kinakaze_engine_libX11_XkbUseExtension")]
pub unsafe extern "sysv64" fn XkbUseExtension(
    d: *mut Display,
    major: *mut c_int,
    minor: *mut c_int,
) -> Bool {
    unsafe {
        crate::XkbQueryExtension(
            d,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            major,
            minor,
        )
    }
}
absent!(XkbGetGeometry(_d:*mut Display,_x:*mut XkbDescRec)->Status=10);
absent!(XkbSetGeometry(_d:*mut Display,_device:c_uint,_g:*mut c_void)->Status=10);
#[unsafe(export_name = "kinakaze_engine_libX11_XkbGetIndicatorMap")]
pub unsafe extern "sysv64" fn XkbGetIndicatorMap(
    _d: *mut Display,
    which: c_ulong,
    x: *mut XkbDescRec,
) -> Status {
    unsafe { data::load_indicators(which, x) }
}
absent!(XkbSetIndicatorMap(_d:*mut Display,_which:c_ulong,_x:*mut XkbDescRec)->Bool=0);
#[unsafe(export_name = "kinakaze_engine_libX11_XkbGetCompatMap")]
pub unsafe extern "sysv64" fn XkbGetCompatMap(
    _d: *mut Display,
    which: c_uint,
    x: *mut XkbDescRec,
) -> Status {
    if which & !3 != 0 {
        return 2;
    }
    unsafe { data::XkbAllocCompatMap(x, which, 0) }
}
absent!(XkbSetCompatMap(_d:*mut Display,_which:c_uint,_x:*mut XkbDescRec,_actions:Bool)->Bool=0);
absent!(XkbSetNames(_d:*mut Display,_which:c_uint,_first:c_uint,_count:c_uint,_x:*mut XkbDescRec)->Bool=0);
absent!(XkbSetAutoRepeatRate(_d:*mut Display,_device:c_uint,_delay:c_uint,_interval:c_uint)->Bool=0);
absent!(XkbChangeEnabledControls(_d:*mut Display,_device:c_uint,_affect:c_uint,_values:c_uint)->Bool=0);
#[unsafe(export_name = "kinakaze_engine_libX11_XkbLockGroup")]
pub unsafe extern "sysv64" fn XkbLockGroup(
    _d: *mut Display,
    device: c_uint,
    group: c_uint,
) -> Bool {
    i32::from(device <= 65535 && server::valid_device(device as u16) && group == 0)
}
absent!(XkbChangeMap(_d:*mut Display,_x:*mut XkbDescRec,_changes:*mut c_void)->Bool=0);
absent!(XkbGetKeyboardByName(_d:*mut Display,_device:c_uint,_names:*mut c_void,_want:c_uint,_need:c_uint,_load:Bool)->*mut XkbDescRec=core::ptr::null_mut());
absent!(XkbRefreshKeyboardMapping(_event:*mut c_void)->Status=10);
absent!(XkbBellEvent(_d:*mut Display,_window:usize,_percent:c_int,_name:usize)->Bool=0);

#[unsafe(export_name = "kinakaze_engine_libX11_XkbForceDeviceBell")]
pub unsafe extern "sysv64" fn XkbForceDeviceBell(
    d: *mut Display,
    _device: c_int,
    _class: c_int,
    _id: c_int,
    percent: c_int,
) -> Bool {
    // Xlib falls back to the core Bell request when extension negotiation fails.
    unsafe {
        crate::XBell(d, percent);
    }
    0
}
