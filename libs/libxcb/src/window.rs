//! Window creation uses the shared native display and one XCB resource mapping.
use super::*;
use property::native_window;

macro_rules! visibility_export {
    ($name:ident, $checked:expr, $operation:ident, $opcode:expr, $map:expr) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libxcb_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let window = native(id, $opcode)?;
                    // Restoring an iconified window reports its MapNotify
                    // through the native size change instead.
                    let changes = mapped(window) != $map && !($map && iconic(window));
                    x11::errors::capture(|| unsafe {
                        x11::$operation(x11::connection::xcb_display(), window)
                    })?;
                    // Xlib queues its own MapNotify/UnmapNotify; XCB clients
                    // read this queue instead.
                    if changes {
                        let flag =
                            u8::from($map && kinakaze_libdisplay::ui::is_override_redirect(window));
                        crate::queue_map_notify(id, $map, flag);
                    }
                    Ok(None)
                }),
            }
        }
    };
}
visibility_export!(xcb_map_window, false, XMapWindow, 8, true);
visibility_export!(xcb_map_window_checked, true, XMapWindow, 8, true);
visibility_export!(xcb_unmap_window, false, XUnmapWindow, 10, false);
visibility_export!(xcb_unmap_window_checked, true, XUnmapWindow, 10, false);

/// Whether the native window is currently mapped, read before a map or unmap
/// so MapNotify/UnmapNotify are reported only for an actual change.
fn mapped(window: usize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_STYLE, GetWindowLongPtrW, WS_VISIBLE};
    let style = unsafe { GetWindowLongPtrW(window as _, GWL_STYLE) };
    style & WS_VISIBLE as isize != 0
}
fn iconic(window: usize) -> bool {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsIconic(window as _) != 0 }
}

pub(super) fn native(id: u32, opcode: u8) -> Result<usize, x11::errors::ProtocolError> {
    let w = native_window(id);
    if w == 0 {
        Err(request::error(3, opcode, id))
    } else {
        Ok(w)
    }
}

macro_rules! configure_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
            mask: u16,
            p: *const c_void,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let target = native(id, 12)?;
                    if mask & !127 != 0 || (mask != 0 && p.is_null()) {
                        return Err(request::error(2, 12, mask as u32));
                    }
                    let mut a = unsafe { core::mem::zeroed::<x11::wm::XWindowChanges>() };
                    let mut index = 0;
                    for bit in 0..7 {
                        if mask & (1 << bit) == 0 {
                            continue;
                        }
                        let v = unsafe { p.cast::<u32>().add(index).read_unaligned() };
                        index += 1;
                        match bit {
                            0 => a.x = v as i16 as i32,
                            1 => a.y = v as i16 as i32,
                            2 => a.width = v as u16 as i32,
                            3 => a.height = v as u16 as i32,
                            4 => a.border_width = v as u16 as i32,
                            5 => a.sibling = native(v, 12)?,
                            _ => a.stack_mode = v as u8 as i32,
                        }
                    }
                    x11::errors::capture(|| unsafe {
                        x11::wm::XConfigureWindow(
                            x11::connection::xcb_display(),
                            target,
                            mask as u32,
                            &mut a,
                        )
                    })?;
                    Ok(None)
                }),
            }
        }
    };
}
configure_export!(xcb_configure_window, false);
configure_export!(xcb_configure_window_checked, true);
macro_rules! change_attributes_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
            mask: u32,
            p: *const c_void,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let target = native(id, 2)?;
                    let mut a = unsafe { attributes(mask, p) }.map_err(|mut e| {
                        e.request = 2;
                        e
                    })?;
                    let cursor = a.cursor as u32;
                    let map = a.colormap as u32;
                    if mask & (1 << 14) != 0 {
                        resources::validate_cursor(cursor, 2)?;
                    }
                    if mask & (1 << 13) != 0 {
                        a.colormap = resources::native_map(map, 2)?;
                    }
                    x11::errors::capture(|| unsafe {
                        x11::wm::XChangeWindowAttributes(
                            x11::connection::xcb_display(),
                            target,
                            mask as u64,
                            &mut a,
                        )
                    })?;
                    if mask & (1 << 14) != 0 {
                        resources::apply_cursor(id, cursor, 2)?;
                    }
                    if mask & (1 << 13) != 0 {
                        resources::set_map(id, if map == 0 { 1 } else { map });
                    }
                    Ok(None)
                }),
            }
        }
    };
}
change_attributes_export!(xcb_change_window_attributes, false);
change_attributes_export!(xcb_change_window_attributes_checked, true);
macro_rules! reparent_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            id: u32,
            parent: u32,
            x: i16,
            y: i16,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let target = native(id, 7)?;
                    let parent = native(parent, 7)?;
                    x11::errors::capture(|| unsafe {
                        x11::XReparentWindow(
                            x11::connection::xcb_display(),
                            target,
                            parent,
                            x as i32,
                            y as i32,
                        )
                    })?;
                    Ok(None)
                }),
            }
        }
    };
}
reparent_export!(xcb_reparent_window, false);
reparent_export!(xcb_reparent_window_checked, true);
macro_rules! clear_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            exposures: u8,
            id: u32,
            x: i16,
            y: i16,
            w: u16,
            h: u16,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            let mut event = None;
            let sequence = request::submit($checked, false, || {
                if exposures > 1 {
                    return Err(request::error(2, 61, exposures as u32));
                }
                let target = native(id, 61)?;
                let success = x11::errors::capture(|| unsafe {
                    x11::graphics::XClearArea(
                        x11::connection::xcb_display(),
                        target,
                        x as i32,
                        y as i32,
                        w as u32,
                        h as u32,
                        0,
                    )
                })?;
                if success == 0 {
                    return Err(request::error(11, 61, id));
                }
                if exposures != 0 {
                    let (dw, dh, _) = x11::graphics::drawable_dimensions(target)
                        .ok_or(request::error(11, 61, id))?;
                    let left = (x as i32).max(0);
                    let top = (y as i32).max(0);
                    let right = if w == 0 {
                        dw
                    } else {
                        (x as i32 + w as i32).min(dw)
                    };
                    let bottom = if h == 0 {
                        dh
                    } else {
                        (y as i32 + h as i32).min(dh)
                    };
                    if right > left && bottom > top {
                        let mut b = [0u8; 36];
                        b[0] = 12;
                        request::put32(&mut b, 4, id);
                        for (offset, v) in
                            [(8, left), (10, top), (12, right - left), (14, bottom - top)]
                        {
                            request::put16(&mut b, offset, v as u16);
                        }
                        event = Some(b);
                    }
                }
                Ok(None)
            });
            if let Some(mut b) = event {
                request::put16(&mut b, 2, sequence as u16);
                request::put32(&mut b, 32, sequence);
                xcb_state()
                    .lock()
                    .unwrap()
                    .pending_events
                    .push_back(unsafe {
                        b.as_ptr().cast::<xcb_generic_event_t>().read_unaligned()
                    });
                x11::notify_event();
            }
            xcb_void_cookie_t { sequence }
        }
    };
}
clear_export!(xcb_clear_area, false);
clear_export!(xcb_clear_area_checked, true);

unsafe fn attributes(
    mask: u32,
    values: *const c_void,
) -> Result<x11::XSetWindowAttributes, x11::errors::ProtocolError> {
    if mask & !0x7fff != 0 || (mask != 0 && values.is_null()) {
        return Err(request::error(2, 1, mask));
    }
    let mut a = unsafe { core::mem::zeroed::<x11::XSetWindowAttributes>() };
    let mut index = 0;
    for bit in 0..15 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let value = unsafe { values.cast::<u32>().add(index).read_unaligned() };
        index += 1;
        match bit {
            0 => a.background_pixmap = value as usize,
            1 => a.background_pixel = value as u64,
            2 => a.border_pixmap = value as usize,
            3 => a.border_pixel = value as u64,
            4 => a.bit_gravity = value as i32,
            5 => a.win_gravity = value as i32,
            6 => a.backing_store = value as i32,
            7 => a.backing_planes = value as u64,
            8 => a.backing_pixel = value as u64,
            9 => a.override_redirect = value as i32,
            10 => a.save_under = value as i32,
            11 => a.event_mask = value as i64,
            12 => a.do_not_propagate_mask = value as i64,
            13 => a.colormap = value as usize,
            14 => a.cursor = value as usize,
            _ => unreachable!(),
        }
    }
    Ok(a)
}
unsafe fn create(
    checked: bool,
    depth: u8,
    wid: u32,
    parent: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    border: u16,
    class: u16,
    visual: u32,
    mask: u32,
    values: *const c_void,
) -> xcb_void_cookie_t {
    let sequence = request::submit(checked, false, || {
        if !drawing::valid_id(&xcb_state().lock().unwrap(), wid) {
            return Err(request::error(14, 1, wid));
        }
        if width == 0 || height == 0 || class > 2 {
            return Err(request::error(2, 1, wid));
        }
        if !matches!(depth, 0 | 24) || visual > 1 {
            return Err(request::error(8, 1, wid));
        }
        let parent_native = native_window(parent);
        if parent_native == 0 {
            return Err(request::error(3, 1, parent));
        }
        let mut a = unsafe { attributes(mask, values) }?;
        let cursor = a.cursor as u32;
        let map = if mask & (1 << 13) == 0 || a.colormap == 0 {
            resources::window_map(parent)
        } else {
            a.colormap as u32
        };
        if mask & (1 << 14) != 0 {
            resources::validate_cursor(cursor, 1)?;
        }
        if mask & (1 << 13) != 0 {
            a.colormap = resources::native_map(map, 1)?;
        }
        let native = x11::errors::capture(|| unsafe {
            x11::XCreateWindow(
                x11::connection::xcb_display(),
                parent_native,
                x as i32,
                y as i32,
                width as u32,
                height as u32,
                border as u32,
                depth as i32,
                class as u32,
                core::ptr::null_mut(),
                mask as u64,
                &raw mut a,
            )
        })?;
        if native == 0 {
            return Err(request::error(11, 1, wid));
        }
        let logical = kinakaze_libdisplay::window::logical_for_native(native)
            .ok_or(request::error(11, 1, wid))?;
        if !kinakaze_libdisplay::window::x11::bind(wid, logical) {
            unsafe { x11::XDestroyWindow(x11::connection::xcb_display(), native) };
            return Err(request::error(11, 1, wid));
        }
        let mut state = xcb_state().lock().unwrap();
        state.wid_to_hwnd.insert(wid, native);
        state.hwnd_to_wid.insert(native, wid);
        state.wid_to_guest.insert(wid, logical);
        state.guest_to_wid.insert(logical, wid);
        drop(state);
        resources::set_map(wid, map);
        if mask & (1 << 14) != 0 {
            resources::apply_cursor(wid, cursor, 1)?;
        }
        Ok(None)
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_create_window")]
pub unsafe extern "sysv64" fn xcb_create_window(
    _c: *mut xcb_connection_t,
    d: u8,
    w: u32,
    p: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    b: u16,
    class: u16,
    v: u32,
    m: u32,
    a: *const c_void,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { create(false, d, w, p, x, y, width, height, b, class, v, m, a) }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_create_window_checked")]
pub unsafe extern "sysv64" fn xcb_create_window_checked(
    _c: *mut xcb_connection_t,
    d: u8,
    w: u32,
    p: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    b: u16,
    class: u16,
    v: u32,
    m: u32,
    a: *const c_void,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { create(true, d, w, p, x, y, width, height, b, class, v, m, a) }
}
