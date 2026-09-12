use core::ffi::{c_int, c_void};
use libc::fdio::PollFd;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

pub type IoCallback =
    unsafe extern "sysv64" fn(*mut pa_mainloop_api, *mut c_void, c_int, c_int, *mut c_void);
pub type TimeCallback =
    unsafe extern "sysv64" fn(*mut pa_mainloop_api, *mut c_void, *const Timeval, *mut c_void);
pub type DeferCallback = unsafe extern "sysv64" fn(*mut pa_mainloop_api, *mut c_void, *mut c_void);
pub type DestroyCallback = DeferCallback;
pub type PollCallback = unsafe extern "sysv64" fn(*mut PollFd, u64, c_int, *mut c_void) -> c_int;
pub type OnceCallback = unsafe extern "sysv64" fn(*mut pa_mainloop_api, *mut c_void);

#[repr(C)]
pub struct pa_mainloop_api {
    pub userdata: *mut c_void,
    pub io_new:
        unsafe extern "sysv64" fn(*mut Self, c_int, c_int, IoCallback, *mut c_void) -> *mut c_void,
    pub io_enable: unsafe extern "sysv64" fn(*mut c_void, c_int),
    pub io_free: unsafe extern "sysv64" fn(*mut c_void),
    pub io_set_destroy: unsafe extern "sysv64" fn(*mut c_void, Option<DestroyCallback>),
    pub time_new: unsafe extern "sysv64" fn(
        *mut Self,
        *const Timeval,
        TimeCallback,
        *mut c_void,
    ) -> *mut c_void,
    pub time_restart: unsafe extern "sysv64" fn(*mut c_void, *const Timeval),
    pub time_free: unsafe extern "sysv64" fn(*mut c_void),
    pub time_set_destroy: unsafe extern "sysv64" fn(*mut c_void, Option<DestroyCallback>),
    pub defer_new: unsafe extern "sysv64" fn(*mut Self, DeferCallback, *mut c_void) -> *mut c_void,
    pub defer_enable: unsafe extern "sysv64" fn(*mut c_void, c_int),
    pub defer_free: unsafe extern "sysv64" fn(*mut c_void),
    pub defer_set_destroy: unsafe extern "sysv64" fn(*mut c_void, Option<DestroyCallback>),
    pub quit: unsafe extern "sysv64" fn(*mut Self, c_int),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_vtable_matches_linux_lp64() {
        assert_eq!(size_of::<pa_mainloop_api>(), 14 * 8);
        assert_eq!(core::mem::offset_of!(pa_mainloop_api, time_new), 5 * 8);
        assert_eq!(core::mem::offset_of!(pa_mainloop_api, quit), 13 * 8);
        assert_eq!(size_of::<Timeval>(), 16);
    }
}
