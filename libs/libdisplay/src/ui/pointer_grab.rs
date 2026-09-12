//! A session pointer grab also applies to HWNDs owned by other guest processes.
//! Windows capture alone cannot redirect a foreground application's messages
//! to Mutter's background UI thread. Forward those messages to the grab window.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileMappingW(
        file: Handle,
        attributes: *const c_void,
        protection: u32,
        high: u32,
        low: u32,
        name: *const u16,
    ) -> Handle;
    fn MapViewOfFile(mapping: Handle, access: u32, high: u32, low: u32, size: usize)
    -> *mut c_void;
    fn CloseHandle(handle: Handle) -> i32;
    fn GetCurrentProcessId() -> u32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn IsWindow(window: Handle) -> i32;
    fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
    fn GetAsyncKeyState(key: i32) -> i16;
}

#[repr(C)]
struct SharedCapture {
    target: AtomicUsize,
    source: AtomicUsize,
    origin: AtomicUsize,
    keyboard: AtomicUsize,
    keyboard_window: AtomicUsize,
}
fn shared() -> Option<&'static SharedCapture> {
    static MAPPING: OnceLock<usize> = OnceLock::new();
    let address = *MAPPING.get_or_init(|| unsafe {
        let name = wide(&format!(
            "Local\\kinakaze.pointer-grab.v4.{:016x}",
            kinakaze_runtime::authority::domain_id()
        ));
        let size = core::mem::size_of::<SharedCapture>();
        let mapping = CreateFileMappingW(
            -1isize as Handle,
            core::ptr::null(),
            4,
            0,
            size as u32,
            name.as_ptr(),
        );
        if mapping.is_null() {
            return 0;
        }
        let view = MapViewOfFile(mapping, 0xf001f, 0, 0, size);
        if view.is_null() {
            CloseHandle(mapping);
        }
        // Keep the section handle for this process's lifetime. A view retains
        // memory, but closing the last handle removes its shared object name.
        view as usize
    });
    (address != 0).then(|| unsafe { &*(address as *const SharedCapture) })
}
fn record(window: usize) -> usize {
    let mut process = 0;
    unsafe {
        GetWindowThreadProcessId(window as Handle, &raw mut process);
    }
    ((process as usize) << 32) | (window as u32 as usize)
}
fn valid(record: usize) -> Option<usize> {
    let window = record as u32 as usize;
    if window <= 1 {
        return None;
    }
    let mut process = 0;
    unsafe {
        GetWindowThreadProcessId(window as Handle, &raw mut process);
    }
    (process == (record >> 32) as u32
        && unsafe { IsWindow(window as Handle) } != 0
        && kinakaze_runtime::job::namespace_pid(process).is_some())
    .then_some(window)
}
/// Preserve capture on the thread that received the physical button press.
/// A background WM cannot use SetCapture to capture another process's input.
pub(super) fn source_changed(window: usize, enabled: bool) {
    let Some(shared) = shared() else {
        return;
    };
    if enabled {
        shared.source.store(record(window), Ordering::Release);
    } else {
        let _ = shared
            .source
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                (value as u32 == window as u32).then_some(0)
            });
    }
}
pub(super) fn has_foreign_source(window: usize) -> bool {
    let Some(shared) = shared() else {
        return false;
    };
    let source = shared.source.load(Ordering::Acquire);
    valid(source).is_some() && source >> 32 != record(window) >> 32
}
pub(super) fn source_active(window: usize) -> bool {
    shared().is_some_and(|shared| shared.source.load(Ordering::Acquire) == record(window))
}

pub fn changed(window: usize, enabled: bool) {
    let Some(shared) = shared() else {
        return;
    };
    let slot = &shared.target;
    if enabled {
        let source = shared.source.load(Ordering::Acquire);
        shared.origin.store(source, Ordering::Release);
        slot.store(record(window), Ordering::Release);
        if let Some(source) = valid(source).filter(|&source| source != window) {
            unsafe {
                PostMessageW(source as Handle, interaction::GRAB_CROSSING, 1, 0);
            }
        }
    } else {
        if slot
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                ((value as u32) == window as u32).then_some(0)
            })
            .is_ok()
        {
            if let Some(origin) =
                valid(shared.origin.swap(0, Ordering::AcqRel)).filter(|&origin| origin != window)
            {
                unsafe {
                    PostMessageW(origin as Handle, interaction::GRAB_CROSSING, 2, 0);
                }
            }
            let source = shared.source.swap(0, Ordering::AcqRel);
            if let Some(source) = valid(source) {
                unsafe {
                    PostMessageW(source as Handle, interaction::RELEASE_SOURCE, 0, 0);
                }
            }
        }
    }
}

pub fn release_owned() {
    let Some(shared) = shared() else {
        return;
    };
    let target = shared.target.load(Ordering::Acquire);
    if (target >> 32) as u32 == unsafe { GetCurrentProcessId() } {
        changed(target as u32 as usize, false);
    }
}

/// The grab window may belong to another client (Mutter does this during a
/// resize). Deliver to the grabbing process; XI/core routing then supplies the
/// requested X window, without transferring Windows focus or raising a surface.
pub fn keyboard_changed(window: usize, enabled: bool) {
    let Some(shared) = shared() else {
        return;
    };
    if enabled {
        let receiver = if crate::window::logical_for_native(window).is_some() {
            Some(window)
        } else {
            crate::window::hierarchy(None).and_then(|(_, children)| children.into_iter().next())
        };
        if let Some(receiver) = receiver {
            shared.keyboard_window.store(window, Ordering::Release);
            shared.keyboard.store(record(receiver), Ordering::Release);
        }
    } else if shared.keyboard_window.load(Ordering::Acquire) == window {
        let current = shared.keyboard.load(Ordering::Acquire);
        if (current >> 32) as u32 == unsafe { GetCurrentProcessId() } {
            let _ =
                shared
                    .keyboard
                    .compare_exchange(current, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

// Keep forwarded input distinct from physical Win32 input. Tracking a native
// mouse leave on the background grab window produces enter/leave on every move.
pub(super) const FORWARDED_INPUT: u32 = 0x0414;
fn post_input(target: usize, message: u32, value: usize, parameter: isize) -> bool {
    let packed =
        value as u32 as usize | ((message as usize) << 32) | ((input_modifiers() as usize) << 48);
    unsafe { PostMessageW(target as Handle, FORWARDED_INPUT, packed, parameter) != 0 }
}

pub(super) fn forward(window: Handle, message: u32, buttons: usize, position: isize) -> bool {
    if matches!(message, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) {
        let Some(shared) = shared() else {
            return false;
        };
        let receiver = shared.keyboard.load(Ordering::Acquire);
        // Events already in the grabbing process follow its normal XI/core
        // routing, including owner_events. Only cross the process boundary here.
        if receiver == 0 || (receiver >> 32) as u32 == unsafe { GetCurrentProcessId() } {
            return false;
        }
        let Some(target) = valid(receiver) else {
            let _ =
                shared
                    .keyboard
                    .compare_exchange(receiver, 0, Ordering::AcqRel, Ordering::Acquire);
            return false;
        };
        return post_input(target, message, buttons, position);
    }
    // CSD resize edges use native non-client hit testing. Capture owned by a
    // background WM thread does not convert those messages into client mouse
    // messages, so forwarding only WM_MOUSE* loses motion and button release.
    let client_message = match message {
        0x00a0 => Some(WM_MOUSEMOVE),
        0x00a1 => Some(WM_LBUTTONDOWN),
        0x00a2 => Some(WM_LBUTTONUP),
        0x00a4 => Some(WM_RBUTTONDOWN),
        0x00a5 => Some(WM_RBUTTONUP),
        0x00a7 => Some(WM_MBUTTONDOWN),
        0x00a8 => Some(WM_MBUTTONUP),
        0x00ab => Some(WM_XBUTTONDOWN),
        0x00ac => Some(WM_XBUTTONUP),
        _ => None,
    };
    let non_client = client_message.is_some();
    let message = client_message.unwrap_or(message);
    if !matches!(
        message,
        WM_MOUSEMOVE
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
            | WM_XBUTTONDOWN
            | WM_XBUTTONUP
    ) {
        return false;
    }
    let Some(shared) = shared() else {
        return false;
    };
    let slot = &shared.target;
    let record = slot.load(Ordering::Acquire);
    let target = record as u32 as usize;
    if target <= 1 || target == window as usize {
        return false;
    }
    if valid(record).is_none() {
        let _ = slot.compare_exchange(record, 0, Ordering::AcqRel, Ordering::Acquire);
        return false;
    }
    let buttons = if non_client {
        // Non-client wParam contains a hit-test code, not MK_* button flags.
        let mut state = buttons & 0xffff_0000;
        for (key, flag) in [
            (1, 1),
            (2, 2),
            (0x10, 4),
            (0x11, 8),
            (4, 0x10),
            (5, 0x20),
            (6, 0x40),
        ] {
            if unsafe { GetAsyncKeyState(key) } < 0 {
                state |= flag;
            }
        }
        state
    } else {
        buttons
    };
    if matches!(
        message,
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP
    ) {
        interaction::button(window as usize, false, buttons);
    }
    let mut point = Point {
        x: signed_low(position),
        y: signed_high(position),
    };
    unsafe {
        if !non_client {
            ClientToScreen(window, &raw mut point);
        }
        ScreenToClient(target as Handle, &raw mut point);
        let position = ((point.x as u16 as u32) | ((point.y as u16 as u32) << 16)) as isize;
        post_input(target, message, buttons, position)
    }
}
