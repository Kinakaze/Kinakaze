//! X11 resource IDs refer to rebuildable display windows, never directly to HWNDs.
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

fn ids() -> &'static Mutex<HashMap<u32, u64>> {
    static IDS: OnceLock<Mutex<HashMap<u32, u64>>> = OnceLock::new();
    IDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Called by the X11 frontend after creating or restoring a window.
pub fn bind(resource: u32, window: u64) -> bool {
    let Ok(windows) = super::windows().lock() else {
        return false;
    };
    if resource == 0 || !windows.contains_key(&window) {
        return false;
    }
    let Ok(mut ids) = ids().lock() else {
        return false;
    };
    match ids.entry(resource) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(window);
            true
        }
        std::collections::hash_map::Entry::Occupied(entry) => *entry.get() == window,
    }
}

pub fn native_handle(resource: u32) -> Option<usize> {
    let window = *ids().lock().ok()?.get(&resource)?;
    super::native_handle(window)
}

pub(super) fn retain(windows: &HashMap<u64, super::WindowState>) {
    if let Ok(mut ids) = ids().lock() {
        ids.retain(|_, window| windows.contains_key(window));
    }
}

pub(super) fn clear() {
    if let Ok(mut ids) = ids().lock() {
        ids.clear();
    }
}
