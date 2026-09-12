//! XCB display-name parsing and caller-owned hostname storage.
//! Contract: https://xcb.freedesktop.org/PublicApi/
use core::ffi::{CStr, c_char, c_int};

fn parse(name: &[u8]) -> Option<(&[u8], i32, i32)> {
    let colon = name.iter().rposition(|&byte| byte == b':')?;
    let host = &name[..colon];
    let tail = &name[colon + 1..];
    let number = |part: &[u8]| {
        if part.is_empty() {
            return None;
        }
        part.iter().try_fold(0i32, |value, &byte| {
            if !byte.is_ascii_digit() {
                return None;
            }
            value.checked_mul(10)?.checked_add(i32::from(byte - b'0'))
        })
    };
    let (display, screen) = match tail.iter().position(|&byte| byte == b'.') {
        Some(dot) => (number(&tail[..dot])?, number(&tail[dot + 1..])?),
        None => (number(tail)?, 0),
    };
    Some((host, display, screen))
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_parse_display")]
pub unsafe extern "sysv64" fn xcb_parse_display(
    name: *const c_char,
    host: *mut *mut c_char,
    display: *mut c_int,
    screen: *mut c_int,
) -> c_int {
    if host.is_null() || display.is_null() {
        return 0;
    }
    let name = if name.is_null() || unsafe { *name } == 0 {
        unsafe { libc::process::kinakaze_abi_getenv(c"DISPLAY".as_ptr()) }
    } else {
        name
    };
    if name.is_null() {
        return 0;
    }
    let Some((hostname, number, preferred)) = parse(unsafe { CStr::from_ptr(name) }.to_bytes())
    else {
        return 0;
    };
    let copy = unsafe { kinakaze_alloc::c::malloc(hostname.len() + 1) };
    if copy.is_null() {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(hostname.as_ptr(), copy, hostname.len());
        copy.add(hostname.len()).write(0);
        *host = copy.cast();
        *display = number;
        if !screen.is_null() {
            *screen = preferred;
        }
    }
    1
}
