//! The UI thread: the one thread that owns every window and pumps its messages.
//!
//! ## Why a dedicated thread, rather than the caller's
//!
//! Win32 windows are thread-affine in a way that has no Linux counterpart. A window
//! only ever receives messages on the thread that created it, `GetMessage` only
//! retrieves from the *calling* thread's queue, and a thread that stops pumping
//! freezes every window it owns — Windows marks them unresponsive and eventually
//! draws the ghost overlay.
//!
//! A guest does none of that. It creates a window from `main`, then blocks on a
//! Vulkan fence, then draws, and never calls anything resembling a message pump. On
//! Linux that is fine, because an X11 or Wayland connection is a socket and events
//! queue in the kernel until read. On Windows it means the window is dead on
//! arrival.
//!
//! So this library owns a thread whose entire job is to create windows and pump
//! them. Creation is marshalled to it, messages are translated to Linux-shaped
//! events on it, and the guest reads those events from anywhere at any time. The
//! guest never has to know a pump exists.
//!
//! ## How marshalling works, and why not a channel
//!
//! The obvious design is a command channel plus a reply channel. It needs a way to
//! wake a thread blocked in `GetMessage`, which means `PostThreadMessage` anyway,
//! and then two synchronisation mechanisms exist where one would do.
//!
//! Instead there is a hidden *controller* window. `SendMessageW` to a window on
//! another thread blocks until that thread's message loop dispatches it and returns
//! the window procedure's value — Windows already implements exactly the
//! synchronous call-across-threads this needs, including the wake-up. A request
//! travels as a pointer in `lParam`, valid for the duration of the blocking call,
//! and the reply is the return value. No channels, no reply plumbing, and the
//! blocking is the operating system's rather than ours.

pub mod barriers;
pub mod decorations;
pub mod desktop;
pub mod input_only;
pub mod input_shape;
pub mod interaction;
pub mod pointer_grab;
pub mod presentation;
mod reparent;
mod scroll;
mod taskbar;
mod thumbnails;
mod touch;
pub mod transient;
static ROOT_CURSOR_HIDDEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub fn hide_root_cursor(hidden: bool) {
    ROOT_CURSOR_HIDDEN.store(hidden, std::sync::atomic::Ordering::Release);
}
pub fn reparent(window: usize, parent: usize, x: i32, y: i32) -> bool {
    reparent::apply(window, parent, x, y)
}

use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::mpsc;

use crate::event::{
    BTN_EXTRA, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE, EVENT_CLOSE, EVENT_FOCUS, EVENT_KEY,
    EVENT_POINTER_BUTTON, EVENT_POINTER_MOTION, EVENT_RAW_BUTTON, EVENT_RAW_KEY, EVENT_RAW_MOTION,
    EVENT_RESIZE, EVENT_SCROLL, Event, STATE_PRESSED, STATE_RELEASED, STATE_REPEAT, publish,
    timestamp,
};
use crate::keycode;

// ---------------------------------------------------------------------------
// Win32 declarations.
//
// Declared inline rather than pulled from `windows-sys` to keep this crate
// dependency-free, which matters because it is loaded into a guest process whose
// allocator and TLS are already this project's own. Every signature matches the
// Windows headers: `BOOL` is `i32`, `WPARAM`/`LPARAM` are pointer-width, and
// `LRESULT` is signed pointer-width.
// ---------------------------------------------------------------------------

type Handle = *mut c_void;
type WindowProcedure = unsafe extern "system" fn(Handle, u32, usize, isize) -> isize;

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn ScreenToClient(window: Handle, point: *mut Point) -> i32;
    fn GetKeyState(key: i32) -> i16;
    fn ClientToScreen(window: Handle, point: *mut Point) -> i32;
    fn SetCursorPos(x: i32, y: i32) -> i32;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn IsIconic(window: Handle) -> i32;
}

#[repr(C)]
struct WindowClass {
    size: u32,
    style: u32,
    procedure: Option<WindowProcedure>,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    small_icon: Handle,
}

#[repr(C)]
#[derive(Default)]
struct Message {
    window: Handle,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    point_x: i32,
    point_y: i32,
    private: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}

#[derive(Clone, Copy)]
struct FullscreenRestore {
    style: isize,
    bounds: Rect,
}

#[derive(Clone, Copy)]
struct OverrideRedirectRestore {
    style: isize,
    extended_style: isize,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    rc_paint: Rect,
    restore: i32,
    inc_update: i32,
    rgb_reserved: [u8; 32],
}

/// The first field of Win32's `CREATESTRUCTW`.  That is the only field needed to
/// recover the per-window value passed to `CreateWindowExW`.
#[repr(C)]
struct CreateStruct {
    create_params: *const c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawInputDevice {
    usage_page: u16,
    usage: u16,
    flags: u32,
    target: Handle,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawInputHeader {
    kind: u32,
    size: u32,
    device: Handle,
    w_param: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawMouse {
    flags: u16,
    _align: u16,
    buttons: u32,
    raw_buttons: u32,
    last_x: i32,
    last_y: i32,
    extra_information: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawKeyboard {
    make_code: u16,
    flags: u16,
    reserved: u16,
    virtual_key: u16,
    message: u32,
    extra_information: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
union RawInputData {
    mouse: RawMouse,
    keyboard: RawKeyboard,
    _hid: [u8; 24],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawInput {
    header: RawInputHeader,
    data: RawInputData,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WindowClass) -> u16;
    fn CreateWindowExW(
        extended_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        parameter: *mut c_void,
    ) -> Handle;
    fn DestroyWindow(window: Handle) -> i32;
    fn DefWindowProcW(window: Handle, message: u32, w_param: usize, l_param: isize) -> isize;
    fn GetMessageW(message: *mut Message, window: Handle, first: u32, last: u32) -> i32;
    fn TranslateMessage(message: *const Message) -> i32;
    fn DispatchMessageW(message: *const Message) -> isize;
    fn SendMessageW(window: Handle, message: u32, w_param: usize, l_param: isize) -> isize;
    fn PostMessageW(window: Handle, message: u32, w_param: usize, l_param: isize) -> i32;
    fn ShowWindow(window: Handle, command: i32) -> i32;
    fn UpdateWindow(window: Handle) -> i32;
    fn SetForegroundWindow(window: Handle) -> i32;
    fn SetFocus(window: Handle) -> Handle;
    fn GetFocus() -> Handle;
    fn GetAncestor(window: Handle, flags: u32) -> Handle;
    fn BringWindowToTop(window: Handle) -> i32;
    fn SetWindowTextW(window: Handle, text: *const u16) -> i32;
    fn GetClientRect(window: Handle, rect: *mut Rect) -> i32;
    fn GetWindowRect(window: Handle, rect: *mut Rect) -> i32;
    fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, extended_style: u32) -> i32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn SetCursor(cursor: Handle) -> Handle;
    fn GetCursor() -> Handle;
    fn CopyIcon(icon: Handle) -> Handle;
    fn DestroyIcon(icon: Handle) -> i32;
    fn PostQuitMessage(code: i32);
    fn BeginPaint(window: Handle, paint: *mut PaintStruct) -> Handle;
    fn EndPaint(window: Handle, paint: *const PaintStruct) -> i32;
    fn InvalidateRect(window: Handle, rect: *const Rect, erase: i32) -> i32;
    fn GetWindowLongPtrW(window: Handle, index: i32) -> isize;
    fn SetWindowLongPtrW(window: Handle, index: i32, value: isize) -> isize;
    fn SetWindowPos(
        window: Handle,
        insert_after: Handle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    fn MonitorFromWindow(window: Handle, flags: u32) -> Handle;
    fn GetMonitorInfoW(monitor: Handle, info: *mut MonitorInfo) -> i32;
    fn RegisterRawInputDevices(devices: *const RawInputDevice, count: u32, size: u32) -> i32;
    fn GetRawInputData(
        input: Handle,
        command: u32,
        data: *mut c_void,
        size: *mut u32,
        header_size: u32,
    ) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn GetLastError() -> u32;
}

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CLIPCHILDREN: u32 = 0x0200_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const CW_USEDEFAULT: i32 = i32::MIN;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const IDC_ARROW: *const u16 = 32512 as *const u16;
const IDC_IBEAM: *const u16 = 32513 as *const u16;
const IDC_CROSS: *const u16 = 32515 as *const u16;
const IDC_SIZENWSE: *const u16 = 32642 as *const u16;
const IDC_SIZENESW: *const u16 = 32643 as *const u16;
const IDC_SIZEWE: *const u16 = 32644 as *const u16;
const IDC_SIZENS: *const u16 = 32645 as *const u16;
const IDC_SIZEALL: *const u16 = 32646 as *const u16;
const IDC_NO: *const u16 = 32648 as *const u16;
const IDC_HAND: *const u16 = 32649 as *const u16;
const CS_OWNDC: u32 = 0x0020;

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETCURSOR: u32 = 0x0020;
const WM_NCCREATE: u32 = 0x0081;
const WM_INPUT: u32 = 0x00FF;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_XBUTTONDOWN: u32 = 0x020B;
const WM_XBUTTONUP: u32 = 0x020C;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;

const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPE_MOUSE: u32 = 0;
const RIM_TYPE_KEYBOARD: u32 = 1;
const RI_KEY_BREAK: u16 = 0x0001;
const RI_KEY_E0: u16 = 0x0002;
const MOUSE_MOVE_ABSOLUTE: u16 = 0x0001;
const RI_MOUSE_LEFT_BUTTON_DOWN: u16 = 0x0001;
const RI_MOUSE_LEFT_BUTTON_UP: u16 = 0x0002;
const RI_MOUSE_RIGHT_BUTTON_DOWN: u16 = 0x0004;
const RI_MOUSE_RIGHT_BUTTON_UP: u16 = 0x0008;
const RI_MOUSE_MIDDLE_BUTTON_DOWN: u16 = 0x0010;
const RI_MOUSE_MIDDLE_BUTTON_UP: u16 = 0x0020;
const RI_MOUSE_BUTTON_4_DOWN: u16 = 0x0040;
const RI_MOUSE_BUTTON_4_UP: u16 = 0x0080;
const RI_MOUSE_BUTTON_5_DOWN: u16 = 0x0100;
const RI_MOUSE_BUTTON_5_UP: u16 = 0x0200;

/// One notch of the wheel, as Windows counts it.
const WHEEL_DELTA: i32 = 120;

/// Our own messages, sent to the controller window to marshal work.
///
/// `WM_USER` is the first value Windows reserves for an application, and these are
/// only ever sent to a window class this library registered, so no collision is
/// possible.
const WM_CREATE_WINDOW: u32 = 0x0400;
const WM_DESTROY_WINDOW: u32 = 0x0401;
const WM_SET_TITLE: u32 = 0x0402;
const WM_SET_VISIBLE: u32 = 0x0403;
const WM_SET_FOCUS: u32 = 0x040b;
const WM_SET_CURSOR_VISIBLE: u32 = 0x0404;
const WM_SET_FULLSCREEN: u32 = 0x0405;
const WM_SET_OVERRIDE_REDIRECT: u32 = 0x0406;
const WM_SET_CURSOR_SHAPE: u32 = 0x0407;
const GWLP_USERDATA: i32 = -21;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const CURSOR_VISIBLE_OFFSET: i32 = 0;
const CURSOR_SHAPE_OFFSET: i32 = core::mem::size_of::<isize>() as i32;
const CURSOR_IMAGE_OFFSET: i32 = (2 * core::mem::size_of::<isize>()) as i32;
/// Non-zero while the window is minimized, so a restore can be reported once.
const ICONIC_OFFSET: i32 = (3 * core::mem::size_of::<isize>()) as i32;
/// Non-zero while the pointer is inside the client area, for crossing events.
const POINTER_INSIDE_OFFSET: i32 = (4 * core::mem::size_of::<isize>()) as i32;
/// Non-zero while the window is maximized, so each change is reported once.
const MAXIMIZED_OFFSET: i32 = (5 * core::mem::size_of::<isize>()) as i32;
const SIZE_MINIMIZED: usize = 1;
const SIZE_MAXIMIZED: usize = 2;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TME_LEAVE: u32 = 0x0000_0002;

#[repr(C)]
struct TrackMouseEventRequest {
    size: u32,
    flags: u32,
    track: Handle,
    hover_time: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn TrackMouseEvent(request: *mut TrackMouseEventRequest) -> i32;
}

/// Reports the pointer crossing into or out of the client area, as X11's
/// EnterNotify/LeaveNotify do; Win32 only reports the leave, and only after
/// `TrackMouseEvent` asked for it.
unsafe fn track_crossing(
    window: Handle,
    logical_window: u64,
    x: i32,
    y: i32,
    inside: bool,
    at: u64,
) {
    let was_inside = unsafe { GetWindowLongPtrW(window, POINTER_INSIDE_OFFSET) } != 0;
    if inside == was_inside {
        return;
    }
    unsafe { SetWindowLongPtrW(window, POINTER_INSIDE_OFFSET, isize::from(inside)) };
    if inside {
        let mut request = TrackMouseEventRequest {
            size: core::mem::size_of::<TrackMouseEventRequest>() as u32,
            flags: TME_LEAVE,
            track: window,
            hover_time: 0,
        };
        unsafe { TrackMouseEvent(&raw mut request) };
    }
    if logical_window != 0 {
        publish(Event {
            kind: if inside {
                crate::event::EVENT_POINTER_ENTER
            } else {
                crate::event::EVENT_POINTER_LEAVE
            },
            x,
            y,
            time_ms: at,
            window: logical_window,
            ..Event::default()
        });
    }
}
const WM_SET_CURSOR_IMAGE: u32 = 0x040a;
const HTCLIENT: usize = 1;
const MONITOR_DEFAULTTONEAREST: u32 = 2;
const WS_EX_TOPMOST: isize = 0x0000_0008;
const HWND_TOPMOST: isize = -1;
const HWND_NOTOPMOST: isize = -2;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_FRAMECHANGED: u32 = 0x0020;
const SWP_NOOWNERZORDER: u32 = 0x0200;

/// The class name for guest windows.
const WINDOW_CLASS: &str = "kinakaze.display.window";
/// The class name for the hidden controller window.
const CONTROLLER_CLASS: &str = "kinakaze.display.controller";

/// What `WM_CREATE_WINDOW` carries in `lParam`.
///
/// Lives on the *sending* thread's stack, which is sound because `SendMessageW`
/// does not return until the receiving thread has finished with it.
#[repr(C)]
struct CreateRequest {
    /// Parent HWND, or null for a top-level window.
    parent: Handle,
    /// Parent-client coordinates for a child window.
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    /// NUL-terminated UTF-16, owned by the sender for the call's duration.
    title: *const u16,
    /// Stable Linux-visible window id; zero for raw UI-only windows.
    logical_window: u64,
    visible: bool,
}

/// What `WM_SET_TITLE` carries.
#[repr(C)]
struct TitleRequest {
    window: Handle,
    title: *const u16,
}

/// Converts a Rust string to the NUL-terminated UTF-16 Win32 wants.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Portable system cursor shapes used by the Linux-facing window adapters.
///
/// The variants intentionally describe appearance instead of exposing Win32
/// `HCURSOR` values.  X11, Wayland and other guests can therefore request the
/// same policy without acquiring a dependency on Windows types.
#[repr(usize)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CursorShape {
    #[default]
    Arrow,
    IBeam,
    Crosshair,
    Hand,
    ResizeHorizontal,
    ResizeVertical,
    ResizeNorthwestSoutheast,
    ResizeNortheastSouthwest,
    ResizeAll,
    NotAllowed,
}

fn system_cursor(shape: CursorShape) -> Handle {
    let resource = match shape {
        CursorShape::Arrow => IDC_ARROW,
        CursorShape::IBeam => IDC_IBEAM,
        CursorShape::Crosshair => IDC_CROSS,
        CursorShape::Hand => IDC_HAND,
        CursorShape::ResizeHorizontal => IDC_SIZEWE,
        CursorShape::ResizeVertical => IDC_SIZENS,
        CursorShape::ResizeNorthwestSoutheast => IDC_SIZENWSE,
        CursorShape::ResizeNortheastSouthwest => IDC_SIZENESW,
        CursorShape::ResizeAll => IDC_SIZEALL,
        CursorShape::NotAllowed => IDC_NO,
    };
    // SAFETY: every resource is a predefined shared system cursor and a null
    // instance is the documented way to load it. `LoadCursorW` does not allocate
    // a cursor that the caller must destroy.
    unsafe { LoadCursorW(core::ptr::null_mut(), resource) }
}

fn fullscreen_windows() -> &'static Mutex<HashMap<usize, FullscreenRestore>> {
    static WINDOWS: OnceLock<Mutex<HashMap<usize, FullscreenRestore>>> = OnceLock::new();
    WINDOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn override_redirect_windows() -> &'static Mutex<HashMap<usize, OverrideRedirectRestore>> {
    static WINDOWS: OnceLock<Mutex<HashMap<usize, OverrideRedirectRestore>>> = OnceLock::new();
    WINDOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Applies X11's `override_redirect` policy without inventing a fullscreen
/// transition.  An override-redirect window is not decorated or positioned by
/// the window manager, but its geometry remains under the client's control.
/// Keep the drawable size stable while changing the Win32 non-client frame;
/// later XMove/XResize requests then determine the actual fullscreen geometry.
unsafe fn apply_override_redirect(window: Handle, override_redirect: bool) -> bool {
    let Ok(mut windows) = override_redirect_windows().lock() else {
        return false;
    };
    if override_redirect == windows.contains_key(&(window as usize)) {
        return true;
    }

    let mut client = Rect::default();
    let mut bounds = Rect::default();
    if unsafe { GetClientRect(window, &raw mut client) } == 0
        || unsafe { GetWindowRect(window, &raw mut bounds) } == 0
    {
        return false;
    }
    let current_style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) };
    let current_extended_style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
    let (style, extended_style, insert_after) = if override_redirect {
        windows.insert(
            window as usize,
            OverrideRedirectRestore {
                style: current_style,
                extended_style: current_extended_style,
            },
        );
        (
            current_style & !(WS_OVERLAPPEDWINDOW as isize),
            current_extended_style | WS_EX_TOPMOST,
            HWND_TOPMOST as Handle,
        )
    } else {
        let restore = windows
            .remove(&(window as usize))
            .unwrap_or(OverrideRedirectRestore {
                style: current_style,
                extended_style: current_extended_style,
            });
        let insert_after = if restore.extended_style & WS_EX_TOPMOST != 0 {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        (
            restore.style,
            restore.extended_style,
            insert_after as Handle,
        )
    };
    drop(windows);
    desktop::mark_override_redirect(window, override_redirect);
    unsafe { SetWindowLongPtrW(window, GWL_STYLE, style) };
    unsafe { SetWindowLongPtrW(window, GWL_EXSTYLE, extended_style) };

    let mut adjusted = Rect {
        left: 0,
        top: 0,
        right: client.right - client.left,
        bottom: client.bottom - client.top,
    };
    if unsafe { AdjustWindowRectEx(&raw mut adjusted, style as u32, 0, extended_style as u32) } == 0
    {
        return false;
    }
    unsafe {
        SetWindowPos(
            window,
            insert_after,
            bounds.left,
            bounds.top,
            adjusted.right - adjusted.left,
            adjusted.bottom - adjusted.top,
            SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
        ) != 0
    }
}

unsafe fn apply_fullscreen(window: Handle, fullscreen: bool) -> bool {
    let Ok(mut windows) = fullscreen_windows().lock() else {
        return false;
    };
    if fullscreen {
        if windows.contains_key(&(window as usize)) {
            return true;
        }
        let mut bounds = Rect::default();
        if unsafe { GetWindowRect(window, &raw mut bounds) } == 0 {
            return false;
        }
        let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
        let mut info = MonitorInfo {
            size: core::mem::size_of::<MonitorInfo>() as u32,
            monitor: Rect::default(),
            work: Rect::default(),
            flags: 0,
        };
        if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &raw mut info) } == 0 {
            return false;
        }
        let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) };
        windows.insert(window as usize, FullscreenRestore { style, bounds });
        drop(windows);
        unsafe {
            SetWindowLongPtrW(window, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW as isize));
        }
        let monitor = info.monitor;
        unsafe {
            SetWindowPos(
                window,
                core::ptr::null_mut(),
                monitor.left,
                monitor.top,
                monitor.right - monitor.left,
                monitor.bottom - monitor.top,
                SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
            ) != 0
        }
    } else {
        let Some(restore) = windows.remove(&(window as usize)) else {
            return true;
        };
        drop(windows);
        unsafe { SetWindowLongPtrW(window, GWL_STYLE, restore.style) };
        unsafe {
            SetWindowPos(
                window,
                core::ptr::null_mut(),
                restore.bounds.left,
                restore.bounds.top,
                restore.bounds.right - restore.bounds.left,
                restore.bounds.bottom - restore.bounds.top,
                SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
            ) != 0
        }
    }
}

unsafe fn apply_window_cursor(window: Handle) {
    let visible = !ROOT_CURSOR_HIDDEN.load(std::sync::atomic::Ordering::Acquire)
        && unsafe { GetWindowLongPtrW(window, CURSOR_VISIBLE_OFFSET) != 0 };
    let image = unsafe { GetWindowLongPtrW(window, CURSOR_IMAGE_OFFSET) as Handle };
    let cursor = if visible {
        if !image.is_null() {
            unsafe { SetCursor(image) };
            return;
        }
        let shape = unsafe { GetWindowLongPtrW(window, CURSOR_SHAPE_OFFSET) as usize };
        system_cursor(match shape {
            1 => CursorShape::IBeam,
            2 => CursorShape::Crosshair,
            3 => CursorShape::Hand,
            4 => CursorShape::ResizeHorizontal,
            5 => CursorShape::ResizeVertical,
            6 => CursorShape::ResizeNorthwestSoutheast,
            7 => CursorShape::ResizeNortheastSouthwest,
            8 => CursorShape::ResizeAll,
            9 => CursorShape::NotAllowed,
            _ => CursorShape::Arrow,
        })
    } else {
        core::ptr::null_mut()
    };
    // SAFETY: a null cursor is the documented way for a window procedure to
    // leave the client-area cursor hidden.
    unsafe { SetCursor(cursor) };
}

/// The started UI thread's controller window.
struct Controller {
    window: usize,
}

// SAFETY: the handle is only ever passed back to Win32, which is what owns the
// window's thread affinity. It is never dereferenced here.
unsafe impl Send for Controller {}
unsafe impl Sync for Controller {}

/// Starts the UI thread once and returns its controller window.
///
/// Returns `None` if the thread could not register its classes or create the
/// controller, which is the only honest answer: without a pumping thread no window
/// created here would ever respond, so reporting success would produce a window
/// that appears and then hangs.
fn controller() -> Option<Handle> {
    static CONTROLLER: OnceLock<Option<Controller>> = OnceLock::new();
    let started = CONTROLLER.get_or_init(|| {
        let (ready, started) = mpsc::channel();
        // Detached deliberately: the thread lives as long as the process, and there
        // is no later moment at which every window is known to be gone.
        std::thread::Builder::new()
            .name("kinakaze-display-ui".to_owned())
            .spawn(move || run(ready))
            .ok()?;
        // Blocks until the thread has its classes registered and its controller
        // created, so that a `SendMessageW` immediately after this cannot race.
        let window = started.recv().ok()?;
        (window != 0).then_some(Controller { window })
    });
    started.as_ref().map(|c| c.window as Handle)
}

/// The UI thread body: register, create the controller, pump forever.
fn run(ready: mpsc::Sender<usize>) {
    // SAFETY: passing null asks for this module's own base address.
    let instance = unsafe { GetModuleHandleW(core::ptr::null()) };

    let window_class = wide(WINDOW_CLASS);
    let controller_class = wide(CONTROLLER_CLASS);
    // SAFETY: `IDC_ARROW` is a predefined cursor id, which is passed as an integer
    // rather than a string; a null instance selects the system cursors.
    let cursor = unsafe { LoadCursorW(core::ptr::null_mut(), IDC_ARROW) };

    let guest = WindowClass {
        size: core::mem::size_of::<WindowClass>() as u32,
        // WGL requires the pixel-formatted DC to remain stable for the lifetime
        // of the window.  The rendering thread may use an owned DC even though
        // the window itself was created by the UI thread.
        style: CS_OWNDC,
        procedure: Some(window_procedure),
        class_extra: 0,
        // HWND-local slots hold cursor visibility, shape, image, the
        // minimized, pointer-inside and maximized flags.  Independent guest
        // windows therefore cannot disturb one another through Win32's
        // process-wide ShowCursor counter or class-level cursor.
        window_extra: (6 * core::mem::size_of::<isize>()) as i32,
        instance,
        icon: core::ptr::null_mut(),
        cursor,
        // No background brush: the driver owns every pixel, and letting Windows
        // erase would produce a flash of the brush colour on every resize.
        background: core::ptr::null_mut(),
        menu_name: core::ptr::null(),
        class_name: window_class.as_ptr(),
        small_icon: core::ptr::null_mut(),
    };
    let controller = WindowClass {
        size: core::mem::size_of::<WindowClass>() as u32,
        style: 0,
        procedure: Some(controller_procedure),
        class_extra: 0,
        window_extra: 0,
        instance,
        icon: core::ptr::null_mut(),
        cursor: core::ptr::null_mut(),
        background: core::ptr::null_mut(),
        menu_name: core::ptr::null(),
        class_name: controller_class.as_ptr(),
        small_icon: core::ptr::null_mut(),
    };

    // SAFETY: both descriptors are live and fully populated, with `size` set.
    let registered = unsafe {
        RegisterClassExW(&raw const guest) != 0 && RegisterClassExW(&raw const controller) != 0
    };
    if !registered {
        let _ = ready.send(0);
        return;
    }

    // A message-only window: `HWND_MESSAGE` as the parent means it is never shown,
    // never activated and never enumerated, but still receives sent messages.
    const HWND_MESSAGE: isize = -3;
    // SAFETY: the class was just registered; the name is live for this call.
    let window = unsafe {
        CreateWindowExW(
            0,
            controller_class.as_ptr(),
            core::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE as Handle,
            core::ptr::null_mut(),
            instance,
            core::ptr::null_mut(),
        )
    };
    if window.is_null() {
        let _ = ready.send(0);
        return;
    }

    // Register mouse and keyboard as generic-desktop Raw Input devices.  Keeping
    // the target null preserves normal foreground routing: the focused guest HWND
    // receives WM_INPUT alongside the legacy messages, without suppressing either
    // path.  XInput2 consumes the raw records while core X11 keeps using legacy
    // messages for portable applications.
    let raw_devices = [
        RawInputDevice {
            usage_page: 0x01,
            usage: 0x02,
            flags: 0,
            target: core::ptr::null_mut(),
        },
        RawInputDevice {
            usage_page: 0x01,
            usage: 0x06,
            flags: 0,
            target: core::ptr::null_mut(),
        },
    ];
    let raw_registered = unsafe {
        RegisterRawInputDevices(
            raw_devices.as_ptr(),
            raw_devices.len() as u32,
            core::mem::size_of::<RawInputDevice>() as u32,
        )
    };
    if raw_registered == 0 && std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        eprintln!("[libdisplay] RegisterRawInputDevices failed: {}", unsafe {
            GetLastError()
        });
    }
    if ready.send(window as usize).is_err() {
        return;
    }

    let mut message = Message::default();
    // SAFETY: `message` is a live, writable `MSG` for every iteration.
    while unsafe { GetMessageW(&raw mut message, core::ptr::null_mut(), 0, 0) } > 0 {
        // SAFETY: `message` was just filled by `GetMessageW`.
        unsafe {
            TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

/// The controller window's procedure: performs marshalled work.
///
/// Every branch runs on the UI thread, which is the entire point — each of these
/// calls would be invalid, or would create an unpumped window, on any other.
unsafe extern "system" fn controller_procedure(
    window: Handle,
    message: u32,
    w_param: usize,
    l_param: isize,
) -> isize {
    match message {
        reparent::MESSAGE => unsafe { reparent::dispatch(l_param) },
        interaction::CAPTURE => {
            isize::from(unsafe { interaction::capture_native(w_param, l_param != 0) })
        }
        WM_SET_FOCUS => unsafe {
            let target = w_param as Handle;
            if !target.is_null() {
                taskbar::refresh(GetAncestor(target, 2));
                SetForegroundWindow(GetAncestor(target, 2));
            }
            SetFocus(target);
            (GetFocus() == target) as isize
        },
        WM_CREATE_WINDOW => {
            // SAFETY: the sender is blocked in `SendMessageW`, so its stack-allocated
            // request is still live.
            let request = unsafe { &*(l_param as *const CreateRequest) };
            create_on_this_thread(request) as isize
        }
        WM_DESTROY_WINDOW => {
            // SAFETY: the guest promises a handle from `create`.
            unsafe { DestroyWindow(w_param as Handle) as isize }
        }
        WM_SET_TITLE => {
            // SAFETY: as `WM_CREATE_WINDOW`.
            let request = unsafe { &*(l_param as *const TitleRequest) };
            // SAFETY: the title is live for the blocking call.
            unsafe { SetWindowTextW(request.window, request.title) as isize }
        }
        WM_SET_VISIBLE => {
            let activate = activates_on_map(w_param);
            if l_param != 0 && !unsafe { taskbar::window_hidden(w_param as Handle) }.is_null() {
                return 1;
            }
            presentation::visibility(w_param, l_param != 0);
            let command = if l_param == 0 {
                SW_HIDE
            } else if activate {
                SW_SHOW
            } else {
                4
            };
            // SAFETY: the guest promises a handle from `create`.
            unsafe {
                if l_param != 0 {
                    transient::position_on_show(w_param);
                    taskbar::refresh(w_param as Handle);
                }
                ShowWindow(w_param as Handle, command);
                if l_param != 0 {
                    UpdateWindow(w_param as Handle);
                    if activate {
                        interaction::raise_native(w_param as Handle, true);
                    }
                }
                if desktop::is_desktop(w_param) {
                    // Apps may already have activated while Shell was starting.
                    taskbar::activate(w_param as Handle, false);
                }
                1
            }
        }
        // SAFETY: the default handler is always valid for an unhandled message.
        _ => unsafe { DefWindowProcW(window, message, w_param, l_param) },
    }
}

fn activates_on_map(window: usize) -> bool {
    !input_only::is_input_only(window)
        && (desktop::is_desktop(window) || !is_override_redirect(window))
        && unsafe { GetWindowLongPtrW(window as Handle, GWL_STYLE) } & WS_CHILD as isize == 0
}

/// Creates a guest window. Must run on the UI thread.
fn create_on_this_thread(request: &CreateRequest) -> usize {
    let class = wide(WINDOW_CLASS);
    // SAFETY: null asks for this module's base address.
    let instance = unsafe { GetModuleHandleW(core::ptr::null()) };

    let is_child = !request.parent.is_null();
    let style = (if is_child {
        WS_CHILD
    } else {
        WS_OVERLAPPEDWINDOW
    }) | WS_CLIPCHILDREN
        | WS_CLIPSIBLINGS;
    let mut frame = Rect {
        left: 0,
        top: 0,
        right: request.width,
        bottom: request.height,
    };
    if !is_child {
        // SAFETY: `frame` is a live, writable RECT. Child dimensions already name
        // the drawable rectangle and have no non-client frame to account for.
        unsafe { AdjustWindowRectEx(&raw mut frame, style, 0, 0) };
    }

    // SAFETY: the class is registered and every pointer is live for the call.
    let window = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            request.title,
            style,
            if is_child {
                request.x
            } else if request.logical_window != 0 {
                request.x + frame.left
            } else {
                CW_USEDEFAULT
            },
            if is_child {
                request.y
            } else if request.logical_window != 0 {
                request.y + frame.top
            } else {
                CW_USEDEFAULT
            },
            frame.right - frame.left,
            frame.bottom - frame.top,
            request.parent,
            core::ptr::null_mut(),
            instance,
            (&raw const request.logical_window).cast_mut().cast(),
        )
    };
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        let tid = unsafe { GetCurrentThreadId() };
        eprintln!(
            "[libdisplay] CreateWindowExW thread={tid} HWND={window:?} error={}",
            unsafe { GetLastError() }
        );
    }
    if !window.is_null() && !is_child {
        taskbar::refresh(window);
    }
    if !window.is_null() && request.visible {
        unsafe {
            ShowWindow(window, SW_SHOW);
            UpdateWindow(window);
            if !is_child {
                SetForegroundWindow(window);
                BringWindowToTop(window);
            }
        }
    }
    window as usize
}

fn input_modifiers() -> u32 {
    let mut modifiers = 0;
    for (key, bit) in [(0x10, 0), (0x11, 2), (0x12, 3), (0x5b, 6), (0x5c, 6)] {
        if unsafe { GetKeyState(key) } < 0 {
            modifiers |= 1 << bit;
        }
    }
    for (key, bit) in [(0x14, 1), (0x90, 4), (0x91, 5)] {
        if unsafe { GetKeyState(key) } & 1 != 0 {
            modifiers |= 1 << bit;
        }
    }
    modifiers
}

/// A guest window's procedure: translates messages into Linux-shaped events.
unsafe extern "system" fn window_procedure(
    window: Handle,
    message: u32,
    w_param: usize,
    l_param: isize,
) -> isize {
    if message == WM_NCCREATE && l_param != 0 {
        // Window extra bytes are zero-initialized; X11's default cursor is
        // visible and zero names `CursorShape::Arrow`, so establish visibility
        // before the first WM_SETCURSOR.
        unsafe { SetWindowLongPtrW(window, CURSOR_VISIBLE_OFFSET, 1) };
        // SAFETY: for WM_NCCREATE lParam is a live CREATESTRUCTW for the duration
        // of this call. Its first member is the pointer supplied as
        // CreateWindowExW's final argument.
        let create = unsafe { &*(l_param as *const CreateStruct) };
        if !create.create_params.is_null() {
            let logical_window = unsafe { *create.create_params.cast::<u64>() };
            unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, logical_window as isize) };
        }
    }
    let forwarded = message == pointer_grab::FORWARDED_INPUT;
    if !forwarded && pointer_grab::forward(window, message, w_param, l_param) {
        return 0;
    }
    let forwarded_modifiers = (w_param >> 48) as u32;
    let message = if forwarded {
        ((w_param >> 32) & 0xffff) as u32
    } else {
        message
    };
    let w_param = if forwarded {
        w_param as u32 as usize
    } else {
        w_param
    };
    let at = timestamp();
    // This value belongs to the HWND itself. Simultaneous creation or traffic on
    // another window therefore cannot relabel this message.
    let logical_window = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as u64 };
    let modifiers = if forwarded {
        Some(forwarded_modifiers)
    } else if matches!(
        message,
        WM_KEYDOWN
            | WM_KEYUP
            | WM_SYSKEYDOWN
            | WM_SYSKEYUP
            | WM_MOUSEMOVE
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_RBUTTONDOWN
            | WM_RBUTTONUP
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
            | WM_XBUTTONDOWN
            | WM_XBUTTONUP
            | WM_MOUSEWHEEL
            | WM_MOUSEHWHEEL
    ) {
        // GetKeyState follows this UI thread's message order. GetAsyncKeyState
        // on the guest thread observes a later release when rendering is busy.
        Some(input_modifiers())
    } else {
        None
    };
    let publish = |event| {
        if let Some(modifiers) = modifiers {
            crate::event::publish_with_modifiers(event, modifiers);
        } else {
            publish(event);
        }
    };
    match message {
        0x0006 => {
            taskbar::activate(window, w_param & 0xffff != 0);
            unsafe { DefWindowProcW(window, message, w_param, l_param) }
        }
        WM_SET_FOCUS => unsafe {
            taskbar::refresh(GetAncestor(window, 2));
            SetForegroundWindow(GetAncestor(window, 2));
            SetFocus(window);
            (GetFocus() == window) as isize
        },
        // An X11 popup or grab window must not steal its application's keyboard focus.
        0x0021
            if !activates_on_map(window as usize)
                && unsafe { GetWindowLongPtrW(window, GWL_STYLE) } & WS_CHILD as isize == 0 =>
        {
            3
        }
        transient::MESSAGE => isize::from(unsafe { transient::apply(window, w_param) }),
        0x245..=0x247
            if touch::handle(
                window,
                message,
                (w_param & 0xffff) as u32,
                logical_window,
                at,
            ) =>
        {
            0
        }
        0x0046 => {
            unsafe {
                desktop::positioning(window, l_param);
            }
            unsafe { DefWindowProcW(window, message, w_param, l_param) }
        }
        0x0047 => {
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
            if l_param != 0
                && logical_window != 0
                && unsafe { (*(l_param as *const WindowPosition)).flags } & SWP_NOZORDER == 0
            {
                publish(Event {
                    kind: crate::event::EVENT_STACK,
                    time_ms: at,
                    window: logical_window,
                    ..Event::default()
                });
            }
            // DefWindowProc still generates WM_MOVE and WM_SIZE.
            unsafe { DefWindowProcW(window, message, w_param, l_param) }
        }
        0x00a1
            if (10..=17).contains(&w_param) && decorations::has_client_frame(window as usize) =>
        {
            interaction::native_resize(
                window as usize,
                signed_low(l_param),
                signed_high(l_param),
                w_param,
            );
            0
        }
        decorations::MESSAGE => isize::from(unsafe { decorations::apply(window, w_param != 0) }),
        decorations::CUSTOM_SHAPE => {
            decorations::apply_custom_shape(window as usize, w_param != 0);
            0
        }
        interaction::RAISE => {
            isize::from(unsafe { interaction::raise_native(window, w_param != 0) })
        }
        interaction::MOVE_RESIZE => isize::from(unsafe { interaction::begin_drag(l_param) }),
        interaction::RELEASE_SOURCE => {
            if !pointer_grab::source_active(window as usize) {
                interaction::forget(window as usize);
            }
            0
        }
        interaction::GRAB_CROSSING => {
            let mut point = Point { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&raw mut point);
                ScreenToClient(window, &raw mut point);
            }
            let mut buttons = 0;
            for (key, bit) in [(1, 1), (4, 2), (2, 3), (5, 8), (6, 9)] {
                if unsafe { GetKeyState(key) } < 0 {
                    buttons |= 1 << bit;
                }
            }
            publish(Event {
                kind: if w_param == 1 {
                    crate::event::EVENT_POINTER_LEAVE
                } else {
                    crate::event::EVENT_POINTER_ENTER
                },
                state: w_param as u32,
                button: buttons,
                x: point.x,
                y: point.y,
                time_ms: at,
                window: logical_window,
                ..Event::default()
            });
            0
        }
        // WM_CANCELMODE ends any drag; WM_CAPTURECHANGED does too unless the
        // capture merely moved to this same window (SetCapture on the holder).
        0x001f => {
            interaction::forget(window as usize);
            0
        }
        0x0215 => {
            if l_param as usize != window as usize {
                interaction::forget(window as usize);
            }
            0
        }
        WM_SET_CURSOR_VISIBLE => {
            let visible = w_param != 0;
            unsafe {
                SetWindowLongPtrW(window, CURSOR_VISIBLE_OFFSET, isize::from(visible));
                apply_window_cursor(window);
            }
            1
        }
        WM_SET_CURSOR_SHAPE => {
            unsafe {
                let previous = SetWindowLongPtrW(window, CURSOR_IMAGE_OFFSET, 0) as Handle;
                SetWindowLongPtrW(window, CURSOR_SHAPE_OFFSET, w_param as isize);
                apply_window_cursor(window);
                if !previous.is_null() {
                    DestroyIcon(previous);
                }
            }
            1
        }
        WM_SET_CURSOR_IMAGE => {
            // Copy on the owner thread: the window keeps its cursor after the
            // client's XFreeCursor and releases it on replacement/destruction.
            let copy = unsafe { CopyIcon(w_param as Handle) };
            if copy.is_null() {
                return 0;
            }
            unsafe {
                let previous =
                    SetWindowLongPtrW(window, CURSOR_IMAGE_OFFSET, copy as isize) as Handle;
                apply_window_cursor(window);
                if !previous.is_null() {
                    DestroyIcon(previous);
                }
            }
            1
        }
        WM_SET_FULLSCREEN => isize::from(unsafe { apply_fullscreen(window, w_param != 0) }),
        WM_SET_OVERRIDE_REDIRECT => {
            isize::from(unsafe { apply_override_redirect(window, w_param != 0) })
        }
        WM_SETCURSOR if (l_param as usize & 0xffff) == HTCLIENT => {
            // Windows sends WM_SETCURSOR again on mouse movement. Handling it
            // here prevents the class arrow from reappearing after a hide.
            unsafe { apply_window_cursor(window) };
            1
        }
        WM_CLOSE => {
            publish(Event {
                kind: EVENT_CLOSE,
                time_ms: at,
                window: logical_window,
                ..Event::default()
            });
            // Reported, not obeyed.  This is the Win32 side of
            // WM_DELETE_WINDOW: the Linux client decides whether to destroy the
            // window or keep it alive (for example to show an unsaved-work
            // prompt).  Terminating here would kill the whole loader from the UI
            // thread and bypass guest cleanup.
            0
        }
        WM_ERASEBKGND => 1,
        0x004a => unsafe { thumbnails::receive(window, l_param) },
        0x0018 => {
            // A hidden window can lose its presented contents while retaining
            // the same drawable size. Showing it is not a resize notification.
            if w_param != 0 {
                unsafe {
                    PostMessageW(window, 0x84e1, 0, 0);
                }
            }
            unsafe { DefWindowProcW(window, message, w_param, l_param) }
        }
        0x84e1 => {
            // Run after ShowWindow's synchronous positioning/restoration has
            // committed. Painting inside WM_SHOWWINDOW can be overwritten by
            // Windows restoring its saved client bits later in that same call.
            unsafe {
                InvalidateRect(window, core::ptr::null(), 0);
            }
            0
        }
        WM_PAINT => {
            unsafe {
                let mut ps: PaintStruct = core::mem::zeroed();
                BeginPaint(window, &mut ps);
                let painted = presentation::paint(window as usize, ps.hdc as usize, ps.rc_paint);
                EndPaint(window, &ps);
                // Validating the Win32 region does not repaint the Linux
                // drawable. Wake its event loop even if its size is unchanged.
                if logical_window != 0
                    && ps.rc_paint.right > ps.rc_paint.left
                    && ps.rc_paint.bottom > ps.rc_paint.top
                {
                    publish(Event {
                        kind: crate::event::EVENT_EXPOSE,
                        state: u32::from(painted),
                        x: ps.rc_paint.left,
                        y: ps.rc_paint.top,
                        scroll_x: ps.rc_paint.right - ps.rc_paint.left,
                        scroll_y: ps.rc_paint.bottom - ps.rc_paint.top,
                        time_ms: at,
                        window: logical_window,
                        ..Event::default()
                    });
                }
            }
            0
        }
        0x0003 => {
            // WM_MOVE reports the client origin, including for CSD windows.
            // A minimized window is parked at -32000,-32000; that is not a move
            // the X11 client can observe.
            if logical_window != 0 && unsafe { IsIconic(window) } == 0 {
                publish(Event {
                    kind: crate::event::EVENT_MOVE,
                    x: signed_low(l_param),
                    y: signed_high(l_param),
                    time_ms: at,
                    window: logical_window,
                    ..Event::default()
                });
            }
            0
        }
        WM_SIZE => {
            // Minimizing reports a 0x0 client area; X11 clients see an unmap
            // instead, and a map again when the window is restored.
            let was_iconic = unsafe { GetWindowLongPtrW(window, ICONIC_OFFSET) } != 0;
            let iconic = w_param == SIZE_MINIMIZED;
            if iconic != was_iconic {
                unsafe { SetWindowLongPtrW(window, ICONIC_OFFSET, isize::from(iconic)) };
                if !iconic {
                    unsafe {
                        PostMessageW(window, 0x84e1, 0, 0);
                    }
                }
                if logical_window != 0 {
                    publish(Event {
                        kind: crate::event::EVENT_ICONIC,
                        state: u32::from(iconic),
                        time_ms: at,
                        window: logical_window,
                        ..Event::default()
                    });
                }
            }
            let was_maximized = unsafe { GetWindowLongPtrW(window, MAXIMIZED_OFFSET) } != 0;
            let maximized = w_param == SIZE_MAXIMIZED;
            if !iconic && maximized != was_maximized {
                unsafe { SetWindowLongPtrW(window, MAXIMIZED_OFFSET, isize::from(maximized)) };
                if logical_window != 0 {
                    publish(Event {
                        kind: crate::event::EVENT_MAXIMIZED,
                        state: u32::from(maximized),
                        time_ms: at,
                        window: logical_window,
                        ..Event::default()
                    });
                }
            }
            decorations::refresh_shape(window as usize);
            // The low and high words of `lParam` are the new client size, which is
            // already the drawable size — no adjustment needed on this path.
            let width = (l_param & 0xffff) as i32;
            let height = ((l_param >> 16) & 0xffff) as i32;
            presentation::resized(window as usize, width, height);
            if !iconic && width > 0 && height > 0 && logical_window != 0 {
                publish(Event {
                    kind: EVENT_RESIZE,
                    x: width,
                    y: height,
                    time_ms: at,
                    window: logical_window,
                    ..Event::default()
                });
            }
            0
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            let released = message == WM_KEYUP || message == WM_SYSKEYUP;
            // Bit 30 of `lParam` is the previous key state: set means the key was
            // already down, which is an auto-repeat. Linux reports that as value 2,
            // so the distinction survives instead of being flattened to a press.
            let repeat = !released && (l_param & (1 << 30)) != 0;
            let scancode = ((l_param >> 16) & 0xff) as u16;
            let extended = (l_param & (1 << 24)) != 0;
            let code = keycode::translate(scancode, extended, w_param as u16);
            if code != keycode::KEY_RESERVED {
                publish(Event {
                    kind: EVENT_KEY,
                    state: if released {
                        STATE_RELEASED
                    } else if repeat {
                        STATE_REPEAT
                    } else {
                        STATE_PRESSED
                    },
                    keycode: code,
                    time_ms: at,
                    window: logical_window,
                    ..Event::default()
                });
            }
            // System keys still go to the default handler, or Alt+F4 and the window
            // menu stop working — behaviour a user expects from any window.
            if message == WM_SYSKEYDOWN || message == WM_SYSKEYUP {
                // SAFETY: the default handler is valid for these messages.
                return unsafe { DefWindowProcW(window, message, w_param, l_param) };
            }
            0
        }
        WM_INPUT => {
            let mut raw: RawInput = unsafe { core::mem::zeroed() };
            let mut size = core::mem::size_of::<RawInput>() as u32;
            let read = unsafe {
                GetRawInputData(
                    l_param as Handle,
                    RID_INPUT,
                    (&raw mut raw).cast(),
                    &raw mut size,
                    core::mem::size_of::<RawInputHeader>() as u32,
                )
            };
            if read != u32::MAX && logical_window != 0 {
                match raw.header.kind {
                    RIM_TYPE_MOUSE => {
                        let mouse = unsafe { raw.data.mouse };
                        // Relative devices provide the unaccelerated counts XI2
                        // promises. Absolute HID devices need desktop-coordinate
                        // normalization and are left to the core motion path.
                        if mouse.flags & MOUSE_MOVE_ABSOLUTE == 0
                            && (mouse.last_x != 0 || mouse.last_y != 0)
                        {
                            publish(Event {
                                kind: EVENT_RAW_MOTION,
                                x: mouse.last_x,
                                y: mouse.last_y,
                                time_ms: at,
                                window: logical_window,
                                ..Event::default()
                            });
                        }

                        let button_flags = mouse.buttons as u16;
                        for (flag, button, pressed) in [
                            (RI_MOUSE_LEFT_BUTTON_DOWN, BTN_LEFT, true),
                            (RI_MOUSE_LEFT_BUTTON_UP, BTN_LEFT, false),
                            (RI_MOUSE_RIGHT_BUTTON_DOWN, BTN_RIGHT, true),
                            (RI_MOUSE_RIGHT_BUTTON_UP, BTN_RIGHT, false),
                            (RI_MOUSE_MIDDLE_BUTTON_DOWN, BTN_MIDDLE, true),
                            (RI_MOUSE_MIDDLE_BUTTON_UP, BTN_MIDDLE, false),
                            (RI_MOUSE_BUTTON_4_DOWN, BTN_SIDE, true),
                            (RI_MOUSE_BUTTON_4_UP, BTN_SIDE, false),
                            (RI_MOUSE_BUTTON_5_DOWN, BTN_EXTRA, true),
                            (RI_MOUSE_BUTTON_5_UP, BTN_EXTRA, false),
                        ] {
                            if button_flags & flag != 0 {
                                publish(Event {
                                    kind: EVENT_RAW_BUTTON,
                                    state: if pressed {
                                        STATE_PRESSED
                                    } else {
                                        STATE_RELEASED
                                    },
                                    button,
                                    time_ms: at,
                                    window: logical_window,
                                    ..Event::default()
                                });
                            }
                        }
                    }
                    RIM_TYPE_KEYBOARD => {
                        let keyboard = unsafe { raw.data.keyboard };
                        let code = keycode::translate(
                            keyboard.make_code,
                            keyboard.flags & RI_KEY_E0 != 0,
                            keyboard.virtual_key,
                        );
                        if code != keycode::KEY_RESERVED {
                            publish(Event {
                                kind: EVENT_RAW_KEY,
                                state: if keyboard.flags & RI_KEY_BREAK != 0 {
                                    STATE_RELEASED
                                } else {
                                    STATE_PRESSED
                                },
                                keycode: code,
                                time_ms: at,
                                window: logical_window,
                                ..Event::default()
                            });
                        }
                    }
                    _ => {}
                }
            }
            // Windows requires foreground WM_INPUT messages to reach the default
            // handler so its internal cleanup can run.
            unsafe { DefWindowProcW(window, message, w_param, l_param) }
        }
        0x0084 => {
            let mut point = Point {
                x: signed_low(l_param),
                y: signed_high(l_param),
            };
            unsafe {
                ScreenToClient(window, &raw mut point);
            }
            if !input_shape::contains(window as usize, point.x, point.y)
                && !desktop::owns_surface(window)
            {
                -1
            } else {
                decorations::resize_hit(window as usize, signed_low(l_param), signed_high(l_param))
                    .unwrap_or_else(|| unsafe { DefWindowProcW(window, message, w_param, l_param) })
            }
        }
        WM_MOUSELEAVE => {
            // X11 crossing coordinates describe the actual pointer, even
            // outside the client. A fabricated (-1,-1) can trigger hot corners.
            let mut point = Point { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&raw mut point);
                ScreenToClient(window, &raw mut point);
                track_crossing(window, logical_window, point.x, point.y, false, at);
            }
            0
        }
        WM_MOUSEMOVE => {
            let mut point = Point {
                x: signed_low(l_param),
                y: signed_high(l_param),
            };
            unsafe {
                ClientToScreen(window, &raw mut point);
            }
            if interaction::motion(window as usize, point.x, point.y) {
                return 0;
            }
            let (x, y) = barriers::motion(point.x, point.y);
            if (x, y) != (point.x, point.y) {
                unsafe {
                    SetCursorPos(x, y);
                }
                point.x = x;
                point.y = y;
            }
            unsafe {
                ScreenToClient(window, &raw mut point);
            }
            // A captured window also hears about motion outside its client
            // area; that is not the pointer being inside it.
            if !forwarded {
                let mut client = Rect::default();
                let inside = unsafe { GetClientRect(window, &raw mut client) } != 0
                    && (0..client.right).contains(&point.x)
                    && (0..client.bottom).contains(&point.y);
                unsafe { track_crossing(window, logical_window, point.x, point.y, inside, at) };
            }
            publish(Event {
                kind: EVENT_POINTER_MOTION,
                x: point.x,
                y: point.y,
                time_ms: at,
                window: logical_window,
                ..Event::default()
            });
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let (button, pressed) = match message {
                WM_LBUTTONDOWN => (BTN_LEFT, true),
                WM_LBUTTONUP => (BTN_LEFT, false),
                WM_RBUTTONDOWN => (BTN_RIGHT, true),
                WM_RBUTTONUP => (BTN_RIGHT, false),
                WM_MBUTTONDOWN => (BTN_MIDDLE, true),
                WM_MBUTTONUP => (BTN_MIDDLE, false),
                // The X buttons report *which* in the high word of `wParam`.
                WM_XBUTTONDOWN | WM_XBUTTONUP => {
                    let which = (w_param >> 16) & 0xffff;
                    let button = if which == 2 { BTN_EXTRA } else { BTN_SIDE };
                    (button, message == WM_XBUTTONDOWN)
                }
                _ => unreachable!("the match arm lists exactly these messages"),
            };
            interaction::button(window as usize, pressed, w_param);
            publish(Event {
                kind: EVENT_POINTER_BUTTON,
                state: if pressed {
                    STATE_PRESSED
                } else {
                    STATE_RELEASED
                },
                button,
                x: signed_low(l_param),
                y: signed_high(l_param),
                time_ms: at,
                window: logical_window,
                ..Event::default()
            });
            0
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let horizontal = message == WM_MOUSEHWHEEL;
            let delta = scroll::accumulate(
                window as usize,
                horizontal,
                ((w_param >> 16) & 0xffff) as i16 as i32,
            );
            if delta != 0 {
                let mut point = Point {
                    x: signed_low(l_param),
                    y: signed_high(l_param),
                };
                unsafe { ScreenToClient(window, &raw mut point) };
                publish(Event {
                    kind: EVENT_SCROLL,
                    x: point.x,
                    y: point.y,
                    scroll_x: if horizontal { delta } else { 0 },
                    scroll_y: if horizontal { 0 } else { delta },
                    time_ms: at,
                    window: logical_window,
                    ..Event::default()
                });
            }
            0
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            publish(Event {
                kind: EVENT_FOCUS,
                state: u32::from(message == WM_SETFOCUS),
                time_ms: at,
                window: logical_window,
                ..Event::default()
            });
            0
        }
        WM_DESTROY => {
            thumbnails::clear(window as usize);
            transient::forget(window as usize);
            presentation::forget(window as usize);
            scroll::forget(window as usize);
            interaction::forget(window as usize);
            decorations::forget(window as usize);
            unsafe {
                let previous = SetWindowLongPtrW(window, CURSOR_IMAGE_OFFSET, 0) as Handle;
                if !previous.is_null() {
                    if GetCursor() == previous {
                        SetCursor(system_cursor(CursorShape::Arrow));
                    }
                    DestroyIcon(previous);
                }
            }
            if let Ok(mut windows) = fullscreen_windows().lock() {
                windows.remove(&(window as usize));
            }
            if let Ok(mut windows) = override_redirect_windows().lock() {
                windows.remove(&(window as usize));
            }
            0
        }
        // SAFETY: the default handler is always valid for an unhandled message.
        _ => unsafe { DefWindowProcW(window, message, w_param, l_param) },
    }
}

/// The low word of `lParam` as a signed coordinate.
///
/// Signed because the pointer can be dragged outside the client area, where
/// Windows reports negative coordinates. Reading it unsigned would turn a drag one
/// pixel off the left edge into 65535.
fn signed_low(value: isize) -> i32 {
    (value & 0xffff) as u16 as i16 as i32
}

/// The high word of `lParam` as a signed coordinate.
fn signed_high(value: isize) -> i32 {
    ((value >> 16) & 0xffff) as u16 as i16 as i32
}

// ---------------------------------------------------------------------------
// The operations the C ABI exposes, each marshalled to the UI thread.
// ---------------------------------------------------------------------------

/// Reports whether the UI thread is running.
pub fn available() -> bool {
    controller().is_some()
}

/// Creates a window with the given drawable size, and returns its `HWND`.
pub fn create(width: i32, height: i32, title: &str) -> Option<usize> {
    create_for_logical(0, width, height, title, true)
}

/// Creates an HWND whose messages carry `logical_window` back to the guest.
pub(crate) fn create_for_logical(
    logical_window: u64,
    width: i32,
    height: i32,
    title: &str,
    visible: bool,
) -> Option<usize> {
    create_for_logical_at(logical_window, 0, 0, width, height, title, visible)
}

pub(crate) fn create_for_logical_at(
    logical_window: u64,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    title: &str,
    visible: bool,
) -> Option<usize> {
    create_impl(
        logical_window,
        core::ptr::null_mut(),
        x,
        y,
        width,
        height,
        title,
        visible,
    )
}

/// Creates a child HWND. Its coordinates and size are relative to the parent's
/// client area, matching X11 child-window geometry.
pub(crate) fn create_child_for_logical(
    logical_window: u64,
    parent: usize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    title: &str,
    visible: bool,
) -> Option<usize> {
    if parent == 0 {
        return None;
    }
    create_impl(
        logical_window,
        parent as Handle,
        x,
        y,
        width,
        height,
        title,
        visible,
    )
}

fn create_impl(
    logical_window: u64,
    parent: Handle,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    title: &str,
    visible: bool,
) -> Option<usize> {
    let controller = controller()?;
    let title_wide = wide(title);
    let request = CreateRequest {
        parent,
        x,
        y,
        width,
        height,
        title: title_wide.as_ptr(),
        logical_window,
        visible,
    };
    let window =
        unsafe { SendMessageW(controller, WM_CREATE_WINDOW, 0, &raw const request as isize) };
    (window != 0).then_some(window as usize)
}

/// Destroys a window.
pub fn destroy(window: usize) -> bool {
    let Some(controller) = controller() else {
        return false;
    };
    unsafe { SendMessageW(controller, WM_DESTROY_WINDOW, window, 0) != 0 }
}

/// Sets a window's title.
pub fn set_title(window: usize, title: &str) -> bool {
    let Some(controller) = controller() else {
        return false;
    };
    let title_wide = wide(title);
    let request = TitleRequest {
        window: window as Handle,
        title: title_wide.as_ptr(),
    };
    unsafe { SendMessageW(controller, WM_SET_TITLE, 0, &raw const request as isize) != 0 }
}

/// Asks a window to close, as the frame's close button does; the guest hears
/// it as a close event and decides.
pub fn request_close(window: usize) -> bool {
    window != 0 && unsafe { PostMessageW(window as Handle, WM_CLOSE, 0, 0) != 0 }
}

/// Shows or hides a window.
pub fn set_visible(window: usize, visible: bool) -> bool {
    presentation::forget(window);
    let Some(controller) = controller() else {
        return false;
    };
    unsafe { SendMessageW(controller, WM_SET_VISIBLE, window, isize::from(visible)) != 0 }
}

/// Focus must be changed by the thread which owns the native window.
pub fn set_focus(window: usize) -> bool {
    let Some(controller) = controller() else {
        return false;
    };
    unsafe {
        SendMessageW(
            if window > 1 {
                window as Handle
            } else {
                controller
            },
            WM_SET_FOCUS,
            window,
            0,
        ) != 0
    }
}

/// Shows or hides the cursor while it is over a guest window's client area.
pub fn set_cursor_visible(window: usize, visible: bool) -> bool {
    if window == 0 {
        return false;
    }
    // A direct synchronous window message runs this operation on the owning UI
    // thread and immediately updates both the stored state and current cursor.
    unsafe {
        SendMessageW(
            window as Handle,
            WM_SET_CURSOR_VISIBLE,
            usize::from(visible),
            0,
        ) != 0
    }
}

/// Selects the system cursor shape for a guest window without changing whether
/// the cursor is currently visible.
pub fn set_cursor_shape(window: usize, shape: CursorShape) -> bool {
    if window == 0 {
        return false;
    }
    unsafe { SendMessageW(window as Handle, WM_SET_CURSOR_SHAPE, shape as usize, 0) != 0 }
}

/// Copies a native cursor into a window-owned resource on the UI thread.
pub fn set_cursor_image(window: usize, cursor: usize) -> bool {
    window != 0
        && cursor != 0
        && unsafe { SendMessageW(window as Handle, WM_SET_CURSOR_IMAGE, cursor, 0) != 0 }
}

/// Enters or leaves borderless fullscreen on the window's nearest monitor.
pub fn set_fullscreen(window: usize, fullscreen: bool) -> bool {
    if window == 0 {
        return false;
    }
    unsafe {
        SendMessageW(
            window as Handle,
            WM_SET_FULLSCREEN,
            usize::from(fullscreen),
            0,
        ) != 0
    }
}

pub fn is_fullscreen(window: usize) -> bool {
    fullscreen_windows()
        .lock()
        .ok()
        .is_some_and(|windows| windows.contains_key(&window))
}

/// Enables or disables X11 `override_redirect` semantics for a top-level
/// window.  This changes window-manager decoration/ownership only; it does not
/// choose a monitor or alter the application's requested geometry.
pub fn set_override_redirect(window: usize, override_redirect: bool) -> bool {
    if window == 0 {
        return false;
    }
    unsafe {
        SendMessageW(
            window as Handle,
            WM_SET_OVERRIDE_REDIRECT,
            usize::from(override_redirect),
            0,
        ) != 0
    }
}

pub fn is_override_redirect(window: usize) -> bool {
    override_redirect_windows()
        .lock()
        .ok()
        .is_some_and(|windows| windows.contains_key(&window))
}

/// A window's current drawable size.
///
/// Read directly rather than marshalled: `GetClientRect` only reads state Windows
/// maintains and is documented to work from any thread, so a round trip would add
/// latency for nothing.
pub fn client_size(window: usize) -> Option<(i32, i32)> {
    let mut rect = Rect::default();
    // SAFETY: `rect` is live and writable; the guest promises a live window.
    let ok = unsafe { GetClientRect(window as Handle, &raw mut rect) != 0 };
    ok.then(|| (rect.right - rect.left, rect.bottom - rect.top))
}

/// The `HINSTANCE` a Vulkan Win32 surface needs alongside the `HWND`.
pub fn module_instance() -> usize {
    // SAFETY: null asks for this module's own base address.
    unsafe { GetModuleHandleW(core::ptr::null()) as usize }
}

/// Asks the UI thread to stop pumping, for an orderly shutdown.
pub fn stop() {
    if let Some(controller) = controller() {
        // SAFETY: posting is valid from any thread and does not block.
        unsafe { PostMessageW(controller, WM_DESTROY, 0, 0) };
    }
}

/// Ends the message loop. Reached only through [`stop`].
#[allow(dead_code)]
fn quit() {
    // SAFETY: valid on the thread with the message loop.
    unsafe { PostQuitMessage(0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Coordinates must be read signed: dragging off the left or top edge yields
    /// negative values, which read unsigned become ~65535 and send a guest's
    /// pointer to the far corner.
    #[test]
    fn coordinates_are_signed() {
        // (10, 20)
        let positive = (20_isize << 16) | 10;
        assert_eq!(signed_low(positive), 10);
        assert_eq!(signed_high(positive), 20);

        // (-1, -5), as Windows packs them.
        let negative = ((-5_i32 as u16 as isize) << 16) | (-1_i32 as u16 as isize);
        assert_eq!(signed_low(negative), -1);
        assert_eq!(signed_high(negative), -5);
    }

    /// The UI thread must start, or every window this library creates would be
    /// unpumped and would hang.
    #[test]
    fn the_ui_thread_starts() {
        assert!(available(), "the UI thread should start on this platform");
    }

    /// A real window, created on the UI thread from this one — which is the whole
    /// point of the marshalling — with the client area the size that was asked for.
    #[test]
    fn a_created_window_has_the_requested_drawable_size() {
        assert!(available());
        let window = create(640, 480, "kinakaze test").expect("window creation");

        let (width, height) = client_size(window).expect("client size");
        // Exact, not approximate: `AdjustWindowRectEx` exists precisely so that the
        // client area is what was requested. An off-by-a-border result here would
        // mean every swapchain is built at the wrong size.
        assert_eq!((width, height), (640, 480));

        assert!(set_title(window, "renamed"));
        assert!(destroy(window));
    }

    #[test]
    fn cursor_visibility_is_local_to_each_window() {
        assert!(available());
        let first = create(160, 120, "cursor first").expect("first window");
        let second = create(160, 120, "cursor second").expect("second window");
        assert_eq!(
            unsafe { GetWindowLongPtrW(first as Handle, CURSOR_VISIBLE_OFFSET) },
            1
        );
        assert!(set_cursor_visible(first, false));
        assert_eq!(
            unsafe { GetWindowLongPtrW(first as Handle, CURSOR_VISIBLE_OFFSET) },
            0
        );
        assert_eq!(
            unsafe { GetWindowLongPtrW(second as Handle, CURSOR_VISIBLE_OFFSET) },
            1
        );
        assert!(set_cursor_visible(first, true));
        assert!(destroy(first));
        assert!(destroy(second));
    }

    #[test]
    fn cursor_shape_is_local_to_each_window() {
        assert!(available());
        let first = create(160, 120, "shape first").expect("first window");
        let second = create(160, 120, "shape second").expect("second window");
        assert!(set_cursor_shape(
            first,
            CursorShape::ResizeNortheastSouthwest
        ));
        assert_eq!(
            unsafe { GetWindowLongPtrW(first as Handle, CURSOR_SHAPE_OFFSET) },
            CursorShape::ResizeNortheastSouthwest as isize
        );
        assert_eq!(
            unsafe { GetWindowLongPtrW(second as Handle, CURSOR_SHAPE_OFFSET) },
            CursorShape::Arrow as isize
        );
        assert!(destroy(first));
        assert!(destroy(second));
    }

    #[test]
    fn override_redirect_roundtrip_restores_stacking_policy() {
        assert!(available());
        let window = create(320, 240, "override redirect roundtrip").expect("window");
        let style = unsafe { GetWindowLongPtrW(window as Handle, GWL_STYLE) };
        let extended_style = unsafe { GetWindowLongPtrW(window as Handle, GWL_EXSTYLE) };

        assert!(set_override_redirect(window, true));
        assert!(is_override_redirect(window));
        assert_eq!(
            unsafe { GetWindowLongPtrW(window as Handle, GWL_STYLE) }
                & WS_OVERLAPPEDWINDOW as isize,
            0
        );
        assert_ne!(
            unsafe { GetWindowLongPtrW(window as Handle, GWL_EXSTYLE) } & WS_EX_TOPMOST,
            0
        );

        assert!(set_override_redirect(window, false));
        assert!(!is_override_redirect(window));
        assert_eq!(
            unsafe { GetWindowLongPtrW(window as Handle, GWL_STYLE) },
            style
        );
        assert_eq!(
            unsafe { GetWindowLongPtrW(window as Handle, GWL_EXSTYLE) },
            extended_style
        );
        assert!(destroy(window));
    }

    #[test]
    fn fullscreen_roundtrip_restores_window_style_and_bounds() {
        assert!(available());
        let window = create(320, 240, "fullscreen roundtrip").expect("window");
        let mut before = Rect::default();
        assert_ne!(
            unsafe { GetWindowRect(window as Handle, &raw mut before) },
            0
        );
        let style = unsafe { GetWindowLongPtrW(window as Handle, GWL_STYLE) };

        assert!(set_fullscreen(window, true));
        assert!(is_fullscreen(window));
        assert_eq!(
            unsafe { GetWindowLongPtrW(window as Handle, GWL_STYLE) }
                & WS_OVERLAPPEDWINDOW as isize,
            0
        );

        assert!(set_fullscreen(window, false));
        assert!(!is_fullscreen(window));
        let mut after = Rect::default();
        assert_ne!(
            unsafe { GetWindowRect(window as Handle, &raw mut after) },
            0
        );
        assert_eq!(
            unsafe { GetWindowLongPtrW(window as Handle, GWL_STYLE) },
            style
        );
        assert_eq!(
            (after.left, after.top, after.right, after.bottom),
            (before.left, before.top, before.right, before.bottom)
        );
        assert!(destroy(window));
    }

    /// Two windows must be independent, and creating a second must not disturb the
    /// first.
    #[test]
    fn two_windows_coexist() {
        assert!(available());
        let first = create(320, 240, "first").expect("first window");
        let second = create(400, 300, "second").expect("second window");
        assert_ne!(first, second);
        assert_eq!(client_size(first), Some((320, 240)));
        assert_eq!(client_size(second), Some((400, 300)));
        assert!(destroy(first));
        assert!(destroy(second));
    }

    /// The `HINSTANCE` must be real: `vkCreateWin32SurfaceKHR` requires it, and a
    /// null one fails surface creation.
    #[test]
    fn the_module_instance_is_real() {
        assert_ne!(module_instance(), 0);
    }

    /// A destroyed window's size can no longer be read, which is how a guest that
    /// uses a stale handle finds out rather than drawing into nothing.
    #[test]
    fn a_destroyed_window_stops_answering() {
        assert!(available());
        let window = create(200, 100, "transient").expect("window creation");
        assert!(destroy(window));
        assert_eq!(client_size(window), None);
    }

    /// Win32 messages must retain the stable guest id and Linux input values all
    /// the way through the UI thread's event queue.  This covers the boundary that
    /// a native HWND smoke test alone cannot see.
    #[test]
    fn native_messages_reach_the_guest_event_queue() {
        let _queue_test = crate::event::test_queue_lock();
        let id = crate::window::create(320, 240, "message bridge").expect("window creation");
        let native = crate::window::native_handle(id).expect("native HWND mapping");
        assert_ne!(id as usize, native, "the guest id must not be the HWND");
        assert_eq!(crate::window::logical_for_native(native), Some(id));

        // Ignore creation/focus messages; every injected message below is
        // synchronous and therefore appears after this drain.
        if let Ok(mut queue) = crate::event::queue().lock() {
            while queue.pop().is_some() {}
        }

        let point = (34_isize << 16) | 12;
        unsafe {
            SendMessageW(native as Handle, WM_MOUSEMOVE, 0, point);
            SendMessageW(native as Handle, WM_LBUTTONDOWN, 1, point);
            SendMessageW(native as Handle, WM_LBUTTONUP, 0, point);
            SendMessageW(native as Handle, WM_KEYDOWN, 0x20, 0x39 << 16);
            SendMessageW(
                native as Handle,
                WM_KEYUP,
                0x20,
                (0x39 << 16) | (1_isize << 30) | (1_isize << 31),
            );
            SendMessageW(native as Handle, WM_MOUSEWHEEL, 120 << 16, point);
            SendMessageW(native as Handle, WM_SIZE, 0, (456_isize << 16) | 123);
            SendMessageW(native as Handle, WM_CLOSE, 0, 0);
        }

        let mut events = Vec::new();
        if let Ok(mut queue) = crate::event::queue().lock() {
            while let Some(event) = queue.pop() {
                // Showing a newly-created HWND can deliver a focus transition at
                // any point around these synchronous messages.  Focus is tested
                // by its own translation arm; it is not one of the messages this
                // test injected.
                if event.window == id && event.kind != EVENT_FOCUS {
                    events.push(event);
                }
            }
        }
        // Native HWNDs can receive real pointer/focus traffic while this test is
        // running.  Linux event queues allow that traffic to be interleaved too,
        // so require the injected sequence as an ordered subsequence instead of
        // pretending the test owns the desktop input queue.
        let wanted: [fn(&Event) -> bool; 8] = [
            |event: &Event| event.kind == EVENT_POINTER_MOTION && event.x == 12 && event.y == 34,
            |event: &Event| {
                event.kind == EVENT_POINTER_BUTTON
                    && event.button == BTN_LEFT
                    && event.state == STATE_PRESSED
            },
            |event: &Event| {
                event.kind == EVENT_POINTER_BUTTON
                    && event.button == BTN_LEFT
                    && event.state == STATE_RELEASED
            },
            |event: &Event| {
                event.kind == EVENT_KEY && event.keycode == 57 && event.state == STATE_PRESSED
            },
            |event: &Event| {
                event.kind == EVENT_KEY && event.keycode == 57 && event.state == STATE_RELEASED
            },
            |event: &Event| event.kind == EVENT_SCROLL && event.scroll_y == 1,
            |event: &Event| event.kind == EVENT_RESIZE && event.x == 123 && event.y == 456,
            |event: &Event| event.kind == EVENT_CLOSE,
        ];
        let mut matched = Vec::with_capacity(wanted.len());
        for event in events.iter().copied() {
            if wanted
                .get(matched.len())
                .is_some_and(|matches| matches(&event))
            {
                matched.push(event);
            }
        }
        assert_eq!(
            matched.len(),
            wanted.len(),
            "every injected message should become an ordered event: {events:?}"
        );
        let events = matched;
        assert_eq!(
            (events[0].kind, events[0].x, events[0].y),
            (EVENT_POINTER_MOTION, 12, 34)
        );
        assert_eq!(
            (events[1].kind, events[1].button, events[1].state),
            (EVENT_POINTER_BUTTON, BTN_LEFT, STATE_PRESSED)
        );
        assert_eq!(
            (events[2].kind, events[2].button, events[2].state),
            (EVENT_POINTER_BUTTON, BTN_LEFT, STATE_RELEASED)
        );
        assert_eq!(
            (events[3].kind, events[3].keycode, events[3].state),
            (EVENT_KEY, 57, STATE_PRESSED)
        );
        assert_eq!(
            (events[4].kind, events[4].keycode, events[4].state),
            (EVENT_KEY, 57, STATE_RELEASED)
        );
        assert_eq!((events[5].kind, events[5].scroll_y), (EVENT_SCROLL, 1));
        assert_eq!(
            (events[6].kind, events[6].x, events[6].y),
            (EVENT_RESIZE, 123, 456)
        );
        assert_eq!(events[7].kind, EVENT_CLOSE);

        // WM_CLOSE is advisory, just like WM_DELETE_WINDOW.  The guest window is
        // still live until the client explicitly destroys it.
        assert!(crate::window::client_size(id).is_some());
        assert!(crate::window::destroy(id));
    }
}
