//! Window regions shared by XFixes and the native window backend.
use super::*;
use windows_sys::Win32::{Foundation::POINT, Graphics::Gdi::*, UI::WindowsAndMessaging::*};
fn key(w: usize, kind: u8) -> String {
    format!("shape/{w}/{kind}")
}
pub fn custom(w: usize, kind: u8) -> Option<RegionRec> {
    let bytes = crate::shared::get(&key(w, kind))?;
    if bytes.len() % 8 != 0 {
        return None;
    }
    let mut region = RegionRec {
        rects: (0..bytes.len())
            .step_by(8)
            .map(|i| super::rectangle(&bytes, i))
            .collect(),
        extents: XRectangle::default(),
    };
    crate::region::update_extents(&mut region);
    Some(region)
}
pub fn rounded_radius(w: usize) -> i32 {
    kinakaze_libdisplay::ui::decorations::rounded_radius(w)
}
pub fn native_bounds(w: usize) -> Option<RegionRec> {
    let mut rect = unsafe { core::mem::zeroed() };
    if unsafe { GetClientRect(w as _, &mut rect) } == 0 {
        return None;
    }
    let width = (rect.right - rect.left).clamp(0, 65535);
    let height = (rect.bottom - rect.top).clamp(0, 65535);
    let radius = rounded_radius(w).min(width / 2).min(height / 2);
    let mut region = RegionRec::default();
    if radius == 0 {
        region.rects.push(XRectangle {
            x: 0,
            y: 0,
            width: width as u16,
            height: height as u16,
        });
    } else {
        for y in 0..height {
            let edge = y.min(height - 1 - y);
            let inset = if edge >= radius {
                0
            } else {
                let dy = radius as f64 - edge as f64 - 0.5;
                (radius as f64 - ((radius * radius) as f64 - dy * dy).sqrt()).ceil() as i32
            };
            let row = XRectangle {
                x: inset as i16,
                y: y as i16,
                width: (width - 2 * inset).max(0) as u16,
                height: 1,
            };
            if let Some(last) = region.rects.last_mut()
                && last.x == row.x
                && last.width == row.width
            {
                last.height += 1;
            } else {
                region.rects.push(row);
            }
        }
    }
    crate::region::update_extents(&mut region);
    Some(region)
}
pub fn get(d: *mut Display, w: usize, kind: u8) -> Option<RegionRec> {
    if let Some(r) = custom(w, kind) {
        return Some(r);
    }
    if kind == 0 && rounded_radius(w) > 0 {
        return native_bounds(w);
    }
    window_region(d, w)
}
pub fn clear() {
    kinakaze_libdisplay::ui::input_shape::clear();
}
pub fn set(
    d: *mut Display,
    w: usize,
    kind: u8,
    mut region: Option<RegionRec>,
    x: i16,
    y: i16,
) -> Result<(), u8> {
    if kind > 2 {
        return Err(2);
    }
    let bounds = window_region(d, w).ok_or(3u8)?;
    if let Some(r) = region.as_mut() {
        unsafe {
            crate::region::XOffsetRegion(r, x as i32, y as i32);
        }
    }
    if kind == 2 {
        kinakaze_libdisplay::ui::input_shape::set(
            w,
            region.as_ref().map(|r| {
                r.rects
                    .iter()
                    .map(|r| {
                        (
                            r.x as i32,
                            r.y as i32,
                            r.x as i32 + r.width as i32,
                            r.y as i32 + r.height as i32,
                        )
                    })
                    .collect()
            }),
        );
    }
    if kind == 0 && w != 1 {
        // A complete rectangular bounding region has no custom silhouette.
        // Keeping an HRGN for it disables antialiased DWM corners entirely.
        let custom = region.as_ref().is_some_and(|r| r.rects != bounds.rects);
        unsafe {
            let handle = w as _;
            let native = if let Some(r) = &region
                && custom
            {
                let total = CreateRectRgn(0, 0, 0, 0);
                if total.is_null() {
                    return Err(11);
                }
                let mut origin = POINT { x: 0, y: 0 };
                ClientToScreen(handle, &mut origin);
                let mut outer = core::mem::zeroed();
                GetWindowRect(handle, &mut outer);
                for r in &r.rects {
                    let x = r.x as i32 + origin.x - outer.left;
                    let y = r.y as i32 + origin.y - outer.top;
                    let part = CreateRectRgn(x, y, x + r.width as i32, y + r.height as i32);
                    if part.is_null() {
                        DeleteObject(total);
                        return Err(11);
                    }
                    CombineRgn(total, total, part, RGN_OR);
                    DeleteObject(part);
                }
                total
            } else {
                core::ptr::null_mut()
            };
            if SetWindowRgn(handle, native, 1) == 0 {
                if !native.is_null() {
                    DeleteObject(native);
                }
                return Err(8);
            }
            kinakaze_libdisplay::ui::decorations::custom_shape(w, custom);
        }
    }
    if let Some(r) = &region {
        let mut bytes = vec![0; r.rects.len() * 8];
        for (i, rect) in r.rects.iter().enumerate() {
            super::encode(&mut bytes, i * 8, *rect);
        }
        crate::shared::set(key(w, kind), bytes)?;
    } else {
        crate::shared::remove(&key(w, kind));
    }
    let shaped = region.is_some() || kind == 0 && rounded_radius(w) > 0;
    let effective = region.or_else(|| get(d, w, kind)).unwrap_or(bounds);
    crate::shape::changed(w, kind, effective, shaped);
    Ok(())
}
