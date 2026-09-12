//! Gamma arrays use one checked guest allocation and actual GDI ramp queries.
use super::*;
use windows_sys::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRAllocGamma")]
pub unsafe extern "sysv64" fn XRRAllocGamma(size: c_int) -> *mut XRRCrtcGamma {
    if size < 0 {
        return ptr::null_mut();
    }
    let Some(bytes) = (size as usize)
        .checked_mul(6)
        .and_then(|n| n.checked_add(mem::size_of::<XRRCrtcGamma>()))
    else {
        return ptr::null_mut();
    };
    let gamma = unsafe { zeroed(bytes) }.cast::<XRRCrtcGamma>();
    if !gamma.is_null() {
        unsafe {
            (*gamma).size = size;
            (*gamma).red = gamma.add(1).cast();
            (*gamma).green = (*gamma).red.add(size as usize);
            (*gamma).blue = (*gamma).green.add(size as usize);
        }
    }
    gamma
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreeGamma")]
pub unsafe extern "sysv64" fn XRRFreeGamma(gamma: *mut XRRCrtcGamma) {
    unsafe {
        guest::free(gamma.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetCrtcGamma")]
pub unsafe extern "sysv64" fn XRRGetCrtcGamma(
    _dpy: *mut Display,
    crtc: RRCrtc,
) -> *mut XRRCrtcGamma {
    if crtc != 1 {
        return ptr::null_mut();
    }
    let dc = unsafe { GetDC(ptr::null_mut()) };
    if dc.is_null() {
        return unsafe { XRRAllocGamma(0) };
    }
    let mut ramp = [[0u16; 256]; 3];
    let ok = unsafe { GetDeviceGammaRamp(dc, ramp.as_mut_ptr().cast()) } != 0;
    unsafe {
        ReleaseDC(ptr::null_mut(), dc);
    }
    if !ok {
        // A valid CRTC with no exposed gamma LUT has a zero-sized reply.
        // NULL denotes an invalid resource/failed request, not zero capability.
        return unsafe { XRRAllocGamma(0) };
    }
    let gamma = unsafe { XRRAllocGamma(256) };
    if !gamma.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(ramp.as_ptr().cast::<u16>(), (*gamma).red, 768);
        }
    }
    gamma
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetCrtcGammaSize")]
pub unsafe extern "sysv64" fn XRRGetCrtcGammaSize(dpy: *mut Display, crtc: RRCrtc) -> c_int {
    let gamma = unsafe { XRRGetCrtcGamma(dpy, crtc) };
    if gamma.is_null() {
        return 0;
    }
    let size = unsafe { (*gamma).size };
    unsafe {
        XRRFreeGamma(gamma);
    }
    size
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRSetCrtcGamma")]
pub unsafe extern "sysv64" fn XRRSetCrtcGamma(
    dpy: *mut Display,
    crtc: RRCrtc,
    gamma: *mut XRRCrtcGamma,
) {
    if crtc != 1 || gamma.is_null() {
        unsafe {
            error(dpy, 24, crtc);
        }
        return;
    }
    let g = unsafe { &*gamma };
    if g.size != 256 || g.red.is_null() || g.green.is_null() || g.blue.is_null() {
        unsafe {
            error(dpy, 24, crtc);
        }
        return;
    }
    let mut ramp = [[0u16; 256]; 3];
    for (from, to) in [g.red, g.green, g.blue].into_iter().zip(ramp.iter_mut()) {
        unsafe {
            ptr::copy_nonoverlapping(from, to.as_mut_ptr(), 256);
        }
    }
    let dc = unsafe { GetDC(ptr::null_mut()) };
    let ok = if dc.is_null() {
        false
    } else {
        let result = unsafe { SetDeviceGammaRamp(dc, ramp.as_ptr().cast()) } != 0;
        unsafe {
            ReleaseDC(ptr::null_mut(), dc);
        }
        result
    };
    if !ok {
        unsafe {
            error(dpy, 24, crtc);
        }
    }
}
