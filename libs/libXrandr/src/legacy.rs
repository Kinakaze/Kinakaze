//! Legacy screen configuration snapshots own no host handles or private heap.
use super::*;
#[repr(C)]
pub struct XRRScreenSize {
    width: c_int,
    height: c_int,
    mwidth: c_int,
    mheight: c_int,
}
#[repr(C)]
pub struct XRRScreenConfiguration {
    size: XRRScreenSize,
    rate: i16,
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetScreenInfo")]
pub unsafe extern "sysv64" fn XRRGetScreenInfo(
    _dpy: *mut Display,
    _window: Drawable,
) -> *mut XRRScreenConfiguration {
    let Some(screen) = current() else {
        return ptr::null_mut();
    };
    let config = unsafe { allocate::<XRRScreenConfiguration>() };
    if !config.is_null() {
        unsafe {
            (*config).size = XRRScreenSize {
                width: screen.width as i32,
                height: screen.height as i32,
                mwidth: screen.mm_width,
                mheight: screen.mm_height,
            };
            (*config).rate = screen.rate;
        }
    }
    config
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreeScreenConfigInfo")]
pub unsafe extern "sysv64" fn XRRFreeScreenConfigInfo(config: *mut XRRScreenConfiguration) {
    unsafe {
        guest::free(config.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRConfigSizes")]
pub unsafe extern "sysv64" fn XRRConfigSizes(
    config: *mut XRRScreenConfiguration,
    count: *mut c_int,
) -> *mut XRRScreenSize {
    if !count.is_null() {
        unsafe {
            *count = i32::from(!config.is_null());
        }
    }
    if config.is_null() {
        ptr::null_mut()
    } else {
        unsafe { &raw mut (*config).size }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRConfigRates")]
pub unsafe extern "sysv64" fn XRRConfigRates(
    config: *mut XRRScreenConfiguration,
    size: c_int,
    count: *mut c_int,
) -> *mut i16 {
    let valid = !config.is_null() && size == 0 && unsafe { (*config).rate != 0 };
    if !count.is_null() {
        unsafe {
            *count = i32::from(valid);
        }
    }
    if valid {
        unsafe { &raw mut (*config).rate }
    } else {
        ptr::null_mut()
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRConfigCurrentConfiguration")]
pub unsafe extern "sysv64" fn XRRConfigCurrentConfiguration(
    config: *mut XRRScreenConfiguration,
    rotation: *mut Rotation,
) -> u16 {
    if !rotation.is_null() {
        unsafe {
            *rotation = if config.is_null() { 0 } else { 1 };
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRConfigCurrentRate")]
pub unsafe extern "sysv64" fn XRRConfigCurrentRate(config: *mut XRRScreenConfiguration) -> i16 {
    if config.is_null() {
        0
    } else {
        unsafe { (*config).rate }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRSetScreenConfigAndRate")]
pub unsafe extern "sysv64" fn XRRSetScreenConfigAndRate(
    _dpy: *mut Display,
    config: *mut XRRScreenConfiguration,
    _window: Drawable,
    size: c_int,
    rotation: Rotation,
    rate: i16,
    _time: Time,
) -> Status {
    let Some(screen) = current() else {
        return 3;
    };
    if config.is_null() || size != 0 || rotation != 1 {
        return 3;
    }
    let requested = unsafe { &*config };
    if requested.size.width != screen.width as i32
        || requested.size.height != screen.height as i32
        || rate != 0 && rate != screen.rate
    {
        3
    } else {
        0
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRSetScreenConfig")]
pub unsafe extern "sysv64" fn XRRSetScreenConfig(
    dpy: *mut Display,
    config: *mut XRRScreenConfiguration,
    window: Drawable,
    size: c_int,
    rotation: Rotation,
    time: Time,
) -> Status {
    unsafe { XRRSetScreenConfigAndRate(dpy, config, window, size, rotation, 0, time) }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetScreenSizeRange")]
pub unsafe extern "sysv64" fn XRRGetScreenSizeRange(
    _dpy: *mut Display,
    _window: Window,
    min_w: *mut c_int,
    min_h: *mut c_int,
    max_w: *mut c_int,
    max_h: *mut c_int,
) -> Status {
    let Some(screen) = current() else {
        return 0;
    };
    for (out, value) in [
        (min_w, screen.width),
        (max_w, screen.width),
        (min_h, screen.height),
        (max_h, screen.height),
    ] {
        if !out.is_null() {
            unsafe {
                *out = value as i32;
            }
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRSetScreenSize")]
pub unsafe extern "sysv64" fn XRRSetScreenSize(
    dpy: *mut Display,
    window: Window,
    width: c_int,
    height: c_int,
    mm_width: c_int,
    mm_height: c_int,
) {
    if current().is_some_and(|s| {
        s.width == width as u32
            && s.height == height as u32
            && s.mm_width == mm_width
            && s.mm_height == mm_height
    }) {
        return;
    }
    unsafe {
        error(dpy, 7, window);
    }
}
