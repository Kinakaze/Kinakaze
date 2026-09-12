//! InputOnly X windows have event identity but no drawable or taskbar surface.
use super::*;

#[link(name = "user32")]
unsafe extern "system" {
    fn SetPropW(window: Handle, name: *const u16, value: Handle) -> i32;
    fn GetPropW(window: Handle, name: *const u16) -> Handle;
    fn SetLayeredWindowAttributes(window: Handle, color: u32, alpha: u8, flags: u32) -> i32;
}

pub fn is_input_only(window: usize) -> bool {
    !unsafe { GetPropW(window as Handle, wide("KinakazeInputOnly").as_ptr()) }.is_null()
}

pub fn set(window: usize) {
    let window = window as Handle;
    unsafe { SetPropW(window, wide("KinakazeInputOnly").as_ptr(), 1usize as Handle) };
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) };
    if style & WS_CHILD as isize == 0 {
        decorations::set(window as usize, false);
        unsafe {
            let extended = GetWindowLongPtrW(window, GWL_EXSTYLE);
            SetWindowLongPtrW(window, GWL_EXSTYLE, (extended | 0x80000 | 0x80) & !0x40000);
            SetLayeredWindowAttributes(window, 0, 0, 2);
        }
    }
}
