//! X11 property wire values. Native storage stays private; query results use
//! guest memory, including the LP64 expansion of format-32 values.
mod atoms;
mod decorations;
mod hints;
mod icon_sizes;
pub(crate) mod wm_support;
pub use icon_sizes::*;
mod lifecycle;
pub mod notify;
pub mod selection;
use crate::{Atom, Bool, Display, Window, c_int, c_long, c_uchar, c_ulong};
pub use atoms::*;
use core::ptr;
pub use hints::*;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone)]
struct Property {
    kind: Atom,
    format: c_int,
    bytes: Vec<u8>,
}
struct Store {
    atoms: Vec<Vec<u8>>,
    properties: BTreeMap<(Window, Atom), Property>,
    selections: BTreeMap<Atom, selection::Owner>,
    masks: BTreeMap<Window, u32>,
}
static STORE: Mutex<Store> = Mutex::new(Store {
    atoms: Vec::new(),
    properties: BTreeMap::new(),
    selections: BTreeMap::new(),
    masks: BTreeMap::new(),
});

fn valid_window(window: Window) -> bool {
    crate::shared::valid(window)
}

fn property(window: Window, atom: Atom) -> Option<Property> {
    decode_property(&crate::shared::get(&format!("p/{window}/{atom}"))?)
}
fn decode_property(bytes: &[u8]) -> Option<Property> {
    Some(Property {
        kind: u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?) as usize,
        format: i32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?),
        bytes: bytes.get(12..)?.to_vec(),
    })
}
fn update_property<T>(
    window: Window,
    atom: Atom,
    apply: impl FnOnce(&mut Option<Property>) -> Result<T, u8>,
) -> Result<T, u8> {
    return crate::shared::transaction(|tx| {
        let key = format!("p/{window}/{atom}");
        let mut value = tx.get(&key).and_then(decode_property);
        let result = apply(&mut value)?;
        if let Some(value) = value {
            let mut bytes = (value.kind as u64).to_le_bytes().to_vec();
            bytes.extend_from_slice(&value.format.to_le_bytes());
            bytes.extend_from_slice(&value.bytes);
            tx.set(key, bytes);
        } else {
            tx.remove(&key);
        }
        Ok(result)
    });
}

pub(crate) fn forget_window(window: Window) {
    crate::frame_sync::bind(window, None);
    let mut store = STORE.lock().unwrap();
    store.properties.retain(|(w, _), _| *w != window);
    store.masks.remove(&window);
    let mut destroyed = Vec::new();
    for (&atom, selection) in store.selections.iter_mut() {
        if selection.window == window {
            selection.window = 0;
            destroyed.push((atom, selection.time));
        }
    }
    drop(store);
    for (atom, time) in destroyed {
        crate::xfixes::tracking::selection(atom, 0, time, 1);
    }
    crate::xfixes::tracking::forget(window);
}
pub(crate) fn close_display() {
    let mut store = STORE.lock().unwrap();
    store.properties.clear();
    store.selections.clear();
    store.masks.clear();
}

/// Records the ICCCM `WM_STATE` a window manager would set: 0 withdrawn,
/// 1 normal, 3 iconic.  Clients such as GDK read it after PropertyNotify.
pub(crate) fn set_wm_state(display: *mut Display, window: Window, state: u32) {
    // A real window manager owns WM_STATE. Its workspace-driven unmaps do
    // not mean that the application withdrew its window.
    if !crate::shared::recipients(1, 1 << 20).is_empty() {
        return;
    }
    let atom = atoms::intern_bytes(b"WM_STATE", false);
    let values = [u64::from(state), 0u64];
    unsafe {
        XChangeProperty(
            display,
            window,
            atom,
            atom,
            32,
            0,
            values.as_ptr().cast(),
            2,
        );
    }
}

/// Format-32 property values as stored, for native consumers of hints.
pub(crate) fn words(window: Window, atom: Atom) -> Option<Vec<u32>> {
    let value = property(window, atom)?;
    (value.format == 32).then(|| {
        value
            .bytes
            .chunks_exact(4)
            .map(|word| u32::from_ne_bytes(word.try_into().unwrap()))
            .collect()
    })
}

#[unsafe(export_name = "kinakaze_engine_libX11_XChangeProperty")]
pub unsafe extern "sysv64" fn XChangeProperty(
    display: *mut Display,
    window: Window,
    property: Atom,
    kind: Atom,
    format: c_int,
    mode: c_int,
    data: *const c_uchar,
    count: c_int,
) -> c_int {
    unsafe { crate::issue_request(display) };
    let result = (|| -> Result<(), u8> {
        if !valid_window(window) {
            return Err(3);
        }
        if !matches!(format, 8 | 16 | 32) || !(0..=2).contains(&mode) || count < 0 {
            return Err(2);
        }
        if count != 0 && data.is_null() {
            return Err(2);
        }
        if atom_name(property).is_none() || atom_name(kind).is_none() {
            return Err(5);
        }
        update_property(window, property, |value| {
            if let Some(old) = value.as_ref() {
                if mode != 0 && (old.kind != kind || old.format != format) {
                    return Err(8);
                }
            }
            let size = (count as usize)
                .checked_mul(format as usize / 8)
                .ok_or(11u8)?;
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(size).map_err(|_| 11u8)?;
            if format == 32 {
                for index in 0..count as usize {
                    let value = unsafe { data.cast::<u64>().add(index).read_unaligned() } as u32;
                    bytes.extend_from_slice(&value.to_ne_bytes());
                }
            } else if size != 0 {
                bytes.extend_from_slice(unsafe { core::slice::from_raw_parts(data, size) });
            }
            if mode != 0 {
                if let Some(old) = value.as_mut() {
                    old.bytes.try_reserve(bytes.len()).map_err(|_| 11u8)?;
                    let original = old.bytes.len();
                    old.bytes.extend_from_slice(&bytes);
                    if mode == 1 {
                        let added = old.bytes.len() - original;
                        old.bytes.rotate_right(added);
                    }
                    return Ok(());
                }
            }
            *value = Some(Property {
                kind,
                format,
                bytes,
            });
            Ok(())
        })
    })();
    if let Err(code) = result {
        unsafe { crate::errors::report(display, code, 18, 0, property) };
        return 0;
    }
    notify::changed(display, window, property, false);
    decorations::changed(window, property);
    if atoms::is_named(property, b"_NET_WM_SYNC_REQUEST_COUNTER") {
        let counter = words(window, property).and_then(|values| values.get(1).copied());
        crate::frame_sync::bind(
            window,
            counter
                .filter(|value| *value != 0)
                .map(|value| value as usize),
        );
    }
    // Preserve the native window title behavior while retaining the property.
    if window != 1 && format == 8 && (property == 39 || atoms::is_named(property, b"_NET_WM_NAME"))
    {
        let title = {
            self::property(window, property)
                .map(|value| String::from_utf8_lossy(&value.bytes).into_owned())
        };
        if let Some(title) = title {
            crate::set_window_title(window, &title);
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetWindowProperty")]
pub unsafe extern "sysv64" fn XGetWindowProperty(
    display: *mut Display,
    window: Window,
    property: Atom,
    offset: c_long,
    length: c_long,
    delete: Bool,
    requested: Atom,
    actual_type: *mut Atom,
    actual_format: *mut c_int,
    nitems: *mut c_ulong,
    bytes_after: *mut c_ulong,
    output: *mut *mut c_uchar,
) -> c_int {
    unsafe { crate::issue_request(display) };
    let mut deleted = false;
    // Xlib marshals these public long arguments into CARD32 protocol fields.
    // In particular, -1L requests up to 0xffffffff words, as used by CEF.
    if (offset < 0 || length < 0) && std::env::var_os("KINAKAZE_X11_PROPERTY_TRACE").is_some() {
        crate::diagnostic!(
            "XGetWindowProperty window={window:#x} property={property} offset={offset} length={length}"
        );
    }
    let offset = offset as u32 as usize;
    let length = length as u32 as usize;
    unsafe {
        *actual_type = 0;
        *actual_format = 0;
        *nitems = 0;
        *bytes_after = 0;
        *output = ptr::null_mut();
    }
    let result = (|| -> Result<(), u8> {
        if !valid_window(window) {
            return Err(3);
        }
        if atom_name(property).is_none() || (requested != 0 && atom_name(requested).is_none()) {
            return Err(5);
        }
        let mut read = |slot: &mut Option<Property>| {
            let Some(value) = slot.as_ref() else {
                return Ok(());
            };
            unsafe {
                *actual_type = value.kind;
                *actual_format = value.format;
            }
            if requested != 0 && requested != value.kind {
                let memory = unsafe { kinakaze_alloc::guest::malloc(1) };
                if memory.is_null() {
                    return Err(11);
                }
                unsafe {
                    memory.write(0);
                    *output = memory;
                    *bytes_after = value.bytes.len() as u64;
                }
                return Ok(());
            }
            let begin = offset.checked_mul(4).ok_or(2u8)?;
            if begin > value.bytes.len() {
                return Err(2);
            }
            let count = length.saturating_mul(4).min(value.bytes.len() - begin);
            let after = value.bytes.len() - begin - count;
            let bytes = &value.bytes[begin..begin + count];
            let size = if value.format == 32 {
                count.checked_mul(2).ok_or(11u8)?
            } else {
                count
            };
            let memory = unsafe { kinakaze_alloc::guest::malloc(size.checked_add(1).ok_or(11u8)?) };
            if memory.is_null() {
                return Err(11);
            }
            if value.format == 32 {
                for (index, word) in bytes.chunks_exact(4).enumerate() {
                    unsafe {
                        memory
                            .cast::<i64>()
                            .add(index)
                            .write(i32::from_ne_bytes(word.try_into().unwrap()) as i64);
                    }
                }
            } else {
                unsafe {
                    ptr::copy_nonoverlapping(bytes.as_ptr(), memory, size);
                }
            }
            unsafe {
                memory.add(size).write(0);
                *output = memory;
                *nitems = (count / (value.format as usize / 8)) as u64;
                *bytes_after = after as u64;
            }
            if delete != 0 && after == 0 {
                *slot = None;
                deleted = true;
            }
            Ok(())
        };
        if delete != 0 {
            update_property(window, property, read)
        } else {
            read(&mut self::property(window, property))
        }
    })();
    match result {
        Ok(()) => {
            if deleted {
                notify::changed(display, window, property, true);
                decorations::changed(window, property);
            }
            0
        }
        Err(code) => unsafe { crate::errors::report(display, code, 20, 0, property) },
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDeleteProperty")]
pub unsafe extern "sysv64" fn XDeleteProperty(
    display: *mut Display,
    window: Window,
    property: Atom,
) -> c_int {
    unsafe { crate::issue_request(display) };
    if !valid_window(window) {
        unsafe {
            crate::errors::report(display, 3, 19, 0, window);
        }
        return 0;
    }
    if atom_name(property).is_none() {
        unsafe { crate::errors::report(display, 5, 19, 0, property) };
        return 0;
    }
    let deleted =
        update_property(window, property, |value| Ok(value.take().is_some())).unwrap_or(false);
    if deleted {
        notify::changed(display, window, property, true);
        decorations::changed(window, property);
        if atoms::is_named(property, b"_NET_WM_SYNC_REQUEST_COUNTER") {
            crate::frame_sync::bind(window, None);
        }
    }
    1
}
