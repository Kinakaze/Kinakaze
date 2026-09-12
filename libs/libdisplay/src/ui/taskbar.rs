//! A fullscreen guest desktop remains fullscreen when one of its apps activates.
//! This is a per-HWND Shell hint; host taskbar preferences are never changed.
use super::*;
use std::cell::Cell;

#[repr(C)]
struct Guid {
    a: u32,
    b: u16,
    c: u16,
    d: [u8; 8],
}
static CLASS: Guid = Guid {
    a: 0x56fdf344,
    b: 0xfd6d,
    c: 0x11d0,
    d: [0x95, 0x8a, 0, 0x60, 0x97, 0xc9, 0xa0, 0x90],
};
static INTERFACE: Guid = Guid {
    a: 0x602d4995,
    b: 0xb13a,
    c: 0x429b,
    d: [0xa6, 0x6e, 0x19, 0x35, 0xe4, 0x4f, 0x43, 0x17],
};
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    fn CoCreateInstance(
        class: *const Guid,
        outer: Handle,
        context: u32,
        interface: *const Guid,
        object: *mut Handle,
    ) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn EnumWindows(callback: unsafe extern "system" fn(Handle, isize) -> i32, data: isize) -> i32;
    fn IsWindowVisible(window: Handle) -> i32;
    fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
    fn SetPropW(window: Handle, name: *const u16, value: Handle) -> i32;
    fn GetPropW(window: Handle, name: *const u16) -> Handle;
}
pub(super) unsafe fn window_hidden(window: Handle) -> Handle {
    unsafe { GetPropW(window, wide("KinakazeWorkspaceHidden").as_ptr()) }
}
thread_local! { static TASKBAR: Cell<usize> = const { Cell::new(0) }; }
struct Search {
    monitor: Handle,
    found: bool,
}
unsafe extern "system" fn find_desktop(window: Handle, data: isize) -> i32 {
    if !desktop::is_desktop(window as usize) || unsafe { IsWindowVisible(window) } == 0 {
        return 1;
    }
    let mut process = 0;
    unsafe {
        GetWindowThreadProcessId(window, &raw mut process);
    }
    if kinakaze_runtime::job::namespace_pid(process).is_none() {
        return 1;
    }
    let search = unsafe { &mut *(data as *mut Search) };
    if unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) } == search.monitor
        && covers_monitor(window)
    {
        search.found = true;
        return 0;
    }
    1
}
fn covers_monitor(window: Handle) -> bool {
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    let mut info = MonitorInfo {
        size: core::mem::size_of::<MonitorInfo>() as u32,
        monitor: Rect::default(),
        work: Rect::default(),
        flags: 0,
    };
    let mut bounds = Rect::default();
    (unsafe {
        GetMonitorInfoW(monitor, &raw mut info) != 0 && GetWindowRect(window, &raw mut bounds) != 0
    }) && bounds.left <= info.monitor.left
        && bounds.top <= info.monitor.top
        && bounds.right >= info.monitor.right
        && bounds.bottom >= info.monitor.bottom
}

pub(super) fn activate(window: Handle, active: bool) {
    // The Shell applies MarkFullscreenWindow only while that HWND is active.
    // Clearing background peers on a host transition races the next guest map.
    if active {
        refresh(window);
    }
}
pub(super) fn refresh(window: Handle) {
    if unsafe { GetWindowLongPtrW(window, GWL_STYLE) } & WS_CHILD as isize != 0 {
        return;
    }
    let mut search = Search {
        monitor: unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) },
        found: false,
    };
    unsafe {
        EnumWindows(find_desktop, &raw mut search as isize);
    }
    let fullscreen = search.found
        || desktop::is_desktop(window as usize) && covers_monitor(window)
        || fullscreen_windows()
            .lock()
            .unwrap()
            .contains_key(&(window as usize));
    set_hint(window, fullscreen);
}
fn set_hint(window: Handle, fullscreen: bool) {
    // Zero means never configured; 1/2 distinguish a positive/negative hint.
    let property = wide("KinakazeFullscreenSession");
    let state = if fullscreen { 1usize } else { 2usize };
    let previous = unsafe { GetPropW(window, property.as_ptr()) } as usize;
    if previous == state || !fullscreen && previous == 0 {
        return;
    }
    if mark(window, fullscreen) {
        unsafe {
            SetPropW(window, property.as_ptr(), state as Handle);
        }
    }
}

fn mark(window: Handle, fullscreen: bool) -> bool {
    TASKBAR.with(|slot| unsafe {
        let mut object = slot.get() as Handle;
        if object.is_null() {
            // The dedicated UI thread owns this apartment and interface for its lifetime.
            CoInitializeEx(core::ptr::null_mut(), 2);
            if CoCreateInstance(
                &raw const CLASS,
                core::ptr::null_mut(),
                1,
                &raw const INTERFACE,
                &raw mut object,
            ) < 0
            {
                return false;
            }
            let table = *object.cast::<*const usize>();
            let initialize: unsafe extern "system" fn(Handle) -> i32 =
                core::mem::transmute(*table.add(3));
            if initialize(object) < 0 {
                let release: unsafe extern "system" fn(Handle) -> u32 =
                    core::mem::transmute(*table.add(2));
                release(object);
                return false;
            }
            slot.set(object as usize);
        }
        let table = *object.cast::<*const usize>();
        let apply: unsafe extern "system" fn(Handle, Handle, i32) -> i32 =
            core::mem::transmute(*table.add(8));
        apply(object, window, i32::from(fullscreen)) >= 0
    })
}
