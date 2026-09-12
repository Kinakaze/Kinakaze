//! XCB atoms and properties share the Xlib store; only the wire width differs.
use super::*;
use request::{error, put16, put32, reply, submit};

fn intern(only: u8, len: u16, name: *const c_char) -> xcb_intern_atom_cookie_t {
    let sequence = submit(true, true, || {
        if name.is_null() && len != 0 {
            return Err(error(2, 16, 0));
        }
        let bytes = if len == 0 {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(name.cast(), len as usize) }
        };
        let atom = x11::property::intern_bytes(bytes, only != 0);
        let mut result = reply(0);
        put32(&mut result, 8, atom as u32);
        Ok(Some(result))
    });
    xcb_intern_atom_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_intern_atom")]
pub unsafe extern "sysv64" fn xcb_intern_atom(
    _c: *mut xcb_connection_t,
    only: u8,
    len: u16,
    name: *const c_char,
) -> xcb_intern_atom_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    intern(only, len, name)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_intern_atom_reply")]
pub unsafe extern "sysv64" fn xcb_intern_atom_reply(
    _c: *mut xcb_connection_t,
    cookie: xcb_intern_atom_cookie_t,
    e: *mut *mut xcb_generic_error_t,
) -> *mut xcb_intern_atom_reply_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { request::take(cookie.sequence, e).cast() }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_atom_name")]
pub unsafe extern "sysv64" fn xcb_get_atom_name(
    _c: *mut xcb_connection_t,
    atom: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    let sequence = submit(true, true, || {
        let name = x11::property::atom_name(atom as usize).ok_or(error(5, 17, atom))?;
        let mut result = reply(name.len());
        put16(&mut result, 8, name.len() as u16);
        result[32..32 + name.len()].copy_from_slice(&name);
        Ok(Some(result))
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_atom_name_reply")]
pub unsafe extern "sysv64" fn xcb_get_atom_name_reply(
    _c: *mut xcb_connection_t,
    cookie: xcb_void_cookie_t,
    e: *mut *mut xcb_generic_error_t,
) -> *mut c_void {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { request::take(cookie.sequence, e) }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_atom_name_name")]
pub unsafe extern "sysv64" fn xcb_get_atom_name_name(r: *const c_void) -> *mut c_char {
    if r.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { r.cast::<u8>().add(32).cast_mut().cast() }
    }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_atom_name_name_length")]
pub unsafe extern "sysv64" fn xcb_get_atom_name_name_length(r: *const c_void) -> c_int {
    if r.is_null() {
        0
    } else {
        unsafe { r.cast::<u8>().add(8).cast::<u16>().read_unaligned() as c_int }
    }
}

pub(super) fn native_window(window: u32) -> usize {
    if window == 1 {
        1
    } else {
        get_native_hwnd(window).map_or(0, |hwnd| hwnd as usize)
    }
}
fn change(
    checked: bool,
    mode: u8,
    window: u32,
    property: u32,
    kind: u32,
    format: u8,
    len: u32,
    data: *const c_void,
) -> xcb_void_cookie_t {
    let sequence = submit(checked, false, || {
        if !matches!(format, 8 | 16 | 32) || len > i32::MAX as u32 || (len != 0 && data.is_null()) {
            return Err(error(2, 18, property));
        }
        let wide: Vec<u64> = if format == 32 {
            (0..len as usize)
                .map(|i| unsafe { data.cast::<u32>().add(i).read_unaligned() as u64 })
                .collect()
        } else {
            Vec::new()
        };
        let input = if format == 32 {
            wide.as_ptr().cast()
        } else {
            data.cast()
        };
        x11::errors::capture(|| unsafe {
            x11::XChangeProperty(
                x11::connection::xcb_display(),
                native_window(window),
                property as usize,
                kind as usize,
                format as i32,
                mode as i32,
                input,
                len as i32,
            )
        })?;
        Ok(None)
    });
    xcb_void_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_change_property")]
pub unsafe extern "sysv64" fn xcb_change_property(
    _c: *mut xcb_connection_t,
    mode: u8,
    w: u32,
    p: u32,
    t: u32,
    f: u8,
    n: u32,
    d: *const c_void,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    change(false, mode, w, p, t, f, n, d)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_change_property_checked")]
pub unsafe extern "sysv64" fn xcb_change_property_checked(
    _c: *mut xcb_connection_t,
    mode: u8,
    w: u32,
    p: u32,
    t: u32,
    f: u8,
    n: u32,
    d: *const c_void,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    change(true, mode, w, p, t, f, n, d)
}

fn get(
    checked: bool,
    del: u8,
    w: u32,
    p: u32,
    t: u32,
    offset: u32,
    length: u32,
) -> xcb_get_property_cookie_t {
    let sequence = submit(checked, true, || {
        let (mut actual, mut format, mut count, mut after, mut data) =
            (0, 0, 0, 0, core::ptr::null_mut());
        x11::errors::capture(|| unsafe {
            x11::XGetWindowProperty(
                x11::connection::xcb_display(),
                native_window(w),
                p as usize,
                offset as i64,
                length as i64,
                del as i32,
                t as usize,
                &raw mut actual,
                &raw mut format,
                &raw mut count,
                &raw mut after,
                &raw mut data,
            )
        })?;
        let size = count as usize * (format as usize / 8);
        let mut result = reply(size);
        result[1] = format as u8;
        put32(&mut result, 8, actual as u32);
        put32(&mut result, 12, after as u32);
        put32(&mut result, 16, count as u32);
        if format == 32 {
            for i in 0..count as usize {
                put32(&mut result, 32 + 4 * i, unsafe {
                    data.cast::<u64>().add(i).read_unaligned() as u32
                });
            }
        } else if size != 0 {
            result[32..32 + size]
                .copy_from_slice(unsafe { core::slice::from_raw_parts(data, size) });
        }
        unsafe { kinakaze_alloc::guest::free(data) };
        Ok(Some(result))
    });
    xcb_get_property_cookie_t { sequence }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_property")]
pub unsafe extern "sysv64" fn xcb_get_property(
    _c: *mut xcb_connection_t,
    d: u8,
    w: u32,
    p: u32,
    t: u32,
    o: u32,
    l: u32,
) -> xcb_get_property_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    get(true, d, w, p, t, o, l)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_property_unchecked")]
pub unsafe extern "sysv64" fn xcb_get_property_unchecked(
    _c: *mut xcb_connection_t,
    d: u8,
    w: u32,
    p: u32,
    t: u32,
    o: u32,
    l: u32,
) -> xcb_get_property_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    get(false, d, w, p, t, o, l)
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_property_reply")]
pub unsafe extern "sysv64" fn xcb_get_property_reply(
    _c: *mut xcb_connection_t,
    cookie: xcb_get_property_cookie_t,
    e: *mut *mut xcb_generic_error_t,
) -> *mut xcb_get_property_reply_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { request::take(cookie.sequence, e).cast() }
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_delete_property")]
pub unsafe extern "sysv64" fn xcb_delete_property(
    _c: *mut xcb_connection_t,
    w: u32,
    p: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    request::command(false, || {
        unsafe {
            x11::XDeleteProperty(x11::connection::xcb_display(), native_window(w), p as usize)
        };
    })
}
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_delete_property_checked")]
pub unsafe extern "sysv64" fn xcb_delete_property_checked(
    _c: *mut xcb_connection_t,
    w: u32,
    p: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    request::command(true, || {
        unsafe {
            x11::XDeleteProperty(x11::connection::xcb_display(), native_window(w), p as usize)
        };
    })
}
