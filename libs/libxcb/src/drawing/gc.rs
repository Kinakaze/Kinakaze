//! Packed GC values are decoded in mask order, never in host struct layout.
use super::*;

unsafe fn values(g: &mut XcbGc, mask: u32, p: *const c_void, opcode: u8) -> Result<(), Failure> {
    if mask & !0x7fffff != 0 || (mask != 0 && p.is_null()) {
        return Err(error(2, opcode, mask));
    }
    let mut index = 0;
    for bit in 0..23 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let v = unsafe { p.cast::<u32>().add(index).read_unaligned() };
        index += 1;
        match bit {
            0 if v <= 15 => g.function = v as u8,
            1 => g.plane_mask = v,
            2 => g.foreground = v,
            3 => g.background = v,
            4 => g.line_width = v as u16,
            5 if v == 0 => {}       // solid lines
            6 if v == 1 => {}       // Butt
            7 if v == 0 => {}       // Miter
            8 if v == 0 => {}       // solid fills
            9 if v == 0 => {}       // EvenOdd (rectangles have no self intersections)
            12 | 13 if v == 0 => {} // unused tile origin
            15 if v == 0 => {}      // ClipByChildren
            16 if v <= 1 => g.graphics_exposures = v != 0,
            17 => g.clip_origin.0 = v as i16,
            18 => g.clip_origin.1 = v as i16,
            19 if v == 0 => g.clip = None,
            20 if v == 0 => {} // solid line dash offset
            21 if v == 4 => {} // protocol default dash
            22 if v == 1 => {} // PieSlice
            0 | 5 | 6 | 7 | 8 | 9 | 15 | 16 | 22
                if v > match bit {
                    0 => 15,
                    5 => 2,
                    6 => 3,
                    7 => 2,
                    8 => 3,
                    _ => 1,
                } =>
            {
                return Err(error(2, opcode, v));
            }
            _ => return Err(error(17, opcode, v)),
        }
    }
    Ok(())
}
unsafe fn gc_change(
    checked: bool,
    create: Option<u32>,
    id: u32,
    mask: u32,
    p: *const c_void,
) -> xcb_void_cookie_t {
    let opcode = if create.is_some() { 55 } else { 56 };
    let sequence = request::submit(checked, false, || {
        let mut g = if let Some(drawable) = create {
            if !valid_id(&xcb_state().lock().unwrap(), id) {
                return Err(error(14, opcode, id));
            }
            XcbGc {
                depth: dimensions(drawable, opcode)?.2,
                ..Default::default()
            }
        } else {
            xcb_state()
                .lock()
                .unwrap()
                .gcs
                .get(&id)
                .cloned()
                .ok_or(error(13, opcode, id))?
        };
        unsafe { values(&mut g, mask, p, opcode) }?;
        xcb_state().lock().unwrap().gcs.insert(id, g);
        Ok(None)
    });
    xcb_void_cookie_t { sequence }
}
macro_rules! create_gc_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
            drawable: u32,
            mask: u32,
            p: *const c_void,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            unsafe { gc_change($checked, Some(drawable), id, mask, p) }
        }
    };
}
create_gc_export!(xcb_create_gc, false);
create_gc_export!(xcb_create_gc_checked, true);
macro_rules! change_gc_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
            mask: u32,
            p: *const c_void,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            unsafe { gc_change($checked, None, id, mask, p) }
        }
    };
}
change_gc_export!(xcb_change_gc, false);
change_gc_export!(xcb_change_gc_checked, true);
macro_rules! clip_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            order: u8,
            id: u32,
            x: i16,
            y: i16,
            n: u32,
            p: *const xcb_rectangle_t,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    if order > 3 || (n != 0 && p.is_null()) {
                        return Err(error(2, 59, order as u32));
                    }
                    let mut s = xcb_state().lock().unwrap();
                    let g = s.gcs.get_mut(&id).ok_or(error(13, 59, id))?;
                    let mut rectangles = Vec::new();
                    rectangles
                        .try_reserve_exact(n as usize)
                        .map_err(|_| error(11, 59, id))?;
                    for i in 0..n as usize {
                        rectangles.push(unsafe { p.add(i).read_unaligned() });
                    }
                    g.clip_origin = (x, y);
                    g.clip = Some(rectangles.into());
                    Ok(None)
                }),
            }
        }
    };
}
clip_export!(xcb_set_clip_rectangles, false);
clip_export!(xcb_set_clip_rectangles_checked, true);

pub(super) fn get(id: u32, drawable: u32, opcode: u8) -> Result<XcbGc, Failure> {
    let g = xcb_state()
        .lock()
        .unwrap()
        .gcs
        .get(&id)
        .cloned()
        .ok_or(error(13, opcode, id))?;
    if dimensions(drawable, opcode)?.2 != g.depth {
        return Err(error(8, opcode, drawable));
    }
    Ok(g)
}
impl XcbGc {
    pub(super) fn contains(&self, x: i32, y: i32) -> bool {
        let Some(rects) = &self.clip else {
            return true;
        };
        let x = x - self.clip_origin.0 as i32;
        let y = y - self.clip_origin.1 as i32;
        rects.iter().any(|r| {
            x >= r.x as i32
                && y >= r.y as i32
                && x < r.x as i32 + r.width as i32
                && y < r.y as i32 + r.height as i32
        })
    }
    pub(super) fn pixel(&self, src: u32, dst: u32) -> u32 {
        let value = match self.function {
            0 => 0,
            1 => src & dst,
            2 => src & !dst,
            3 => src,
            4 => !src & dst,
            5 => dst,
            6 => src ^ dst,
            7 => src | dst,
            8 => !(src | dst),
            9 => !(src ^ dst),
            10 => !dst,
            11 => src | !dst,
            12 => !src,
            13 => !src | dst,
            14 => !(src & dst),
            15 => u32::MAX,
            _ => unreachable!(),
        };
        let mask = self.plane_mask & depth_mask(self.depth);
        ((value & mask) | (dst & !mask)) & depth_mask(self.depth)
    }
}
