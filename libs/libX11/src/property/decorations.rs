//! Honor the established Motif decoration request used by GTK and Qt CSD.
use super::*;

pub(super) fn changed(window: Window, atom: Atom) {
    if atom == 68 && window != 1 {
        let owner = {
            property(window, atom)
                .filter(|value| value.format == 32 && value.kind == 33 && value.bytes.len() == 4)
                .map(|value| u32::from_ne_bytes(value.bytes[..4].try_into().unwrap()) as usize)
                .unwrap_or(0)
        };
        kinakaze_libdisplay::ui::transient::set(window, owner);
    }
    if atom == 40 && window != 1 {
        let mut limits = kinakaze_libdisplay::ui::interaction::SizeLimits::default();
        if let Some(hints) = words(window, atom).filter(|values| values.len() >= 9) {
            if hints[0] & 16 != 0 {
                limits.min_width = hints[5] as i32;
                limits.min_height = hints[6] as i32;
            }
            if hints[0] & 32 != 0 {
                limits.max_width = hints[7] as i32;
                limits.max_height = hints[8] as i32;
            }
        }
        kinakaze_libdisplay::ui::interaction::set_size_limits(window, limits);
    }
    if window == 1 || !atoms::is_named(atom, b"_MOTIF_WM_HINTS") {
        return;
    }
    let decorated = {
        match property(window, atom) {
            Some(value) if value.format == 32 && value.bytes.len() >= 12 => {
                let flags = u32::from_ne_bytes(value.bytes[0..4].try_into().unwrap());
                let decorations = u32::from_ne_bytes(value.bytes[8..12].try_into().unwrap());
                flags & 2 == 0 || decorations != 0
            }
            _ => true,
        }
    };
    kinakaze_libdisplay::ui::decorations::set(window, decorated);
}

pub(super) fn restore() {
    let keys: Vec<_> = STORE.lock().unwrap().properties.keys().copied().collect();
    for (window, atom) in keys {
        changed(window, atom);
    }
}
