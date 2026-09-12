//! ICCCM helpers use the same property store and guest allocation contract.
use super::*;
use crate::wm::{XTextProperty, XWMHints};

#[unsafe(export_name = "kinakaze_engine_libX11_XFetchName")]
pub unsafe extern "sysv64" fn XFetchName(
    display: *mut Display,
    window: Window,
    name: *mut *mut core::ffi::c_char,
) -> c_int {
    if name.is_null() {
        return 0;
    }
    unsafe {
        *name = core::ptr::null_mut();
    }
    let mut text: XTextProperty = unsafe { core::mem::zeroed() };
    if unsafe { XGetWMName(display, window, &raw mut text) } == 0 {
        return 0;
    }
    if text.encoding != 31 || text.format != 8 {
        unsafe {
            crate::XFree(text.value.cast());
        }
        return 0;
    }
    unsafe {
        *name = text.value.cast();
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetWMName")]
pub unsafe extern "sysv64" fn XGetWMName(
    display: *mut Display,
    window: Window,
    text: *mut XTextProperty,
) -> c_int {
    unsafe { XGetTextProperty(display, window, text, 39) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetWMIconName")]
pub unsafe extern "sysv64" fn XGetWMIconName(
    display: *mut Display,
    window: Window,
    text: *mut XTextProperty,
) -> c_int {
    unsafe { XGetTextProperty(display, window, text, 37) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetTextProperty")]
pub unsafe extern "sysv64" fn XSetTextProperty(
    display: *mut Display,
    window: Window,
    text: *const XTextProperty,
    property: Atom,
) {
    if text.is_null() {
        return;
    }
    let text = unsafe { &*text };
    let Ok(count) = c_int::try_from(text.nitems) else {
        unsafe { crate::errors::report(display, 2, 18, 0, property) };
        return;
    };
    unsafe {
        XChangeProperty(
            display,
            window,
            property,
            text.encoding,
            text.format,
            0,
            text.value,
            count,
        );
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetTextProperty")]
pub unsafe extern "sysv64" fn XGetTextProperty(
    display: *mut Display,
    window: Window,
    text: *mut XTextProperty,
    property: Atom,
) -> c_int {
    if text.is_null() {
        return 0;
    }
    let mut after = 0;
    let text = unsafe { &mut *text };
    let status = unsafe {
        XGetWindowProperty(
            display,
            window,
            property,
            0,
            i32::MAX as i64,
            0,
            0,
            &mut text.encoding,
            &mut text.format,
            &mut text.nitems,
            &mut after,
            &mut text.value,
        )
    };
    c_int::from(status == 0 && text.encoding != 0)
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetWMHints")]
pub unsafe extern "sysv64" fn XSetWMHints(
    display: *mut Display,
    window: Window,
    hints: *const XWMHints,
) -> c_int {
    if hints.is_null() {
        return 0;
    }
    let h = unsafe { &*hints };
    let values = [
        h.flags as u64,
        h.input as u64,
        h.initial_state as u64,
        h.icon_pixmap as u64,
        h.icon_window as u64,
        h.icon_x as u64,
        h.icon_y as u64,
        h.icon_mask as u64,
        h.window_group as u64,
    ];
    unsafe { XChangeProperty(display, window, 35, 35, 32, 0, values.as_ptr().cast(), 9) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetWMHints")]
pub unsafe extern "sysv64" fn XGetWMHints(display: *mut Display, window: Window) -> *mut XWMHints {
    let (mut kind, mut format, mut count, mut after, mut bytes) = (0, 0, 0, 0, ptr::null_mut());
    if unsafe {
        XGetWindowProperty(
            display,
            window,
            35,
            0,
            9,
            0,
            35,
            &mut kind,
            &mut format,
            &mut count,
            &mut after,
            &mut bytes,
        )
    } != 0
    {
        return ptr::null_mut();
    }
    let result = if kind == 35 && format == 32 && count >= 8 {
        let values = unsafe { core::slice::from_raw_parts(bytes.cast::<u64>(), count as usize) };
        let result = unsafe { kinakaze_alloc::guest::malloc(core::mem::size_of::<XWMHints>()) }
            .cast::<XWMHints>();
        if !result.is_null() {
            unsafe {
                result.write(XWMHints {
                    flags: values[0] as i64,
                    input: c_int::from(values[1] != 0),
                    initial_state: values[2] as i32,
                    icon_pixmap: values[3] as u32 as usize,
                    icon_window: values[4] as u32 as usize,
                    icon_x: values[5] as i32,
                    icon_y: values[6] as i32,
                    icon_mask: values[7] as u32 as usize,
                    window_group: if count > 8 {
                        values[8] as u32 as usize
                    } else {
                        0
                    },
                });
            }
        }
        result
    } else {
        ptr::null_mut()
    };
    unsafe {
        kinakaze_alloc::guest::free(bytes);
    }
    result
}
