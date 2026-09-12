//! X11 Region geometry implementation.
//!
//! Provides rectangle-based Region representations and boolean set operations.

use std::os::raw::{c_int, c_short, c_uint, c_ushort};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XRectangle {
    pub x: c_short,
    pub y: c_short,
    pub width: c_ushort,
    pub height: c_ushort,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XPoint {
    pub x: c_short,
    pub y: c_short,
}

#[derive(Clone, Debug, Default)]
pub struct RegionRec {
    pub rects: Vec<XRectangle>,
    pub extents: XRectangle,
}

pub type Region = *mut RegionRec;
type Bool = c_int;

pub(crate) fn update_extents(region: &mut RegionRec) {
    if region.rects.is_empty() {
        region.extents = XRectangle::default();
        return;
    }
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for r in &region.rects {
        let rx1 = r.x as i32;
        let ry1 = r.y as i32;
        let rx2 = rx1 + r.width as i32;
        let ry2 = ry1 + r.height as i32;

        min_x = min_x.min(rx1);
        min_y = min_y.min(ry1);
        max_x = max_x.max(rx2);
        max_y = max_y.max(ry2);
    }

    region.extents = XRectangle {
        x: min_x.max(i16::MIN as i32).min(i16::MAX as i32) as c_short,
        y: min_y.max(i16::MIN as i32).min(i16::MAX as i32) as c_short,
        width: (max_x - min_x).max(0).min(u16::MAX as i32) as c_ushort,
        height: (max_y - min_y).max(0).min(u16::MAX as i32) as c_ushort,
    };
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateRegion")]
pub unsafe extern "sysv64" fn XCreateRegion() -> Region {
    Box::into_raw(Box::new(RegionRec::default()))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDestroyRegion")]
pub unsafe extern "sysv64" fn XDestroyRegion(r: Region) -> c_int {
    if !r.is_null() {
        unsafe { drop(Box::from_raw(r)) };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUnionRectWithRegion")]
pub unsafe extern "sysv64" fn XUnionRectWithRegion(
    rectangle: *const XRectangle,
    src_region: Region,
    dest_region: Region,
) -> c_int {
    if rectangle.is_null() || dest_region.is_null() {
        return 0;
    }
    let rect = unsafe { *rectangle };
    if src_region != dest_region && !src_region.is_null() {
        unsafe {
            (*dest_region).rects = (*src_region).rects.clone();
        }
    }
    if rect.width > 0 && rect.height > 0 {
        unsafe {
            (*dest_region).rects.push(rect);
            update_extents(&mut *dest_region);
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUnionRegion")]
pub unsafe extern "sysv64" fn XUnionRegion(sra: Region, srb: Region, dr: Region) -> c_int {
    if dr.is_null() {
        return 0;
    }
    let mut new_rects = Vec::new();
    if !sra.is_null() {
        unsafe { new_rects.extend_from_slice(&(*sra).rects) };
    }
    if !srb.is_null() {
        unsafe { new_rects.extend_from_slice(&(*srb).rects) };
    }
    unsafe {
        (*dr).rects = new_rects;
        update_extents(&mut *dr);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XIntersectRegion")]
pub unsafe extern "sysv64" fn XIntersectRegion(sra: Region, srb: Region, dr: Region) -> c_int {
    if dr.is_null() {
        return 0;
    }
    let mut new_rects = Vec::new();
    if !sra.is_null() && !srb.is_null() {
        unsafe {
            for ra in &(*sra).rects {
                for rb in &(*srb).rects {
                    let x1 = (ra.x as i32).max(rb.x as i32);
                    let y1 = (ra.y as i32).max(rb.y as i32);
                    let x2 = (ra.x as i32 + ra.width as i32).min(rb.x as i32 + rb.width as i32);
                    let y2 = (ra.y as i32 + ra.height as i32).min(rb.y as i32 + rb.height as i32);
                    if x2 > x1 && y2 > y1 {
                        new_rects.push(XRectangle {
                            x: x1 as c_short,
                            y: y1 as c_short,
                            width: (x2 - x1) as c_ushort,
                            height: (y2 - y1) as c_ushort,
                        });
                    }
                }
            }
        }
    }
    unsafe {
        (*dr).rects = new_rects;
        update_extents(&mut *dr);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSubtractRegion")]
pub unsafe extern "sysv64" fn XSubtractRegion(sra: Region, srb: Region, dr: Region) -> c_int {
    if dr.is_null() {
        return 0;
    }
    if !sra.is_null() {
        unsafe {
            let a = (*sra).rects.clone();
            let b = if srb.is_null() {
                Vec::new()
            } else {
                (*srb).rects.clone()
            };
            (*dr).rects = subtract(&a, &b);
            update_extents(&mut *dr);
        }
    } else {
        unsafe {
            (*dr).rects.clear();
            (*dr).extents = XRectangle::default();
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XXorRegion")]
pub unsafe extern "sysv64" fn XXorRegion(sra: Region, srb: Region, dr: Region) -> c_int {
    if dr.is_null() {
        return 0;
    }
    unsafe {
        let a = if sra.is_null() {
            Vec::new()
        } else {
            (*sra).rects.clone()
        };
        let b = if srb.is_null() {
            Vec::new()
        } else {
            (*srb).rects.clone()
        };
        let mut out = subtract(&a, &b);
        out.extend(subtract(&b, &a));
        (*dr).rects = out;
        update_extents(&mut *dr);
    }
    1
}

pub(crate) fn subtract(a: &[XRectangle], b: &[XRectangle]) -> Vec<XRectangle> {
    let mut pieces = a.to_vec();
    for cut in b {
        let mut next = Vec::new();
        for r in pieces {
            let (x, y, right, bottom) = (
                r.x as i32,
                r.y as i32,
                r.x as i32 + r.width as i32,
                r.y as i32 + r.height as i32,
            );
            let (left, top, end, base) = (
                x.max(cut.x as i32),
                y.max(cut.y as i32),
                right.min(cut.x as i32 + cut.width as i32),
                bottom.min(cut.y as i32 + cut.height as i32),
            );
            if left >= end || top >= base {
                next.push(r);
                continue;
            }
            for (x1, y1, x2, y2) in [
                (x, y, right, top),
                (x, base, right, bottom),
                (x, top, left, base),
                (end, top, right, base),
            ] {
                if x2 > x1 && y2 > y1 {
                    next.push(XRectangle {
                        x: x1 as i16,
                        y: y1 as i16,
                        width: (x2 - x1) as u16,
                        height: (y2 - y1) as u16,
                    });
                }
            }
        }
        pieces = next;
    }
    pieces
}

#[unsafe(export_name = "kinakaze_engine_libX11_XOffsetRegion")]
pub unsafe extern "sysv64" fn XOffsetRegion(r: Region, dx: c_int, dy: c_int) -> c_int {
    if r.is_null() {
        return 0;
    }
    unsafe {
        for rect in &mut (*r).rects {
            rect.x = rect.x.wrapping_add(dx as c_short);
            rect.y = rect.y.wrapping_add(dy as c_short);
        }
        update_extents(&mut *r);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XShrinkRegion")]
pub unsafe extern "sysv64" fn XShrinkRegion(r: Region, dx: c_int, dy: c_int) -> c_int {
    if r.is_null() {
        return 0;
    }
    unsafe {
        for rect in &mut (*r).rects {
            rect.x += dx as c_short;
            rect.y += dy as c_short;
            rect.width = rect.width.saturating_sub((2 * dx).max(0) as u16);
            rect.height = rect.height.saturating_sub((2 * dy).max(0) as u16);
        }
        update_extents(&mut *r);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XEmptyRegion")]
pub unsafe extern "sysv64" fn XEmptyRegion(r: Region) -> Bool {
    if r.is_null() {
        1
    } else {
        unsafe { if (*r).rects.is_empty() { 1 } else { 0 } }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XEqualRegion")]
pub unsafe extern "sysv64" fn XEqualRegion(r1: Region, r2: Region) -> Bool {
    if r1 == r2 {
        return 1;
    }
    if r1.is_null() || r2.is_null() {
        return 0;
    }
    unsafe {
        if (*r1).extents == (*r2).extents && (*r1).rects.len() == (*r2).rects.len() {
            1
        } else {
            0
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XPointInRegion")]
pub unsafe extern "sysv64" fn XPointInRegion(r: Region, x: c_int, y: c_int) -> Bool {
    if r.is_null() {
        return 0;
    }
    unsafe {
        let e = &(*r).extents;
        if x < e.x as i32
            || x >= e.x as i32 + e.width as i32
            || y < e.y as i32
            || y >= e.y as i32 + e.height as i32
        {
            return 0;
        }
        for rect in &(*r).rects {
            if x >= rect.x as i32
                && x < rect.x as i32 + rect.width as i32
                && y >= rect.y as i32
                && y < rect.y as i32 + rect.height as i32
            {
                return 1;
            }
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRectInRegion")]
pub unsafe extern "sysv64" fn XRectInRegion(
    r: Region,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
) -> c_int {
    if r.is_null() || width == 0 || height == 0 {
        return 0; // RectangleOut
    }
    unsafe {
        let e = &(*r).extents;
        let rx2 = x + width as i32;
        let ry2 = y + height as i32;
        let ex2 = e.x as i32 + e.width as i32;
        let ey2 = e.y as i32 + e.height as i32;
        if rx2 <= e.x as i32 || x >= ex2 || ry2 <= e.y as i32 || y >= ey2 {
            return 0; // RectangleOut
        }
    }
    1 // RectangleIn / RectanglePart
}

#[unsafe(export_name = "kinakaze_engine_libX11_XClipBox")]
pub unsafe extern "sysv64" fn XClipBox(r: Region, rect_return: *mut XRectangle) -> c_int {
    if r.is_null() || rect_return.is_null() {
        return 0;
    }
    unsafe { *rect_return = (*r).extents };
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XPolygonRegion")]
pub unsafe extern "sysv64" fn XPolygonRegion(
    points: *const XPoint,
    n: c_int,
    _fill_rule: c_int,
) -> Region {
    let r = unsafe { XCreateRegion() };
    if points.is_null() || n <= 0 {
        return r;
    }
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for i in 0..n as usize {
        let pt = unsafe { *points.add(i) };
        min_x = min_x.min(pt.x as i32);
        min_y = min_y.min(pt.y as i32);
        max_x = max_x.max(pt.x as i32);
        max_y = max_y.max(pt.y as i32);
    }
    let rect = XRectangle {
        x: min_x as c_short,
        y: min_y as c_short,
        width: (max_x - min_x).max(1) as c_ushort,
        height: (max_y - min_y).max(1) as c_ushort,
    };
    unsafe { XUnionRectWithRegion(&rect, r, r) };
    r
}
