//! Client images use the Linux XImage ABI and guest-owned storage. Drawable
//! transfers operate on the existing DIB surfaces, without opening new windows.
use crate::{Display, Visual, errors, graphics};
use core::ffi::{c_char, c_int, c_uint, c_void};
use core::{mem, ptr};
use kinakaze_alloc::guest;
use windows_sys::Win32::Foundation::RECT;
mod readback;
pub use readback::{XGetImage, read_into};

#[repr(C)]
pub struct XImage {
    pub width: c_int,
    pub height: c_int,
    pub xoffset: c_int,
    pub format: c_int,
    pub data: *mut c_char,
    pub byte_order: c_int,
    pub bitmap_unit: c_int,
    pub bitmap_bit_order: c_int,
    pub bitmap_pad: c_int,
    pub depth: c_int,
    pub bytes_per_line: c_int,
    pub bits_per_pixel: c_int,
    pub red_mask: u64,
    pub green_mask: u64,
    pub blue_mask: u64,
    pub obdata: *mut c_void,
    pub functions: [usize; 6],
}
#[repr(C)]
pub struct PixmapFormat {
    depth: c_int,
    bits_per_pixel: c_int,
    scanline_pad: c_int,
}

// This table is also published through Display and XListPixmapFormats.
pub(crate) static FORMATS: [PixmapFormat; 5] = [
    PixmapFormat {
        depth: 1,
        bits_per_pixel: 1,
        scanline_pad: 32,
    },
    PixmapFormat {
        depth: 8,
        bits_per_pixel: 8,
        scanline_pad: 32,
    },
    PixmapFormat {
        depth: 16,
        bits_per_pixel: 16,
        scanline_pad: 32,
    },
    PixmapFormat {
        depth: 24,
        bits_per_pixel: 32,
        scanline_pad: 32,
    },
    PixmapFormat {
        depth: 32,
        bits_per_pixel: 32,
        scanline_pad: 32,
    },
];

#[repr(C)]
pub(crate) struct ScreenFormat {
    ext_data: usize,
    depth: c_int,
    bits_per_pixel: c_int,
    scanline_pad: c_int,
}
pub(crate) static DISPLAY_FORMATS: [ScreenFormat; 5] = [
    ScreenFormat {
        ext_data: 0,
        depth: 1,
        bits_per_pixel: 1,
        scanline_pad: 32,
    },
    ScreenFormat {
        ext_data: 0,
        depth: 8,
        bits_per_pixel: 8,
        scanline_pad: 32,
    },
    ScreenFormat {
        ext_data: 0,
        depth: 16,
        bits_per_pixel: 16,
        scanline_pad: 32,
    },
    ScreenFormat {
        ext_data: 0,
        depth: 24,
        bits_per_pixel: 32,
        scanline_pad: 32,
    },
    ScreenFormat {
        ext_data: 0,
        depth: 32,
        bits_per_pixel: 32,
        scanline_pad: 32,
    },
];

unsafe fn display_format(display: *mut Display, depth: c_int) -> Option<(c_int, c_int)> {
    if display.is_null() {
        return None;
    }
    let display = unsafe { &*display };
    if display.nformats <= 0 || display.pixmap_format.is_null() {
        return None;
    }
    unsafe {
        core::slice::from_raw_parts(
            display.pixmap_format.cast::<ScreenFormat>(),
            display.nformats as usize,
        )
    }
    .iter()
    .find(|format| format.depth == depth)
    .map(|format| (format.bits_per_pixel, format.scanline_pad))
}

#[unsafe(export_name = "kinakaze_engine_libX11__XGetScanlinePad")]
pub unsafe extern "sysv64" fn _XGetScanlinePad(display: *mut Display, depth: c_int) -> c_int {
    unsafe { display_format(display, depth) }.map_or_else(
        || {
            if display.is_null() {
                32
            } else {
                unsafe { (*display).bitmap_pad }
            }
        },
        |format| format.1,
    )
}

#[unsafe(export_name = "kinakaze_engine_libX11__XGetBitsPerPixel")]
pub unsafe extern "sysv64" fn _XGetBitsPerPixel(display: *mut Display, depth: c_int) -> c_int {
    unsafe { display_format(display, depth) }.map_or_else(
        || match depth {
            ..=4 => 4,
            5..=8 => 8,
            9..=16 => 16,
            _ => 32,
        },
        |format| format.0,
    )
}

unsafe fn layout(image: *mut XImage) -> bool {
    if image.is_null() {
        return false;
    }
    let i = unsafe { &mut *image };
    if i.width < 0
        || i.height < 0
        || i.xoffset < 0
        || !(0..=2).contains(&i.format)
        || !(1..=32).contains(&i.depth)
        || ![8, 16, 32].contains(&i.bitmap_pad)
        || ![8, 16, 32].contains(&i.bitmap_unit)
        || ![0, 1].contains(&i.byte_order)
        || ![0, 1].contains(&i.bitmap_bit_order)
        || i.format == 0 && i.depth != 1
    {
        return false;
    }
    if i.format != 2 {
        i.bits_per_pixel = 1;
    }
    if ![1, 8, 16, 24, 32].contains(&i.bits_per_pixel) {
        return false;
    }
    if i.format == 2 && i.bits_per_pixel < i.depth {
        return false;
    }
    let Some(bits) = (i.width as usize + i.xoffset as usize).checked_mul(i.bits_per_pixel as usize)
    else {
        return false;
    };
    // Reversing byte order within a bitmap unit touches the entire last unit,
    // even when only one bit of it is used. Require storage for that unit.
    let alignment = if i.bits_per_pixel == 1 && i.byte_order != i.bitmap_bit_order {
        i.bitmap_pad.max(i.bitmap_unit)
    } else {
        i.bitmap_pad
    } as usize;
    let bytes = bits.div_ceil(alignment) * (alignment / 8);
    if bytes > i32::MAX as usize || i.bytes_per_line < 0 {
        return false;
    }
    if i.bytes_per_line == 0 {
        i.bytes_per_line = bytes as i32;
    }
    if (i.bytes_per_line as usize) < bytes {
        return false;
    }
    true
}
fn install_functions(image: &mut XImage) {
    image.functions = [
        XCreateImage as *const () as usize,
        XDestroyImage as *const () as usize,
        XGetPixel as *const () as usize,
        XPutPixel as *const () as usize,
        XSubImage as *const () as usize,
        XAddPixel as *const () as usize,
    ];
}
#[unsafe(export_name = "kinakaze_engine_libX11_XInitImage")]
pub unsafe extern "sysv64" fn XInitImage(image: *mut XImage) -> c_int {
    if !unsafe { layout(image) } {
        return 0;
    }
    install_functions(unsafe { &mut *image });
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateImage")]
pub unsafe extern "sysv64" fn XCreateImage(
    _display: *mut Display,
    visual: *mut Visual,
    depth: c_uint,
    format: c_int,
    offset: c_int,
    data: *mut c_char,
    width: c_uint,
    height: c_uint,
    pad: c_int,
    stride: c_int,
) -> *mut XImage {
    if width > i32::MAX as u32 || height > i32::MAX as u32 || depth > 32 {
        return ptr::null_mut();
    }
    let mut value = XImage {
        width: width as i32,
        height: height as i32,
        xoffset: offset,
        format,
        data,
        byte_order: 0,
        bitmap_unit: 32,
        bitmap_bit_order: 0,
        bitmap_pad: pad,
        depth: depth as i32,
        bytes_per_line: stride,
        bits_per_pixel: match depth {
            1 => 1,
            2..=8 => 8,
            9..=16 => 16,
            _ => 32,
        },
        red_mask: 0,
        green_mask: 0,
        blue_mask: 0,
        obdata: ptr::null_mut(),
        functions: [0; 6],
    };
    if !visual.is_null() {
        unsafe {
            value.red_mask = (*visual).red_mask as u64;
            value.green_mask = (*visual).green_mask as u64;
            value.blue_mask = (*visual).blue_mask as u64;
        }
    }
    if !unsafe { layout(&raw mut value) } {
        return ptr::null_mut();
    }
    install_functions(&mut value);
    let result = unsafe { guest::malloc(mem::size_of::<XImage>()) }.cast::<XImage>();
    if !result.is_null() {
        unsafe {
            result.write(value);
        }
    }
    result
}
#[unsafe(export_name = "kinakaze_engine_libX11_XDestroyImage")]
pub unsafe extern "sysv64" fn XDestroyImage(image: *mut XImage) -> c_int {
    if !image.is_null() {
        let destroy = unsafe { (*image).functions[1] };
        if destroy != 0 && destroy != XDestroyImage as *const () as usize {
            let callback: unsafe extern "sysv64" fn(*mut XImage) -> c_int =
                unsafe { mem::transmute(destroy) };
            return unsafe { callback(image) };
        }
        unsafe {
            guest::free((*image).data.cast());
            guest::free(image.cast());
        }
    }
    1
}
fn in_bounds(i: &XImage, x: i32, y: i32) -> bool {
    !i.data.is_null() && x >= 0 && y >= 0 && x < i.width && y < i.height
}
unsafe fn bit(i: &XImage, x: usize, y: usize, plane: usize) -> (*mut u8, u8) {
    let bit = x + i.xoffset as usize;
    let unit = i.bitmap_unit as usize / 8;
    let byte = bit / 8;
    let byte = if i.byte_order == i.bitmap_bit_order {
        byte
    } else {
        byte / unit * unit + unit - 1 - byte % unit
    };
    let shift = if i.bitmap_bit_order == 0 {
        bit % 8
    } else {
        7 - bit % 8
    };
    (
        unsafe {
            i.data
                .cast::<u8>()
                .add((plane * i.height as usize + y) * i.bytes_per_line as usize + byte)
        },
        1 << shift,
    )
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetPixel")]
pub unsafe extern "sysv64" fn XGetPixel(image: *mut XImage, x: c_int, y: c_int) -> u64 {
    if image.is_null() {
        return 0;
    }
    let i = unsafe { &*image };
    if !in_bounds(i, x, y) {
        return 0;
    }
    if i.bits_per_pixel == 1 {
        let planes = if i.format == 1 { i.depth } else { 1 };
        let mut value = 0;
        for plane in 0..planes as usize {
            let (p, m) = unsafe { bit(i, x as usize, y as usize, plane) };
            value = (value << 1) | u64::from(unsafe { *p } & m != 0);
        }
        value
    } else {
        let n = i.bits_per_pixel as usize / 8;
        let p = unsafe {
            i.data
                .cast::<u8>()
                .add(y as usize * i.bytes_per_line as usize + (x as usize + i.xoffset as usize) * n)
        };
        let mut value = 0;
        for byte in 0..n {
            let shift = if i.byte_order == 0 {
                byte
            } else {
                n - 1 - byte
            };
            value |= u64::from(unsafe { *p.add(byte) }) << (shift * 8);
        }
        value & ((1u64 << i.depth) - 1)
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XPutPixel")]
pub unsafe extern "sysv64" fn XPutPixel(
    image: *mut XImage,
    x: c_int,
    y: c_int,
    value: u64,
) -> c_int {
    if image.is_null() {
        return 0;
    }
    let i = unsafe { &*image };
    if !in_bounds(i, x, y) {
        return 0;
    }
    if i.bits_per_pixel == 1 {
        let planes = if i.format == 1 { i.depth } else { 1 };
        for plane in 0..planes as usize {
            let (p, m) = unsafe { bit(i, x as usize, y as usize, plane) };
            unsafe {
                *p = (*p & !m)
                    | if value & (1u64 << (planes as usize - plane - 1)) != 0 {
                        m
                    } else {
                        0
                    };
            }
        }
    } else {
        let n = i.bits_per_pixel as usize / 8;
        let p = unsafe {
            i.data
                .cast::<u8>()
                .add(y as usize * i.bytes_per_line as usize + (x as usize + i.xoffset as usize) * n)
        };
        for byte in 0..n {
            let shift = if i.byte_order == 0 {
                byte
            } else {
                n - 1 - byte
            };
            unsafe {
                *p.add(byte) = (value >> (shift * 8)) as u8;
            }
        }
    }
    1
}
unsafe fn allocate_data(image: *mut XImage) -> bool {
    let i = unsafe { &mut *image };
    let planes = if i.format == 1 { i.depth as usize } else { 1 };
    let Some(bytes) = (i.bytes_per_line as usize)
        .checked_mul(i.height as usize)
        .and_then(|v| v.checked_mul(planes))
    else {
        return false;
    };
    i.data = unsafe { guest::malloc(bytes) }.cast();
    if i.data.is_null() {
        return false;
    }
    unsafe {
        ptr::write_bytes(i.data.cast::<u8>(), 0, bytes);
    }
    true
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSubImage")]
pub unsafe extern "sysv64" fn XSubImage(
    source: *mut XImage,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
) -> *mut XImage {
    if source.is_null() || x < 0 || y < 0 {
        return ptr::null_mut();
    }
    let s = unsafe { &*source };
    if x as u64 + width as u64 > s.width as u64 || y as u64 + height as u64 > s.height as u64 {
        return ptr::null_mut();
    }
    let result = unsafe {
        XCreateImage(
            ptr::null_mut(),
            ptr::null_mut(),
            s.depth as u32,
            s.format,
            0,
            ptr::null_mut(),
            width,
            height,
            s.bitmap_pad,
            0,
        )
    };
    if result.is_null() {
        return result;
    }
    if !unsafe { allocate_data(result) } {
        unsafe { XDestroyImage(result) };
        return ptr::null_mut();
    }
    unsafe {
        (*result).red_mask = s.red_mask;
        (*result).green_mask = s.green_mask;
        (*result).blue_mask = s.blue_mask;
    }
    for row in 0..height as i32 {
        for col in 0..width as i32 {
            unsafe {
                XPutPixel(result, col, row, XGetPixel(source, x + col, y + row));
            }
        }
    }
    result
}
#[unsafe(export_name = "kinakaze_engine_libX11_XAddPixel")]
pub unsafe extern "sysv64" fn XAddPixel(image: *mut XImage, value: i64) -> c_int {
    if image.is_null() {
        return 0;
    }
    for y in 0..unsafe { (*image).height } {
        for x in 0..unsafe { (*image).width } {
            unsafe {
                XPutPixel(
                    image,
                    x,
                    y,
                    XGetPixel(image, x, y).wrapping_add(value as u64),
                );
            }
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XPutImage")]
pub unsafe extern "sysv64" fn XPutImage(
    display: *mut Display,
    drawable: usize,
    gc: graphics::GC,
    image: *mut XImage,
    sx: c_int,
    sy: c_int,
    dx: c_int,
    dy: c_int,
    width: c_uint,
    height: c_uint,
) -> c_int {
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] XPutImage drawable={drawable:#x} image={image:p} size={width}x{height}"
        );
    }
    if gc.is_null() {
        return unsafe { errors::report(display, 13, 72, 0, 0) };
    }
    if image.is_null() || sx < 0 || sy < 0 {
        return unsafe { errors::report(display, 2, 72, 0, 0) };
    }
    // XPutImage accepts client-built packed images without XInitImage. Chromium
    // leaves bitmap_pad zero and supplies an explicit byte stride. Padding is
    // irrelevant to reading those pixels; validate the stride at byte alignment
    // without changing the caller's header or installing image callbacks.
    let mut view = unsafe { image.read() };
    if view.format == 2
        && view.bitmap_pad == 0
        && view.bytes_per_line > 0
        && view.bits_per_pixel >= 8
    {
        view.bitmap_pad = 8;
    }
    let image = &raw mut view;
    if !unsafe { layout(image) } {
        if crate::trace_enabled() {
            let i = unsafe { &*image };
            crate::diagnostic!(
                "[libX11] image layout rejected w={} h={} offset={} format={} data={:p} order={} unit={} bitorder={} pad={} depth={} stride={} bpp={}",
                i.width,
                i.height,
                i.xoffset,
                i.format,
                i.data,
                i.byte_order,
                i.bitmap_unit,
                i.bitmap_bit_order,
                i.bitmap_pad,
                i.depth,
                i.bytes_per_line,
                i.bits_per_pixel
            );
        }
        return unsafe { errors::report(display, 2, 72, 0, 0) };
    }
    let i = unsafe { &*image };
    if i.data.is_null()
        || sx as u64 + width as u64 > i.width as u64
        || sy as u64 + height as u64 > i.height as u64
    {
        return unsafe { errors::report(display, 2, 72, 0, 0) };
    }
    let values = unsafe { (*gc).values };
    if values.function != 3 {
        return unsafe { errors::report(display, 17, 72, 0, drawable) };
    }
    let dirty = RECT {
        left: dx,
        top: dy,
        right: dx.saturating_add(width.min(i32::MAX as u32) as i32),
        bottom: dy.saturating_add(height.min(i32::MAX as u32) as i32),
    };
    let ok = graphics::mutate_drawable(drawable, dirty, |pixels, w, h, depth| {
        let mask = values.plane_mask as u32
            & if depth == 32 {
                u32::MAX
            } else {
                (1u32 << depth) - 1
            };
        for row in 0..height.min(i32::MAX as u32) as i32 {
            let y = dy.saturating_add(row);
            if y < 0 || y >= h {
                continue;
            }
            for col in 0..width.min(i32::MAX as u32) as i32 {
                let x = dx.saturating_add(col);
                if x < 0 || x >= w || !unsafe { graphics::clipping::contains(gc, x, y) } {
                    continue;
                }
                let mut value = unsafe { XGetPixel(image, sx + col, sy + row) } as u32;
                if i.format == 0 {
                    value = if value == 0 {
                        values.background
                    } else {
                        values.foreground
                    } as u32;
                }
                let dest = &mut pixels[(y * w + x) as usize];
                *dest = (*dest & !mask) | (value & mask);
            }
        }
    });
    if !ok {
        return unsafe { errors::report(display, 9, 72, 0, drawable) };
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libX11_XListPixmapFormats")]
pub unsafe extern "sysv64" fn XListPixmapFormats(
    _display: *mut Display,
    count: *mut c_int,
) -> *mut PixmapFormat {
    if count.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        *count = 0;
    }
    let result = unsafe { guest::malloc(FORMATS.len() * mem::size_of::<PixmapFormat>()) }
        .cast::<PixmapFormat>();
    if result.is_null() {
        return result;
    }
    unsafe {
        ptr::copy_nonoverlapping(FORMATS.as_ptr(), result, FORMATS.len());
        *count = FORMATS.len() as i32;
    }
    result
}
