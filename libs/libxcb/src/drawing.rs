//! Drawable storage and pixel access shared by image and drawing requests.
use super::*;
use request::error;
pub(super) type Failure = x11::errors::ProtocolError;
mod gc;
mod image;
mod raster;
pub use gc::*;
pub use image::*;
pub use raster::*;

pub(super) fn valid_id(s: &XcbState, id: u32) -> bool {
    (0x1000_0000..0x2000_0000).contains(&id)
        && !s.gcs.contains_key(&id)
        && !s.pixmaps.contains_key(&id)
        && !s.wid_to_hwnd.contains_key(&id)
        && !resources::used(id)
}
fn dimensions(id: u32, opcode: u8) -> Result<(i32, i32, u8), Failure> {
    if id == 1 {
        return Ok(unsafe {
            (
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
                24,
            )
        });
    }
    if let Some(p) = xcb_state().lock().unwrap().pixmaps.get(&id) {
        return Ok((p.width as i32, p.height as i32, p.depth));
    }
    let native = get_native_hwnd(id).ok_or(error(9, opcode, id))?;
    x11::graphics::drawable_dimensions(native as usize)
        .map(|(w, h, d)| (w, h, d as u8))
        .ok_or(error(11, opcode, id))
}
fn read<T>(id: u32, opcode: u8, f: impl FnOnce(&[u32], i32, i32, u8) -> T) -> Result<T, Failure> {
    if let Some(p) = xcb_state().lock().unwrap().pixmaps.get(&id) {
        return Ok(f(&p.data, p.width as i32, p.height as i32, p.depth));
    }
    let native = get_native_hwnd(id).ok_or(error(9, opcode, id))?;
    x11::graphics::read_drawable(native as usize, |p, w, h, d| f(p, w, h, d as u8))
        .ok_or(error(11, opcode, id))
}
fn mutate(
    id: u32,
    opcode: u8,
    dirty: RECT,
    f: impl FnOnce(&mut [u32], i32, i32, u8),
) -> Result<(), Failure> {
    if let Some(p) = xcb_state().lock().unwrap().pixmaps.get_mut(&id) {
        f(&mut p.data, p.width as i32, p.height as i32, p.depth);
        return Ok(());
    }
    let native = get_native_hwnd(id).ok_or(error(9, opcode, id))?;
    if x11::graphics::mutate_drawable(native as usize, dirty, |p, w, h, d| f(p, w, h, d as u8)) {
        Ok(())
    } else {
        Err(error(11, opcode, id))
    }
}
fn zeroed<T: Default + Clone>(n: usize, opcode: u8, id: u32) -> Result<Vec<T>, Failure> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| error(11, opcode, id))?;
    v.resize(n, T::default());
    Ok(v)
}
fn depth_mask(depth: u8) -> u32 {
    u32::MAX >> (32 - depth)
}

fn create_pixmap(
    checked: bool,
    depth: u8,
    id: u32,
    drawable: u32,
    w: u16,
    h: u16,
) -> xcb_void_cookie_t {
    let sequence = request::submit(checked, false, || {
        if !valid_id(&xcb_state().lock().unwrap(), id) {
            return Err(error(14, 53, id));
        }
        dimensions(drawable, 53)?;
        if w == 0 || h == 0 || !matches!(depth, 1 | 24 | 32) {
            return Err(error(2, 53, id));
        }
        let data = zeroed(w as usize * h as usize, 53, id)?;
        xcb_state().lock().unwrap().pixmaps.insert(
            id,
            XcbPixmap {
                width: w,
                height: h,
                depth,
                data,
            },
        );
        Ok(None)
    });
    xcb_void_cookie_t { sequence }
}
macro_rules! create_pixmap_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            depth: u8,
            id: u32,
            drawable: u32,
            w: u16,
            h: u16,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            create_pixmap($checked, depth, id, drawable, w, h)
        }
    };
}
create_pixmap_export!(xcb_create_pixmap, false);
create_pixmap_export!(xcb_create_pixmap_checked, true);
macro_rules! free_export {
    ($name:ident,$checked:expr,$store:ident,$opcode:expr,$bad:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    xcb_state()
                        .lock()
                        .unwrap()
                        .$store
                        .remove(&id)
                        .ok_or(error($bad, $opcode, id))?;
                    Ok(None)
                }),
            }
        }
    };
}
free_export!(xcb_free_pixmap, false, pixmaps, 54, 4);
free_export!(xcb_free_pixmap_checked, true, pixmaps, 54, 4);
free_export!(xcb_free_gc, false, gcs, 60, 13);
free_export!(xcb_free_gc_checked, true, gcs, 60, 13);
