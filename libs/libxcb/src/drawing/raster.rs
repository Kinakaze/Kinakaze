//! Rectangle rasterization and overlap-safe copies on shared backing surfaces.
use super::*;

fn fill(p: &mut [u32], w: i32, h: i32, r: RECT, g: &XcbGc) {
    let left = r.left.max(0);
    let right = r.right.min(w);
    if left >= right {
        return;
    }
    for y in r.top.max(0)..r.bottom.min(h) {
        let row = &mut p
            [y as usize * w as usize + left as usize..y as usize * w as usize + right as usize];
        if g.clip.is_none()
            && g.function == 3
            && g.plane_mask & depth_mask(g.depth) == depth_mask(g.depth)
        {
            row.fill(g.foreground & depth_mask(g.depth));
        } else {
            for (offset, dst) in row.iter_mut().enumerate() {
                if g.contains(left + offset as i32, y) {
                    *dst = g.pixel(g.foreground, *dst);
                }
            }
        }
    }
}
unsafe fn rectangles(
    checked: bool,
    outline: bool,
    id: u32,
    gc: u32,
    n: u32,
    p: *const xcb_rectangle_t,
) -> xcb_void_cookie_t {
    let opcode = if outline { 67 } else { 70 };
    let sequence = request::submit(checked, false, || {
        let g = gc::get(gc, id, opcode)?;
        if n != 0 && p.is_null() {
            return Err(error(16, opcode, n));
        }
        for i in 0..n as usize {
            let r = unsafe { p.add(i).read_unaligned() };
            let x = r.x as i32;
            let y = r.y as i32;
            let w = r.width as i32;
            let h = r.height as i32;
            let line = g.line_width.max(1) as i32;
            let half = line / 2;
            let dirty = if outline {
                RECT {
                    left: x - half,
                    top: y - half,
                    right: x + w + line - half,
                    bottom: y + h + line - half,
                }
            } else {
                RECT {
                    left: x,
                    top: y,
                    right: x + w,
                    bottom: y + h,
                }
            };
            mutate(id, opcode, dirty, |pixels, dw, dh, _| {
                if !outline {
                    fill(pixels, dw, dh, dirty, &g);
                    return;
                }
                // Disjoint spans make XOR corners touch each pixel exactly once.
                let top = (y + line - half).min(dirty.bottom);
                let bottom = (y + h - half).max(top);
                fill(
                    pixels,
                    dw,
                    dh,
                    RECT {
                        bottom: top,
                        ..dirty
                    },
                    &g,
                );
                fill(
                    pixels,
                    dw,
                    dh,
                    RECT {
                        top: bottom,
                        ..dirty
                    },
                    &g,
                );
                if top < bottom {
                    let left = (x + line - half).min(dirty.right);
                    let right = (x + w - half).max(left);
                    fill(
                        pixels,
                        dw,
                        dh,
                        RECT {
                            top,
                            bottom,
                            right: left,
                            ..dirty
                        },
                        &g,
                    );
                    fill(
                        pixels,
                        dw,
                        dh,
                        RECT {
                            top,
                            bottom,
                            left: right,
                            ..dirty
                        },
                        &g,
                    );
                }
            })?;
        }
        Ok(None)
    });
    xcb_void_cookie_t { sequence }
}
macro_rules! rectangles_export {
    ($name:ident,$checked:expr,$outline:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
            gc: u32,
            n: u32,
            p: *const xcb_rectangle_t,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            unsafe { rectangles($checked, $outline, id, gc, n, p) }
        }
    };
}
rectangles_export!(xcb_poly_fill_rectangle, false, false);
rectangles_export!(xcb_poly_fill_rectangle_checked, true, false);
rectangles_export!(xcb_poly_rectangle, false, true);
rectangles_export!(xcb_poly_rectangle_checked, true, true);

fn copy_area(
    checked: bool,
    src: u32,
    dst: u32,
    gc: u32,
    sx: i16,
    sy: i16,
    dx: i16,
    dy: i16,
    w: u16,
    h: u16,
) -> xcb_void_cookie_t {
    let mut exposures = None;
    let sequence = request::submit(checked, false, || {
        let g = gc::get(gc, dst, 62)?;
        let (sw, sh, depth) = dimensions(src, 62)?;
        if depth != g.depth {
            return Err(error(8, 62, src));
        }
        let (dw, dh, _) = dimensions(dst, 62)?;
        let left = 0.max(-(sx as i32)).max(-(dx as i32));
        let top = 0.max(-(sy as i32)).max(-(dy as i32));
        let right = (w as i32).min(sw - sx as i32).min(dw - dx as i32).max(left);
        let bottom = (h as i32).min(sh - sy as i32).min(dh - dy as i32).max(top);
        let cw = (right - left) as usize;
        let ch = (bottom - top) as usize;
        if cw > 0 && ch > 0 {
            let pixels = image::region(src, sx as i32 + left, sy as i32 + top, cw, ch, 62)?;
            let rect = RECT {
                left: dx as i32 + left,
                top: dy as i32 + top,
                right: dx as i32 + right,
                bottom: dy as i32 + bottom,
            };
            mutate(dst, 62, rect, |out, stride, _, _| {
                for y in 0..ch {
                    for x in 0..cw {
                        let tx = rect.left + x as i32;
                        let ty = rect.top + y as i32;
                        if g.contains(tx, ty) {
                            let p = &mut out[ty as usize * stride as usize + tx as usize];
                            *p = g.pixel(pixels[y * cw + x], *p);
                        }
                    }
                }
            })?;
        }
        if g.graphics_exposures {
            // Report unavailable source pixels after destination and GC clipping.
            let mut missing = Vec::new();
            if sx < 0 || sy < 0 || sx as i32 + w as i32 > sw || sy as i32 + h as i32 > sh {
                for y in (dy as i32).max(0)..(dy as i32 + h as i32).min(dh) {
                    let mut run = None;
                    let end = (dx as i32 + w as i32).min(dw);
                    for x in (dx as i32).max(0)..=end {
                        let outside = sx as i32 + x - (dx as i32) < 0
                            || sx as i32 + x - dx as i32 >= sw
                            || sy as i32 + y - (dy as i32) < 0
                            || sy as i32 + y - dy as i32 >= sh;
                        if x < end && outside && g.contains(x, y) {
                            run.get_or_insert(x);
                        } else if let Some(start) = run.take() {
                            missing.push((start, y, x - start));
                        }
                    }
                }
            }
            exposures = Some(missing);
        }
        Ok(None)
    });
    if let Some(missing) = exposures {
        let mut store = xcb_state().lock().unwrap();
        if missing.is_empty() {
            let mut e = [0u8; 36];
            e[0] = 14;
            request::put16(&mut e, 2, sequence as u16);
            request::put32(&mut e, 4, dst);
            e[10] = 62;
            request::put32(&mut e, 32, sequence);
            store
                .pending_events
                .push_back(unsafe { e.as_ptr().cast::<xcb_generic_event_t>().read_unaligned() });
        } else {
            for (i, (x, y, w)) in missing.iter().enumerate() {
                let mut e = [0u8; 36];
                e[0] = 13;
                request::put16(&mut e, 2, sequence as u16);
                request::put32(&mut e, 4, dst);
                request::put16(&mut e, 8, *x as u16);
                request::put16(&mut e, 10, *y as u16);
                request::put16(&mut e, 12, *w as u16);
                request::put16(&mut e, 14, 1);
                request::put16(
                    &mut e,
                    18,
                    (missing.len() - i - 1).min(u16::MAX as usize) as u16,
                );
                e[20] = 62;
                request::put32(&mut e, 32, sequence);
                store.pending_events.push_back(unsafe {
                    e.as_ptr().cast::<xcb_generic_event_t>().read_unaligned()
                });
            }
        }
    }
    x11::notify_event();
    xcb_void_cookie_t { sequence }
}
macro_rules! copy_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            src: u32,
            dst: u32,
            gc: u32,
            sx: i16,
            sy: i16,
            dx: i16,
            dy: i16,
            w: u16,
            h: u16,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            copy_area($checked, src, dst, gc, sx, sy, dx, dy, w, h)
        }
    };
}
copy_export!(xcb_copy_area, false);
copy_export!(xcb_copy_area_checked, true);
