//! Primary-output snapshots live in guest memory, including interior pointers.
use super::*;

#[repr(C)]
pub struct XRRPanning {
    timestamp: Time,
    left: c_uint,
    top: c_uint,
    width: c_uint,
    height: c_uint,
    track_left: c_uint,
    track_top: c_uint,
    track_width: c_uint,
    track_height: c_uint,
    border_left: c_int,
    border_top: c_int,
    border_right: c_int,
    border_bottom: c_int,
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetPanning")]
pub unsafe extern "sysv64" fn XRRGetPanning(
    d: *mut Display,
    _resources: *mut XRRScreenResources,
    crtc: RRCrtc,
) -> *mut XRRPanning {
    if crtc != 1 || current().is_none() {
        unsafe {
            kinakaze_libX11::errors::report(d, 176, 140, 28, crtc);
        }
        return ptr::null_mut();
    }
    // A fixed primary output has panning disabled: all rectangles are empty.
    unsafe { allocate::<XRRPanning>() }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreePanning")]
pub unsafe extern "sysv64" fn XRRFreePanning(panning: *mut XRRPanning) {
    unsafe { guest::free(panning.cast()) };
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRSetOutputPrimary")]
pub unsafe extern "sysv64" fn XRRSetOutputPrimary(
    d: *mut Display,
    window: Window,
    _output: RROutput,
) {
    unsafe { error(d, 30, window) };
}
#[repr(C)]
struct Resources {
    value: XRRScreenResources,
    crtc: RRCrtc,
    output: RROutput,
    mode: XRRModeInfo,
    name: [u8; 48],
}
#[repr(C)]
struct Crtc {
    value: XRRCrtcInfo,
    output: RROutput,
}
#[repr(C)]
struct Output {
    value: XRROutputInfo,
    crtc: RRCrtc,
    mode: RRMode,
    name: [u8; 8],
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetScreenResourcesCurrent")]
pub unsafe extern "sysv64" fn XRRGetScreenResourcesCurrent(
    _dpy: *mut Display,
    _window: Window,
) -> *mut XRRScreenResources {
    let Some(screen) = current() else {
        return ptr::null_mut();
    };
    let block = unsafe { allocate::<Resources>() };
    if block.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let b = &mut *block;
        b.crtc = 1;
        b.output = 1;
        let name = format!("{}x{}", screen.width, screen.height);
        b.name[..name.len()].copy_from_slice(name.as_bytes());
        b.mode.id = 1;
        b.mode.width = screen.width;
        b.mode.height = screen.height;
        timing::apply(screen, &mut b.mode);
        b.mode.name = b.name.as_mut_ptr().cast();
        b.mode.nameLength = name.len() as u32;
        b.value.ncrtc = 1;
        b.value.crtcs = &raw mut b.crtc;
        b.value.noutput = 1;
        b.value.outputs = &raw mut b.output;
        b.value.nmode = 1;
        b.value.modes = &raw mut b.mode;
    }
    block.cast()
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetScreenResources")]
pub unsafe extern "sysv64" fn XRRGetScreenResources(
    dpy: *mut Display,
    window: Window,
) -> *mut XRRScreenResources {
    unsafe { XRRGetScreenResourcesCurrent(dpy, window) }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreeScreenResources")]
pub unsafe extern "sysv64" fn XRRFreeScreenResources(resources: *mut XRRScreenResources) {
    unsafe {
        guest::free(resources.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetCrtcInfo")]
pub unsafe extern "sysv64" fn XRRGetCrtcInfo(
    _dpy: *mut Display,
    resources: *mut XRRScreenResources,
    crtc: RRCrtc,
) -> *mut XRRCrtcInfo {
    if resources.is_null() || crtc != 1 {
        return ptr::null_mut();
    }
    let Some(screen) = current() else {
        return ptr::null_mut();
    };
    let block = unsafe { allocate::<Crtc>() };
    if block.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let b = &mut *block;
        b.output = 1;
        b.value.width = screen.width;
        b.value.height = screen.height;
        b.value.mode = 1;
        b.value.rotation = 1;
        b.value.rotations = 1;
        b.value.noutput = 1;
        b.value.outputs = &raw mut b.output;
        b.value.npossible = 1;
        b.value.possible = &raw mut b.output;
    }
    block.cast()
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreeCrtcInfo")]
pub unsafe extern "sysv64" fn XRRFreeCrtcInfo(info: *mut XRRCrtcInfo) {
    unsafe {
        guest::free(info.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetOutputInfo")]
pub unsafe extern "sysv64" fn XRRGetOutputInfo(
    _dpy: *mut Display,
    resources: *mut XRRScreenResources,
    output: RROutput,
) -> *mut XRROutputInfo {
    if resources.is_null() || output != 1 {
        return ptr::null_mut();
    }
    let Some(screen) = current() else {
        return ptr::null_mut();
    };
    let block = unsafe { allocate::<Output>() };
    if block.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let b = &mut *block;
        b.crtc = 1;
        b.mode = 1;
        b.name = *b"Primary\0";
        b.value.crtc = 1;
        b.value.name = b.name.as_mut_ptr().cast();
        b.value.nameLen = 7;
        b.value.mm_width = screen.mm_width.max(0) as u64;
        b.value.mm_height = screen.mm_height.max(0) as u64;
        b.value.connection = RR_Connected;
        b.value.subpixel_order = 0;
        b.value.ncrtc = 1;
        b.value.crtcs = &raw mut b.crtc;
        b.value.nmode = 1;
        b.value.npreferred = 1;
        b.value.modes = &raw mut b.mode;
    }
    block.cast()
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreeOutputInfo")]
pub unsafe extern "sysv64" fn XRRFreeOutputInfo(info: *mut XRROutputInfo) {
    unsafe {
        guest::free(info.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetOutputPrimary")]
pub unsafe extern "sysv64" fn XRRGetOutputPrimary(_dpy: *mut Display, _window: Window) -> RROutput {
    usize::from(current().is_some())
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRSetCrtcConfig")]
pub unsafe extern "sysv64" fn XRRSetCrtcConfig(
    _dpy: *mut Display,
    resources: *mut XRRScreenResources,
    crtc: RRCrtc,
    _time: Time,
    x: c_int,
    y: c_int,
    mode: RRMode,
    rotation: Rotation,
    outputs: *mut RROutput,
    count: c_int,
) -> Status {
    if resources.is_null()
        || crtc != 1
        || x != 0
        || y != 0
        || mode != 1
        || rotation != 1
        || count != 1
        || outputs.is_null()
        || unsafe { *outputs != 1 }
        || current().is_none()
    {
        3
    } else {
        0
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRListOutputProperties")]
pub unsafe extern "sysv64" fn XRRListOutputProperties(
    _dpy: *mut Display,
    _output: RROutput,
    count: *mut c_int,
) -> *mut u64 {
    if !count.is_null() {
        unsafe {
            *count = 0;
        }
    }
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRQueryOutputProperty")]
pub unsafe extern "sysv64" fn XRRQueryOutputProperty(
    dpy: *mut Display,
    _output: RROutput,
    property: u64,
) -> *mut core::ffi::c_void {
    unsafe {
        kinakaze_libX11::errors::report(dpy, 5, 140, 11, property as usize);
    }
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetOutputProperty")]
pub unsafe extern "sysv64" fn XRRGetOutputProperty(
    _dpy: *mut Display,
    _output: RROutput,
    _property: u64,
    offset: i64,
    length: i64,
    _delete: Bool,
    _pending: Bool,
    _type: u64,
    actual: *mut u64,
    format: *mut c_int,
    count: *mut u64,
    after: *mut u64,
    data: *mut *mut u8,
) -> c_int {
    unsafe {
        if !actual.is_null() {
            *actual = 0;
        }
        if !format.is_null() {
            *format = 0;
        }
        if !count.is_null() {
            *count = 0;
        }
        if !after.is_null() {
            *after = 0;
        }
        if !data.is_null() {
            *data = ptr::null_mut();
        }
    }
    if offset < 0 || length < 0 { 2 } else { 0 } // Absent property: type None.
}
