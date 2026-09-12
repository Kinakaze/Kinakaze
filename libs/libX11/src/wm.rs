//! Window management hints, property, selection, atom, font, and event helpers for Xlib.

use std::ffi::{CStr, CString, c_char, c_int, c_uchar, c_uint, c_void};

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetCursorPos, SetWindowPos,
    ShowWindow,
};

use crate::{
    Atom, Bool, Display, KeySym, Pixmap, Status, Time, Visual, Window, XEvent, XSizeHints,
};

type c_long = i64;
type c_ulong = u64;

// ---------------------------------------------------------------------------
// Atoms & Properties
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XInternAtoms")]
pub unsafe extern "sysv64" fn XInternAtoms(
    dpy: *mut Display,
    names: *mut *mut c_char,
    count: c_int,
    only_if_exists: Bool,
    atoms_return: *mut Atom,
) -> Status {
    if names.is_null() || atoms_return.is_null() || count <= 0 {
        return 0;
    }
    for i in 0..count as usize {
        let name_ptr = *names.add(i);
        let atom = crate::XInternAtom(dpy, name_ptr, only_if_exists);
        *atoms_return.add(i) = atom;
    }
    1
}

// ---------------------------------------------------------------------------
// Window Management & Hints
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct XClassHint {
    pub res_name: *mut c_char,
    pub res_class: *mut c_char,
}

#[repr(C)]
pub struct XWMHints {
    pub flags: c_long,
    pub input: Bool,
    pub initial_state: c_int,
    pub icon_pixmap: Pixmap,
    pub icon_window: Window,
    pub icon_x: c_int,
    pub icon_y: c_int,
    pub icon_mask: Pixmap,
    pub window_group: usize,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XAllocClassHint")]
pub unsafe extern "sysv64" fn XAllocClassHint() -> *mut XClassHint {
    unsafe { libc::kinakaze_abi_calloc(1, core::mem::size_of::<XClassHint>()).cast() }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XAllocWMHints")]
pub unsafe extern "sysv64" fn XAllocWMHints() -> *mut XWMHints {
    unsafe { libc::kinakaze_abi_calloc(1, core::mem::size_of::<XWMHints>()).cast() }
}

#[repr(C)]
pub struct XTextProperty {
    pub value: *mut c_uchar,
    pub encoding: Atom,
    pub format: c_int,
    pub nitems: c_ulong,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XAllocSizeHints")]
pub unsafe extern "sysv64" fn XAllocSizeHints() -> *mut XSizeHints {
    unsafe { libc::kinakaze_abi_calloc(1, core::mem::size_of::<XSizeHints>()).cast() }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDefineCursor")]
pub unsafe extern "sysv64" fn XDefineCursor(
    _dpy: *mut Display,
    w: Window,
    cursor: crate::Cursor,
) -> c_int {
    let shape_set = crate::xcursor::define(w, cursor);
    let visibility_set =
        kinakaze_libdisplay::ui::set_cursor_visible(w, crate::xcursor::cursor_visible(cursor));
    crate::xfixes::tracking::define(w, cursor);
    c_int::from(shape_set && visibility_set)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUndefineCursor")]
pub unsafe extern "sysv64" fn XUndefineCursor(_dpy: *mut Display, w: Window) -> c_int {
    let shape_set =
        kinakaze_libdisplay::ui::set_cursor_shape(w, kinakaze_libdisplay::ui::CursorShape::Arrow);
    let visibility_set = kinakaze_libdisplay::ui::set_cursor_visible(w, true);
    crate::xfixes::tracking::define(w, 0);
    c_int::from(shape_set && visibility_set)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XWarpPointer")]
pub unsafe extern "sysv64" fn XWarpPointer(
    _dpy: *mut Display,
    _src_w: Window,
    dest_w: Window,
    _src_x: c_int,
    _src_y: c_int,
    _src_width: c_uint,
    _src_height: c_uint,
    dest_x: c_int,
    dest_y: c_int,
) -> c_int {
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, IsWindow};
    unsafe { crate::issue_request(_dpy) };
    for window in [_src_w, dest_w] {
        if window > 1 && unsafe { IsWindow(window as _) } == 0 {
            return unsafe { crate::errors::report(_dpy, 3, 41, 0, window) };
        }
    }
    let mut current = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut current) } == 0 {
        return 0;
    }
    if _src_w != 0 {
        let mut point = current;
        if _src_w != 1 {
            unsafe { ScreenToClient(_src_w as _, &mut point) };
        }
        let mut rect: windows_sys::Win32::Foundation::RECT = unsafe { core::mem::zeroed() };
        if _src_w == 1 {
            rect.right = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            rect.bottom = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        } else {
            unsafe { GetClientRect(_src_w as _, &mut rect) };
        }
        let right = if _src_width == 0 {
            rect.right
        } else {
            _src_x.saturating_add(_src_width.min(i32::MAX as u32) as i32)
        };
        let bottom = if _src_height == 0 {
            rect.bottom
        } else {
            _src_y.saturating_add(_src_height.min(i32::MAX as u32) as i32)
        };
        if point.x < _src_x || point.y < _src_y || point.x >= right || point.y >= bottom {
            return 1;
        }
    }
    let mut point = POINT {
        x: dest_x,
        y: dest_y,
    };
    if dest_w == 0 {
        point.x = current.x.saturating_add(dest_x);
        point.y = current.y.saturating_add(dest_y);
    } else if dest_w != 1 && unsafe { ClientToScreen(dest_w as _, &mut point) } == 0 {
        return 0;
    }
    kinakaze_libdisplay::ui::barriers::warped(point.x, point.y);
    c_int::from(unsafe { SetCursorPos(point.x, point.y) } != 0)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetScreenSaver")]
pub unsafe extern "sysv64" fn XGetScreenSaver(
    _dpy: *mut Display,
    timeout_return: *mut c_int,
    interval_return: *mut c_int,
    prefer_blanking_return: *mut c_int,
    allow_exposures_return: *mut c_int,
) -> c_int {
    if !timeout_return.is_null() {
        unsafe { *timeout_return = 0 };
    }
    if !interval_return.is_null() {
        unsafe { *interval_return = 0 };
    }
    if !prefer_blanking_return.is_null() {
        unsafe { *prefer_blanking_return = 0 };
    }
    if !allow_exposures_return.is_null() {
        unsafe { *allow_exposures_return = 0 };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetScreenSaver")]
pub unsafe extern "sysv64" fn XSetScreenSaver(
    _dpy: *mut Display,
    _timeout: c_int,
    _interval: c_int,
    _prefer_blanking: c_int,
    _allow_exposures: c_int,
) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetXCBConnection")]
pub unsafe extern "sysv64" fn XGetXCBConnection(dpy: *mut Display) -> *mut c_void {
    if !dpy.is_null() {
        unsafe { (*(*dpy).xcb).connection }
    } else {
        core::ptr::null_mut()
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetEventData")]
pub unsafe extern "sysv64" fn XGetEventData(_dpy: *mut Display, cookie: *mut c_void) -> Bool {
    if cookie.is_null() {
        return 0;
    }
    let cookie = unsafe { &*cookie.cast::<crate::XGenericEventCookie>() };
    if cookie.r#type != crate::GenericEvent {
        return 0;
    }
    if !cookie.data.is_null() {
        // XI2 payloads have the same generic-event header as their cookie.
        // GDK compares the payload serial with grab/ungrab requests; leaving
        // it at zero makes post-grab motion and wheel events look obsolete.
        unsafe { (*cookie.data.cast::<crate::XAnyEvent>()).serial = cookie.serial };
    }
    Bool::from(!cookie.data.is_null())
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeEventData")]
pub unsafe extern "sysv64" fn XFreeEventData(_dpy: *mut Display, cookie: *mut c_void) {
    if cookie.is_null() {
        return;
    }
    let cookie = unsafe { &mut *cookie.cast::<crate::XGenericEventCookie>() };
    if cookie.r#type != crate::GenericEvent {
        return;
    }
    unsafe { crate::input_bridge::free_event_data(cookie.data) };
    cookie.data = core::ptr::null_mut();
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRegisterIMInstantiateCallback")]
pub unsafe extern "sysv64" fn XRegisterIMInstantiateCallback(
    _dpy: *mut Display,
    _rdb: *mut c_void,
    _res_name: *mut c_char,
    _res_class: *mut c_char,
    _callback: *mut c_void,
    _client_data: *mut c_void,
) -> Bool {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUnregisterIMInstantiateCallback")]
pub unsafe extern "sysv64" fn XUnregisterIMInstantiateCallback(
    _dpy: *mut Display,
    _rdb: *mut c_void,
    _res_name: *mut c_char,
    _res_class: *mut c_char,
    _callback: *mut c_void,
    _client_data: *mut c_void,
) -> Bool {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XVaCreateNestedList")]
pub unsafe extern "sysv64" fn XVaCreateNestedList(_unused: c_int) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libX11_Xutf8SetWMProperties")]
pub unsafe extern "sysv64" fn Xutf8SetWMProperties(
    dpy: *mut Display,
    w: Window,
    window_name: *const c_char,
    icon_name: *const c_char,
    argv: *mut *mut c_char,
    argc: c_int,
    normal: *mut XSizeHints,
    hints: *mut XWMHints,
    class: *mut XClassHint,
) {
    unsafe {
        crate::wm_properties::set_names(
            dpy,
            w,
            window_name,
            icon_name,
            argv,
            argc,
            normal,
            hints,
            class,
        );
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XmbSetWMProperties")]
pub unsafe extern "sysv64" fn XmbSetWMProperties(
    dpy: *mut Display,
    w: Window,
    window_name: *const c_char,
    icon_name: *const c_char,
    argv: *mut *mut c_char,
    argc: c_int,
    normal: *mut XSizeHints,
    hints: *mut XWMHints,
    class: *mut XClassHint,
) {
    unsafe {
        crate::wm_properties::set_names(
            dpy,
            w,
            window_name,
            icon_name,
            argv,
            argc,
            normal,
            hints,
            class,
        );
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetWMProperties")]
pub unsafe extern "sysv64" fn XSetWMProperties(
    dpy: *mut Display,
    w: Window,
    name: *mut XTextProperty,
    icon: *mut XTextProperty,
    argv: *mut *mut c_char,
    argc: c_int,
    normal: *mut XSizeHints,
    hints: *mut XWMHints,
    class: *mut c_void,
) {
    unsafe {
        if !name.is_null() {
            XSetWMName(dpy, w, name);
        }
        if !icon.is_null() {
            XSetWMIconName(dpy, w, icon);
        }
        crate::wm_properties::set_hints(dpy, w, argv, argc, normal, hints, class.cast());
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWMNormalHints")]
pub unsafe extern "sysv64" fn XSetWMNormalHints(
    dpy: *mut Display,
    w: Window,
    hints: *mut XSizeHints,
) {
    unsafe {
        crate::wm_properties::XSetWMSizeHints(dpy, w, hints, 40);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetWMNormalHints")]
pub unsafe extern "sysv64" fn XGetWMNormalHints(
    dpy: *mut Display,
    w: Window,
    hints_return: *mut XSizeHints,
    supplied_return: *mut c_long,
) -> Status {
    unsafe {
        crate::wm_properties::query::XGetWMSizeHints(dpy, w, hints_return, supplied_return, 40)
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWMName")]
pub unsafe extern "sysv64" fn XSetWMName(
    dpy: *mut Display,
    w: Window,
    text_prop: *mut XTextProperty,
) {
    unsafe {
        crate::property::XSetTextProperty(dpy, w, text_prop, 39);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWMIconName")]
pub unsafe extern "sysv64" fn XSetWMIconName(
    dpy: *mut Display,
    w: Window,
    text_prop: *mut XTextProperty,
) {
    unsafe {
        crate::property::XSetTextProperty(dpy, w, text_prop, 37);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetCommand")]
pub unsafe extern "sysv64" fn XSetCommand(
    _dpy: *mut Display,
    _w: Window,
    _argv: *mut *mut c_char,
    _argc: c_int,
) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetTransientForHint")]
pub unsafe extern "sysv64" fn XSetTransientForHint(
    dpy: *mut Display,
    w: Window,
    prop_window: Window,
) -> c_int {
    unsafe {
        crate::property::XChangeProperty(dpy, w, 68, 33, 32, 0, (&raw const prop_window).cast(), 1)
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetTransientForHint")]
pub unsafe extern "sysv64" fn XGetTransientForHint(
    dpy: *mut Display,
    w: Window,
    prop_window: *mut Window,
) -> c_int {
    if prop_window.is_null() {
        return 0;
    }
    unsafe {
        *prop_window = 0;
    }
    let (mut actual_type, mut format, mut count, mut after, mut data) =
        (0, 0, 0, 0, core::ptr::null_mut());
    let status = unsafe {
        crate::property::XGetWindowProperty(
            dpy,
            w,
            68,
            0,
            1,
            0,
            33,
            &raw mut actual_type,
            &raw mut format,
            &raw mut count,
            &raw mut after,
            &raw mut data,
        )
    };
    let valid = status == 0 && actual_type == 33 && format == 32 && count == 1 && !data.is_null();
    if valid {
        unsafe {
            *prop_window = *data.cast::<Window>();
        }
    }
    unsafe {
        crate::XFree(data.cast());
    }
    c_int::from(valid)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XWMGeometry")]
pub unsafe extern "sysv64" fn XWMGeometry(
    _dpy: *mut Display,
    _screen_number: c_int,
    _user_geom: *const c_char,
    _def_geom: *const c_char,
    _bwidth: c_uint,
    hints: *mut XSizeHints,
    x_return: *mut c_int,
    y_return: *mut c_int,
    width_return: *mut c_int,
    height_return: *mut c_int,
    gravity_return: *mut c_int,
) -> c_int {
    let def_w = if !hints.is_null() && (*hints).width > 0 {
        (*hints).width
    } else {
        300
    };
    let def_h = if !hints.is_null() && (*hints).height > 0 {
        (*hints).height
    } else {
        200
    };
    if !x_return.is_null() {
        unsafe { *x_return = 100 };
    }
    if !y_return.is_null() {
        unsafe { *y_return = 100 };
    }
    if !width_return.is_null() {
        unsafe { *width_return = def_w };
    }
    if !height_return.is_null() {
        unsafe { *height_return = def_h };
    }
    if !gravity_return.is_null() {
        unsafe { *gravity_return = 1 }; // NorthWestGravity
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XIconifyWindow")]
pub unsafe extern "sysv64" fn XIconifyWindow(
    dpy: *mut Display,
    w: Window,
    _screen_number: c_int,
) -> Status {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    unsafe { crate::issue_request(dpy) };
    if w == 1 || unsafe { IsWindow(w as HWND) } == 0 {
        unsafe { crate::errors::report(dpy, 3, 25, 0, w) };
        return 0;
    }
    // The native minimize reports the resulting unmap through the window's
    // own procedure, so a taskbar restore is observed the same way.
    unsafe { ShowWindow(w as HWND, SW_MINIMIZE) };
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XWithdrawWindow")]
pub unsafe extern "sysv64" fn XWithdrawWindow(
    dpy: *mut Display,
    w: Window,
    _screen_number: c_int,
) -> Status {
    unsafe { crate::issue_request(dpy) };
    // ICCCM withdrawal is an unmap; the client sees UnmapNotify either way.
    unsafe { crate::XUnmapWindow(dpy, w) }
}

/// Route native CSD edge hits through the real WM's interactive resize. Direct
/// SetWindowPos on every pointer event bypasses its _NET_WM_SYNC_REQUEST
/// handshake and can invalidate WGL storage in the middle of a GTK frame.
pub(crate) unsafe extern "system" fn native_resize(
    window: usize,
    x: i32,
    y: i32,
    direction: i32,
) -> bool {
    let recipients = crate::shared::recipients(1, 1 << 20);
    if recipients.is_empty() {
        return false;
    }
    let atom =
        unsafe { crate::XInternAtom(core::ptr::null_mut(), c"_NET_WM_MOVERESIZE".as_ptr(), 0) };
    let mut wire = [0u8; 32];
    wire[0] = crate::ClientMessage as u8 | 0x80;
    wire[1] = 32;
    wire[4..8].copy_from_slice(&(window as u32).to_le_bytes());
    wire[8..12].copy_from_slice(&(atom as u32).to_le_bytes());
    for (index, value) in [x, y, direction, 1, 1].into_iter().enumerate() {
        wire[12 + index * 4..16 + index * 4].copy_from_slice(&value.to_le_bytes());
    }
    // Once a WM owns the root, it is the sole geometry authority even when
    // delivery fails: falling back would start two competing drags.
    for recipient in recipients {
        crate::shared::send_delivery(recipient, wire, Some((1, 1 << 20)));
    }
    true
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRaiseWindow")]
pub unsafe extern "sysv64" fn XRaiseWindow(dpy: *mut Display, w: Window) -> c_int {
    use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_TOP, IsWindow};
    unsafe { crate::issue_request(dpy) };
    if w == 1 {
        return 1;
    }
    if unsafe { IsWindow(w as HWND) } == 0 {
        unsafe { crate::errors::report(dpy, 3, 12, 0, w) };
        return 0;
    }
    // Stacking alone does not need the owning thread; activation (see
    // _NET_ACTIVE_WINDOW) does and goes through the window's own procedure.
    c_int::from(
        unsafe {
            SetWindowPos(
                w as HWND,
                HWND_TOP,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        } != 0,
    )
}

#[unsafe(export_name = "kinakaze_engine_libX11_XMapSubwindows")]
pub unsafe extern "sysv64" fn XMapSubwindows(_dpy: *mut Display, _w: Window) -> c_int {
    1
}

#[repr(C)]
pub struct XWindowChanges {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub sibling: Window,
    pub stack_mode: c_int,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWindowBorderWidth")]
pub unsafe extern "sysv64" fn XSetWindowBorderWidth(
    dpy: *mut Display,
    window: Window,
    width: c_uint,
) -> c_int {
    if width > i32::MAX as c_uint {
        return unsafe { crate::errors::report(dpy, 2, 12, 0, width as usize) };
    }
    let mut values: XWindowChanges = unsafe { core::mem::zeroed() };
    values.border_width = width as c_int;
    unsafe { XConfigureWindow(dpy, window, 16, &raw mut values) }
}

unsafe fn change_save_set(dpy: *mut Display, window: Window) -> c_int {
    let mut attrs = unsafe { core::mem::zeroed() };
    if unsafe { crate::XGetWindowAttributes(dpy, window, &raw mut attrs) } == 0 {
        return 0;
    }
    // All hosted windows belong to the shared client. X11 forbids putting a
    // client's own window in its save-set (or removing it from that save-set).
    unsafe { crate::errors::report(dpy, 8, 6, 0, window) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XAddToSaveSet")]
pub unsafe extern "sysv64" fn XAddToSaveSet(dpy: *mut Display, window: Window) -> c_int {
    unsafe { change_save_set(dpy, window) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XRemoveFromSaveSet")]
pub unsafe extern "sysv64" fn XRemoveFromSaveSet(dpy: *mut Display, window: Window) -> c_int {
    unsafe { change_save_set(dpy, window) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XConfigureWindow")]
pub unsafe extern "sysv64" fn XConfigureWindow(
    dpy: *mut Display,
    w: Window,
    mask: c_uint,
    values: *mut XWindowChanges,
) -> c_int {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, WS_CHILD,
    };
    unsafe { crate::issue_request(dpy) };
    if values.is_null() || mask & !127 != 0 {
        return unsafe { crate::errors::report(dpy, 2, 12, 0, mask as usize) };
    }
    if w == 1 {
        return 1;
    }
    if crate::input_trace_enabled() {
        let v = unsafe { &*values };
        crate::diagnostic!(
            "[input-configure] at={} window={w:#x} mask={mask:#x} xy={},{} size={},{}",
            crate::focus::now(),
            v.x,
            v.y,
            v.width,
            v.height
        );
    }
    if crate::trace_enabled() {
        let v = unsafe { &*values };
        crate::diagnostic!(
            "[libX11] XConfigureWindow {w:#x} mask={mask:#x} x={} y={} w={} h={}",
            v.x,
            v.y,
            v.width,
            v.height
        );
    }
    let hwnd = w as HWND;
    let mut client = unsafe { core::mem::zeroed() };
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
        return unsafe { crate::errors::report(dpy, 3, 12, 0, w) };
    }
    let v = unsafe { &*values };
    {
        let mut request = [0u8; 32];
        request[0] = 23;
        request[1] = v.stack_mode as u8;
        request[12..16].copy_from_slice(&(v.sibling as u32).to_le_bytes());
        for (offset, value) in [
            (16, v.x),
            (18, v.y),
            (20, v.width),
            (22, v.height),
            (24, v.border_width),
            (26, mask as i32),
        ] {
            request[offset..offset + 2].copy_from_slice(&(value as u16).to_le_bytes());
        }
        if crate::shared::redirect_request(dpy, w, request) {
            return 1;
        }
    }
    if mask & 4 != 0 && v.width <= 0 || mask & 8 != 0 && v.height <= 0 {
        return unsafe { crate::errors::report(dpy, 2, 12, 0, w) };
    }
    if mask & 16 != 0 && v.border_width != 0 {
        return unsafe { crate::errors::report(dpy, 17, 12, 0, w) };
    }
    let origin = crate::window_tree::origin(w).unwrap_or_default();
    let (x, y) = (
        if mask & 1 != 0 { v.x } else { origin.0 },
        if mask & 2 != 0 { v.y } else { origin.1 },
    );
    let (width, height) = (
        if mask & 4 != 0 { v.width } else { client.right },
        if mask & 8 != 0 {
            v.height
        } else {
            client.bottom
        },
    );
    let mut frame = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    if unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32 & WS_CHILD == 0 {
        unsafe {
            AdjustWindowRectEx(
                &mut frame,
                GetWindowLongW(hwnd, GWL_STYLE) as u32,
                0,
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
            );
        }
    }
    let mut flags =
        SWP_NOACTIVATE | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSENDCHANGING;
    if mask & 3 == 0 {
        flags |= SWP_NOMOVE;
    }
    if mask & 12 == 0 {
        flags |= SWP_NOSIZE;
    }
    let after = match crate::window_tree::configure_stack(w, mask, v, (x, y, width, height)) {
        Ok(Some(after)) => after as HWND,
        Ok(None) => {
            flags |= SWP_NOZORDER;
            core::ptr::null_mut()
        }
        Err(error) => return unsafe { crate::errors::report(dpy, error, 12, 0, w) },
    };
    let result = unsafe {
        SetWindowPos(
            hwnd,
            after,
            x + frame.left,
            y + frame.top,
            frame.right - frame.left,
            frame.bottom - frame.top,
            flags,
        )
    };
    if crate::trace_enabled() {
        let mut bounds: windows_sys::Win32::Foundation::RECT = unsafe { core::mem::zeroed() };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut bounds) };
        crate::diagnostic!(
            "[libX11] XConfigureWindow {w:#x} -> SetWindowPos({}, {}, {}, {}, flags={flags:#x}) result={result} error={} bounds=({}, {})-({}, {})",
            x + frame.left,
            y + frame.top,
            frame.right - frame.left,
            frame.bottom - frame.top,
            unsafe { windows_sys::Win32::Foundation::GetLastError() },
            bounds.left,
            bounds.top,
            bounds.right,
            bounds.bottom
        );
    }
    result
}

// ---------------------------------------------------------------------------
// Selections
// ---------------------------------------------------------------------------

pub use crate::property::selection::{XConvertSelection, XGetSelectionOwner, XSetSelectionOwner};

// ---------------------------------------------------------------------------
// Text and String conversions
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XConvertCase")]
pub unsafe extern "sysv64" fn XConvertCase(
    keysym: KeySym,
    lower_return: *mut KeySym,
    upper_return: *mut KeySym,
) {
    if !lower_return.is_null() {
        unsafe { *lower_return = keysym };
    }
    if !upper_return.is_null() {
        unsafe { *upper_return = keysym };
    }
}

// ---------------------------------------------------------------------------
// Event queries & Filters
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XEventsQueued")]
pub unsafe extern "sysv64" fn XEventsQueued(dpy: *mut Display, mode: c_int) -> c_int {
    if mode == 2 {
        crate::graphics::flush_all();
    }
    if mode == 1 || mode == 2 {
        crate::drain_native_events(dpy);
    }
    let state = crate::state().lock().unwrap_or_else(|e| e.into_inner());
    let count = state.count_for(dpy);
    crate::set_display_queue_len(dpy, count);
    count.min(c_int::MAX as usize) as c_int
}

#[unsafe(export_name = "kinakaze_engine_libX11_XIfEvent")]
pub unsafe extern "sysv64" fn XIfEvent(
    dpy: *mut Display,
    event_return: *mut XEvent,
    predicate: Option<unsafe extern "sysv64" fn(*mut Display, *mut XEvent, *mut c_char) -> Bool>,
    arg: *mut c_char,
) -> c_int {
    if unsafe { crate::event_select::predicate(dpy, event_return, predicate, arg, true, true) } != 0
    {
        0
    } else {
        -1
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCheckIfEvent")]
pub unsafe extern "sysv64" fn XCheckIfEvent(
    dpy: *mut Display,
    event_return: *mut XEvent,
    predicate: Option<unsafe extern "sysv64" fn(*mut Display, *mut XEvent, *mut c_char) -> Bool>,
    arg: *mut c_char,
) -> Bool {
    unsafe { crate::event_select::predicate(dpy, event_return, predicate, arg, false, true) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFilterEvent")]
pub unsafe extern "sysv64" fn XFilterEvent(_event: *mut XEvent, _w: Window) -> Bool {
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbLookupKeySym")]
pub unsafe extern "sysv64" fn XkbLookupKeySym(
    _dpy: *mut Display,
    key: KeySym,
    _state: c_uint,
    _mods_rtrn: *mut c_uint,
    keysym_rtrn: *mut KeySym,
) -> Bool {
    if !keysym_rtrn.is_null() {
        unsafe { *keysym_rtrn = key };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSynchronize")]
pub unsafe extern "sysv64" fn XSynchronize(
    dpy: *mut Display,
    _onoff: Bool,
) -> Option<unsafe extern "sysv64" fn(*mut Display) -> c_int> {
    None
}

// ---------------------------------------------------------------------------
// Visual & Depth information
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct XVisualInfo {
    pub visual: *mut Visual,
    pub visualid: usize,
    pub screen: c_int,
    pub depth: c_int,
    pub class: c_int,
    pub red_mask: c_ulong,
    pub green_mask: c_ulong,
    pub blue_mask: c_ulong,
    pub colormap_size: c_int,
    pub bits_per_rgb: c_int,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetVisualInfo")]
pub unsafe extern "sysv64" fn XGetVisualInfo(
    dpy: *mut Display,
    _vinfo_mask: c_long,
    _vinfo_template: *mut XVisualInfo,
    nitems_return: *mut c_int,
) -> *mut XVisualInfo {
    if !nitems_return.is_null() {
        unsafe { *nitems_return = 1 };
    }
    let vinfo = Box::new(XVisualInfo {
        visual: crate::XDefaultVisual(dpy, 0),
        visualid: 1,
        screen: 0,
        depth: 24,
        class: 4, // TrueColor
        red_mask: 0x00ff0000,
        green_mask: 0x0000ff00,
        blue_mask: 0x000000ff,
        colormap_size: 256,
        bits_per_rgb: 8,
    });
    Box::into_raw(vinfo)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XMatchVisualInfo")]
pub unsafe extern "sysv64" fn XMatchVisualInfo(
    dpy: *mut Display,
    _screen: c_int,
    _depth: c_int,
    _class: c_int,
    vinfo_return: *mut XVisualInfo,
) -> Status {
    if !vinfo_return.is_null() {
        unsafe {
            (*vinfo_return).visual = crate::XDefaultVisual(dpy, 0);
            (*vinfo_return).visualid = 1;
            (*vinfo_return).screen = 0;
            (*vinfo_return).depth = 24;
            (*vinfo_return).class = 4;
            (*vinfo_return).red_mask = 0x00ff0000;
            (*vinfo_return).green_mask = 0x0000ff00;
            (*vinfo_return).blue_mask = 0x000000ff;
            (*vinfo_return).colormap_size = 256;
            (*vinfo_return).bits_per_rgb = 8;
        }
        1
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XVisualIDFromVisual")]
pub unsafe extern "sysv64" fn XVisualIDFromVisual(visual: *mut Visual) -> usize {
    if !visual.is_null() {
        unsafe { (*visual).visualid }
    } else {
        1
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XListDepths")]
pub unsafe extern "sysv64" fn XListDepths(
    _dpy: *mut Display,
    _screen_number: c_int,
    count_return: *mut c_int,
) -> *mut c_int {
    if !count_return.is_null() {
        unsafe { *count_return = 1 };
    }
    static mut DEPTHS: [c_int; 1] = [24];
    unsafe { &raw mut DEPTHS as *mut c_int }
}

// ---------------------------------------------------------------------------
// Error Handling
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XGetErrorDatabaseText")]
pub unsafe extern "sysv64" fn XGetErrorDatabaseText(
    _dpy: *mut Display,
    _name: *const c_char,
    _message: *const c_char,
    default_string: *const c_char,
    buffer_return: *mut c_char,
    length: c_int,
) -> c_int {
    if !buffer_return.is_null() && length > 0 {
        let src = if !default_string.is_null() {
            default_string
        } else {
            b"X error\0".as_ptr() as *const c_char
        };
        let len = unsafe { CStr::from_ptr(src) }.to_bytes().len() + 1;
        let copy_len = len.min(length as usize);
        unsafe {
            core::ptr::copy_nonoverlapping(src, buffer_return, copy_len);
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XLoadFont")]
pub unsafe extern "sysv64" fn XLoadFont(_dpy: *mut Display, _name: *const c_char) -> usize {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUnloadFont")]
pub unsafe extern "sysv64" fn XUnloadFont(_dpy: *mut Display, _font: usize) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XQueryFont")]
pub unsafe extern "sysv64" fn XQueryFont(_dpy: *mut Display, _font_ID: usize) -> *mut c_void {
    static mut DUMMY_FONT_STRUCT: [usize; 64] = [0; 64];
    unsafe { &raw mut DUMMY_FONT_STRUCT as *mut c_void }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XChangeWindowAttributes")]
pub unsafe extern "sysv64" fn XChangeWindowAttributes(
    dpy: *mut Display,
    w: Window,
    valuemask: c_ulong,
    attributes: *mut crate::XSetWindowAttributes,
) -> c_int {
    unsafe { crate::issue_request(dpy) };
    if valuemask & (1 << 11) != 0 && !attributes.is_null() {
        let result = unsafe { crate::XSelectInput(dpy, w, (*attributes).event_mask) };
        if result != 1 {
            return result;
        }
    }
    if valuemask & 1 != 0 && !attributes.is_null() {
        unsafe {
            crate::graphics::background::XSetWindowBackgroundPixmap(
                dpy,
                w,
                (*attributes).background_pixmap,
            );
        }
    }
    if valuemask & (1 << 1) != 0 && !attributes.is_null() {
        crate::graphics::set_window_background(w, unsafe { (*attributes).background_pixel });
    }
    if valuemask & (1 << 13) != 0 && !attributes.is_null() {
        if unsafe { crate::colormap::XSetWindowColormap(dpy, w, (*attributes).colormap) } == 0 {
            return 0;
        }
    }
    // `override_redirect` asks the X server to bypass the window manager.  It is
    // commonly one part of a fullscreen fallback, but it does not itself select
    // a monitor or prescribe geometry; those remain separate X requests.
    const CW_OVERRIDE_REDIRECT: c_ulong = 1 << 9;
    if valuemask & CW_OVERRIDE_REDIRECT != 0 && !attributes.is_null() {
        kinakaze_libdisplay::ui::set_override_redirect(w, unsafe {
            (*attributes).override_redirect != 0
        });
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDisplayOfScreen")]
pub unsafe extern "sysv64" fn XDisplayOfScreen(screen: *mut crate::Screen) -> *mut Display {
    if screen.is_null() {
        core::ptr::null_mut()
    } else {
        unsafe { (*screen).display }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDisplayString")]
pub unsafe extern "sysv64" fn XDisplayString(_dpy: *mut Display) -> *const c_char {
    c":0.0".as_ptr()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeFont")]
pub unsafe extern "sysv64" fn XFreeFont(_dpy: *mut Display, _font_struct: *mut c_void) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetDefault")]
pub unsafe extern "sysv64" fn XGetDefault(
    _dpy: *mut Display,
    _program: *const c_char,
    _option: *const c_char,
) -> *mut c_char {
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XLoadQueryFont")]
pub unsafe extern "sysv64" fn XLoadQueryFont(
    _dpy: *mut Display,
    _name: *const c_char,
) -> *mut c_void {
    static mut DUMMY_FONT: [usize; 64] = [0; 64];
    unsafe { &raw mut DUMMY_FONT as *mut c_void }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XNextRequest")]
pub unsafe extern "sysv64" fn XNextRequest(dpy: *mut Display) -> c_ulong {
    if !dpy.is_null() {
        unsafe { (*dpy).request.wrapping_add(1) as c_ulong }
    } else {
        1
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRefreshKeyboardMapping")]
pub unsafe extern "sysv64" fn XRefreshKeyboardMapping(_event_map: *mut c_void) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XResourceManagerString")]
pub unsafe extern "sysv64" fn XResourceManagerString(_dpy: *mut Display) -> *mut c_char {
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XScreenResourceString")]
pub unsafe extern "sysv64" fn XScreenResourceString(_screen: *mut crate::Screen) -> *mut c_char {
    core::ptr::null_mut()
}

/// Applies a `_NET_WM_STATE` client message: action 0 removes, 1 adds and 2
/// toggles each named state, and the window's `_NET_WM_STATE` property is
/// updated as the window manager would.  States without a native counterpart
/// are refused rather than recorded.
unsafe fn change_net_wm_state(
    dpy: *mut Display,
    window: Window,
    action: c_long,
    atoms: [c_long; 2],
) -> Status {
    use crate::property::wm_support::set_state;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, IsIconic, IsWindow, IsZoomed, WS_VISIBLE,
    };
    if !(0..=2).contains(&action) || unsafe { IsWindow(window as HWND) } == 0 {
        return 0;
    }
    // EWMH: the message is for mapped windows; before mapping, the client
    // sets the property itself and ShowWindow must not map it here.
    let mapped = unsafe { GetWindowLongPtrW(window as HWND, GWL_STYLE) } & WS_VISIBLE as isize != 0;
    let mut maximize: Option<bool> = None;
    for atom in atoms.into_iter().filter(|atom| *atom != 0) {
        let atom = atom as usize;
        let apply = |current: bool| match action {
            0 => false,
            1 => true,
            _ => !current,
        };
        if crate::property::is_named(atom, b"_NET_WM_STATE_FULLSCREEN") {
            let enabled = apply(kinakaze_libdisplay::ui::is_fullscreen(window));
            if !kinakaze_libdisplay::ui::set_fullscreen(window, enabled) {
                return 0;
            }
            set_state(dpy, window, b"_NET_WM_STATE_FULLSCREEN", enabled);
        } else if crate::property::is_named(atom, b"_NET_WM_STATE_MAXIMIZED_VERT")
            || crate::property::is_named(atom, b"_NET_WM_STATE_MAXIMIZED_HORZ")
        {
            // Win32 maximizes in both directions; the two atoms travel together.
            if mapped {
                maximize = Some(apply(unsafe { IsZoomed(window as HWND) } != 0));
            }
        } else if crate::property::is_named(atom, b"_NET_WM_STATE_HIDDEN") {
            let iconic = unsafe { IsIconic(window as HWND) } != 0;
            let hidden = apply(iconic);
            if mapped && hidden != iconic {
                unsafe {
                    ShowWindow(
                        window as HWND,
                        if hidden { SW_MINIMIZE } else { SW_RESTORE },
                    )
                };
            }
        } else {
            if crate::trace_enabled() {
                crate::diagnostic!("[libX11] _NET_WM_STATE atom {atom} has no native state");
            }
            return 0;
        }
    }
    if let Some(maximized) = maximize {
        // The property follows the native state change the window reports.
        unsafe {
            ShowWindow(
                window as HWND,
                if maximized { SW_MAXIMIZE } else { SW_RESTORE },
            )
        };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSendEvent")]
pub unsafe extern "sysv64" fn XSendEvent(
    _dpy: *mut Display,
    _w: Window,
    _propagate: Bool,
    _event_mask: c_long,
    event_send: *mut XEvent,
) -> Status {
    unsafe { crate::issue_request(_dpy) };
    if event_send.is_null() {
        return 0;
    }
    if crate::frame_sync::trace_enabled() && unsafe { (*event_send).r#type } == crate::ClientMessage
    {
        let client = unsafe { (*event_send).xclient };
        crate::frame_sync::trace(format_args!(
            "client target={_w:#x} window={:#x} drawn={} timings={} data={:?}",
            client.window,
            crate::property::is_named(client.message_type, b"_NET_WM_FRAME_DRAWN"),
            crate::property::is_named(client.message_type, b"_NET_WM_FRAME_TIMINGS"),
            client.data
        ));
    }

    if _event_mask < 0 || _event_mask & !0x01ff_ffff != 0 {
        unsafe {
            crate::errors::report(_dpy, 2, 25, 0, _event_mask as usize);
        }
        return 0;
    }
    if !crate::shared::valid(_w) {
        unsafe {
            crate::errors::report(_dpy, 3, 25, 0, _w);
        }
        return 0;
    }
    if crate::input_trace_enabled() && unsafe { (*event_send).r#type } == crate::ClientMessage {
        let client = unsafe { (*event_send).xclient };
        if crate::property::is_named(client.message_type, b"_NET_WM_MOVERESIZE") {
            crate::diagnostic!(
                "[input-moveresize] at={} window={:#x} data={:?}",
                crate::focus::now(),
                client.window,
                client.data
            );
        }
    }
    if _w == 1 && crate::shared::recipients(1, 1 << 20).is_empty() {
        let event = unsafe { *event_send };
        if unsafe { event.r#type } == crate::ClientMessage {
            let client = unsafe { event.xclient };
            if crate::trace_enabled() {
                crate::diagnostic!(
                    "[libX11] XSendEvent ClientMessage to={_w:#x} window={:#x} type={} data={:?}",
                    client.window,
                    client.message_type,
                    client.data
                );
            }
            if _w == 1 && client.format == 32 {
                if crate::property::is_named(client.message_type, b"_NET_ACTIVE_WINDOW") {
                    return c_int::from(kinakaze_libdisplay::ui::interaction::raise(
                        client.window,
                        true,
                    ));
                }
                if crate::property::is_named(client.message_type, b"_NET_WM_MOVERESIZE") {
                    // WM_NORMAL_HINTS: flags, x, y, width, height, min w/h, max w/h,
                    // ...; PMinSize is bit 4 and PMaxSize bit 5.
                    let mut limits = kinakaze_libdisplay::ui::interaction::SizeLimits::default();
                    if let Some(hints) = crate::property::words(client.window, 40)
                        && hints.len() >= 9
                    {
                        if hints[0] & 16 != 0 {
                            limits.min_width = hints[5] as i32;
                            limits.min_height = hints[6] as i32;
                        }
                        if hints[0] & 32 != 0 {
                            limits.max_width = hints[7] as i32;
                            limits.max_height = hints[8] as i32;
                        }
                    }
                    return c_int::from(kinakaze_libdisplay::ui::interaction::move_resize(
                        client.window,
                        client.data[0] as i32,
                        client.data[1] as i32,
                        client.data[2] as i32,
                        limits,
                    ));
                }
            }
            if client.format == 32
                && crate::property::is_named(client.message_type, b"_NET_CLOSE_WINDOW")
            {
                // The window manager asks the client to close; that is the native
                // close request, which reaches the client as WM_DELETE_WINDOW.
                return c_int::from(kinakaze_libdisplay::ui::request_close(client.window));
            }
            if client.format == 32
                && crate::property::is_named(client.message_type, b"_NET_WM_STATE")
            {
                return unsafe {
                    change_net_wm_state(
                        _dpy,
                        client.window,
                        client.data[0],
                        [client.data[1], client.data[2]],
                    )
                };
            }
        }
    }
    let mut wire = [0u8; 32];
    if unsafe { crate::event_wire::_XEventToWire(_dpy, event_send, wire.as_mut_ptr()) } == 0 {
        return 0;
    }
    wire[0] |= 0x80;
    if _event_mask == 0 {
        return crate::shared::window_owner(_w).is_some_and(|pid| crate::shared::send(pid, wire))
            as i32;
    }
    let mut window = _w;
    loop {
        let recipients = crate::shared::recipients(window, _event_mask as u32);
        if !recipients.is_empty() {
            for pid in recipients {
                crate::shared::send_delivery(pid, wire, Some((window, _event_mask as u32)));
            }
            break;
        }
        if _propagate == 0 || window == 1 {
            break;
        }
        window = crate::shared::hierarchy(window)
            .and_then(|h| h.0)
            .unwrap_or(1);
    }
    return 1;
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWindowBackground")]
pub unsafe extern "sysv64" fn XSetWindowBackground(
    display: *mut Display,
    window: Window,
    pixel: c_ulong,
) -> c_int {
    if kinakaze_libdisplay::window::logical_for_native(window).is_none() {
        unsafe {
            crate::errors::report(display, 3, 2, 0, window);
        }
        return 0;
    }
    crate::graphics::set_window_background(window, pixel);
    1
}
