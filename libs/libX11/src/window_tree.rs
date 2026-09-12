//! Window hierarchy operations keep the display owner's fork metadata current.
use crate::{Display, Window, XEvent};
use core::ffi::c_int;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPINVISIBLE, ChildWindowFromPointEx, GW_HWNDNEXT, GWL_STYLE, GetParent, GetTopWindow,
    GetWindow, GetWindowLongW, IsWindow, WS_CHILD,
};

fn valid(window: Window) -> bool {
    window == 1 || unsafe { IsWindow(window as _) != 0 }
}

// XCB allocates resource names before creating the native window. Embedders
// pass those names back through Xlib (QWindow::winId -> XReparentWindow).
// Keep the mapping in its owning frontend so its fork/destroy lifecycle is
// also authoritative here.
static WINDOW_RESOLVER: std::sync::OnceLock<fn(Window) -> Option<Window>> =
    std::sync::OnceLock::new();

pub fn register_window_resolver(resolve: fn(Window) -> Option<Window>) {
    let _ = WINDOW_RESOLVER.set(resolve);
}

pub fn native_window(window: Window) -> Window {
    if valid(window) {
        return window;
    }
    WINDOW_RESOLVER
        .get()
        .and_then(|resolve| resolve(window))
        .unwrap_or(window)
}

/// ConfigureNotify.above names the sibling immediately *below* the window.
/// None means bottommost, so reporting zero for every geometry update makes a
/// real WM move popups underneath their application in its compositor stack.
pub(crate) fn sibling_below(window: Window) -> Window {
    let parent = crate::shared::parent(window);
    let mut sibling = unsafe { GetWindow(window as _, GW_HWNDNEXT) };
    while !sibling.is_null() {
        let candidate = sibling as usize;
        if crate::shared::valid(candidate) && crate::shared::parent(candidate) == parent {
            return candidate;
        }
        sibling = unsafe { GetWindow(sibling, GW_HWNDNEXT) };
    }
    0
}

/// X11 stacking is relative to actual siblings, not Win32 transient owners.
/// Conditional modes use the final geometry from the same Configure request.
pub(crate) fn configure_stack(
    window: Window,
    mask: u32,
    values: &crate::wm::XWindowChanges,
    geometry: (i32, i32, i32, i32),
) -> Result<Option<usize>, u8> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GW_HWNDPREV, HWND_BOTTOM, HWND_TOP, IsWindowVisible,
    };
    if mask & 32 != 0 && mask & 64 == 0 {
        return Err(8);
    }
    if mask & 64 == 0 {
        return Ok(None);
    }
    if !(0..=4).contains(&values.stack_mode) {
        return Err(2);
    }
    let (parent, _) = crate::shared::hierarchy(window).ok_or(3u8)?;
    let sibling = (mask & 32 != 0).then_some(values.sibling);
    if let Some(sibling) = sibling {
        let (owner, _) = crate::shared::hierarchy(sibling).ok_or(3u8)?;
        if sibling == window || parent != owner {
            return Err(8);
        }
    }
    if values.stack_mode == 0 {
        let mut after = sibling.map_or(HWND_TOP, |s| unsafe { GetWindow(s as _, GW_HWNDPREV) });
        if after as usize == window {
            after = unsafe { GetWindow(after, GW_HWNDPREV) };
        }
        return Ok(Some(after as usize));
    }
    if values.stack_mode == 1 {
        return Ok(Some(sibling.unwrap_or(HWND_BOTTOM as usize)));
    }
    if unsafe { IsWindowVisible(window as _) } == 0 {
        return Ok(None);
    }
    let (_, members) = crate::shared::hierarchy(parent.unwrap_or(1)).ok_or(3u8)?;
    let (x, y, width, height) = geometry;
    let mut above = false;
    let mut below = false;
    for other in members {
        if other == window
            || sibling.is_some_and(|s| s != other)
            || unsafe { IsWindowVisible(other as _) } == 0
        {
            continue;
        }
        let Some((ox, oy)) = origin(other) else {
            continue;
        };
        let mut bounds = unsafe { core::mem::zeroed() };
        if unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(other as _, &mut bounds)
        } == 0
            || x as i64 >= ox as i64 + bounds.right as i64
            || ox as i64 >= x as i64 + width as i64
            || y as i64 >= oy as i64 + bounds.bottom as i64
            || oy as i64 >= y as i64 + height as i64
        {
            continue;
        }
        let mut previous = unsafe { GetWindow(window as _, GW_HWNDPREV) };
        let mut is_above = false;
        while !previous.is_null() {
            if previous as usize == other {
                is_above = true;
                break;
            }
            previous = unsafe { GetWindow(previous, GW_HWNDPREV) };
        }
        above |= is_above;
        below |= !is_above;
    }
    Ok(match values.stack_mode {
        2 | 4 if above => Some(HWND_TOP as usize),
        3 | 4 if below => Some(HWND_BOTTOM as usize),
        _ => None,
    })
}

/// A window's client origin in its parent's client coordinates, or `None` when
/// the handle is no longer a window.
pub fn origin(window: Window) -> Option<(i32, i32)> {
    if window == 1 {
        return Some((0, 0));
    }
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { ClientToScreen(window as _, &raw mut point) } == 0 {
        return None;
    }
    // GetParent also returns the Win32 owner of a top-level transient. An X11
    // transient remains a child of the root; subtract only a real child parent.
    let parent = if unsafe { GetWindowLongW(window as _, GWL_STYLE) } as u32 & WS_CHILD != 0 {
        unsafe { GetParent(window as _) }
    } else {
        core::ptr::null_mut()
    };
    if !parent.is_null() {
        unsafe {
            ScreenToClient(parent, &raw mut point);
        }
    }
    Some((point.x, point.y))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XLowerWindow")]
pub unsafe extern "sysv64" fn XLowerWindow(display: *mut Display, window: Window) -> c_int {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };
    unsafe { crate::issue_request(display) };
    if !valid(window) {
        unsafe {
            crate::errors::report(display, 3, 12, 0, window);
        }
        return 0;
    }
    if window == 1 {
        return 1;
    }
    c_int::from(
        unsafe {
            SetWindowPos(
                window as _,
                HWND_BOTTOM,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        } != 0,
    )
}
#[unsafe(export_name = "kinakaze_engine_libX11_XRestackWindows")]
pub unsafe extern "sysv64" fn XRestackWindows(
    display: *mut Display,
    windows: *const Window,
    count: c_int,
) -> c_int {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };
    unsafe { crate::issue_request(display) };
    if count <= 1 {
        return 1;
    }
    let windows = unsafe { core::slice::from_raw_parts(windows, count as usize) };
    let mut previous = windows[0];
    for &window in &windows[1..] {
        let Some((parent, _)) = crate::shared::hierarchy(window) else {
            unsafe {
                crate::errors::report(display, 3, 12, 0, window);
            }
            return 0;
        };
        let Some((sibling_parent, _)) = crate::shared::hierarchy(previous) else {
            unsafe {
                crate::errors::report(display, 3, 12, 0, previous);
            }
            return 0;
        };
        if parent != sibling_parent || previous == window {
            unsafe {
                crate::errors::report(display, 8, 12, 0, window);
            }
            return 0;
        }
        if unsafe {
            SetWindowPos(
                window as _,
                previous as _,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        } == 0
        {
            return 0;
        }
        previous = window;
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XQueryTree")]
pub unsafe extern "sysv64" fn XQueryTree(
    display: *mut Display,
    window: Window,
    root: *mut Window,
    parent: *mut Window,
    children: *mut *mut Window,
    count: *mut u32,
) -> c_int {
    unsafe {
        *children = core::ptr::null_mut();
        *count = 0;
    }
    let Some((owner, members)) = crate::shared::hierarchy(window) else {
        unsafe {
            crate::errors::report(display, 3, 15, 0, window);
        }
        return 0;
    };
    let mut ordered = Vec::with_capacity(members.len());
    let mut cursor = unsafe {
        GetTopWindow(if window == 1 {
            core::ptr::null_mut()
        } else {
            window as _
        })
    };
    while !cursor.is_null() {
        if members.contains(&(cursor as usize)) {
            ordered.push(cursor as usize);
        }
        cursor = unsafe { GetWindow(cursor, GW_HWNDNEXT) };
    }
    // XQueryTree returns bottommost first; Win32 walks topmost first.
    ordered.reverse();
    let output = if ordered.is_empty() {
        core::ptr::null_mut()
    } else {
        let output = unsafe {
            kinakaze_alloc::guest::malloc(ordered.len() * core::mem::size_of::<Window>())
        }
        .cast::<Window>();
        if output.is_null() {
            return 0;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(ordered.as_ptr(), output, ordered.len());
        }
        output
    };
    unsafe {
        *root = 1;
        *parent = if window == 1 { 0 } else { owner.unwrap_or(1) };
        *children = output;
        *count = ordered.len() as u32;
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XTranslateCoordinates")]
pub unsafe extern "sysv64" fn XTranslateCoordinates(
    display: *mut Display,
    source: Window,
    destination: Window,
    x: c_int,
    y: c_int,
    dest_x: *mut c_int,
    dest_y: *mut c_int,
    child: *mut Window,
) -> c_int {
    let source = native_window(source);
    let destination = native_window(destination);
    for window in [source, destination] {
        if !valid(window) {
            if crate::trace_enabled() {
                crate::diagnostic!(
                    "[libX11] XTranslateCoordinates {source:#x} -> {destination:#x}: {window:#x} is not a window"
                );
            }
            unsafe {
                crate::errors::report(display, 3, 40, 0, window);
            }
            return 0;
        }
    }
    let mut point = POINT { x, y };
    if (source != 1 && unsafe { ClientToScreen(source as _, &raw mut point) } == 0)
        || (destination != 1 && unsafe { ScreenToClient(destination as _, &raw mut point) } == 0)
    {
        return 0;
    }
    let target = if destination == 1 {
        // Restrict root children to this display's owned windows.
        let mut result = 0;
        if let Some((_, members)) = crate::shared::hierarchy(1) {
            let mut cursor = unsafe { GetTopWindow(core::ptr::null_mut()) };
            while !cursor.is_null() {
                if members.contains(&(cursor as usize)) {
                    let mut rect = unsafe { core::mem::zeroed() };
                    if unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(
                            cursor,
                            &raw mut rect,
                        )
                    } != 0
                        && point.x >= rect.left
                        && point.x < rect.right
                        && point.y >= rect.top
                        && point.y < rect.bottom
                    {
                        result = cursor as usize;
                        break;
                    }
                }
                cursor = unsafe { GetWindow(cursor, GW_HWNDNEXT) };
            }
        }
        result
    } else {
        let target =
            unsafe { ChildWindowFromPointEx(destination as _, point, CWP_SKIPINVISIBLE) } as usize;
        if target == destination || !crate::shared::valid(target) {
            0
        } else {
            target
        }
    };
    unsafe {
        *dest_x = point.x;
        *dest_y = point.y;
        *child = target;
    }
    1
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ReparentEvent {
    pub kind: i32,
    pub serial: u64,
    pub send_event: i32,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub parent: Window,
    pub x: i32,
    pub y: i32,
    pub override_redirect: i32,
}
#[unsafe(export_name = "kinakaze_engine_libX11_XReparentWindow")]
pub unsafe extern "sysv64" fn XReparentWindow(
    display: *mut Display,
    window: Window,
    parent: Window,
    x: c_int,
    y: c_int,
) -> c_int {
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] XReparentWindow {window:#x} parent={parent:#x} native={:#x}",
            native_window(parent)
        );
    }
    let window = native_window(window);
    let parent = native_window(parent);
    unsafe { crate::issue_request(display) };
    if !crate::shared::valid(window) || !crate::shared::valid(parent) {
        unsafe {
            crate::errors::report(display, 3, 7, 0, window);
        }
        return 0;
    }
    if kinakaze_libdisplay::window::logical_for_native(window).is_none()
        || parent != 1 && kinakaze_libdisplay::window::logical_for_native(parent).is_none()
    {
        let mut ancestor = parent;
        for _ in 0..1024 {
            if ancestor == window {
                unsafe {
                    crate::errors::report(display, 8, 7, 0, window);
                }
                return 0;
            }
            if ancestor == 1 {
                break;
            }
            ancestor = crate::shared::parent(ancestor);
        }
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            SetParent, SetWindowLongPtrW, SetWindowPos, WS_POPUP,
        };
        let old_parent = crate::shared::parent(window);
        // Win32 derives cross-thread child input attachment during SetParent.
        // Set WS_CHILD first: changing it only afterwards leaves a Mutter frame
        // and its client on separate input queues, so SetFocus(client) fails.
        let old_style = unsafe { GetWindowLongPtrW(window as _, GWL_STYLE) };
        let style = old_style
            & !(windows_sys::Win32::UI::WindowsAndMessaging::WS_OVERLAPPEDWINDOW as isize);
        let style = if parent == 1 {
            (style & !(WS_CHILD as isize)) | WS_POPUP as isize
        } else {
            (style & !(WS_POPUP as isize)) | WS_CHILD as isize
        };
        if parent != 1 {
            unsafe {
                windows_sys::Win32::Foundation::SetLastError(0);
            }
            if unsafe { SetWindowLongPtrW(window as _, GWL_STYLE, style) } == 0
                && unsafe { windows_sys::Win32::Foundation::GetLastError() } != 0
            {
                unsafe {
                    crate::errors::report(display, 8, 7, 0, window);
                }
                return 0;
            }
        }
        unsafe {
            windows_sys::Win32::Foundation::SetLastError(0);
        }
        let old = unsafe {
            SetParent(
                window as _,
                if parent == 1 {
                    core::ptr::null_mut()
                } else {
                    parent as _
                },
            )
        };
        if old.is_null() && unsafe { windows_sys::Win32::Foundation::GetLastError() } != 0 {
            if parent != 1 {
                unsafe {
                    SetWindowLongPtrW(window as _, GWL_STYLE, old_style);
                }
            }
            unsafe {
                crate::errors::report(display, 8, 7, 0, window);
            }
            return 0;
        }
        unsafe {
            if parent == 1 {
                SetWindowLongPtrW(window as _, GWL_STYLE, style);
            }
            SetWindowPos(
                window as _,
                core::ptr::null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE | SWP_FRAMECHANGED,
            );
        }
        if let Some(mut record) = crate::shared::get(&format!("w/{window}")) {
            record[4..12].copy_from_slice(&(parent as u64).to_le_bytes());
            let _ = crate::shared::set(format!("w/{window}"), record);
        }
        let mut wire = [0u8; 32];
        wire[0] = 21;
        wire[8..12].copy_from_slice(&(window as u32).to_le_bytes());
        wire[12..16].copy_from_slice(&(parent as u32).to_le_bytes());
        wire[16..18].copy_from_slice(&(x as i16).to_le_bytes());
        wire[18..20].copy_from_slice(&(y as i16).to_le_bytes());
        wire[20] = u8::from(crate::shared::override_redirect(window));
        for (target, mask) in [(window, 1 << 17), (old_parent, 1 << 19), (parent, 1 << 19)] {
            wire[4..8].copy_from_slice(&(target as u32).to_le_bytes());
            crate::shared::broadcast(target, mask, wire, true);
        }
        return 1;
    }
    let Some(id) = kinakaze_libdisplay::window::logical_for_native(window) else {
        unsafe {
            crate::errors::report(display, 3, 7, 0, window);
        }
        return 0;
    };
    let target = if parent == 1 {
        None
    } else {
        let Some(id) = kinakaze_libdisplay::window::logical_for_native(parent) else {
            unsafe {
                crate::errors::report(display, 3, 7, 0, parent);
            }
            return 0;
        };
        Some(id)
    };
    match kinakaze_libdisplay::window::reparent(id, target, x, y) {
        Ok(()) => {
            if let Some(mut record) = crate::shared::get(&format!("w/{window}")) {
                record[4..12].copy_from_slice(&(parent as u64).to_le_bytes());
                let _ = crate::shared::set(format!("w/{window}"), record);
            }
            let mut event: XEvent = unsafe { core::mem::zeroed() };
            unsafe {
                (&raw mut event)
                    .cast::<ReparentEvent>()
                    .write(ReparentEvent {
                        kind: 21,
                        serial: if display.is_null() {
                            0
                        } else {
                            (*display).request as u64
                        },
                        send_event: 0,
                        display,
                        event: window,
                        window,
                        parent,
                        x,
                        y,
                        override_redirect: i32::from(
                            kinakaze_libdisplay::ui::is_override_redirect(window),
                        ),
                    });
            }
            crate::queue_structure_event(display, window, event);
            1
        }
        Err(code) => {
            unsafe {
                crate::errors::report(display, code, 7, 0, window);
            }
            0
        }
    }
}
