//! Explicit activity notification; no periodic input or idle-state sampling.
use crate::Display;
use core::ffi::c_int;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETBLOCKSENDINPUTRESETS, SystemParametersInfoW,
};

#[unsafe(export_name = "kinakaze_engine_libX11_XResetScreenSaver")]
pub unsafe extern "sysv64" fn XResetScreenSaver(display: *mut Display) -> c_int {
    if display.is_null() {
        return 0;
    }
    // Respect the host policy that can forbid synthetic activity from resetting
    // the saver. Never alter that policy or the user's configured timeout.
    let mut blocked: i32 = 0;
    if unsafe {
        SystemParametersInfoW(SPI_GETBLOCKSENDINPUTRESETS, 0, (&raw mut blocked).cast(), 0)
    } == 0
        || blocked != 0
    {
        return 0;
    }
    // A zero relative move marks activity without changing position or buttons.
    // Unlike SetThreadExecutionState, SendInput can reset the saver timer.
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    c_int::from(unsafe { SendInput(1, &input, core::mem::size_of::<INPUT>() as i32) } == 1)
}
