//! Core GC clipping shared by GDI and direct pixel drawing.
use super::*;
use windows_sys::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, RGN_OR, RestoreDC, SaveDC, SelectClipRgn,
};

pub(crate) unsafe fn contains(gc: GC, x: i32, y: i32) -> bool {
    let Some(gc) = (unsafe { gc.as_ref() }) else {
        return true;
    };
    gc.clip_rectangles.as_ref().is_none_or(|rects| {
        let x = i64::from(x) - i64::from(gc.values.clip_x_origin);
        let y = i64::from(y) - i64::from(gc.values.clip_y_origin);
        rects.iter().any(|r| {
            x >= i64::from(r[0])
                && y >= i64::from(r[1])
                && x < i64::from(r[2])
                && y < i64::from(r[3])
        })
    })
}

pub(super) unsafe fn get(d: usize, gc: GC) -> Option<(HWND, HDC, i32)> {
    let (hwnd, dc) = unsafe { get_win_hdc(d) }?;
    let saved = unsafe { SaveDC(dc) };
    if saved == 0 {
        unsafe { release_win_hdc(hwnd, dc) };
        return None;
    }
    let applied = (|| {
        let Some(gc) = (unsafe { gc.as_ref() }) else {
            return true;
        };
        let Some(rects) = &gc.clip_rectangles else {
            return true;
        };
        let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
        if region.is_null() {
            return false;
        }
        let mut ok = true;
        for r in rects {
            let rect = unsafe {
                CreateRectRgn(
                    r[0].saturating_add(gc.values.clip_x_origin),
                    r[1].saturating_add(gc.values.clip_y_origin),
                    r[2].saturating_add(gc.values.clip_x_origin),
                    r[3].saturating_add(gc.values.clip_y_origin),
                )
            };
            if rect.is_null() {
                ok = false;
                break;
            }
            ok = unsafe { CombineRgn(region, region, rect, RGN_OR) } != 0;
            unsafe { DeleteObject(rect) };
            if !ok {
                break;
            }
        }
        if ok {
            ok = unsafe { SelectClipRgn(dc, region) } != 0;
        }
        unsafe { DeleteObject(region) };
        ok
    })();
    if !applied {
        unsafe { release(hwnd, dc, saved) };
        return None;
    }
    Some((hwnd, dc, saved))
}

pub(super) unsafe fn release(hwnd: HWND, dc: HDC, saved: i32) {
    unsafe {
        RestoreDC(dc, saved);
        release_win_hdc(hwnd, dc);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetRegion")]
pub unsafe extern "sysv64" fn XSetRegion(
    d: *mut Display,
    gc: GC,
    region: crate::region::Region,
) -> c_int {
    if gc.is_null() {
        return unsafe { crate::errors::report(d, 13, 59, 0, 0) };
    }
    let Some(region) = (unsafe { region.as_ref() }) else {
        return 0;
    };
    unsafe {
        (*gc).clip_rectangles = Some(
            region
                .rects
                .iter()
                .map(|r| {
                    [
                        r.x as i32,
                        r.y as i32,
                        r.x as i32 + r.width as i32,
                        r.y as i32 + r.height as i32,
                    ]
                })
                .collect(),
        );
        (*gc).values.clip_mask = 0;
        (*gc).values.clip_x_origin = 0;
        (*gc).values.clip_y_origin = 0;
    }
    1
}

pub(super) fn bitmap_rectangles(pixmap: usize) -> Option<Vec<[i32; 4]>> {
    let mask = drawable_snapshot(pixmap)?;
    if mask.depth != 1 {
        return None;
    }
    let mut rectangles = Vec::new();
    for y in 0..mask.height {
        let mut x = 0;
        while x < mask.width {
            if mask.pixels[(y * mask.width + x) as usize] & 1 == 0 {
                x += 1;
                continue;
            }
            let left = x;
            while x < mask.width && mask.pixels[(y * mask.width + x) as usize] & 1 != 0 {
                x += 1;
            }
            rectangles.push([left, y, x, y + 1]);
        }
    }
    Some(rectangles)
}
