//! Core protocol queries. Replies capture request-time state and use wire widths.
use super::*;
use property::native_window;
use request::{error, put16, put32, reply, submit};

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_setup_roots_length")]
pub unsafe extern "sysv64" fn xcb_setup_roots_length(setup: *const xcb_setup_t) -> c_int {
    if setup.is_null() {
        0
    } else {
        unsafe { (*setup).roots_len as c_int }
    }
}

#[repr(C)]
pub struct xcb_format_iterator_t {
    pub data: *mut xcb_format_t,
    pub rem: c_int,
    pub index: c_int,
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_setup_pixmap_formats_iterator")]
pub unsafe extern "sysv64" fn xcb_setup_pixmap_formats_iterator(
    setup: *const xcb_setup_t,
) -> xcb_format_iterator_t {
    if setup.is_null() {
        return xcb_format_iterator_t {
            data: core::ptr::null_mut(),
            rem: 0,
            index: 0,
        };
    }
    xcb_format_iterator_t {
        data: unsafe { xcb_setup_pixmap_formats(setup) },
        rem: unsafe { (*setup).pixmap_formats_len as c_int },
        index: (size_of::<xcb_setup_t>() + unsafe { (*setup).vendor_len as usize }.div_ceil(4) * 4)
            as c_int,
    }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_format_next")]
pub unsafe extern "sysv64" fn xcb_format_next(i: *mut xcb_format_iterator_t) {
    if !i.is_null() && unsafe { (*i).rem } > 0 {
        unsafe {
            (*i).data = (*i).data.add(1);
            (*i).rem -= 1;
            (*i).index += size_of::<xcb_format_t>() as c_int;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_popcount")]
pub extern "sysv64" fn xcb_popcount(value: u32) -> c_int {
    value.count_ones() as c_int
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_no_operation")]
pub unsafe extern "sysv64" fn xcb_no_operation(_c: *mut xcb_connection_t) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    request::command(false, || {})
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_no_operation_checked")]
pub unsafe extern "sysv64" fn xcb_no_operation_checked(
    _c: *mut xcb_connection_t,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    request::command(true, || {})
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_poll_for_queued_event")]
pub unsafe extern "sysv64" fn xcb_poll_for_queued_event(
    c: *mut xcb_connection_t,
) -> *mut xcb_generic_event_t {
    let _request_display = x11::connection::scope_xcb(c.cast());
    unsafe { xcb_poll_for_event(c) }
}

fn wire_window(native: usize) -> u32 {
    if native <= 1 {
        native as u32
    } else {
        xcb_state()
            .lock()
            .unwrap()
            .hwnd_to_wid
            .get(&native)
            .copied()
            .unwrap_or(native as u32)
    }
}
macro_rules! reply_function {
    ($name:ident) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            k: xcb_void_cookie_t,
            e: *mut *mut xcb_generic_error_t,
        ) -> *mut c_void {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            unsafe { request::take(k.sequence, e) }
        }
    };
}
reply_function!(xcb_get_geometry_reply);
reply_function!(xcb_translate_coordinates_reply);
reply_function!(xcb_get_window_attributes_reply);
reply_function!(xcb_query_tree_reply);
reply_function!(xcb_get_keyboard_mapping_reply);
reply_function!(xcb_get_modifier_mapping_reply);
reply_function!(xcb_get_input_focus_reply);

fn geometry(checked: bool, drawable: u32) -> xcb_void_cookie_t {
    let sequence = submit(checked, true, || {
        let pixmap = xcb_state()
            .lock()
            .unwrap()
            .pixmaps
            .get(&drawable)
            .map(|p| (p.width, p.height, p.depth));
        let (mut root, mut x, mut y, mut width, mut height, mut border, mut depth) =
            (1, 0, 0, 0, 0, 0, 0);
        if let Some((w, h, d)) = pixmap {
            width = w as u32;
            height = h as u32;
            depth = d as u32;
        } else {
            let native = native_window(drawable);
            if native == 0 {
                return Err(error(9, 14, drawable));
            }
            let ok = x11::errors::capture(|| unsafe {
                x11::XGetGeometry(
                    x11::connection::xcb_display(),
                    native,
                    &raw mut root,
                    &raw mut x,
                    &raw mut y,
                    &raw mut width,
                    &raw mut height,
                    &raw mut border,
                    &raw mut depth,
                )
            })?;
            if ok == 0 {
                return Err(error(9, 14, drawable));
            }
        }
        let mut r = reply(0);
        r[1] = depth as u8;
        put32(&mut r, 8, root as u32);
        for (offset, value) in [
            (12, x as u16),
            (14, y as u16),
            (16, width as u16),
            (18, height as u16),
            (20, border as u16),
        ] {
            put16(&mut r, offset, value)
        }
        Ok(Some(r))
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_geometry")]
pub unsafe extern "sysv64" fn xcb_get_geometry(
    _c: *mut xcb_connection_t,
    d: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    geometry(true, d)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_geometry_unchecked")]
pub unsafe extern "sysv64" fn xcb_get_geometry_unchecked(
    _c: *mut xcb_connection_t,
    d: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    geometry(false, d)
}

fn translate(checked: bool, src: u32, dst: u32, x: i16, y: i16) -> xcb_void_cookie_t {
    let sequence = submit(checked, true, || {
        let (mut dx, mut dy, mut child) = (0, 0, 0);
        let same = x11::errors::capture(|| unsafe {
            x11::XTranslateCoordinates(
                x11::connection::xcb_display(),
                native_window(src),
                native_window(dst),
                x as i32,
                y as i32,
                &raw mut dx,
                &raw mut dy,
                &raw mut child,
            )
        })?;
        let mut r = reply(0);
        r[1] = u8::from(same != 0);
        put32(&mut r, 8, wire_window(child));
        put16(&mut r, 12, dx as u16);
        put16(&mut r, 14, dy as u16);
        Ok(Some(r))
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_translate_coordinates")]
pub unsafe extern "sysv64" fn xcb_translate_coordinates(
    _c: *mut xcb_connection_t,
    s: u32,
    d: u32,
    x: i16,
    y: i16,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    translate(true, s, d, x, y)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_translate_coordinates_unchecked")]
pub unsafe extern "sysv64" fn xcb_translate_coordinates_unchecked(
    _c: *mut xcb_connection_t,
    s: u32,
    d: u32,
    x: i16,
    y: i16,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    translate(false, s, d, x, y)
}

fn attributes(checked: bool, window: u32) -> xcb_void_cookie_t {
    let sequence = submit(checked, true, || {
        let mut a = unsafe { core::mem::zeroed::<x11::XWindowAttributes>() };
        x11::errors::capture(|| unsafe {
            x11::XGetWindowAttributes(
                x11::connection::xcb_display(),
                native_window(window),
                &raw mut a,
            )
        })?;
        let mut r = reply(12);
        r[1] = a.backing_store as u8;
        put32(
            &mut r,
            8,
            if a.visual.is_null() {
                0
            } else {
                unsafe { (*a.visual).visualid as u32 }
            },
        );
        put16(&mut r, 12, a.class as u16);
        r[14] = a.bit_gravity as u8;
        r[15] = a.win_gravity as u8;
        put32(&mut r, 16, a.backing_planes as u32);
        put32(&mut r, 20, a.backing_pixel as u32);
        r[24] = a.save_under as u8;
        r[25] = a.map_installed as u8;
        r[26] = a.map_state as u8;
        r[27] = a.override_redirect as u8;
        put32(&mut r, 28, resources::window_map(window));
        put32(&mut r, 32, a.all_event_masks as u32);
        put32(&mut r, 36, a.your_event_mask as u32);
        put16(&mut r, 40, a.do_not_propagate_mask as u16);
        Ok(Some(r))
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_window_attributes")]
pub unsafe extern "sysv64" fn xcb_get_window_attributes(
    _c: *mut xcb_connection_t,
    w: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    attributes(true, w)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_window_attributes_unchecked")]
pub unsafe extern "sysv64" fn xcb_get_window_attributes_unchecked(
    _c: *mut xcb_connection_t,
    w: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    attributes(false, w)
}

fn tree(checked: bool, window: u32) -> xcb_void_cookie_t {
    let sequence = submit(checked, true, || {
        let (mut root, mut parent, mut children, mut count) = (0, 0, core::ptr::null_mut(), 0);
        x11::errors::capture(|| unsafe {
            x11::XQueryTree(
                x11::connection::xcb_display(),
                native_window(window),
                &raw mut root,
                &raw mut parent,
                &raw mut children,
                &raw mut count,
            )
        })?;
        let mut values = Vec::new();
        for i in 0..count as usize {
            let id = wire_window(unsafe { children.add(i).read() });
            if id != 0 {
                values.push(id)
            }
        }
        unsafe { kinakaze_alloc::guest::free(children.cast()) };
        let mut r = reply(values.len() * 4);
        put32(&mut r, 8, root as u32);
        put32(&mut r, 12, wire_window(parent));
        put16(&mut r, 16, values.len() as u16);
        for (i, value) in values.into_iter().enumerate() {
            put32(&mut r, 32 + i * 4, value)
        }
        Ok(Some(r))
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_query_tree")]
pub unsafe extern "sysv64" fn xcb_query_tree(
    _c: *mut xcb_connection_t,
    w: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    tree(true, w)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_query_tree_unchecked")]
pub unsafe extern "sysv64" fn xcb_query_tree_unchecked(
    _c: *mut xcb_connection_t,
    w: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    tree(false, w)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_query_tree_children")]
pub unsafe extern "sysv64" fn xcb_query_tree_children(r: *const c_void) -> *mut u32 {
    unsafe { r.cast::<u8>().add(32).cast_mut().cast() }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_query_tree_children_length")]
pub unsafe extern "sysv64" fn xcb_query_tree_children_length(r: *const c_void) -> c_int {
    unsafe { r.cast::<u8>().add(16).cast::<u16>().read_unaligned() as c_int }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_keyboard_mapping")]
pub unsafe extern "sysv64" fn xcb_get_keyboard_mapping(
    _c: *mut xcb_connection_t,
    first: u8,
    count: u8,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    let sequence = submit(true, true, || {
        let mut per = 0;
        let keys = x11::errors::capture(|| unsafe {
            x11::XGetKeyboardMapping(
                x11::connection::xcb_display(),
                first,
                count as i32,
                &raw mut per,
            )
        })?;
        if keys.is_null() {
            return Err(error(11, 101, first as u32));
        }
        let total = count as usize * per as usize;
        let mut r = reply(total * 4);
        r[1] = per as u8;
        for i in 0..total {
            put32(&mut r, 32 + 4 * i, unsafe { keys.add(i).read() as u32 });
        }
        unsafe { kinakaze_alloc::guest::free(keys.cast()) };
        Ok(Some(r))
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_keyboard_mapping_keysyms")]
pub unsafe extern "sysv64" fn xcb_get_keyboard_mapping_keysyms(r: *const c_void) -> *mut u32 {
    unsafe { r.cast::<u8>().add(32).cast_mut().cast() }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_modifier_mapping")]
pub unsafe extern "sysv64" fn xcb_get_modifier_mapping(
    _c: *mut xcb_connection_t,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    let sequence = submit(true, true, || {
        let map = unsafe { x11::XGetModifierMapping(x11::connection::xcb_display()) };
        if map.is_null() {
            return Err(error(11, 119, 0));
        }
        let per = unsafe { (*map).max_keypermod as usize };
        let mut r = reply(per * 8);
        r[1] = per as u8;
        r[32..32 + per * 8]
            .copy_from_slice(unsafe { core::slice::from_raw_parts((*map).modifiermap, per * 8) });
        unsafe { kinakaze_alloc::guest::free(map.cast()) };
        Ok(Some(r))
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_modifier_mapping_keycodes")]
pub unsafe extern "sysv64" fn xcb_get_modifier_mapping_keycodes(r: *const c_void) -> *mut u8 {
    unsafe { r.cast::<u8>().add(32).cast_mut() }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_modifier_mapping_keycodes_length")]
pub unsafe extern "sysv64" fn xcb_get_modifier_mapping_keycodes_length(r: *const c_void) -> c_int {
    unsafe { r.cast::<u8>().add(1).read() as c_int * 8 }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_input_focus")]
pub unsafe extern "sysv64" fn xcb_get_input_focus(_c: *mut xcb_connection_t) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    let sequence = submit(true, true, || {
        let (mut focus, mut revert) = (0, 0);
        unsafe {
            x11::XGetInputFocus(
                x11::connection::xcb_display(),
                &raw mut focus,
                &raw mut revert,
            )
        };
        let mut r = reply(0);
        r[1] = revert as u8;
        put32(&mut r, 8, wire_window(focus));
        Ok(Some(r))
    });
    xcb_void_cookie_t { sequence }
}
