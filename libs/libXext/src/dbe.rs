//! DBE 1.0 client ABI. X11 owns drawable storage; this module owns protocol,
//! allocation of guest-visible results, display cleanup and fork participation.
use super::*;
use kinakaze_libX11::{
    errors,
    graphics::{self, buffered},
};
mod lifecycle;
const OPCODE: u8 = 131;
const BAD_BUFFER: u8 = 140;
#[repr(C)]
pub struct XdbeVisualInfo {
    pub visual: u64,
    pub depth: c_int,
    pub perflevel: c_int,
}
#[repr(C)]
pub struct XdbeScreenVisualInfo {
    pub count: c_int,
    pub visinfo: *mut XdbeVisualInfo,
}
pub type XdbeSwapInfo = buffered::SwapRequest;
#[repr(C)]
pub struct XdbeBackBufferAttributes {
    pub window: Window,
}
unsafe fn fail(display: *mut Display, code: u8, minor: u8, id: usize) -> Status {
    unsafe {
        errors::report(display, code, OPCODE, minor, id);
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeQueryExtension")]
pub unsafe extern "sysv64" fn XdbeQueryExtension(
    _display: *mut Display,
    major: *mut c_int,
    minor: *mut c_int,
) -> Status {
    unsafe {
        if !major.is_null() {
            *major = 1;
        }
        if !minor.is_null() {
            *minor = 0;
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeAllocateBackBufferName")]
pub unsafe extern "sysv64" fn XdbeAllocateBackBufferName(
    display: *mut Display,
    window: Window,
    action: u8,
) -> usize {
    if action > 3 {
        unsafe {
            fail(display, 2, 1, action as usize);
        }
        return 0;
    }
    match buffered::allocate(window, display as usize) {
        Ok(id) => id,
        Err(code) => {
            unsafe {
                fail(display, code, 1, window);
            }
            0
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeDeallocateBackBufferName")]
pub unsafe extern "sysv64" fn XdbeDeallocateBackBufferName(
    display: *mut Display,
    buffer: usize,
) -> Status {
    if buffered::deallocate(buffer, display as usize) {
        1
    } else {
        unsafe { fail(display, BAD_BUFFER, 2, buffer) }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeSwapBuffers")]
pub unsafe extern "sysv64" fn XdbeSwapBuffers(
    display: *mut Display,
    info: *mut XdbeSwapInfo,
    count: c_int,
) -> Status {
    if count < 0 || (count != 0 && info.is_null()) {
        return unsafe { fail(display, 2, 3, count as usize) };
    }
    if count == 0 {
        return 1;
    }
    let info = unsafe { core::slice::from_raw_parts(info, count as usize) };
    match buffered::swap(info) {
        Ok(()) => 1,
        Err((code, id)) => unsafe { fail(display, code, 3, id) },
    }
}
// DBE specifies these as optional optimization markers, including unmatched
// pairs. The renderer already batches each swap; there is no retained idiom state.
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeBeginIdiom")]
pub unsafe extern "sysv64" fn XdbeBeginIdiom(_display: *mut Display) -> Status {
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeEndIdiom")]
pub unsafe extern "sysv64" fn XdbeEndIdiom(_display: *mut Display) -> Status {
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeGetVisualInfo")]
pub unsafe extern "sysv64" fn XdbeGetVisualInfo(
    display: *mut Display,
    screens: *mut usize,
    count: *mut c_int,
) -> *mut XdbeScreenVisualInfo {
    if count.is_null() {
        return ptr::null_mut();
    }
    let requested = unsafe { *count };
    if requested < 0 || (requested != 0 && screens.is_null()) {
        unsafe {
            fail(display, 2, 6, requested as usize);
        }
        return ptr::null_mut();
    }
    for index in 0..requested as usize {
        let drawable = unsafe { *screens.add(index) };
        if drawable != 1 && graphics::drawable_dimensions(drawable).is_none() {
            unsafe {
                fail(display, 9, 6, drawable);
            }
            return ptr::null_mut();
        }
    }
    let length = requested.max(1) as usize;
    let Some(bytes) = length
        .checked_mul(mem::size_of::<XdbeScreenVisualInfo>() + mem::size_of::<XdbeVisualInfo>())
    else {
        return ptr::null_mut();
    };
    // One guest allocation holds both arrays; XdbeFreeVisualInfo need not keep a
    // native pointer/count catalog, and the result survives ordinary fork copy.
    let result = unsafe { guest::malloc(bytes) }.cast::<XdbeScreenVisualInfo>();
    if result.is_null() {
        return result;
    }
    let visual = unsafe { kinakaze_libX11::XDefaultVisual(display, 0) };
    if visual.is_null() {
        unsafe {
            guest::free(result.cast());
        }
        return ptr::null_mut();
    }
    let infos = unsafe { result.add(length).cast::<XdbeVisualInfo>() };
    for index in 0..length {
        unsafe {
            infos.add(index).write(XdbeVisualInfo {
                visual: (*visual).visualid as u64,
                depth: 24,
                perflevel: 0,
            });
            result.add(index).write(XdbeScreenVisualInfo {
                count: 1,
                visinfo: infos.add(index),
            });
        }
    }
    unsafe {
        *count = length as c_int;
    }
    result
}
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeFreeVisualInfo")]
pub unsafe extern "sysv64" fn XdbeFreeVisualInfo(info: *mut XdbeScreenVisualInfo) {
    unsafe {
        guest::free(info.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XdbeGetBackBufferAttributes")]
pub unsafe extern "sysv64" fn XdbeGetBackBufferAttributes(
    display: *mut Display,
    buffer: usize,
) -> *mut XdbeBackBufferAttributes {
    let info = unsafe { guest::malloc(mem::size_of::<XdbeBackBufferAttributes>()) }
        .cast::<XdbeBackBufferAttributes>();
    if !info.is_null() {
        unsafe {
            info.write(XdbeBackBufferAttributes {
                window: buffered::window(buffer, display as usize).unwrap_or(0),
            });
        }
    }
    info
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dbe_matches_lp64_c_layouts() {
        assert_eq!(mem::size_of::<XdbeVisualInfo>(), 16);
        assert_eq!(mem::size_of::<XdbeScreenVisualInfo>(), 16);
        assert_eq!(mem::offset_of!(XdbeScreenVisualInfo, visinfo), 8);
        assert_eq!(mem::size_of::<XdbeSwapInfo>(), 16);
        assert_eq!(mem::offset_of!(XdbeSwapInfo, action), 8);
        assert_eq!(mem::size_of::<XdbeBackBufferAttributes>(), 8);
    }
}
