//! Client-side metrics operate on the supplied XFontStruct without a host font
//! lookup, allocation, or mutable process state.
use core::ffi::{c_char, c_int, c_uint, c_void};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct XCharStruct {
    pub lbearing: i16,
    pub rbearing: i16,
    pub width: i16,
    pub ascent: i16,
    pub descent: i16,
    pub attributes: u16,
}
#[repr(C)]
pub struct XFontStruct {
    pub ext_data: *mut c_void,
    pub fid: u64,
    pub direction: c_uint,
    pub min_char_or_byte2: c_uint,
    pub max_char_or_byte2: c_uint,
    pub min_byte1: c_uint,
    pub max_byte1: c_uint,
    pub all_chars_exist: c_int,
    pub default_char: c_uint,
    pub n_properties: c_int,
    pub properties: *mut c_void,
    pub min_bounds: XCharStruct,
    pub max_bounds: XCharStruct,
    pub per_char: *const XCharStruct,
    pub ascent: c_int,
    pub descent: c_int,
}
const _: () = assert!(core::mem::size_of::<XFontStruct>() == 96);

impl XFontStruct {
    unsafe fn glyph(&self, code: u32) -> Option<XCharStruct> {
        let (row, col) = if self.max_byte1 == 0 {
            (0, code)
        } else {
            (code >> 8, code & 255)
        };
        if row < self.min_byte1
            || row > self.max_byte1
            || col < self.min_char_or_byte2
            || col > self.max_char_or_byte2
        {
            return None;
        }
        if self.per_char.is_null() {
            return Some(self.min_bounds);
        }
        let stride = (self.max_char_or_byte2 - self.min_char_or_byte2) as usize + 1;
        let index =
            (row - self.min_byte1) as usize * stride + (col - self.min_char_or_byte2) as usize;
        let glyph = unsafe { *self.per_char.add(index) };
        if glyph.width == 0
            && glyph.lbearing == 0
            && glyph.rbearing == 0
            && glyph.ascent == 0
            && glyph.descent == 0
        {
            None
        } else {
            Some(glyph)
        }
    }
}
unsafe fn measure(
    font: &XFontStruct,
    bytes: *const u8,
    count: i32,
    wide: bool,
) -> (XCharStruct, i32) {
    let missing = unsafe { font.glyph(font.default_char) };
    let mut total = XCharStruct::default();
    let mut first = true;
    let mut width: i32 = 0;
    for index in 0..count.max(0) as usize {
        let code = if wide {
            unsafe { u32::from(*bytes.add(2 * index)) * 256 + u32::from(*bytes.add(2 * index + 1)) }
        } else {
            unsafe { u32::from(*bytes.add(index)) }
        };
        let Some(glyph) = (unsafe { font.glyph(code) }).or(missing) else {
            continue;
        };
        width = width.wrapping_add(i32::from(glyph.width));
        if first {
            total = glyph;
            first = false;
        } else {
            total.lbearing = i32::from(total.lbearing)
                .min(i32::from(total.width) + i32::from(glyph.lbearing))
                as i16;
            total.rbearing = i32::from(total.rbearing)
                .max(i32::from(total.width) + i32::from(glyph.rbearing))
                as i16;
            total.width = total.width.wrapping_add(glyph.width);
            total.ascent = total.ascent.max(glyph.ascent);
            total.descent = total.descent.max(glyph.descent);
        }
    }
    (total, width)
}
unsafe fn extents(
    font: *const XFontStruct,
    bytes: *const u8,
    count: i32,
    wide: bool,
    direction: *mut i32,
    ascent: *mut i32,
    descent: *mut i32,
    overall: *mut XCharStruct,
) -> i32 {
    let font = unsafe { &*font };
    unsafe {
        *direction = font.direction as i32;
        *ascent = font.ascent;
        *descent = font.descent;
        *overall = measure(font, bytes, count, wide).0;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libX11_XTextExtents")]
pub unsafe extern "sysv64" fn XTextExtents(
    font: *const XFontStruct,
    bytes: *const c_char,
    count: i32,
    direction: *mut i32,
    ascent: *mut i32,
    descent: *mut i32,
    overall: *mut XCharStruct,
) -> i32 {
    unsafe {
        extents(
            font,
            bytes.cast(),
            count,
            false,
            direction,
            ascent,
            descent,
            overall,
        )
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XTextExtents16")]
pub unsafe extern "sysv64" fn XTextExtents16(
    font: *const XFontStruct,
    bytes: *const u8,
    count: i32,
    direction: *mut i32,
    ascent: *mut i32,
    descent: *mut i32,
    overall: *mut XCharStruct,
) -> i32 {
    unsafe {
        extents(
            font, bytes, count, true, direction, ascent, descent, overall,
        )
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XTextWidth")]
pub unsafe extern "sysv64" fn XTextWidth(
    font: *const XFontStruct,
    bytes: *const c_char,
    count: i32,
) -> i32 {
    unsafe { measure(&*font, bytes.cast(), count, false).1 }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XTextWidth16")]
pub unsafe extern "sysv64" fn XTextWidth16(
    font: *const XFontStruct,
    bytes: *const u8,
    count: i32,
) -> i32 {
    unsafe { measure(&*font, bytes, count, true).1 }
}
