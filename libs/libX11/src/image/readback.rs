//! Read drawable pixels directly into the final client buffer. Geometry probes
//! borrow metadata, and subregion reads never allocate a full-surface snapshot.
use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subimage_clips_destination_and_keeps_uncovered_pixels() {
        unsafe {
            let display = crate::shared_display();
            let pixmap = graphics::XCreatePixmap(display, 1, 4, 4, 24);
            assert_ne!(pixmap, 0);
            let gc = graphics::XCreateGC(display, pixmap, 0, ptr::null_mut());
            assert!(!gc.is_null());
            graphics::XSetForeground(display, gc, 0x123456);
            graphics::XFillRectangle(display, pixmap, gc, 0, 0, 4, 4);
            let destination = XCreateImage(
                display,
                crate::XDefaultVisual(display, 0),
                24,
                2,
                0,
                ptr::null_mut(),
                5,
                5,
                32,
                0,
            );
            assert!(!destination.is_null() && allocate_data(destination));
            for y in 0..5 {
                for x in 0..5 {
                    XPutPixel(destination, x, y, 0xaabbcc);
                }
            }
            assert_eq!(
                XGetSubImage(display, pixmap, 0, 0, 4, 4, !0, 2, destination, -1, 2),
                destination
            );
            assert_eq!(XGetPixel(destination, 0, 2), 0x123456);
            assert_eq!(XGetPixel(destination, 2, 4), 0x123456);
            assert_eq!(XGetPixel(destination, 3, 4), 0xaabbcc);
            assert_eq!(XGetPixel(destination, 0, 1), 0xaabbcc);
            XDestroyImage(destination);
            graphics::XFreeGC(display, gc);
            graphics::XFreePixmap(display, pixmap);
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetSubImage")]
pub unsafe extern "sysv64" fn XGetSubImage(
    display: *mut Display,
    drawable: usize,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    plane_mask: u64,
    format: c_int,
    destination: *mut XImage,
    dest_x: c_int,
    dest_y: c_int,
) -> *mut XImage {
    if !unsafe { layout(destination) } || unsafe { (*destination).data.is_null() } {
        return ptr::null_mut();
    }
    let source = unsafe { XGetImage(display, drawable, x, y, width, height, plane_mask, format) };
    if source.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        // XGetSubImage clips the copy against the caller's image. In particular,
        // negative destination offsets skip source pixels rather than wrap.
        let first_x = (-(dest_x as i64)).max(0).min(width as i64);
        let first_y = (-(dest_y as i64)).max(0).min(height as i64);
        let end_x = (width as i64).min((*destination).width as i64 - dest_x as i64);
        let end_y = (height as i64).min((*destination).height as i64 - dest_y as i64);
        for sy in first_y..end_y {
            for sx in first_x..end_x {
                let pixel = XGetPixel(source, sx as c_int, sy as c_int);
                XPutPixel(
                    destination,
                    (sx + dest_x as i64) as c_int,
                    (sy + dest_y as i64) as c_int,
                    pixel,
                );
            }
        }
        XDestroyImage(source);
    }
    destination
}
/// Validate before locking the surface. Return protocol errors for the caller
/// to deliver after the drawing lock has been released (handlers may reenter).
pub unsafe fn read_into(
    drawable: usize,
    image: *mut XImage,
    x: i32,
    y: i32,
    mask: u64,
) -> Result<(), u8> {
    if !unsafe { layout(image) } {
        return Err(2);
    }
    let view = unsafe { &*image };
    if view.data.is_null() {
        return Err(2);
    }
    let mask = mask & ((1u64 << view.depth) - 1);
    graphics::read_drawable(drawable, |pixels, width, height, depth| {
        if x < 0
            || y < 0
            || x as i64 + view.width as i64 > width as i64
            || y as i64 + view.height as i64 > height as i64
            || view.depth as u32 != depth
            || view.format == 0
        {
            return Err(8);
        }
        // XYPixmap with a partial plane mask needs compacted plane storage.
        // Do not return a full-plane layout for that different protocol format.
        if view.format == 1 && mask != (1u64 << depth) - 1 {
            return Err(8);
        }
        for row in 0..view.height {
            let start = (y + row) as usize * width as usize + x as usize;
            let source = &pixels[start..start + view.width as usize];
            if view.format == 2 && view.bits_per_pixel == 32 && view.byte_order == 0 {
                let target = unsafe {
                    view.data.cast::<u8>().add(
                        row as usize * view.bytes_per_line as usize + view.xoffset as usize * 4,
                    )
                };
                if mask == u32::MAX as u64 {
                    unsafe {
                        ptr::copy_nonoverlapping(
                            source.as_ptr().cast::<u8>(),
                            target,
                            source.len() * 4,
                        );
                    }
                } else {
                    for (column, pixel) in source.iter().enumerate() {
                        unsafe {
                            target
                                .add(column * 4)
                                .cast::<u32>()
                                .write_unaligned(*pixel & mask as u32);
                        }
                    }
                }
            } else {
                for (column, pixel) in source.iter().enumerate() {
                    unsafe {
                        XPutPixel(image, column as i32, row, *pixel as u64 & mask);
                    }
                }
            }
        }
        Ok(())
    })
    .ok_or(9)?
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetImage")]
pub unsafe extern "sysv64" fn XGetImage(
    display: *mut Display,
    drawable: usize,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    plane_mask: u64,
    format: c_int,
) -> *mut XImage {
    let Some((surface_width, surface_height, depth)) = graphics::drawable_dimensions(drawable)
    else {
        unsafe {
            errors::report(display, 9, 73, 0, drawable);
        }
        return ptr::null_mut();
    };
    if x < 0
        || y < 0
        || x as u64 + width as u64 > surface_width as u64
        || y as u64 + height as u64 > surface_height as u64
        || format != 2
    {
        unsafe {
            errors::report(display, 8, 73, 0, drawable);
        }
        return ptr::null_mut();
    }
    let result = unsafe {
        XCreateImage(
            display,
            crate::XDefaultVisual(display, 0),
            depth,
            format,
            0,
            ptr::null_mut(),
            width,
            height,
            32,
            0,
        )
    };
    if result.is_null() {
        return result;
    }
    if !unsafe { allocate_data(result) } {
        unsafe {
            XDestroyImage(result);
        }
        return ptr::null_mut();
    }
    if let Err(code) = unsafe { read_into(drawable, result, x, y, plane_mask) } {
        unsafe {
            XDestroyImage(result);
            errors::report(display, code, 73, 0, drawable);
        }
        return ptr::null_mut();
    }
    result
}
