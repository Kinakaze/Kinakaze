//! Core input requests use the same display controls and event descriptor as Xlib.
use super::*;
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_file_descriptor")]
pub unsafe extern "sysv64" fn xcb_get_file_descriptor(c: *mut xcb_connection_t) -> i32 {
    let _request_display = x11::connection::scope_xcb(c.cast());
    if c.is_null() {
        -1
    } else {
        x11::connection::event_fd()
    }
}
macro_rules! keyboard_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            mask: u32,
            values: *const c_void,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    if mask & !255 != 0 || (mask != 0 && values.is_null()) {
                        return Err(request::error(2, 102, mask));
                    }
                    let mut control = x11::keyboard::XKeyboardControl::default();
                    let mut index = 0;
                    for bit in 0..8 {
                        if mask & (1 << bit) == 0 {
                            continue;
                        }
                        let v = unsafe { values.cast::<u32>().add(index).read_unaligned() };
                        index += 1;
                        match bit {
                            0 => control.key_click_percent = v as i8 as i32,
                            1 => control.bell_percent = v as i8 as i32,
                            2 => control.bell_pitch = v as i16 as i32,
                            3 => control.bell_duration = v as i16 as i32,
                            4 => control.led = v as u8 as i32,
                            5 => control.led_mode = v as u8 as i32,
                            6 => control.key = v as u8 as i32,
                            _ => control.auto_repeat_mode = v as u8 as i32,
                        }
                    }
                    x11::errors::capture(|| unsafe {
                        x11::XChangeKeyboardControl(
                            x11::connection::xcb_display(),
                            mask as u64,
                            &control,
                        )
                    })?;
                    Ok(None)
                }),
            }
        }
    };
}
keyboard_export!(xcb_change_keyboard_control, false);
keyboard_export!(xcb_change_keyboard_control_checked, true);
macro_rules! keyboard_query_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(_c: *mut xcb_connection_t) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, true, || {
                    let mut s = unsafe { core::mem::zeroed::<x11::keyboard::XKeyboardState>() };
                    unsafe { x11::XGetKeyboardControl(x11::connection::xcb_display(), &mut s) };
                    let mut b = request::reply(20);
                    b[1] = s.global_auto_repeat as u8;
                    request::put32(&mut b, 8, s.led_mask as u32);
                    b[12] = s.key_click_percent as u8;
                    b[13] = s.bell_percent as u8;
                    request::put16(&mut b, 14, s.bell_pitch as u16);
                    request::put16(&mut b, 16, s.bell_duration as u16);
                    b[20..52].copy_from_slice(&s.auto_repeats);
                    Ok(Some(b))
                }),
            }
        }
    };
}
keyboard_query_export!(xcb_get_keyboard_control, true);
keyboard_query_export!(xcb_get_keyboard_control_unchecked, false);
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_keyboard_control_reply")]
pub unsafe extern "sysv64" fn xcb_get_keyboard_control_reply(
    _c: *mut xcb_connection_t,
    k: xcb_void_cookie_t,
    e: *mut *mut xcb_generic_error_t,
) -> *mut c_void {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { request::take(k.sequence, e) }
}
macro_rules! bell_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            percent: i8,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let result = x11::errors::capture(|| unsafe {
                        x11::graphics::XBell(x11::connection::xcb_display(), percent as i32)
                    })?;
                    if result == 0 {
                        return Err(request::error(17, 104, 0));
                    }
                    Ok(None)
                }),
            }
        }
    };
}
bell_export!(xcb_bell, false);
bell_export!(xcb_bell_checked, true);

macro_rules! warp_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            src: u32,
            dst: u32,
            sx: i16,
            sy: i16,
            w: u16,
            h: u16,
            dx: i16,
            dy: i16,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let source = if src == 0 {
                        0
                    } else {
                        window::native(src, 41)?
                    };
                    let destination = if dst == 0 {
                        0
                    } else {
                        window::native(dst, 41)?
                    };
                    let success = x11::errors::capture(|| unsafe {
                        x11::XWarpPointer(
                            x11::connection::xcb_display(),
                            source,
                            destination,
                            sx as i32,
                            sy as i32,
                            w as u32,
                            h as u32,
                            dx as i32,
                            dy as i32,
                        )
                    })?;
                    if success == 0 {
                        return Err(request::error(17, 41, dst));
                    }
                    Ok(None)
                }),
            }
        }
    };
}
warp_export!(xcb_warp_pointer, false);
warp_export!(xcb_warp_pointer_checked, true);

macro_rules! focus_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            revert: u8,
            id: u32,
            time: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, false, || {
                    let target = if id < 2 {
                        id as usize
                    } else {
                        window::native(id, 42)?
                    };
                    x11::errors::capture(|| unsafe {
                        x11::XSetInputFocus(
                            x11::connection::xcb_display(),
                            target,
                            revert as i32,
                            time as u64,
                        )
                    })?;
                    Ok(None)
                }),
            }
        }
    };
}
focus_export!(xcb_set_input_focus, false);
focus_export!(xcb_set_input_focus_checked, true);

macro_rules! grab_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            owner: u8,
            window: u32,
            time: u32,
            pointer: u8,
            keyboard: u8,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            xcb_void_cookie_t {
                sequence: request::submit($checked, true, || {
                    let target = window::native(window, 31)?;
                    let result = x11::errors::capture(|| unsafe {
                        x11::focus::XGrabKeyboard(
                            x11::connection::xcb_display(),
                            target,
                            owner as i32,
                            pointer as i32,
                            keyboard as i32,
                            time as u64,
                        )
                    })?;
                    let mut b = request::reply(0);
                    b[1] = result as u8;
                    Ok(Some(b))
                }),
            }
        }
    };
}
grab_export!(xcb_grab_keyboard, true);
grab_export!(xcb_grab_keyboard_unchecked, false);
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_grab_keyboard_reply")]
pub unsafe extern "sysv64" fn xcb_grab_keyboard_reply(
    _c: *mut xcb_connection_t,
    k: xcb_void_cookie_t,
    e: *mut *mut xcb_generic_error_t,
) -> *mut c_void {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { request::take(k.sequence, e) }
}
macro_rules! ungrab_export {
    ($name:ident,$checked:expr) => {
        #[unsafe(export_name=concat!("kinakaze_engine_libxcb_",stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(
            _c: *mut xcb_connection_t,
            time: u32,
        ) -> xcb_void_cookie_t {
            let _request_display = x11::connection::scope_xcb(_c.cast());
            request::command($checked, || {
                unsafe { x11::focus::XUngrabKeyboard(x11::connection::xcb_display(), time as u64) };
            })
        }
    };
}
ungrab_export!(xcb_ungrab_keyboard, false);
ungrab_export!(xcb_ungrab_keyboard_checked, true);
