//! Font-set drawing uses the same native font and retained surface as core text.
use crate::graphics::GC;
use crate::{Display, Drawable, XRectangle};
use core::ffi::{c_char, c_void};
use core::ptr;
use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, GetTextExtentPoint32W, GetTextMetricsW, TEXTMETRICW,
};

unsafe fn mb(text: *const c_char, count: i32) -> Vec<u16> {
    if count <= 0 || text.is_null() {
        return Vec::new();
    }
    let bytes = unsafe { core::slice::from_raw_parts(text.cast::<u8>(), count as usize) };
    String::from_utf8_lossy(bytes).encode_utf16().collect()
}
unsafe fn wide(text: *const i32, count: i32) -> Vec<u16> {
    if count <= 0 || text.is_null() {
        return Vec::new();
    }
    unsafe { core::slice::from_raw_parts(text, count as usize) }
        .iter()
        .map(|v| char::from_u32(*v as u32).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect::<String>()
        .encode_utf16()
        .collect()
}
unsafe fn extents(glyphs: &[u16], ink: *mut XRectangle, logical: *mut XRectangle) -> i32 {
    if !ink.is_null() {
        unsafe {
            ink.write(core::mem::zeroed());
        }
    }
    if !logical.is_null() {
        unsafe {
            logical.write(core::mem::zeroed());
        }
    }
    if glyphs.len() > i32::MAX as usize {
        return 0;
    }
    let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
    if dc.is_null() {
        return 0;
    }
    let mut size: SIZE = unsafe { core::mem::zeroed() };
    let mut metrics: TEXTMETRICW = unsafe { core::mem::zeroed() };
    let ok = unsafe {
        GetTextExtentPoint32W(dc, glyphs.as_ptr(), glyphs.len() as i32, &mut size) != 0
            && GetTextMetricsW(dc, &mut metrics) != 0
    };
    unsafe {
        DeleteDC(dc);
    }
    if !ok {
        return 0;
    }
    for output in [ink, logical] {
        if !output.is_null() {
            unsafe {
                output.write(XRectangle {
                    x: 0,
                    y: (-metrics.tmAscent).clamp(i16::MIN as i32, i16::MAX as i32) as i16,
                    width: size.cx.clamp(0, u16::MAX as i32) as u16,
                    height: metrics.tmHeight.clamp(0, u16::MAX as i32) as u16,
                });
            }
        }
    }
    size.cx
}
pub(crate) unsafe fn mb_extents(
    text: *const c_char,
    count: i32,
    ink: *mut XRectangle,
    logical: *mut XRectangle,
) -> i32 {
    unsafe { extents(&mb(text, count), ink, logical) }
}
pub(crate) unsafe fn mb_draw(
    drawable: Drawable,
    gc: GC,
    x: i32,
    y: i32,
    text: *const c_char,
    count: i32,
) {
    unsafe {
        crate::graphics::draw_utf16(drawable, gc, x, y, &mb(text, count));
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XmbTextEscapement")]
pub unsafe extern "sysv64" fn XmbTextEscapement(
    _font: *mut c_void,
    text: *const c_char,
    count: i32,
) -> i32 {
    unsafe { mb_extents(text, count, ptr::null_mut(), ptr::null_mut()) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XwcTextEscapement")]
pub unsafe extern "sysv64" fn XwcTextEscapement(
    _font: *mut c_void,
    text: *const i32,
    count: i32,
) -> i32 {
    unsafe { extents(&wide(text, count), ptr::null_mut(), ptr::null_mut()) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XwcTextExtents")]
pub unsafe extern "sysv64" fn XwcTextExtents(
    _font: *mut c_void,
    text: *const i32,
    count: i32,
    ink: *mut XRectangle,
    logical: *mut XRectangle,
) -> i32 {
    unsafe { extents(&wide(text, count), ink, logical) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XwcDrawString")]
pub unsafe extern "sysv64" fn XwcDrawString(
    _d: *mut Display,
    drawable: Drawable,
    _font: *mut c_void,
    gc: GC,
    x: i32,
    y: i32,
    text: *const i32,
    count: i32,
) {
    unsafe {
        crate::graphics::draw_utf16(drawable, gc, x, y, &wide(text, count));
    }
}
