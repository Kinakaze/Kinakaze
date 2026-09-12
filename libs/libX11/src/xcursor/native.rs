//! Win32 ARGB cursors and snapshots of the user's native cursor shapes.
use super::*;
use windows_sys::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};
pub(super) struct NativeCursor(pub usize);
impl Drop for NativeCursor {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.0 as _);
        }
    }
}
pub(super) fn default_size() -> i32 {
    unsafe { GetSystemMetrics(SM_CXCURSOR) }
}
pub(super) unsafe fn from_image(image: &XcursorImage) -> Option<NativeCursor> {
    if image.width == 0
        || image.height == 0
        || image.width > i32::MAX as u32
        || image.height > i32::MAX as u32
        || image.xhot >= image.width
        || image.yhot >= image.height
        || image.pixels.is_null()
    {
        return None;
    }
    let n = (image.width as usize).checked_mul(image.height as usize)?;
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = image.width as i32;
    info.bmiHeader.biHeight = -(image.height as i32);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    let mut bits = ptr::null_mut();
    let color = unsafe {
        CreateDIBSection(
            ptr::null_mut(),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            ptr::null_mut(),
            0,
        )
    };
    if color.is_null() {
        return None;
    }
    unsafe {
        ptr::copy_nonoverlapping(image.pixels, bits.cast(), n);
    }
    let stride = (image.width as usize).div_ceil(16) * 2;
    let mask_bits = vec![0u8; stride.checked_mul(image.height as usize)?];
    let mask = unsafe {
        CreateBitmap(
            image.width as i32,
            image.height as i32,
            1,
            1,
            mask_bits.as_ptr().cast(),
        )
    };
    let result = if mask.is_null() {
        ptr::null_mut()
    } else {
        unsafe {
            CreateIconIndirect(&ICONINFO {
                fIcon: 0,
                xHotspot: image.xhot,
                yHotspot: image.yhot,
                hbmMask: mask,
                hbmColor: color,
            })
        }
    };
    unsafe {
        DeleteObject(color);
        if !mask.is_null() {
            DeleteObject(mask);
        }
    }
    (!result.is_null()).then_some(NativeCursor(result as usize))
}
fn resource(name: &[u8]) -> Option<*const u16> {
    Some(match name {
        b"default" | b"left_ptr" | b"arrow" | b"top_left_arrow" => IDC_ARROW,
        b"text" | b"xterm" => IDC_IBEAM,
        b"crosshair" | b"cross" | b"tcross" | b"plus" => IDC_CROSS,
        b"pointer" | b"hand" | b"hand1" | b"hand2" => IDC_HAND,
        b"ew-resize" | b"sb_h_double_arrow" | b"left_side" | b"right_side" => IDC_SIZEWE,
        b"ns-resize" | b"sb_v_double_arrow" | b"top_side" | b"bottom_side" => IDC_SIZENS,
        b"nwse-resize" | b"top_left_corner" | b"bottom_right_corner" => IDC_SIZENWSE,
        b"nesw-resize" | b"top_right_corner" | b"bottom_left_corner" => IDC_SIZENESW,
        b"all-scroll" | b"fleur" | b"size_all" | b"sizing" => IDC_SIZEALL,
        b"not-allowed" | b"no-drop" => IDC_NO,
        b"wait" | b"watch" | b"clock" => IDC_WAIT,
        b"progress" | b"left_ptr_watch" => IDC_APPSTARTING,
        b"help" | b"question_arrow" => IDC_HELP,
        _ => return None,
    })
}
unsafe fn bitmap_pixels(bitmap: HBITMAP, width: u32, height: u32) -> Option<Vec<u32>> {
    let mut pixels = vec![0; (width as usize).checked_mul(height as usize)?];
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width as i32;
    info.bmiHeader.biHeight = -(height as i32);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    let dc = unsafe { GetDC(ptr::null_mut()) };
    if dc.is_null() {
        return None;
    }
    let lines = unsafe {
        GetDIBits(
            dc,
            bitmap,
            0,
            height,
            pixels.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        ReleaseDC(ptr::null_mut(), dc);
    }
    (lines == height as i32).then_some(pixels)
}
pub(super) unsafe fn snapshot(cursor: usize, size: i32) -> *mut XcursorImage {
    let mut info = ICONINFO::default();
    if unsafe { GetIconInfo(cursor as _, &mut info) } == 0 {
        return ptr::null_mut();
    }
    let result = (|| {
        let mut bitmap = BITMAP::default();
        if unsafe {
            GetObjectW(
                info.hbmMask,
                size_of::<BITMAP>() as i32,
                (&mut bitmap as *mut BITMAP).cast(),
            )
        } == 0
        {
            return None;
        }
        let w = u32::try_from(bitmap.bmWidth).ok()?;
        let h = u32::try_from(bitmap.bmHeight).ok()? / if info.hbmColor.is_null() { 2 } else { 1 };
        if w == 0 || h == 0 {
            return None;
        }
        let mask = unsafe { bitmap_pixels(info.hbmMask, w, bitmap.bmHeight as u32) }?;
        let mut colors = if info.hbmColor.is_null() {
            mask[w as usize * h as usize..].to_vec()
        } else {
            unsafe { bitmap_pixels(info.hbmColor, w, h) }?
        };
        if info.hbmColor.is_null() {
            monochrome_argb(&mut colors, &mask, w as usize);
        } else if colors.iter().all(|p| p >> 24 == 0) {
            for (p, m) in colors.iter_mut().zip(&mask) {
                *p = (*p & 0xffffff) | if m & 0xffffff == 0 { 0xff000000 } else { 0 };
            }
        }
        let image = unsafe { XcursorImageCreate(w as i32, h as i32) };
        if image.is_null() {
            return None;
        }
        unsafe {
            (*image).size = size as u32;
            (*image).xhot = info.xHotspot;
            (*image).yhot = info.yHotspot;
            ptr::copy_nonoverlapping(colors.as_ptr(), (*image).pixels, colors.len());
        }
        Some(image)
    })()
    .unwrap_or(ptr::null_mut());
    unsafe {
        if !info.hbmColor.is_null() {
            DeleteObject(info.hbmColor);
        }
        DeleteObject(info.hbmMask);
    }
    result
}
/// ARGB cannot encode XOR-with-the-background. Keep those strokes visible as
/// black with a white outline, instead of dropping every pixel of an I-beam.
fn monochrome_argb(colors: &mut [u32], mask: &[u32], width: usize) {
    let inverted: Vec<bool> = colors
        .iter()
        .zip(mask)
        .map(|(xor, and)| xor & and & 0xffffff != 0)
        .collect();
    for (pixel, and) in colors.iter_mut().zip(mask) {
        *pixel = if and & 0xffffff == 0 {
            *pixel | 0xff000000
        } else if *pixel & 0xffffff != 0 {
            0xff000000
        } else {
            0
        };
    }
    let height = colors.len() / width;
    for (index, &invert) in inverted.iter().enumerate() {
        if !invert {
            continue;
        }
        let (x, y) = (index % width, index / width);
        for yy in y.saturating_sub(1)..=(y + 1).min(height - 1) {
            for xx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                let pixel = &mut colors[yy * width + xx];
                if *pixel == 0 {
                    *pixel = 0xffffffff;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inverted_monochrome_cursor_stays_visible_on_light_and_dark_backgrounds() {
        let mut xor = vec![0; 25];
        xor[12] = 0xffffff;
        monochrome_argb(&mut xor, &[0xffffff; 25], 5);
        assert_eq!(xor[12], 0xff000000);
        assert_eq!(xor[11], 0xffffffff);
        assert_eq!(xor[0], 0);
    }
}
pub(super) unsafe fn load_images(name: &[u8], size: i32) -> *mut XcursorImages {
    let Some(resource) = resource(name) else {
        return ptr::null_mut();
    };
    let shared = unsafe { LoadCursorW(ptr::null_mut(), resource) };
    if shared.is_null() {
        return ptr::null_mut();
    }
    let cursor = unsafe { CopyImage(shared, IMAGE_CURSOR, size, size, 0) };
    if cursor.is_null() {
        return ptr::null_mut();
    }
    let image = unsafe { snapshot(cursor as usize, size) };
    unsafe {
        DestroyCursor(cursor);
    }
    if image.is_null() {
        return ptr::null_mut();
    }
    let images = unsafe { XcursorImagesCreate(1) };
    unsafe {
        if images.is_null() {
            XcursorImageDestroy(image);
            return images;
        }
        *(*images).images = image;
        (*images).nimage = 1;
    }
    images
}
