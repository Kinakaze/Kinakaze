//! Owned ICCCM query results, decoded from the ordinary X property store.
use crate::{Atom, Display, Window, XSizeHints};
use core::ffi::{c_char, c_int};
use core::ptr;

struct Value {
    data: *mut u8,
    count: usize,
}
impl Drop for Value {
    fn drop(&mut self) {
        unsafe {
            crate::XFree(self.data.cast());
        }
    }
}
unsafe fn read(
    d: *mut Display,
    w: Window,
    property: Atom,
    kind: Atom,
    format: c_int,
    limit: i64,
) -> Option<Value> {
    let (mut actual, mut actual_format, mut count, mut after, mut data) =
        (0, 0, 0, 0, ptr::null_mut());
    let status = unsafe {
        crate::XGetWindowProperty(
            d,
            w,
            property,
            0,
            limit,
            0,
            kind,
            &mut actual,
            &mut actual_format,
            &mut count,
            &mut after,
            &mut data,
        )
    };
    let value = Value {
        data,
        count: usize::try_from(count).ok()?,
    };
    if status != 0 || actual != kind || actual_format != format || data.is_null() {
        return None;
    }
    Some(value)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetWMProtocols")]
pub unsafe extern "sysv64" fn XGetWMProtocols(
    d: *mut Display,
    w: Window,
    protocols: *mut *mut Atom,
    count: *mut c_int,
) -> c_int {
    if protocols.is_null() || count.is_null() {
        return 0;
    }
    let property = unsafe { crate::XInternAtom(d, c"WM_PROTOCOLS".as_ptr(), 0) };
    let Some(value) = (unsafe { read(d, w, property, 4, 32, c_int::MAX as i64) }) else {
        return 0;
    };
    let Ok(length) = c_int::try_from(value.count) else {
        return 0;
    };
    unsafe {
        *protocols = value.data.cast();
        *count = length;
    }
    core::mem::forget(value);
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetWMColormapWindows")]
pub unsafe extern "sysv64" fn XGetWMColormapWindows(
    d: *mut Display,
    w: Window,
    windows: *mut *mut Window,
    count: *mut c_int,
) -> c_int {
    if windows.is_null() || count.is_null() {
        return 0;
    }
    unsafe {
        *windows = ptr::null_mut();
        *count = 0;
    }
    let atom = unsafe { crate::XInternAtom(d, c"WM_COLORMAP_WINDOWS".as_ptr(), 0) };
    let Some(value) = (unsafe { read(d, w, atom, 33, 32, c_int::MAX as i64) }) else {
        return 0;
    };
    let Ok(length) = c_int::try_from(value.count) else {
        return 0;
    };
    unsafe {
        *windows = value.data.cast();
        *count = length;
    }
    core::mem::forget(value);
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWMColormapWindows")]
pub unsafe extern "sysv64" fn XSetWMColormapWindows(
    d: *mut Display,
    w: Window,
    windows: *mut Window,
    count: c_int,
) -> c_int {
    if count < 0 || count > 0 && windows.is_null() {
        return 0;
    }
    let atom = unsafe { crate::XInternAtom(d, c"WM_COLORMAP_WINDOWS".as_ptr(), 0) };
    unsafe {
        crate::XChangeProperty(d, w, atom, 33, 32, 0, windows.cast(), count);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetClassHint")]
pub unsafe extern "sysv64" fn XGetClassHint(
    d: *mut Display,
    w: Window,
    output: *mut crate::wm::XClassHint,
) -> c_int {
    if output.is_null() {
        return 0;
    }
    let Some(value) = (unsafe { read(d, w, 67, 31, 8, c_int::MAX as i64) }) else {
        return 0;
    };
    let bytes = unsafe { core::slice::from_raw_parts(value.data, value.count) };
    let Some(first) = bytes.iter().position(|b| *b == 0) else {
        return 0;
    };
    let second = &bytes[first + 1..];
    let second_len = second.iter().position(|b| *b == 0).unwrap_or(second.len());
    unsafe fn copy(bytes: &[u8]) -> *mut c_char {
        let value = unsafe { kinakaze_alloc::guest::malloc(bytes.len() + 1) };
        if !value.is_null() {
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), value, bytes.len());
                value.add(bytes.len()).write(0);
            }
        }
        value.cast()
    }
    let name = unsafe { copy(&bytes[..first]) };
    if name.is_null() {
        return 0;
    }
    let class = unsafe { copy(&second[..second_len]) };
    if class.is_null() {
        unsafe {
            crate::XFree(name.cast());
        }
        return 0;
    }
    unsafe {
        *output = crate::wm::XClassHint {
            res_name: name,
            res_class: class,
        };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetWMSizeHints")]
pub unsafe extern "sysv64" fn XGetWMSizeHints(
    d: *mut Display,
    w: Window,
    hints: *mut XSizeHints,
    supplied: *mut i64,
    property: Atom,
) -> c_int {
    if hints.is_null() || supplied.is_null() {
        return 0;
    }
    let Some(value) = (unsafe { read(d, w, property, 41, 32, 18) }) else {
        return 0;
    };
    if value.count < 15 {
        return 0;
    }
    let values = unsafe { core::slice::from_raw_parts(value.data.cast::<i64>(), value.count) };
    let full = values.len() >= 18;
    let flags = if full { 0x3ff } else { 0xff };
    let output = XSizeHints {
        flags: values[0] & flags,
        x: values[1] as i32,
        y: values[2] as i32,
        width: values[3] as i32,
        height: values[4] as i32,
        min_width: values[5] as i32,
        min_height: values[6] as i32,
        max_width: values[7] as i32,
        max_height: values[8] as i32,
        width_inc: values[9] as i32,
        height_inc: values[10] as i32,
        min_aspect: (values[11] as i32, values[12] as i32),
        max_aspect: (values[13] as i32, values[14] as i32),
        base_width: if full { values[15] as i32 } else { 0 },
        base_height: if full { values[16] as i32 } else { 0 },
        win_gravity: if full { values[17] as i32 } else { 0 },
    };
    unsafe {
        *hints = output;
        *supplied = flags;
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetNormalHints")]
pub unsafe extern "sysv64" fn XGetNormalHints(
    d: *mut Display,
    w: Window,
    hints: *mut XSizeHints,
) -> c_int {
    let mut supplied = 0;
    unsafe { XGetWMSizeHints(d, w, hints, &mut supplied, 40) }
}
