//! Point rasterization shares drawable storage and obeys GC logic/plane masks.
use super::*;
fn raster(function: i32, source: u32, dest: u32) -> u32 {
    match function & 15 {
        0 => 0,
        1 => source & dest,
        2 => source & !dest,
        3 => source,
        4 => !source & dest,
        5 => dest,
        6 => source ^ dest,
        7 => source | dest,
        8 => !(source | dest),
        9 => !(source ^ dest),
        10 => !dest,
        11 => source | !dest,
        12 => !source,
        13 => !source | dest,
        14 => !(source & dest),
        _ => u32::MAX,
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XDrawPoint")]
pub unsafe extern "sysv64" fn XDrawPoint(
    d: *mut Display,
    drawable: usize,
    gc: GC,
    x: c_int,
    y: c_int,
) -> c_int {
    let point = XPoint {
        x: x as i16,
        y: y as i16,
    };
    unsafe { XDrawPoints(d, drawable, gc, &point, 1, 0) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XDrawPoints")]
pub unsafe extern "sysv64" fn XDrawPoints(
    display: *mut Display,
    drawable: usize,
    gc: GC,
    points: *const XPoint,
    count: c_int,
    mode: c_int,
) -> c_int {
    if !matches!(mode, 0 | 1) || count < 0 {
        return unsafe { crate::errors::report(display, 2, 64, 0, mode as usize) };
    }
    if count == 0 {
        return 1;
    }
    if gc.is_null() || points.is_null() {
        return 0;
    }
    let values = unsafe { (*gc).values };
    let points = unsafe { core::slice::from_raw_parts(points, count as usize) };
    let positions = || {
        let (mut x, mut y) = (0i32, 0i32);
        points.iter().enumerate().map(move |(i, point)| {
            if mode == 0 || i == 0 {
                x = point.x as i32;
                y = point.y as i32;
            } else {
                x = x.wrapping_add(point.x as i32);
                y = y.wrapping_add(point.y as i32);
            }
            (x, y)
        })
    };
    let Some(bounds) = point_bounds(positions(), 0) else {
        return 1;
    };
    let ok = mutate_drawable(drawable, bounds, |pixels, width, height, depth| {
        let mask = values.plane_mask as u32
            & if depth == 32 {
                u32::MAX
            } else {
                (1u32 << depth) - 1
            };
        for (x, y) in positions() {
            if x < 0 || y < 0 || x >= width || y >= height {
                continue;
            }
            if !unsafe { clipping::contains(gc, x, y) } {
                continue;
            }
            let dest = &mut pixels[(y * width + x) as usize];
            *dest =
                (*dest & !mask) | (raster(values.function, values.foreground as u32, *dest) & mask);
        }
    });
    if !ok {
        return unsafe { crate::errors::report(display, 9, 64, 0, drawable) };
    }
    1
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_boolean_raster_operations() {
        let s = 0xa5a5a5a5;
        let d = 0xcccccccc;
        for function in 0..16 {
            let mut expected = 0;
            for bit in 0..32 {
                let index = (((s >> bit) & 1) << 1) | ((d >> bit) & 1);
                let truth =
                    [0u8, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15][function as usize];
                expected |= (((truth as u32) >> index) & 1) << bit;
            }
            assert_eq!(raster(function, s, d), expected);
        }
    }
}
