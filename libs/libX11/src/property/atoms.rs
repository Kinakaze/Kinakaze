//! Protocol-defined atoms have fixed numbers; application atoms start at 69.
use super::Store;
use crate::{Atom, Bool, Display};
use core::ffi::c_char;
use std::ffi::CStr;

const PREDEFINED: [&[u8]; 68] = [
    b"PRIMARY",
    b"SECONDARY",
    b"ARC",
    b"ATOM",
    b"BITMAP",
    b"CARDINAL",
    b"COLORMAP",
    b"CURSOR",
    b"CUT_BUFFER0",
    b"CUT_BUFFER1",
    b"CUT_BUFFER2",
    b"CUT_BUFFER3",
    b"CUT_BUFFER4",
    b"CUT_BUFFER5",
    b"CUT_BUFFER6",
    b"CUT_BUFFER7",
    b"DRAWABLE",
    b"FONT",
    b"INTEGER",
    b"PIXMAP",
    b"POINT",
    b"RECTANGLE",
    b"RESOURCE_MANAGER",
    b"RGB_COLOR_MAP",
    b"RGB_BEST_MAP",
    b"RGB_BLUE_MAP",
    b"RGB_DEFAULT_MAP",
    b"RGB_GRAY_MAP",
    b"RGB_GREEN_MAP",
    b"RGB_RED_MAP",
    b"STRING",
    b"VISUALID",
    b"WINDOW",
    b"WM_COMMAND",
    b"WM_HINTS",
    b"WM_CLIENT_MACHINE",
    b"WM_ICON_NAME",
    b"WM_ICON_SIZE",
    b"WM_NAME",
    b"WM_NORMAL_HINTS",
    b"WM_SIZE_HINTS",
    b"WM_ZOOM_HINTS",
    b"MIN_SPACE",
    b"NORM_SPACE",
    b"MAX_SPACE",
    b"END_SPACE",
    b"SUPERSCRIPT_X",
    b"SUPERSCRIPT_Y",
    b"SUBSCRIPT_X",
    b"SUBSCRIPT_Y",
    b"UNDERLINE_POSITION",
    b"UNDERLINE_THICKNESS",
    b"STRIKEOUT_ASCENT",
    b"STRIKEOUT_DESCENT",
    b"ITALIC_ANGLE",
    b"X_HEIGHT",
    b"QUAD_WIDTH",
    b"WEIGHT",
    b"POINT_SIZE",
    b"RESOLUTION",
    b"COPYRIGHT",
    b"NOTICE",
    b"FONT_NAME",
    b"FAMILY_NAME",
    b"FULL_NAME",
    b"CAP_HEIGHT",
    b"WM_CLASS",
    b"WM_TRANSIENT_FOR",
];
pub(super) fn valid(_store: &Store, atom: Atom) -> bool {
    atom != 0 && (atom <= PREDEFINED.len() || crate::shared::get(&format!("a/{atom}")).is_some())
}

pub(crate) fn is_named(atom: Atom, value: &[u8]) -> bool {
    atom_name(atom).as_deref() == Some(value)
}

/// Shared byte-based atom operations for the XCB protocol frontend.
pub fn atom_name(atom: Atom) -> Option<Vec<u8>> {
    if atom == 0 {
        None
    } else if atom <= PREDEFINED.len() {
        Some(PREDEFINED[atom - 1].to_vec())
    } else {
        crate::shared::get(&format!("a/{atom}"))
    }
}
pub fn intern_bytes(value: &[u8], only_exists: bool) -> Atom {
    if let Some(index) = PREDEFINED.iter().position(|name| *name == value) {
        return index + 1;
    }
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<Vec<u8>, Atom>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(atom) = cache.lock().unwrap().get(value).copied() {
        return atom;
    }

    let key = format!(
        "an/{}",
        value.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let atom = crate::shared::transaction(|tx| {
        if let Some(id) = tx.get(&key) {
            return Ok(u64::from_le_bytes(id.try_into().unwrap()) as usize);
        }
        if only_exists {
            return Ok(0);
        }
        let id = tx
            .get("a-next")
            .map_or(69, |v| u64::from_le_bytes(v.try_into().unwrap()) as usize);
        tx.set("a-next".into(), ((id + 1) as u64).to_le_bytes().to_vec());
        tx.set(format!("a/{id}"), value.to_vec());
        tx.set(key, (id as u64).to_le_bytes().to_vec());
        Ok(id)
    })
    .unwrap_or(0);
    if atom != 0 {
        cache.lock().unwrap().insert(value.to_vec(), atom);
    }
    atom
}

#[unsafe(export_name = "kinakaze_engine_libX11_XInternAtom")]
pub unsafe extern "sysv64" fn XInternAtom(
    _display: *mut Display,
    value: *const c_char,
    only_exists: Bool,
) -> Atom {
    if value.is_null() {
        return 0;
    }
    let value = unsafe { CStr::from_ptr(value) }.to_bytes();
    intern_bytes(value, only_exists != 0)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetAtomName")]
pub unsafe extern "sysv64" fn XGetAtomName(display: *mut Display, atom: Atom) -> *mut c_char {
    let Some(value) = atom_name(atom) else {
        unsafe { crate::errors::report(display, 5, 17, 0, atom) };
        return core::ptr::null_mut();
    };
    let result = unsafe { kinakaze_alloc::guest::malloc(value.len() + 1) };
    if !result.is_null() {
        unsafe {
            core::ptr::copy_nonoverlapping(value.as_ptr(), result, value.len());
            result.add(value.len()).write(0);
        }
    }
    result.cast()
}
