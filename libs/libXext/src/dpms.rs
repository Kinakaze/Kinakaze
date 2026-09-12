//! A hosted guest cannot control the host monitor's DPMS policy. The extension
//! is absent, and requests follow Xlib's False/zero negotiation failure path.
use super::*;
#[unsafe(export_name = "kinakaze_engine_libXext_DPMSQueryExtension")]
pub unsafe extern "sysv64" fn DPMSQueryExtension(
    _d: *mut Display,
    event: *mut c_int,
    error: *mut c_int,
) -> Bool {
    if !event.is_null() {
        unsafe {
            *event = 0;
        }
    }
    if !error.is_null() {
        unsafe {
            *error = 0;
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_DPMSCapable")]
pub unsafe extern "sysv64" fn DPMSCapable(_d: *mut Display) -> Bool {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_DPMSInfo")]
pub unsafe extern "sysv64" fn DPMSInfo(
    _d: *mut Display,
    _level: *mut u16,
    _state: *mut Bool,
) -> Status {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_DPMSSetTimeouts")]
pub unsafe extern "sysv64" fn DPMSSetTimeouts(
    _d: *mut Display,
    _standby: u16,
    _suspend: u16,
    _off: u16,
) -> Status {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_DPMSForceLevel")]
pub unsafe extern "sysv64" fn DPMSForceLevel(_d: *mut Display, _level: u16) -> Status {
    0
}
