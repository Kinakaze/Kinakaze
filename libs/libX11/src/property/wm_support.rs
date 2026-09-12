//! The EWMH support the native window manager advertises. GDK and Qt read
//! `_NET_SUPPORTED` before using `_NET_WM_MOVERESIZE`, `_NET_WM_STATE` and
//! `_NET_ACTIVE_WINDOW`; without it GTK emulates window moves with its own
//! grab window, which the native drag path here never sees.
use super::*;

/// Requests handled natively; nothing is listed that is only stored.
const SUPPORTED: &[&[u8]] = &[
    b"_NET_SUPPORTED",
    b"_NET_SUPPORTING_WM_CHECK",
    b"_NET_WM_NAME",
    b"_NET_WM_ICON_NAME",
    b"_NET_WM_MOVERESIZE",
    b"_NET_ACTIVE_WINDOW",
    b"_NET_CLOSE_WINDOW",
    b"_NET_WM_STATE",
    b"_NET_WM_STATE_FULLSCREEN",
    b"_NET_WM_STATE_MAXIMIZED_VERT",
    b"_NET_WM_STATE_MAXIMIZED_HORZ",
    b"_NET_WM_STATE_HIDDEN",
];

/// Publishes the root window properties once per display.  The root itself
/// serves as the supporting check window: the check property names it and
/// it names itself, which is what clients verify.
pub(crate) fn install(display: *mut Display) {
    let check = atoms::intern_bytes(b"_NET_SUPPORTING_WM_CHECK", false);
    if words(1, check).is_some() {
        return;
    }
    let root = [1u64];
    let supported: Vec<u64> = SUPPORTED
        .iter()
        .map(|name| atoms::intern_bytes(name, false) as u64)
        .collect();
    let utf8 = atoms::intern_bytes(b"UTF8_STRING", false);
    let name = b"Kinakaze";
    unsafe {
        // 33 is WINDOW, 4 is ATOM.
        XChangeProperty(display, 1, check, 33, 32, 0, root.as_ptr().cast(), 1);
        XChangeProperty(
            display,
            1,
            atoms::intern_bytes(b"_NET_SUPPORTED", false),
            4,
            32,
            0,
            supported.as_ptr().cast(),
            supported.len() as c_int,
        );
        XChangeProperty(
            display,
            1,
            atoms::intern_bytes(b"_NET_WM_NAME", false),
            utf8,
            8,
            0,
            name.as_ptr(),
            name.len() as c_int,
        );
    }
}

/// Adds or removes one `_NET_WM_STATE` atom on a window, as the window
/// manager records the states it applied.
pub(crate) fn set_state(display: *mut Display, window: Window, name: &[u8], present: bool) {
    let state = atoms::intern_bytes(b"_NET_WM_STATE", false);
    let atom = atoms::intern_bytes(name, false);
    let mut values: Vec<u64> = words(window, state)
        .unwrap_or_default()
        .into_iter()
        .map(u64::from)
        .filter(|value| *value != atom as u64)
        .collect();
    if present {
        values.push(atom as u64);
    }
    unsafe {
        XChangeProperty(
            display,
            window,
            state,
            4,
            32,
            0,
            values.as_ptr().cast(),
            values.len() as c_int,
        );
    }
}

/// Whether a `_NET_WM_STATE` atom is currently recorded on the window.
pub(crate) fn has_state(window: Window, name: &[u8]) -> bool {
    let atom = atoms::intern_bytes(name, false) as u32;
    words(window, atoms::intern_bytes(b"_NET_WM_STATE", false))
        .is_some_and(|values| values.contains(&atom))
}
