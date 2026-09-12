//! RandR client ABI for the host primary output and its change notifications.
#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
use core::ffi::{c_char, c_int, c_uint, c_ushort};
use core::{mem, ptr};
use kinakaze_alloc::guest;
use kinakaze_libX11::{Bool, Display, Drawable, Status, Time, Window, XEvent};
type c_ulong = u64;
use windows_sys::Win32::Graphics::Gdi::*;
mod events;
mod gamma;
mod legacy;
mod monitors;
mod object_layout;
mod providers;
mod resources;
mod timing;
mod types;
pub use types::*;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Screen {
    width: u32,
    height: u32,
    rate: i16,
    mm_width: i32,
    mm_height: i32,
}
fn current() -> Option<Screen> {
    let mut mode = DEVMODEW::default();
    mode.dmSize = mem::size_of::<DEVMODEW>() as u16;
    if unsafe { EnumDisplaySettingsW(ptr::null(), ENUM_CURRENT_SETTINGS, &raw mut mode) } == 0
        || mode.dmPelsWidth == 0
        || mode.dmPelsHeight == 0
    {
        return None;
    }
    let dc = unsafe { GetDC(ptr::null_mut()) };
    let (mm_width, mm_height) = if dc.is_null() {
        (0, 0)
    } else {
        let result = unsafe {
            (
                GetDeviceCaps(dc, HORZSIZE as i32),
                GetDeviceCaps(dc, VERTSIZE as i32),
            )
        };
        unsafe {
            ReleaseDC(ptr::null_mut(), dc);
        }
        result
    };
    Some(Screen {
        width: mode.dmPelsWidth,
        height: mode.dmPelsHeight,
        rate: if mode.dmDisplayFrequency > 1 && mode.dmDisplayFrequency <= i16::MAX as u32 {
            mode.dmDisplayFrequency as i16
        } else {
            0
        },
        mm_width,
        mm_height,
    })
}
unsafe fn allocate<T>() -> *mut T {
    unsafe { zeroed(mem::size_of::<T>()).cast() }
}
unsafe fn zeroed(bytes: usize) -> *mut u8 {
    let p = unsafe { guest::malloc(bytes) };
    if !p.is_null() {
        unsafe {
            ptr::write_bytes(p, 0, bytes);
        }
    }
    p
}
unsafe fn error(display: *mut Display, minor: u8, resource: usize) {
    unsafe {
        kinakaze_libX11::errors::report(display, 1, 140, minor, resource);
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRQueryExtension")]
pub unsafe extern "sysv64" fn XRRQueryExtension(
    dpy: *mut Display,
    event: *mut c_int,
    error: *mut c_int,
) -> Bool {
    if !event.is_null() {
        unsafe {
            *event = events::EVENT;
        }
    }
    if !error.is_null() {
        unsafe {
            *error = 176;
        }
    }
    events::initialize(dpy);
    1
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRQueryVersion")]
pub unsafe extern "sysv64" fn XRRQueryVersion(
    dpy: *mut Display,
    major: *mut c_int,
    minor: *mut c_int,
) -> Status {
    events::initialize(dpy);
    unsafe {
        if !major.is_null() {
            *major = 1;
        }
        if !minor.is_null() {
            *minor = 3;
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRSelectInput")]
pub unsafe extern "sysv64" fn XRRSelectInput(dpy: *mut Display, window: Window, mask: c_int) {
    events::select(dpy, window, mask);
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRUpdateConfiguration")]
pub unsafe extern "sysv64" fn XRRUpdateConfiguration(event: *mut XEvent) -> c_int {
    unsafe { events::update(event) }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn randr_discovery_reports_its_registered_protocol() {
        let mut event = -1;
        let mut error = -1;
        assert_eq!(
            unsafe { XRRQueryExtension(ptr::null_mut(), &raw mut event, &raw mut error) },
            1
        );
        assert_eq!((event, error), (112, 176));
        assert_eq!(
            unsafe { XRRQueryVersion(ptr::null_mut(), &raw mut event, &raw mut error) },
            1
        );
        assert_eq!((event, error), (1, 3));
    }
}
