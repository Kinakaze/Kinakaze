//! Property events are generated only for clients which selected them.
use super::*;

#[repr(C)]
pub struct PropertyEvent {
    pub kind: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub atom: Atom,
    pub time: c_ulong,
    pub state: c_int,
}

pub fn selected(window: Window) -> u32 {
    STORE
        .lock()
        .unwrap()
        .masks
        .get(&window)
        .copied()
        .unwrap_or(0)
}

pub unsafe fn select(display: *mut Display, window: Window, mask: c_long) -> c_int {
    if crate::trace_enabled() {
        crate::diagnostic!("[libX11] select display={display:p} window={window:#x} mask={mask:#x}");
    }
    if !valid_window(window) {
        return unsafe { crate::errors::report(display, 3, 2, 0, window) };
    }
    if mask < 0 || mask & !0x01ff_ffff != 0 {
        return unsafe { crate::errors::report(display, 2, 2, 0, mask as usize) };
    }
    let mask = match crate::connection::select(display, window, mask as u32) {
        Ok(mask) => mask,
        Err(code) => return unsafe { crate::errors::report(display, code, 2, 0, window) },
    };
    let mut store = STORE.lock().unwrap();
    if mask == 0 {
        store.masks.remove(&window);
    } else {
        store.masks.insert(window, mask);
    }
    1
}

pub(super) fn changed(display: *mut Display, window: Window, atom: Atom, deleted: bool) {
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] property display={display:p} window={window:#x} atom={atom} recipients={:?}",
            crate::connection::recipients(window, 1 << 22)
        );
    }
    let mut wire = [0u8; 32];
    wire[0] = 28;
    wire[4..8].copy_from_slice(&(window as u32).to_le_bytes());
    wire[8..12].copy_from_slice(&(atom as u32).to_le_bytes());
    wire[12..16].copy_from_slice(&(crate::focus::now()).to_le_bytes());
    wire[16] = u8::from(deleted);
    crate::shared::broadcast(window, 1 << 22, wire, true);
}
