//! Alarms, server counters and rendering fences require the SYNC server
//! extension. Keep their negotiation failure paths consistent with
//! XSyncQueryExtension/Initialize until those event streams are implemented.
use super::*;
#[repr(C)]
pub struct SystemCounter {
    name: *mut c_char,
    counter: usize,
    resolution: XSyncValue,
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncListSystemCounters")]
pub unsafe extern "sysv64" fn XSyncListSystemCounters(
    _d: *mut Display,
    count: *mut c_int,
) -> *mut SystemCounter {
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncFreeSystemCounterList")]
pub unsafe extern "sysv64" fn XSyncFreeSystemCounterList(list: *mut SystemCounter) {
    unsafe { guest::free(list.cast()) };
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncCreateAlarm")]
pub unsafe extern "sysv64" fn XSyncCreateAlarm(
    _d: *mut Display,
    _mask: u64,
    _attributes: *mut c_void,
) -> usize {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncChangeAlarm")]
pub unsafe extern "sysv64" fn XSyncChangeAlarm(
    _d: *mut Display,
    _alarm: usize,
    _mask: u64,
    _attributes: *mut c_void,
) -> Status {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncDestroyAlarm")]
pub unsafe extern "sysv64" fn XSyncDestroyAlarm(_d: *mut Display, _alarm: usize) -> Status {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncSetPriority")]
pub unsafe extern "sysv64" fn XSyncSetPriority(
    _d: *mut Display,
    _client: usize,
    _priority: c_int,
) -> Status {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncCreateFence")]
pub unsafe extern "sysv64" fn XSyncCreateFence(
    _d: *mut Display,
    _drawable: usize,
    _triggered: Bool,
) -> usize {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncTriggerFence")]
pub unsafe extern "sysv64" fn XSyncTriggerFence(_d: *mut Display, _fence: usize) -> Bool {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncResetFence")]
pub unsafe extern "sysv64" fn XSyncResetFence(_d: *mut Display, _fence: usize) -> Bool {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSyncDestroyFence")]
pub unsafe extern "sysv64" fn XSyncDestroyFence(_d: *mut Display, _fence: usize) -> Bool {
    0
}
