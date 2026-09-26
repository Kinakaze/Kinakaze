//! TrueColor maps use the visual's fixed RGB ramps. Installation selects a
//! real owned map; it does not allocate or program a mutable hardware palette.
mod colors;
mod lifecycle;
pub use colors::*;
mod standard;
use crate::{Colormap, Display, Visual, Window};
use core::ffi::c_int;
pub use standard::*;
use std::collections::BTreeSet;
use std::sync::Mutex;
struct State {
    maps: BTreeSet<Colormap>,
    installed: Colormap,
}
static STATE: Mutex<State> = Mutex::new(State {
    maps: BTreeSet::new(),
    installed: 1,
});
pub(crate) fn valid(map: Colormap) -> bool {
    map == 1 || STATE.lock().unwrap().maps.contains(&map)
}

pub(crate) fn window_map(_d: *mut Display, window: Window) -> Colormap {
    // Ordinary session state survives client fork and is removed by the
    // existing d/<window>/ cleanup when the window is destroyed.
    crate::shared::get(&format!("d/{window}/colormap"))
        .and_then(|bytes| bytes.as_slice().try_into().ok().map(u64::from_le_bytes))
        .map_or(1, |value| value as usize)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWindowColormap")]
pub unsafe extern "sysv64" fn XSetWindowColormap(
    d: *mut Display,
    window: Window,
    map: Colormap,
) -> c_int {
    let mut attributes = unsafe { core::mem::zeroed() };
    if unsafe { crate::XGetWindowAttributes(d, window, &mut attributes) } == 0 {
        return 0;
    }
    let map = if map == 0 {
        let parent = crate::shared::parent(window);
        if parent == 0 || window == 1 {
            return unsafe { crate::errors::report(d, 8, 2, 0, window) };
        }
        window_map(d, parent)
    } else {
        map
    };
    if !valid(map) {
        unsafe {
            crate::errors::report(d, 12, 2, 0, map);
        }
        return 0;
    }
    match crate::shared::set(
        format!("d/{window}/colormap"),
        (map as u64).to_le_bytes().to_vec(),
    ) {
        Ok(()) => 1,
        Err(code) => {
            unsafe {
                crate::errors::report(d, code, 2, 0, window);
            }
            0
        }
    }
}
pub(crate) fn close_display() {
    let mut state = STATE.lock().unwrap();
    state.maps.clear();
    state.installed = 1;
}
#[unsafe(export_name = "kinakaze_engine_libX11_XCreateColormap")]
pub unsafe extern "sysv64" fn XCreateColormap(
    display: *mut Display,
    window: Window,
    visual: *mut Visual,
    alloc: c_int,
) -> Colormap {
    let error = if alloc != 0 && alloc != 1 {
        2
    } else if window != 1
        && unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(window as _) } == 0
    {
        3
    } else if visual.is_null() {
        8
    } else {
        let v = unsafe { &*visual };
        if v.visualid != 1 || v.class != 4 || alloc != 0 {
            8
        } else {
            0
        }
    };
    if error != 0 {
        unsafe {
            crate::errors::report(display, error, 78, 0, window);
        }
        return 0;
    }
    let mut state = STATE.lock().unwrap();
    let map = crate::graphics::allocate_drawable_id();
    state.maps.insert(map);
    map
}
#[unsafe(export_name = "kinakaze_engine_libX11_XFreeColormap")]
pub unsafe extern "sysv64" fn XFreeColormap(display: *mut Display, map: Colormap) -> c_int {
    if map == 1 {
        return 1;
    }
    let mut state = STATE.lock().unwrap();
    if !state.maps.remove(&map) {
        drop(state);
        unsafe {
            crate::errors::report(display, 12, 79, 0, map);
        }
        return 0;
    }
    if state.installed == map {
        state.installed = 1;
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XInstallColormap")]
pub unsafe extern "sysv64" fn XInstallColormap(display: *mut Display, map: Colormap) -> c_int {
    let mut state = STATE.lock().unwrap();
    if map != 1 && !state.maps.contains(&map) {
        drop(state);
        unsafe {
            crate::errors::report(display, 12, 81, 0, map);
        }
        return 0;
    }
    state.installed = map;
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XUninstallColormap")]
pub unsafe extern "sysv64" fn XUninstallColormap(display: *mut Display, map: Colormap) -> c_int {
    let mut state = STATE.lock().unwrap();
    if map != 1 && !state.maps.contains(&map) {
        drop(state);
        unsafe {
            crate::errors::report(display, 12, 82, 0, map);
        }
        return 0;
    }
    if state.installed == map {
        state.installed = 1;
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XListInstalledColormaps")]
pub unsafe extern "sysv64" fn XListInstalledColormaps(
    display: *mut Display,
    window: Window,
    count: *mut c_int,
) -> *mut Colormap {
    unsafe {
        *count = 0;
    }
    if window != 1
        && unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(window as _) } == 0
    {
        unsafe {
            crate::errors::report(display, 3, 83, 0, window);
        }
        return core::ptr::null_mut();
    }
    let result = unsafe { kinakaze_alloc::guest::malloc(core::mem::size_of::<Colormap>()) }
        .cast::<Colormap>();
    if !result.is_null() {
        unsafe {
            *result = STATE.lock().unwrap().installed;
            *count = 1;
        }
    }
    result
}
