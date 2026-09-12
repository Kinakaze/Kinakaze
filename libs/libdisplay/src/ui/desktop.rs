//! The desktop takes keyboard focus without covering independent guest HWNDs.
use super::*;
#[link(name = "user32")]
unsafe extern "system" {
    fn SetPropW(window: Handle, name: *const u16, value: Handle) -> i32;
    fn GetPropW(window: Handle, name: *const u16) -> Handle;
    fn EnumWindows(callback: unsafe extern "system" fn(Handle, isize) -> i32, data: isize) -> i32;
    fn GetClassNameW(window: Handle, name: *mut u16, count: i32) -> i32;
    fn IsWindowVisible(window: Handle) -> i32;
    fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
    fn GetWindow(window: Handle, command: u32) -> Handle;
}
pub fn mark(window: usize) {
    unsafe {
        SetPropW(
            window as Handle,
            wide("KinakazeDesktopSurface").as_ptr(),
            1usize as Handle,
        );
    }
}
pub fn is_desktop(window: usize) -> bool {
    !unsafe { GetPropW(window as Handle, wide("KinakazeDesktopSurface").as_ptr()) }.is_null()
}
pub(super) fn mark_override_redirect(window: Handle, enabled: bool) {
    unsafe {
        SetPropW(
            window,
            wide("KinakazeOverrideRedirect").as_ptr(),
            usize::from(enabled) as Handle,
        );
    }
}
/// The compositor is hosted below the other guest applications. Its empty
/// X input regions must not pass background clicks into Windows or the Mutter
/// guard window: those clicks belong to the visible desktop stage.
pub(super) fn owns_surface(window: Handle) -> bool {
    is_desktop(unsafe { GetAncestor(window, 2) } as usize)
}
fn captures_scene(window: Handle) -> bool {
    !unsafe { GetPropW(window, wide("KinakazeCompositorInput").as_ptr()) }.is_null()
        // Mutter also grabs just the pointer while moving/resizing a client.
        // That routes input; it must not lift the entire desktop over the live
        // client and replace its DWM surface with a compositor snapshot.
        || unsafe { GetPropW(window, wide("KinakazeCompositorGrab").as_ptr()) } as usize & 2 != 0
}
pub fn grab_changed(window: usize, device: i32, enabled: bool) {
    let surface = unsafe { GetAncestor(window as Handle, 2) };
    if !is_desktop(surface as usize) || !(2..=3).contains(&device) {
        return;
    }
    unsafe {
        // X grabs survive a host foreground change. Track the compositor's
        // explicit pointer/keyboard grabs, not Windows' temporary capture.
        let key = wide("KinakazeCompositorGrab");
        let previous = GetPropW(surface, key.as_ptr()) as usize;
        let bit = 1usize << (device - 2);
        let next = if enabled {
            previous | bit
        } else {
            previous & !bit
        };
        if next == previous {
            return;
        }
        SetPropW(surface, key.as_ptr(), next as Handle);
        SetWindowPos(
            surface,
            core::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}
pub(super) fn input_region_changed(window: usize) {
    let surface = unsafe { GetAncestor(window as Handle, 2) };
    if !is_desktop(surface as usize) {
        return;
    }
    let Some((width, height)) = client_size(window) else {
        return;
    };
    if client_size(surface as usize) != Some((width, height)) {
        return;
    }
    // GNOME expands the stage input region for its overview and shrinks it
    // back to the panel afterwards. Follow that compositor-owned transition.
    let captures =
        input_shape::contains(window, 0, 0) && input_shape::contains(window, width - 1, height - 1);
    if (!unsafe { GetPropW(surface, wide("KinakazeCompositorInput").as_ptr()) }.is_null())
        == captures
    {
        return;
    }
    unsafe {
        SetPropW(
            surface,
            wide("KinakazeCompositorInput").as_ptr(),
            usize::from(captures) as Handle,
        );
        SetWindowPos(
            surface,
            core::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}
unsafe extern "system" fn find_capture(window: Handle, data: isize) -> i32 {
    if !is_desktop(window as usize)
        || !captures_scene(window)
        || unsafe { IsWindowVisible(window) } == 0
    {
        return 1;
    }
    let mut process = 0;
    unsafe {
        GetWindowThreadProcessId(window, &raw mut process);
    }
    if kinakaze_runtime::job::namespace_pid(process).is_none() {
        return 1;
    }
    unsafe {
        *(data as *mut Handle) = window;
    }
    0
}
#[repr(C)]
struct WindowPosition {
    window: Handle,
    after: Handle,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    flags: u32,
}
unsafe extern "system" fn lowest_application(window: Handle, data: isize) -> i32 {
    if unsafe { IsWindowVisible(window) } == 0
        || unsafe { IsIconic(window) } != 0
        || is_desktop(window as usize)
        || input_only::is_input_only(window as usize)
    {
        return 1;
    }
    let mut process = 0;
    unsafe {
        GetWindowThreadProcessId(window, &raw mut process);
    }
    if kinakaze_runtime::job::namespace_pid(process).is_none() {
        return 1;
    }
    let mut class = [0u16; 64];
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    if length <= 0 || class[..length as usize] != wide(WINDOW_CLASS)[..WINDOW_CLASS.len()] {
        return 1;
    }
    let mut rect = Rect::default();
    unsafe {
        GetClientRect(window, &raw mut rect);
    }
    if rect.right > 32 && rect.bottom > 32 {
        // GTK can start at 1x1 with TOOLWINDOW set, then grow into a normal
        // application. Only InputOnly/small windows are non-drawable helpers;
        // a visible utility window also belongs above the desktop background.
        unsafe {
            *(data as *mut Handle) = window;
        }
    }
    1
}
pub(super) unsafe fn positioning(window: Handle, parameter: isize) {
    if parameter == 0 {
        return;
    }
    let position = unsafe { &mut *(parameter as *mut WindowPosition) };
    if position.flags & SWP_NOZORDER != 0 {
        return;
    }
    if !is_desktop(window as usize) {
        if unsafe { GetWindowLongPtrW(window, GWL_STYLE) } & WS_CHILD as isize == 0 {
            let mut surface: Handle = core::ptr::null_mut();
            unsafe {
                EnumWindows(find_capture, &raw mut surface as isize);
            }
            if !surface.is_null() && position.after != surface {
                // Cap raises at the compositor, but preserve the WM's order
                // among clients below it. Inserting every client immediately
                // below the stage reverses a restack while a panel menu is open.
                let above = position.after.is_null()
                    || position.after as isize == -1
                    || position.after as isize == -2
                    || unsafe {
                        let mut previous = GetWindow(surface, 3); // GW_HWNDPREV
                        let mut found = false;
                        while !previous.is_null() {
                            if previous == position.after {
                                found = true;
                                break;
                            }
                            previous = GetWindow(previous, 3);
                        }
                        found
                    };
                if above {
                    position.after = surface;
                }
            }
        }
        return;
    }
    if captures_scene(window) {
        return;
    }
    let mut lowest: Handle = core::ptr::null_mut();
    unsafe {
        EnumWindows(lowest_application, &raw mut lowest as isize);
    }
    if !lowest.is_null() {
        position.after = lowest;
    }
}
