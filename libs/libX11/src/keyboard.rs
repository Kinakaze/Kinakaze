//! On-demand keyboard state. No background sampler or retained key bitmap.
use crate::Display;
use core::ffi::{c_char, c_int};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyboardLayout, MAPVK_VK_TO_VSC_EX, MapVirtualKeyExW,
};
mod control;
pub use control::*;

fn set_key(bitmap: &mut [u8; 32], scan: u32, virtual_key: u16) {
    let evdev = kinakaze_libdisplay::keycode::translate(
        (scan & 0xff) as u16,
        (scan >> 8) == 0xe0,
        virtual_key,
    );
    let key = usize::from(evdev) + 8;
    if evdev != 0 && key < 256 {
        bitmap[key / 8] |= 1 << (key % 8);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XQueryKeymap")]
pub unsafe extern "sysv64" fn XQueryKeymap(_display: *mut Display, output: *mut c_char) -> c_int {
    if output.is_null() {
        return 0;
    }
    let mut bitmap = [0_u8; 32];
    let layout = unsafe { GetKeyboardLayout(0) };
    // Ignore aggregate Shift/Control/Alt and mouse VKs: the left/right keyboard
    // keys are queried independently, preserving their distinct X keycodes.
    for vk in 8_u16..=254 {
        if matches!(vk, 0x10..=0x12) || unsafe { GetAsyncKeyState(i32::from(vk)) } >= 0 {
            continue;
        }
        let scan = unsafe { MapVirtualKeyExW(u32::from(vk), MAPVK_VK_TO_VSC_EX, layout) };
        set_key(&mut bitmap, scan, vk);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bitmap.as_ptr(), output.cast(), 32);
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extended_and_regular_keys_use_the_event_keycode_space() {
        let mut bitmap = [0; 32];
        set_key(&mut bitmap, 0x1e, 0x41); // A
        set_key(&mut bitmap, 0xe01d, 0xa3); // right control
        set_key(&mut bitmap, 0, 0); // unmapped
        assert_eq!(bitmap[38 / 8], 1 << (38 % 8));
        assert_eq!(bitmap[105 / 8], 1 << (105 % 8));
        assert_eq!(bitmap.iter().map(|byte| byte.count_ones()).sum::<u32>(), 2);
        assert_eq!(bitmap[0], 0);
    }
}

// The evdev + 8 keycodes used by native events, in X modifier-index order.
pub(crate) const MODIFIERS: [u8; 16] = [
    50, 62, 66, 0, 37, 105, 64, 108, 77, 0, 78, 0, 133, 134, 0, 0,
];

#[unsafe(export_name = "kinakaze_engine_libX11_XkbKeysymToModifiers")]
pub unsafe extern "sysv64" fn XkbKeysymToModifiers(_display: *mut Display, keysym: usize) -> u32 {
    if keysym == 0 {
        return 0;
    }
    let mut mask = 0;
    for (index, pair) in MODIFIERS.chunks_exact(2).enumerate() {
        if pair
            .iter()
            .any(|key| *key != 0 && crate::linux_keycode_to_keysym(u32::from(*key)) == keysym)
        {
            mask |= 1 << index;
        }
    }
    mask
}

pub(crate) fn pointer_mask() -> u32 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    let mut mask = 0;
    for (vk, bit) in [
        (0x10, 0),
        (0x11, 2),
        (0x12, 3),
        (0x5b, 6),
        (0x5c, 6),
        (1, 8),
        (4, 9),
        (2, 10),
        (5, 11),
        (6, 12),
    ] {
        if unsafe { GetAsyncKeyState(vk) } < 0 {
            mask |= 1 << bit;
        }
    }
    for (vk, bit) in [(0x14, 1), (0x90, 4), (0x91, 5)] {
        if unsafe { GetKeyState(vk) } & 1 != 0 {
            mask |= 1 << bit;
        }
    }
    EVENT_MODIFIERS.with(|state| {
        state
            .get()
            .map_or(mask, |modifiers| (mask & !255) | modifiers)
    })
}

thread_local! {
    static EVENT_MODIFIERS: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}
/// Scope a queued event's modifier snapshot, including reentrant X calls.
pub struct EventModifiers(Option<u32>);
impl EventModifiers {
    pub fn enter(modifiers: Option<u32>) -> Self {
        Self(EVENT_MODIFIERS.with(|state| state.replace(modifiers)))
    }
}
impl Drop for EventModifiers {
    fn drop(&mut self) {
        EVENT_MODIFIERS.with(|state| state.set(self.0));
    }
}
