//! XCB resource names retain native colormaps and cursor image ownership.
use super::*;
use std::sync::Arc;
mod lifecycle;
struct Cursor {
    native: usize,
    owned: bool,
    width: u16,
    height: u16,
    x: u16,
    y: u16,
    pixels: Arc<[u32]>,
}
impl Drop for Cursor {
    fn drop(&mut self) {
        if self.owned {
            unsafe { x11::graphics::XFreeCursor(x11::connection::xcb_display(), self.native) };
        }
    }
}
#[derive(Default)]
struct State {
    maps: HashMap<u32, usize>,
    cursors: HashMap<u32, Arc<Cursor>>,
    windows: HashMap<u32, Arc<Cursor>>,
    window_maps: HashMap<u32, u32>,
}
fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}
pub(super) fn used(id: u32) -> bool {
    let s = state().lock().unwrap();
    s.maps.contains_key(&id) || s.cursors.contains_key(&id)
}
pub(super) fn native_map(id: u32, opcode: u8) -> Result<usize, x11::errors::ProtocolError> {
    if id < 2 {
        return Ok(id as usize);
    }
    state()
        .lock()
        .unwrap()
        .maps
        .get(&id)
        .copied()
        .ok_or(request::error(12, opcode, id))
}
pub(super) fn validate_cursor(id: u32, opcode: u8) -> Result<(), x11::errors::ProtocolError> {
    if id == 0
        || state().lock().unwrap().cursors.contains_key(&id)
        || x11::xcursor::cursor_image(id as usize).is_some()
    {
        Ok(())
    } else {
        Err(request::error(6, opcode, id))
    }
}
pub(super) fn apply_cursor(
    window: u32,
    id: u32,
    opcode: u8,
) -> Result<(), x11::errors::ProtocolError> {
    let cursor = if id == 0 {
        None
    } else {
        Some(
            state()
                .lock()
                .unwrap()
                .cursors
                .get(&id)
                .cloned()
                .or_else(|| {
                    let image = x11::xcursor::cursor_image(id as usize)?;
                    Some(Arc::new(Cursor {
                        native: id as usize,
                        owned: false,
                        width: image.width,
                        height: image.height,
                        x: image.x,
                        y: image.y,
                        pixels: image.pixels.clone(),
                    }))
                })
                .ok_or(request::error(6, opcode, id))?,
        )
    };
    let native = window::native(window, opcode)?;
    let success = unsafe {
        if let Some(c) = &cursor {
            x11::XDefineCursor(x11::connection::xcb_display(), native, c.native)
        } else {
            x11::XUndefineCursor(x11::connection::xcb_display(), native)
        }
    };
    if success == 0 {
        return Err(request::error(11, opcode, id));
    }
    let mut s = state().lock().unwrap();
    if let Some(c) = cursor {
        s.windows.insert(window, c);
    } else {
        s.windows.remove(&window);
    }
    Ok(())
}
pub(super) fn set_map(window: u32, id: u32) {
    state().lock().unwrap().window_maps.insert(window, id);
}
pub(super) fn window_map(window: u32) -> u32 {
    state()
        .lock()
        .unwrap()
        .window_maps
        .get(&window)
        .copied()
        .unwrap_or(1)
}
pub(super) fn forget_window(window: u32) {
    let mut s = state().lock().unwrap();
    s.windows.remove(&window);
    s.window_maps.remove(&window);
}
fn cursor(
    width: u16,
    height: u16,
    x: u16,
    y: u16,
    mut pixels: Vec<u32>,
) -> Result<Arc<Cursor>, x11::errors::ProtocolError> {
    let image = x11::xcursor::XcursorImage {
        version: 1,
        size: width.max(height) as u32,
        width: width as u32,
        height: height as u32,
        xhot: x as u32,
        yhot: y as u32,
        delay: 0,
        pixels: pixels.as_mut_ptr(),
    };
    let native =
        unsafe { x11::xcursor::XcursorImageLoadCursor(x11::connection::xcb_display(), &image) };
    if native == 0 {
        return Err(request::error(11, 93, 0));
    }
    Ok(Arc::new(Cursor {
        native,
        owned: true,
        width,
        height,
        x,
        y,
        pixels: x11::xcursor::cursor_image(native).unwrap().pixels.clone(),
    }))
}
macro_rules! colormap_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            alloc: u8,
            id: u32,
            window: u32,
            visual: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    if !drawing::valid_id(&xcb_state().lock().unwrap(), id) {
                        return Err(request::error(14, 78, id));
                    }
                    let window = window::native(window, 78)?;
                    if alloc > 1 {
                        return Err(request::error(2, 78, alloc as u32));
                    }
                    if visual != 1 || alloc != 0 {
                        return Err(request::error(8, 78, visual));
                    }
                    let native = x11::errors::capture(|| unsafe {
                        x11::XCreateColormap(
                            x11::connection::xcb_display(),
                            window,
                            x11::XDefaultVisual(x11::connection::xcb_display(), 0),
                            alloc as i32,
                        )
                    })?;
                    if native == 0 {
                        return Err(request::error(11, 78, id));
                    }
                    state().lock().unwrap().maps.insert(id, native);
                    Ok(None)
                }),
            }
        }
    };
}
colormap_export!(xcb_create_colormap, false);
colormap_export!(xcb_create_colormap_checked, true);
macro_rules! free_colormap_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    if id == 1 {
                        return Ok(None);
                    }
                    let map = state()
                        .lock()
                        .unwrap()
                        .maps
                        .remove(&id)
                        .ok_or(request::error(12, 79, id))?;
                    x11::errors::capture(|| unsafe {
                        x11::XFreeColormap(x11::connection::xcb_display(), map)
                    })?;
                    Ok(None)
                }),
            }
        }
    };
}
free_colormap_export!(xcb_free_colormap, false);
free_colormap_export!(xcb_free_colormap_checked, true);
macro_rules! cursor_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
            source: u32,
            mask: u32,
            fr: u16,
            fg: u16,
            fb: u16,
            br: u16,
            bg: u16,
            bb: u16,
            x: u16,
            y: u16,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let (width, height, pixels) = {
                        let s = xcb_state().lock().unwrap();
                        if !drawing::valid_id(&s, id) {
                            return Err(request::error(14, 93, id));
                        }
                        let source = s
                            .pixmaps
                            .get(&source)
                            .ok_or(request::error(4, 93, source))?;
                        let mask = if mask == 0 {
                            None
                        } else {
                            Some(s.pixmaps.get(&mask).ok_or(request::error(4, 93, mask))?)
                        };
                        if source.depth != 1
                            || mask.is_some_and(|m| {
                                m.depth != 1 || m.width != source.width || m.height != source.height
                            })
                        {
                            return Err(request::error(8, 93, id));
                        }
                        if x >= source.width || y >= source.height {
                            return Err(request::error(2, 93, id));
                        }
                        let color = |r: u16, g: u16, b: u16| {
                            0xff000000
                                | ((r as u32 >> 8) << 16)
                                | ((g as u32 >> 8) << 8)
                                | (b as u32 >> 8)
                        };
                        let foreground = color(fr, fg, fb);
                        let background = color(br, bg, bb);
                        let mut pixels = Vec::new();
                        pixels
                            .try_reserve_exact(source.data.len())
                            .map_err(|_| request::error(11, 93, id))?;
                        for (i, p) in source.data.iter().enumerate() {
                            pixels.push(if mask.is_some_and(|m| m.data[i] & 1 == 0) {
                                0
                            } else if p & 1 != 0 {
                                foreground
                            } else {
                                background
                            });
                        }
                        (source.width, source.height, pixels)
                    };
                    let cursor = cursor(width, height, x, y, pixels)?;
                    state().lock().unwrap().cursors.insert(id, cursor);
                    Ok(None)
                }),
            }
        }
    };
}
cursor_export!(xcb_create_cursor, false);
cursor_export!(xcb_create_cursor_checked, true);
macro_rules! free_cursor_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    state()
                        .lock()
                        .unwrap()
                        .cursors
                        .remove(&id)
                        .ok_or(request::error(6, 95, id))?;
                    Ok(None)
                }),
            }
        }
    };
}
free_cursor_export!(xcb_free_cursor, false);
free_cursor_export!(xcb_free_cursor_checked, true);
