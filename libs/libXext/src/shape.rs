//! SHAPE 1.1 client ABI, backed by shared native window regions.
use super::*;
use kinakaze_libX11::{XRectangle, region::RegionRec, shape, xfixes::shapes};
pub const ShapeBounding: c_int = 0;
pub const ShapeClip: c_int = 1;
pub const ShapeInput: c_int = 2;
pub const ShapeSet: c_int = 0;
pub const ShapeUnion: c_int = 1;
pub const ShapeIntersect: c_int = 2;
pub const ShapeSubtract: c_int = 3;
pub const ShapeInvert: c_int = 4;
fn report(d: *mut Display, window: Window, minor: u8, result: Result<(), u8>) {
    if let Err(code) = result {
        unsafe {
            kinakaze_libX11::errors::report(d, code, shape::OPCODE, minor, window);
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeQueryExtension")]
pub unsafe extern "sysv64" fn XShapeQueryExtension(
    d: *mut Display,
    event: *mut c_int,
    error: *mut c_int,
) -> Bool {
    let mut opcode = 0;
    unsafe {
        kinakaze_libX11::xext::XQueryExtension(d, c"SHAPE".as_ptr(), &mut opcode, event, error)
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeQueryVersion")]
pub unsafe extern "sysv64" fn XShapeQueryVersion(
    _d: *mut Display,
    major: *mut c_int,
    minor: *mut c_int,
) -> Status {
    unsafe {
        if !major.is_null() {
            *major = 1;
        }
        if !minor.is_null() {
            *minor = 1;
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeSelectInput")]
pub unsafe extern "sysv64" fn XShapeSelectInput(d: *mut Display, window: Window, mask: u64) {
    report(d, window, 6, shape::select(d, window, mask));
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeInputSelected")]
pub unsafe extern "sysv64" fn XShapeInputSelected(d: *mut Display, window: Window) -> u64 {
    u64::from(shape::selected(d, window))
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeQueryExtents")]
pub unsafe extern "sysv64" fn XShapeQueryExtents(
    d: *mut Display,
    window: Window,
    bounding: *mut Bool,
    bx: *mut c_int,
    by: *mut c_int,
    bw: *mut c_uint,
    bh: *mut c_uint,
    clip: *mut Bool,
    cx: *mut c_int,
    cy: *mut c_int,
    cw: *mut c_uint,
    ch: *mut c_uint,
) -> Status {
    for (kind, shaped, x, y, width, height) in
        [(0, bounding, bx, by, bw, bh), (1, clip, cx, cy, cw, ch)]
    {
        let Some(r) = shapes::get(d, window, kind) else {
            return 0;
        };
        unsafe {
            if !shaped.is_null() {
                *shaped = i32::from(
                    shapes::custom(window, kind).is_some()
                        || kind == 0 && shapes::rounded_radius(window) > 0,
                );
            }
            if !x.is_null() {
                *x = r.extents.x as i32;
            }
            if !y.is_null() {
                *y = r.extents.y as i32;
            }
            if !width.is_null() {
                *width = r.extents.width as u32;
            }
            if !height.is_null() {
                *height = r.extents.height as u32;
            }
        }
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeGetRectangles")]
pub unsafe extern "sysv64" fn XShapeGetRectangles(
    d: *mut Display,
    window: Window,
    kind: c_int,
    count: *mut c_int,
    ordering: *mut c_int,
) -> *mut XRectangle {
    if count.is_null() || ordering.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        *count = 0;
        *ordering = 0;
    }
    if !(0..=2).contains(&kind) {
        report(d, window, 8, Err(2));
        return ptr::null_mut();
    }
    let Some(r) = shapes::get(d, window, kind as u8) else {
        return ptr::null_mut();
    };
    if r.rects.is_empty() {
        return ptr::null_mut();
    }
    let result =
        unsafe { guest::malloc(r.rects.len() * mem::size_of::<XRectangle>()).cast::<XRectangle>() };
    if !result.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(r.rects.as_ptr(), result, r.rects.len());
            *count = r.rects.len() as i32;
        }
    }
    result
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeCombineRectangles")]
pub unsafe extern "sysv64" fn XShapeCombineRectangles(
    d: *mut Display,
    window: Window,
    kind: c_int,
    x: c_int,
    y: c_int,
    rectangles: *mut XRectangle,
    count: c_int,
    op: c_int,
    ordering: c_int,
) {
    if count < 0 || count > 0 && rectangles.is_null() || !(0..=3).contains(&ordering) {
        report(d, window, 1, Err(2));
        return;
    }
    let rects = if count == 0 {
        Vec::new()
    } else {
        unsafe { core::slice::from_raw_parts(rectangles, count as usize).to_vec() }
    };
    report(
        d,
        window,
        1,
        shape::combine(
            d,
            window,
            kind,
            Some(RegionRec {
                rects,
                extents: XRectangle::default(),
            }),
            x,
            y,
            op,
        ),
    );
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeCombineRegion")]
pub unsafe extern "sysv64" fn XShapeCombineRegion(
    d: *mut Display,
    window: Window,
    kind: c_int,
    x: c_int,
    y: c_int,
    region: kinakaze_libX11::region::Region,
    op: c_int,
) {
    let Some(region) = (unsafe { region.as_ref() }) else {
        report(d, window, 1, Err(2));
        return;
    };
    report(
        d,
        window,
        1,
        shape::combine(d, window, kind, Some(region.clone()), x, y, op),
    );
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeCombineMask")]
pub unsafe extern "sysv64" fn XShapeCombineMask(
    d: *mut Display,
    window: Window,
    kind: c_int,
    x: c_int,
    y: c_int,
    bitmap: usize,
    op: c_int,
) {
    let source = if bitmap == 0 {
        Ok(None)
    } else {
        shape::bitmap(bitmap).map(Some)
    };
    report(
        d,
        window,
        2,
        source.and_then(|r| shape::combine(d, window, kind, r, x, y, op)),
    );
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeCombineShape")]
pub unsafe extern "sysv64" fn XShapeCombineShape(
    d: *mut Display,
    window: Window,
    kind: c_int,
    x: c_int,
    y: c_int,
    source: Window,
    source_kind: c_int,
    op: c_int,
) {
    if !(0..=2).contains(&source_kind) {
        report(d, window, 3, Err(2));
        return;
    }
    let source = shapes::get(d, source, source_kind as u8).ok_or(3u8);
    report(
        d,
        window,
        3,
        source.and_then(|r| shape::combine(d, window, kind, Some(r), x, y, op)),
    );
}
#[unsafe(export_name = "kinakaze_engine_libXext_XShapeOffsetShape")]
pub unsafe extern "sysv64" fn XShapeOffsetShape(
    d: *mut Display,
    window: Window,
    kind: c_int,
    x: c_int,
    y: c_int,
) {
    if !(0..=2).contains(&kind) {
        report(d, window, 4, Err(2));
        return;
    }
    if let Some(region) = shapes::custom(window, kind as u8) {
        report(
            d,
            window,
            4,
            shape::combine(d, window, kind, Some(region), x, y, ShapeSet),
        );
    }
}
