//! X11 image formats use protocol scanline padding and bit order.
use super::*;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject,
};

fn with_region<T>(
    id: u32,
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    opcode: u8,
    f: impl FnOnce(&[u32], usize) -> T,
) -> Result<T, Failure> {
    let (dw, dh, _) = dimensions(id, opcode)?;
    if x < 0 || y < 0 || x as usize + w > dw as usize || y as usize + h > dh as usize {
        return Err(error(8, opcode, id));
    }
    if w == 0 || h == 0 {
        return Ok(f(&[], 0));
    }
    if id == 1 {
        let pixels = unsafe { gdi_capture(core::ptr::null_mut(), x, y, w as u32, h as u32) }
            .ok_or(error(11, opcode, id))?;
        Ok(f(&pixels, w))
    } else {
        read(id, opcode, |src, stride, height, _| {
            // Native resize can change dimensions between lookup and locking the backing store.
            if x as usize + w > stride as usize || y as usize + h > height as usize {
                return Err(error(8, opcode, id));
            }
            let first = y as usize * stride as usize + x as usize;
            let end = (y as usize + h - 1) * stride as usize + x as usize + w;
            Ok(f(&src[first..end], stride as usize))
        })?
    }
}
pub(super) fn region(
    id: u32,
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    opcode: u8,
) -> Result<Vec<u32>, Failure> {
    with_region(id, x, y, w, h, opcode, |src, stride| {
        let mut pixels = zeroed(w * h, opcode, id)?;
        for row in 0..h {
            pixels[row * w..(row + 1) * w].copy_from_slice(&src[row * stride..row * stride + w]);
        }
        Ok(pixels)
    })?
}
fn get_image(
    checked: bool,
    format: u8,
    id: u32,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
    mask: u32,
) -> xcb_get_image_cookie_t {
    let sequence = request::submit(checked, true, || {
        if !matches!(format, 1 | 2) {
            return Err(error(2, 73, format as u32));
        }
        let (dw, dh, depth) = dimensions(id, 73)?;
        if x < 0 || y < 0 || x as i32 + w as i32 > dw || y as i32 + h as i32 > dh {
            return Err(error(8, 73, id));
        }
        let mask = mask & depth_mask(depth);
        let w = w as usize;
        let h = h as usize;
        let bitmap = format == 1 || depth == 1;
        let stride = if bitmap { w.div_ceil(32) * 4 } else { w * 4 };
        let planes = if format == 1 {
            mask.count_ones() as usize
        } else {
            1
        };
        let mut bytes = zeroed::<u8>(32 + stride * h * planes, 73, id)?;
        bytes[0] = 1;
        bytes[1] = depth;
        let words = ((bytes.len() - 32) / 4) as u32;
        request::put32(&mut bytes, 4, words);
        let visual = if xcb_state().lock().unwrap().pixmaps.contains_key(&id) {
            0
        } else {
            1
        };
        request::put32(&mut bytes, 8, visual);
        with_region(id, x as i32, y as i32, w, h, 73, |pixels, source_stride| {
            if !bitmap {
                for y in 0..h {
                    for x in 0..w {
                        request::put32(
                            &mut bytes,
                            32 + (y * w + x) * 4,
                            pixels[y * source_stride + x] & mask,
                        );
                    }
                }
            } else {
                let mut plane_index = 0;
                for bit in (0..if format == 1 { depth } else { 1 }).rev() {
                    if format == 1 && mask & (1u32 << bit) == 0 {
                        continue;
                    }
                    for y in 0..h {
                        for x in 0..w {
                            if pixels[y * source_stride + x] & mask & (1u32 << bit) != 0 {
                                bytes[32 + plane_index * stride * h + y * stride + x / 8] |=
                                    1 << (x % 8);
                            }
                        }
                    }
                    plane_index += 1;
                }
            }
        })?;
        Ok(Some(bytes))
    });
    xcb_get_image_cookie_t { sequence }
}
macro_rules! get_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            format: u8,
            id: u32,
            x: i16,
            y: i16,
            w: u16,
            h: u16,
            mask: u32,
        ) -> xcb_get_image_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            get_image($checked, format, id, x, y, w, h, mask)
        }
    };
}
get_export!(xcb_get_image, true);
get_export!(xcb_get_image_unchecked, false);
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_image_reply")]
pub unsafe extern "sysv64" fn xcb_get_image_reply(
    _c: *mut xcb_connection_t,
    cookie: xcb_get_image_cookie_t,
    e: *mut *mut xcb_generic_error_t,
) -> *mut xcb_get_image_reply_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { request::take(cookie.sequence, e).cast() }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_image_data")]
pub unsafe extern "sysv64" fn xcb_get_image_data(r: *const xcb_get_image_reply_t) -> *mut u8 {
    if r.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { r.add(1).cast_mut().cast() }
    }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_image_data_length")]
pub unsafe extern "sysv64" fn xcb_get_image_data_length(r: *const xcb_get_image_reply_t) -> c_int {
    if r.is_null() {
        0
    } else {
        unsafe { ((*r).length * 4) as c_int }
    }
}
unsafe fn put_image(
    checked: bool,
    format: u8,
    id: u32,
    gc: u32,
    w: u16,
    h: u16,
    x: i16,
    y: i16,
    pad: u8,
    depth: u8,
    len: u32,
    p: *const u8,
) -> xcb_void_cookie_t {
    let sequence = request::submit(checked, false, || {
        if format > 2 || pad >= 32 || (format == 2 && pad != 0) {
            return Err(error(2, 72, format as u32));
        }
        let g = gc::get(gc, id, 72)?;
        if (format == 0 && depth != 1) || (format != 0 && depth != g.depth) {
            return Err(error(8, 72, id));
        }
        let w = w as usize;
        let h = h as usize;
        let bitmap = format != 2 || depth == 1;
        let stride = if bitmap {
            (w + pad as usize).div_ceil(32) * 4
        } else {
            w * 4
        };
        let planes = if format == 1 { depth as usize } else { 1 };
        let length = stride * h * planes;
        if length > len as usize || (length != 0 && p.is_null()) {
            return Err(error(16, 72, len));
        }
        if w == 0 || h == 0 {
            return Ok(None);
        }
        let data = unsafe { core::slice::from_raw_parts(p, length) };
        let dirty = RECT {
            left: x as i32,
            top: y as i32,
            right: x as i32 + w as i32,
            bottom: y as i32 + h as i32,
        };
        mutate(id, 72, dirty, |dst, dw, dh, _| {
            for dy in dirty.top.max(0)..dirty.bottom.min(dh) {
                for dx in dirty.left.max(0)..dirty.right.min(dw) {
                    if !g.contains(dx, dy) {
                        continue;
                    }
                    let sx = (dx - dirty.left) as usize;
                    let sy = (dy - dirty.top) as usize;
                    let value = if !bitmap {
                        u32::from_ne_bytes(
                            data[sy * stride + sx * 4..sy * stride + sx * 4 + 4]
                                .try_into()
                                .unwrap(),
                        )
                    } else {
                        let bit = sx + pad as usize;
                        let offset = sy * stride + bit / 8;
                        let mut v = 0;
                        for plane in 0..planes {
                            v = (v << 1)
                                | ((data[plane * stride * h + offset] >> (bit % 8)) & 1) as u32;
                        }
                        if format == 0 {
                            if v == 0 { g.background } else { g.foreground }
                        } else {
                            v
                        }
                    };
                    let target = &mut dst[dy as usize * dw as usize + dx as usize];
                    *target = g.pixel(value, *target);
                }
            }
        })?;
        Ok(None)
    });
    xcb_void_cookie_t { sequence }
}
macro_rules! put_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            format: u8,
            id: u32,
            gc: u32,
            w: u16,
            h: u16,
            x: i16,
            y: i16,
            pad: u8,
            depth: u8,
            len: u32,
            p: *const u8,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            unsafe { put_image($checked, format, id, gc, w, h, x, y, pad, depth, len, p) }
        }
    };
}
put_export!(xcb_put_image, false);
put_export!(xcb_put_image_checked, true);

unsafe fn gdi_capture(hwnd: HWND, x: i32, y: i32, width: u32, height: u32) -> Option<Vec<u32>> {
    let count = width as usize * height as usize;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count).ok()?;
    pixels.resize(count, 0u32);
    let hdc = if hwnd.is_null() {
        unsafe { GetDC(core::ptr::null_mut()) }
    } else {
        unsafe { GetDC(hwnd) }
    };
    if hdc.is_null() {
        return None;
    }
    let memdc = unsafe { CreateCompatibleDC(hdc) };
    if memdc.is_null() {
        unsafe { ReleaseDC(hwnd, hdc) };
        return None;
    }

    let mut bmi: BITMAPINFO = unsafe { core::mem::zeroed() };
    bmi.bmiHeader.biSize = core::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width as i32;
    bmi.bmiHeader.biHeight = -(height as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    let mut bits_ptr: *mut c_void = core::ptr::null_mut();
    let hbitmap = unsafe {
        CreateDIBSection(
            memdc,
            &raw const bmi,
            DIB_RGB_COLORS,
            &raw mut bits_ptr,
            core::ptr::null_mut(),
            0,
        )
    };
    if hbitmap.is_null() || bits_ptr.is_null() {
        unsafe {
            if !hbitmap.is_null() {
                DeleteObject(hbitmap);
            }
            DeleteDC(memdc);
            ReleaseDC(hwnd, hdc);
        }
        return None;
    }

    let old_bmp = unsafe { SelectObject(memdc, hbitmap) };
    let copied = unsafe { BitBlt(memdc, 0, 0, width as i32, height as i32, hdc, x, y, SRCCOPY) };
    unsafe { SelectObject(memdc, old_bmp) };

    if copied != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(bits_ptr.cast::<u32>(), pixels.as_mut_ptr(), count);
        }
    }

    unsafe {
        DeleteObject(hbitmap);
        DeleteDC(memdc);
        ReleaseDC(hwnd, hdc);
    }
    (copied != 0).then_some(pixels)
}
