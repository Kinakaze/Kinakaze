//! `libX11.dll`: native Win32 windowing & event translation for Linux X11/Xlib applications.
//!
//! Provides `libX11.so.6` ABI to Linux guest binaries without requiring an external
//! X server. X11 window creation (`XCreateWindow`, `XCreateSimpleWindow`) maps directly
//! to real Win32 `HWND` windows via `kinakaze_libdisplay`, and input events (`XNextEvent`,
//! `XPending`, `XLookupKeysym`) are translated seamlessly from Windows messages.

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code
)]

use core::ffi::{c_char, c_int, c_short, c_uchar, c_uint, c_ushort, c_void};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Diagnostic output must not unwind through a guest ABI when an inherited
/// native stderr handle is closed (for example, a desktop-launched process).
#[macro_export]
macro_rules! diagnostic {
    ($($args:tt)*) => {{
        let _ = std::io::Write::write_fmt(
            &mut std::io::stderr().lock(), format_args!("{}\n", format_args!($($args)*)));
    }};
}

use windows_sys::Win32::Foundation::{HWND, RECT};
#[cfg(test)]
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, IsWindow, SM_CXSCREEN, SM_CYSCREEN,
};

// These declarations implement the Linux x86_64 Xlib ABI while being compiled
// by a Windows Rust compiler.  Windows C `long` is 32-bit; Linux x86_64 C `long`
// is 64-bit.  Using `core::ffi::c_long` here shifts fields such as
// XConfigureEvent::width and makes a Linux client read zeros or adjacent fields.
type c_long = i64;
type c_ulong = u64;

pub type Window = usize;
pub type Drawable = usize;
pub type Font = usize;
pub type Pixmap = usize;
pub type Cursor = usize;
pub type Colormap = usize;
pub type GContext = usize;
pub type KeySym = usize;
pub type Atom = usize;
pub type Time = c_ulong;
pub type Bool = c_int;
pub type Status = c_int;

pub const True: Bool = 1;
pub const False: Bool = 0;
pub const None: usize = 0;

pub const KeyPress: c_int = 2;
pub const KeyRelease: c_int = 3;
pub const ButtonPress: c_int = 4;
pub const ButtonRelease: c_int = 5;
pub const MotionNotify: c_int = 6;
pub const EnterNotify: c_int = 7;
pub const LeaveNotify: c_int = 8;
pub const FocusIn: c_int = 9;
pub const FocusOut: c_int = 10;
pub const KeymapNotify: c_int = 11;
pub const Expose: c_int = 12;
pub const GraphicsExpose: c_int = 13;
pub const NoExpose: c_int = 14;
pub const VisibilityNotify: c_int = 15;
pub const CreateNotify: c_int = 16;
pub const DestroyNotify: c_int = 17;
pub const UnmapNotify: c_int = 18;
pub const MapNotify: c_int = 19;
pub const MapRequest: c_int = 20;
pub const ReparentNotify: c_int = 21;
pub const ConfigureNotify: c_int = 22;
pub const ConfigureRequest: c_int = 23;
pub const GravityNotify: c_int = 24;
pub const ResizeRequest: c_int = 25;
pub const CirculateNotify: c_int = 26;
pub const CirculateRequest: c_int = 27;
pub const PropertyNotify: c_int = 28;
pub const SelectionClear: c_int = 29;
pub const SelectionRequest: c_int = 30;
pub const SelectionNotify: c_int = 31;
pub const ColormapNotify: c_int = 32;
pub const ClientMessage: c_int = 33;
pub const MappingNotify: c_int = 34;
pub const GenericEvent: c_int = 35;

pub const XK_BackSpace: KeySym = 0xff08;
pub const XK_Tab: KeySym = 0xff09;
pub const XK_Return: KeySym = 0xff0d;
pub const XK_Escape: KeySym = 0xff1b;
pub const XK_Delete: KeySym = 0xffff;
pub const XK_Home: KeySym = 0xff50;
pub const XK_Left: KeySym = 0xff51;
pub const XK_Up: KeySym = 0xff52;
pub const XK_Right: KeySym = 0xff53;
pub const XK_Down: KeySym = 0xff54;
pub const XK_Page_Up: KeySym = 0xff55;
pub const XK_Page_Down: KeySym = 0xff56;
pub const XK_End: KeySym = 0xff57;
pub const XK_Insert: KeySym = 0xff63;
pub const XK_F1: KeySym = 0xffbe;
pub const XK_F2: KeySym = 0xffbf;
pub const XK_F3: KeySym = 0xffc0;
pub const XK_F4: KeySym = 0xffc1;
pub const XK_F5: KeySym = 0xffc2;
pub const XK_F6: KeySym = 0xffc3;
pub const XK_F7: KeySym = 0xffc4;
pub const XK_F8: KeySym = 0xffc5;
pub const XK_F9: KeySym = 0xffc6;
pub const XK_F10: KeySym = 0xffc7;
pub const XK_F11: KeySym = 0xffc8;
pub const XK_F12: KeySym = 0xffc9;
pub const XK_Shift_L: KeySym = 0xffe1;
pub const XK_Shift_R: KeySym = 0xffe2;
pub const XK_Control_L: KeySym = 0xffe3;
pub const XK_Control_R: KeySym = 0xffe4;
pub const XK_Caps_Lock: KeySym = 0xffe5;
pub const XK_Alt_L: KeySym = 0xffe9;
pub const XK_Alt_R: KeySym = 0xffea;
pub const XK_Super_L: KeySym = 0xffeb;
pub const XK_Super_R: KeySym = 0xffec;
pub const XK_Menu: KeySym = 0xff67;
pub const XK_space: KeySym = 0x20;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Visual {
    pub ext_data: *mut c_void,
    pub visualid: usize,
    pub class: c_int,
    pub red_mask: usize,
    pub green_mask: usize,
    pub blue_mask: usize,
    pub bits_per_rgb: c_int,
    pub map_entries: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Depth {
    pub depth: c_int,
    pub nvisuals: c_int,
    pub visuals: *mut Visual,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Screen {
    pub ext_data: *mut c_void,
    pub display: *mut Display,
    pub root: Window,
    pub width: c_int,
    pub height: c_int,
    pub mwidth: c_int,
    pub mheight: c_int,
    pub ndepths: c_int,
    pub depths: *mut Depth,
    pub root_depth: c_int,
    pub root_visual: *mut Visual,
    pub default_gc: *mut c_void,
    pub cmap: Colormap,
    pub white_pixel: usize,
    pub black_pixel: usize,
    pub max_maps: c_int,
    pub min_maps: c_int,
    pub backing_store: c_int,
    pub save_unders: Bool,
    pub root_input_mask: c_long,
}

#[repr(C)]
pub struct X11XCBPrivate {
    pub connection: *mut c_void,
    pub next_xid: u32,
    pub max_xid: u32,
    pub min_xid: u32,
    pub xid_mask: u32,
    pub xid_inc: u32,
    pub xid_base: u32,
    pub pad: [u8; 512],
}

#[repr(C)]
pub struct Display {
    pub ext_data: *mut c_void,
    pub free_funcs: *mut c_void,
    pub fd: c_int,
    pub lock: c_int,
    pub proto_major_version: c_int,
    pub proto_minor_version: c_int,
    pub vendor: *const c_char,
    pub resource_base: usize,
    pub resource_mask: usize,
    pub resource_id: usize,
    pub resource_shift: c_int,
    pub resource_alloc: *mut c_void,
    pub byte_order: c_int,
    pub bitmap_unit: c_int,
    pub bitmap_pad: c_int,
    pub bitmap_bit_order: c_int,
    pub nformats: c_int,
    pub pixmap_format: *mut c_void,
    pub vnumber: c_int,
    pub release: c_int,
    pub head: *mut c_void,
    pub tail: *mut c_void,
    pub qlen: c_int,
    pub last_request_read: usize,
    pub request: usize,
    pub last_req: *mut c_char,
    pub buffer: *mut c_char,
    pub bufptr: *mut c_char,
    pub bufmax: *mut c_char,
    pub max_request_size: c_uint,
    pub db: *mut c_void,
    pub synchandler: *mut c_void,
    pub display_name: *const c_char,
    pub default_screen: c_int,
    pub nscreens: c_int,
    pub screens: *mut Screen,
    pub pad_to_async: [u8; 2152],
    /// Xlib's `_XInternalAsync` chain.  Clients such as GDK link their own
    /// handlers here through the private `_XPrivDisplay` layout and expect
    /// them to see the reply or error of the request they issued.
    pub async_handlers: *mut AsyncHandler,
    pub pad_to_xcb: [u8; 208],
    pub xcb: *mut X11XCBPrivate,
    pub pad_trailing: [u8; 1024],
}

/// Xlib's private `_XInternalAsync` (Xlibint.h).
#[repr(C)]
pub struct AsyncHandler {
    pub next: *mut AsyncHandler,
    pub handler: Option<
        unsafe extern "sysv64" fn(*mut Display, *mut u8, *mut u8, c_int, *mut c_void) -> Bool,
    >,
    pub data: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XAnyEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XKeyEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub keycode: c_uint,
    pub same_screen: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XButtonEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub button: c_uint,
    pub same_screen: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XMotionEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub state: c_uint,
    pub is_hint: c_char,
    pub same_screen: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XCrossingEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub root: Window,
    pub subwindow: Window,
    pub time: Time,
    pub x: c_int,
    pub y: c_int,
    pub x_root: c_int,
    pub y_root: c_int,
    pub mode: c_int,
    pub detail: c_int,
    pub same_screen: Bool,
    pub focus: Bool,
    pub state: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XFocusChangeEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub mode: c_int,
    pub detail: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XExposeEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub count: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XConfigureEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub above: Window,
    pub override_redirect: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XMapEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub override_redirect: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XVisibilityEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub state: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XUnmapEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub event: Window,
    pub window: Window,
    pub from_configure: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct XClientMessageEvent {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub window: Window,
    pub message_type: Atom,
    pub format: c_int,
    pub data: [c_long; 5],
}

/// Xlib's generic-extension event header.  XI2 stores its decoded event data in
/// `data`; the client releases that allocation through `XFreeEventData`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XGenericEventCookie {
    pub r#type: c_int,
    pub serial: c_ulong,
    pub send_event: Bool,
    pub display: *mut Display,
    pub extension: c_int,
    pub evtype: c_int,
    pub cookie: c_uint,
    pub data: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union XEvent {
    pub r#type: c_int,
    pub xany: XAnyEvent,
    pub xkey: XKeyEvent,
    pub xbutton: XButtonEvent,
    pub xmotion: XMotionEvent,
    pub xcrossing: XCrossingEvent,
    pub xfocus: XFocusChangeEvent,
    pub xexpose: XExposeEvent,
    pub xconfigure: XConfigureEvent,
    pub xmap: XMapEvent,
    pub xvisibility: XVisibilityEvent,
    pub xunmap: XUnmapEvent,
    pub xclient: XClientMessageEvent,
    pub xcookie: XGenericEventCookie,
    pub pad: [c_long; 24],
}

unsafe impl Send for XEvent {}
unsafe impl Sync for XEvent {}

#[repr(C)]
pub struct XSizeHints {
    pub flags: c_long,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub min_width: c_int,
    pub min_height: c_int,
    pub max_width: c_int,
    pub max_height: c_int,
    pub width_inc: c_int,
    pub height_inc: c_int,
    pub min_aspect: (c_int, c_int),
    pub max_aspect: (c_int, c_int),
    pub base_width: c_int,
    pub base_height: c_int,
    pub win_gravity: c_int,
}

#[repr(C)]
pub struct XSetWindowAttributes {
    pub background_pixmap: usize,
    pub background_pixel: c_ulong,
    pub border_pixmap: usize,
    pub border_pixel: c_ulong,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: c_int,
    pub event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: c_int,
    pub colormap: usize,
    pub cursor: usize,
}

#[repr(C)]
pub struct XWindowAttributes {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub border_width: c_int,
    pub depth: c_int,
    pub visual: *mut Visual,
    pub root: Window,
    pub class: c_int,
    pub bit_gravity: c_int,
    pub win_gravity: c_int,
    pub backing_store: c_int,
    pub backing_planes: c_ulong,
    pub backing_pixel: c_ulong,
    pub save_under: c_int,
    pub colormap: Colormap,
    pub map_installed: Bool,
    pub map_state: c_int,
    pub all_event_masks: c_long,
    pub your_event_mask: c_long,
    pub do_not_propagate_mask: c_long,
    pub override_redirect: Bool,
    pub screen: *mut Screen,
}

struct InternalState {
    pending_events: VecDeque<XEvent>,
    guest_to_hwnd: HashMap<u64, usize>,
    hwnd_to_guest: HashMap<usize, u64>,
    /// Geometry last reported by ConfigureNotify per window.  Win32 announces
    /// a show as separate move and size messages; X11 reports a change once.
    reported_geometry: HashMap<usize, (i32, i32, i32, i32, Window)>,
}

unsafe impl Send for InternalState {}
unsafe impl Sync for InternalState {}

fn state() -> &'static Mutex<InternalState> {
    static STATE: OnceLock<Mutex<InternalState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(InternalState {
            pending_events: VecDeque::new(),
            guest_to_hwnd: HashMap::new(),
            hwnd_to_guest: HashMap::new(),
            reported_geometry: HashMap::new(),
        })
    })
}

/// Accounts for one protocol request on `dpy`, as Xlib's `NextRequest` and
/// `LastKnownRequestProcessed` expect.  Clients sequence grabs, error handlers
/// and `XIfEvent` predicates against these serials, so events carry the
/// current value and requests that change grab state must advance it.
pub unsafe fn issue_request(dpy: *mut Display) -> c_ulong {
    if dpy.is_null() {
        return 0;
    }
    // ELF extension clients buffer requests through _XGetRequest. Complete
    // those before a native core request observes or changes their resources.
    unsafe { xext::protocol::flush(dpy) };
    unsafe {
        (*dpy).request += 1;
        // Every request completes before the call returns; nothing is pending.
        (*dpy).last_request_read = (*dpy).request;
        (*dpy).request as c_ulong
    }
}

fn current_serial(dpy: *mut Display) -> c_ulong {
    if dpy.is_null() {
        0
    } else {
        unsafe { (*dpy).request as c_ulong }
    }
}

/// `StructureNotifyMask`, `SubstructureNotifyMask` and `ExposureMask`.
const STRUCTURE_NOTIFY: u32 = 1 << 17;
const SUBSTRUCTURE_NOTIFY: u32 = 1 << 19;
const EXPOSURE: u32 = 1 << 15;

impl InternalState {
    /// Queues an event stamped with the serial of the last request, which is
    /// what a client compares against `NextRequest` to order it.
    fn push(&mut self, dpy: *mut Display, mut event: XEvent) {
        let recipients = connection::event_recipients(&event);
        if !recipients.is_empty() {
            for recipient in recipients {
                self.push_to(recipient, event);
            }
        } else {
            self.push_to(dpy, event);
        }
    }
    fn count_for(&self, display: *mut Display) -> usize {
        self.pending_events
            .iter()
            .filter(|event| unsafe { event.xany.display == display })
            .count()
    }
    fn push_to(&mut self, dpy: *mut Display, mut event: XEvent) {
        event.xany.display = dpy;
        event.xany.serial = current_serial(dpy);
        self.pending_events.push_back(event);
        set_display_queue_len(dpy, self.count_for(dpy));
    }

    /// Queues a Map/Unmap/Configure/ReparentNotify as the server would: to the
    /// window itself when it selected StructureNotify, and to its parent, with
    /// `event` naming the parent, when that selected SubstructureNotify.  A
    /// window that selected neither (GDK's InputOnly focus child, for one)
    /// hears nothing.
    fn queue_structure(&mut self, window: Window, mut event: XEvent) {
        let dpy = unsafe { event.xany.display };
        if property::notify::selected(window) & STRUCTURE_NOTIFY != 0 {
            self.push(dpy, event);
        }
        let parent =
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(window as HWND) };
        let parent = if parent.is_null() {
            1
        } else {
            parent as Window
        };
        if property::notify::selected(parent) & SUBSTRUCTURE_NOTIFY != 0 {
            event.xany.window = parent;
            self.push(dpy, event);
        }
    }

    fn queue_map_state(&mut self, dpy: *mut Display, window: Window, mapped: bool) {
        let mut event: XEvent = unsafe { core::mem::zeroed() };
        if mapped {
            event.xmap.r#type = MapNotify;
            event.xmap.display = dpy;
            event.xmap.event = window;
            event.xmap.window = window;
            event.xmap.override_redirect =
                kinakaze_libdisplay::ui::is_override_redirect(window) as Bool;
        } else {
            event.xunmap.r#type = UnmapNotify;
            event.xunmap.display = dpy;
            event.xunmap.event = window;
            event.xunmap.window = window;
        }
        self.queue_structure(window, event);
        if mapped {
            self.queue_map_visibility(window);
        }
    }

    /// A newly viewable InputOutput window receives VisibilityNotify even
    /// before it draws. GLFW waits for this event in glfwShowWindow; MapNotify
    /// and Expose alone leave that wait spinning on unrelated queued events.
    fn queue_map_visibility(&mut self, window: Window) {
        let mut ancestor = window;
        for _ in 0..1024 {
            if ancestor == 1 {
                break;
            }
            if !shared::mapped(ancestor) {
                return;
            }
            ancestor = shared::parent(ancestor);
        }
        if ancestor != 1 {
            return;
        }
        let mut pending = vec![window];
        let mut visited = std::collections::HashSet::new();
        while let Some(window) = pending.pop() {
            if !visited.insert(window) || !shared::mapped(window) {
                continue;
            }
            if let Some((_, children)) = shared::hierarchy(window) {
                pending.extend(children);
            }
            if kinakaze_libdisplay::ui::input_only::is_input_only(window) {
                continue;
            }
            // Native composited windows have a drawable backing surface on
            // mapping. Do not depend on application frame submission here.
            let mut event: XEvent = unsafe { core::mem::zeroed() };
            event.xvisibility.r#type = VisibilityNotify;
            event.xvisibility.window = window;
            event.xvisibility.state = 0; // VisibilityUnobscured
            for recipient in connection::recipients(window, 1 << 16) {
                self.push_to(recipient, event);
            }
            let mut wire = [0u8; 32];
            wire[0] = VisibilityNotify as u8;
            wire[4..8].copy_from_slice(&(window as u32).to_le_bytes());
            shared::broadcast(window, 1 << 16, wire, false);
        }
    }

    /// Queues ConfigureNotify for `window` unless the geometry is the one
    /// already reported, and Expose when the drawable size is new.
    fn report_geometry(
        &mut self,
        dpy: *mut Display,
        window: Window,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) {
        let above = window_tree::sibling_below(window);
        let previous = self
            .reported_geometry
            .insert(window, (x, y, width, height, above));
        if previous == Some((x, y, width, height, above)) {
            return;
        }
        let mut cfg: XEvent = unsafe { core::mem::zeroed() };
        cfg.xconfigure.r#type = ConfigureNotify;
        cfg.xconfigure.display = dpy;
        cfg.xconfigure.event = window;
        cfg.xconfigure.window = window;
        cfg.xconfigure.x = x;
        cfg.xconfigure.y = y;
        cfg.xconfigure.width = width;
        cfg.xconfigure.height = height;
        cfg.xconfigure.above = above;
        cfg.xconfigure.override_redirect = Bool::from(shared::override_redirect(window));
        self.queue_structure(window, cfg);
        shared::structure(window, 22);

        if previous.is_some_and(|(_, _, w, h, _)| (w, h) == (width, height))
            || property::notify::selected(window) & EXPOSURE == 0
        {
            return;
        }
        let mut exp: XEvent = unsafe { core::mem::zeroed() };
        exp.xexpose.r#type = Expose;
        exp.xexpose.display = dpy;
        exp.xexpose.window = window;
        exp.xexpose.width = width;
        exp.xexpose.height = height;
        self.push(dpy, exp);
    }
}

pub(crate) fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some())
}

pub(crate) fn input_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_INPUT_TRACE").is_some())
}

/// Keeps the public Xlib queue-length field in sync with our Rust queue.
///
/// `QLength(display)` is a macro in Xlib and reads `Display.qlen` directly.
/// GLFW deliberately calls `XPending` to pull transport events in and then uses
/// that macro, so returning the right value from `XPending` is not sufficient.
fn set_display_queue_len(dpy: *mut Display, len: usize) {
    if !dpy.is_null() {
        // SAFETY: callers hold the X11 state lock and `dpy` is the live display
        // supplied by the same Xlib consumer.
        unsafe { (*dpy).qlen = len.min(c_int::MAX as usize) as c_int };
    }
}

static mut GLOBAL_VISUAL: Visual = Visual {
    ext_data: core::ptr::null_mut(),
    visualid: 1,
    class: 4,
    red_mask: 0x00ff0000,
    green_mask: 0x0000ff00,
    blue_mask: 0x000000ff,
    bits_per_rgb: 8,
    map_entries: 256,
};

static mut GLOBAL_DEPTH: Depth = Depth {
    depth: 24,
    nvisuals: 1,
    visuals: &raw mut GLOBAL_VISUAL,
};

static mut GLOBAL_SCREEN: Screen = Screen {
    ext_data: core::ptr::null_mut(),
    display: core::ptr::null_mut(),
    root: 1,
    width: 1920,
    height: 1080,
    mwidth: 508,
    mheight: 285,
    ndepths: 1,
    depths: &raw mut GLOBAL_DEPTH,
    root_depth: 24,
    root_visual: &raw mut GLOBAL_VISUAL,
    default_gc: core::ptr::null_mut(),
    cmap: 1,
    white_pixel: 0x00ffffff,
    black_pixel: 0x00000000,
    max_maps: 1,
    min_maps: 1,
    backing_store: 0,
    save_unders: False,
    root_input_mask: 0,
};

static mut GLOBAL_DISPLAY: Display = Display {
    ext_data: core::ptr::null_mut(),
    free_funcs: core::ptr::null_mut(),
    fd: -1,
    lock: 0,
    proto_major_version: 11,
    proto_minor_version: 0,
    vendor: c"Kinakaze X11 Adapter".as_ptr(),
    resource_base: 0x1000,
    resource_mask: 0x0fff,
    resource_id: 0,
    resource_shift: 0,
    resource_alloc: xext::protocol::allocate_id as *mut c_void,
    byte_order: 0,
    bitmap_unit: 32,
    bitmap_pad: 32,
    bitmap_bit_order: 0,
    nformats: image::DISPLAY_FORMATS.len() as c_int,
    pixmap_format: image::DISPLAY_FORMATS.as_ptr() as *mut c_void,
    vnumber: 11,
    release: 0,
    head: core::ptr::null_mut(),
    tail: core::ptr::null_mut(),
    qlen: 0,
    last_request_read: 0,
    request: 0,
    last_req: core::ptr::null_mut(),
    buffer: core::ptr::null_mut(),
    bufptr: core::ptr::null_mut(),
    bufmax: core::ptr::null_mut(),
    max_request_size: 65535,
    db: core::ptr::null_mut(),
    synchandler: core::ptr::null_mut(),
    display_name: c":0.0".as_ptr(),
    default_screen: 0,
    nscreens: 1,
    screens: &raw mut GLOBAL_SCREEN,
    pad_to_async: [0; 2152],
    async_handlers: core::ptr::null_mut(),
    pad_to_xcb: [0; 208],
    xcb: &raw mut GLOBAL_XCB_PRIVATE,
    pad_trailing: [0; 1024],
};

static mut DUMMY_XCB_CONN: u32 = 1;

pub mod connection;
pub use connection::XSetEventQueueOwner;

/// The native display shared by the Xlib and XCB protocol frontends.
pub fn shared_display() -> *mut Display {
    &raw mut GLOBAL_DISPLAY
}

unsafe extern "system" fn notify_x11_event() {
    event_select::notify();
    connection::notify();
}
pub fn notify_event() {
    unsafe { notify_x11_event() };
}

static mut GLOBAL_XCB_PRIVATE: X11XCBPrivate = X11XCBPrivate {
    connection: &raw mut DUMMY_XCB_CONN as *mut c_void,
    next_xid: 1,
    max_xid: 0xffff,
    min_xid: 1,
    xid_mask: 0xffff,
    xid_inc: 1,
    xid_base: 0x1000,
    pad: [0; 512],
};

fn init_display_globals() {
    kinakaze_libdisplay::ui::decorations::set_shape_handler(shape::native_changed);
    unsafe {
        GLOBAL_XCB_PRIVATE.connection = shared_display().cast();
        GLOBAL_SCREEN.display = &raw mut GLOBAL_DISPLAY;
        let w = GetSystemMetrics(SM_CXSCREEN);
        let h = GetSystemMetrics(SM_CYSCREEN);
        if w > 0 && h > 0 {
            GLOBAL_SCREEN.width = w;
            GLOBAL_SCREEN.height = h;
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XOpenDisplay")]
pub unsafe extern "sysv64" fn XOpenDisplay(_display_name: *const c_char) -> *mut Display {
    if trace_enabled() {
        crate::diagnostic!("[libX11] XOpenDisplay begin");
    }
    init_display_globals();
    if trace_enabled() {
        crate::diagnostic!("[libX11] XOpenDisplay globals ready");
    }
    let _ = kinakaze_libdisplay::ui::available();
    if trace_enabled() {
        crate::diagnostic!("[libX11] XOpenDisplay UI ready");
    }
    let display = connection::open();
    if display.is_null() {
        return core::ptr::null_mut();
    }
    kinakaze_libdisplay::ui::presentation::set_paint_handler(Some(graphics::paint));
    kinakaze_libdisplay::ui::interaction::set_resize_handler(wm::native_resize);
    property::wm_support::install(display);
    if trace_enabled() {
        crate::diagnostic!("[libX11] XOpenDisplay complete display={display:p}");
    }
    display
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCloseDisplay")]
pub unsafe extern "sysv64" fn XCloseDisplay(_dpy: *mut Display) -> c_int {
    if _dpy.is_null() {
        return 0;
    }
    let Some(last) = connection::close(_dpy) else {
        return 0;
    };
    xext::protocol::close(_dpy);
    if let Ok(mut state) = state().lock() {
        state
            .pending_events
            .retain(|event| unsafe { event.xany.display != _dpy });
    }
    if !last {
        if _dpy != shared_display() {
            unsafe {
                kinakaze_alloc::guest::free((*_dpy).screens.cast());
                kinakaze_alloc::guest::free((*_dpy).xcb.cast());
                kinakaze_alloc::guest::free(_dpy.cast());
            }
        }
        return 0;
    }
    close_extensions(_dpy);
    xext::protocol::poll(_dpy, true);
    property::close_display();
    colormap::close_display();
    if _dpy != shared_display() {
        unsafe {
            kinakaze_alloc::guest::free((*_dpy).screens.cast());
            kinakaze_alloc::guest::free((*_dpy).xcb.cast());
            kinakaze_alloc::guest::free(_dpy.cast());
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDisplayName")]
pub unsafe extern "sysv64" fn XDisplayName(string: *const c_char) -> *const c_char {
    if !string.is_null() {
        string
    } else {
        c":0.0".as_ptr()
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDefaultScreen")]
pub unsafe extern "sysv64" fn XDefaultScreen(_dpy: *mut Display) -> c_int {
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XScreenCount")]
pub unsafe extern "sysv64" fn XScreenCount(_dpy: *mut Display) -> c_int {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XRootWindow")]
pub unsafe extern "sysv64" fn XRootWindow(_dpy: *mut Display, _screen_number: c_int) -> Window {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDefaultVisual")]
pub unsafe extern "sysv64" fn XDefaultVisual(
    _dpy: *mut Display,
    _screen_number: c_int,
) -> *mut Visual {
    &raw mut GLOBAL_VISUAL
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDefaultColormap")]
pub unsafe extern "sysv64" fn XDefaultColormap(
    _dpy: *mut Display,
    _screen_number: c_int,
) -> Colormap {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDefaultDepth")]
pub unsafe extern "sysv64" fn XDefaultDepth(_dpy: *mut Display, _screen_number: c_int) -> c_int {
    24
}

#[unsafe(export_name = "kinakaze_engine_libX11_XBlackPixel")]
pub unsafe extern "sysv64" fn XBlackPixel(_dpy: *mut Display, _screen_number: c_int) -> c_ulong {
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XWhitePixel")]
pub unsafe extern "sysv64" fn XWhitePixel(_dpy: *mut Display, _screen_number: c_int) -> c_ulong {
    0x00ffffff
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateWindow")]
pub unsafe extern "sysv64" fn XCreateWindow(
    _dpy: *mut Display,
    parent: Window,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    _border_width: c_uint,
    _depth: c_int,
    _class: c_uint,
    _visual: *mut Visual,
    valuemask: c_ulong,
    attributes: *mut XSetWindowAttributes,
) -> Window {
    let parent = window_tree::native_window(parent);
    unsafe { issue_request(_dpy) };
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        unsafe {
            errors::report(_dpy, 2, 1, 0, parent);
        }
        return 0;
    }
    let w = width as i32;
    let h = height as i32;
    if trace_enabled() {
        crate::diagnostic!("[libX11] XCreateWindow parent={parent:#x} at {x},{y} {w}x{h}");
    }
    let parent_id = if parent == 1 {
        Option::<u64>::None
    } else {
        state()
            .lock()
            .ok()
            .and_then(|s| s.hwnd_to_guest.get(&parent).copied())
            .or_else(|| kinakaze_libdisplay::window::logical_for_native(parent))
    };
    let created = if parent == 1 {
        kinakaze_libdisplay::window::create_unmapped(Option::None, x, y, w, h, "X11 Window")
    } else if let Some(parent_id) = parent_id {
        kinakaze_libdisplay::window::create_unmapped(Some(parent_id), x, y, w, h, "X11 Window")
    } else {
        if trace_enabled() {
            crate::diagnostic!("[libX11] XCreateWindow rejected unknown parent={parent:#x}");
        }
        Option::<u64>::None
    };
    if let Some(id) = created {
        if let Some(hwnd_val) = kinakaze_libdisplay::window::native_handle(id) {
            // CWBackPixel. XClearWindow/XClearArea reveal this pixel; clearing to
            // a hard-coded black surface breaks ordinary Xt expose/redraw paths.
            let background = if valuemask & (1 << 1) != 0 && !attributes.is_null() {
                unsafe { (*attributes).background_pixel }
            } else {
                0
            };
            graphics::set_window_background(hwnd_val, background);
            shared::register_window(hwnd_val, parent);
            if trace_enabled() {
                crate::diagnostic!("[libX11] stable id={id} -> HWND={hwnd_val:#x}");
            }
            if let Ok(mut s) = state().lock() {
                s.guest_to_hwnd.insert(id, hwnd_val);
                s.hwnd_to_guest.insert(hwnd_val, id);
            }
            if !attributes.is_null() {
                unsafe {
                    wm::XChangeWindowAttributes(_dpy, hwnd_val, valuemask, attributes);
                }
            }
            // Focus, selection and compositor helper windows are not ordinary
            // application windows. Keep their requested geometry, including
            // off-screen coordinates, instead of Windows' minimum frame size.
            if parent == 1 && (_class == 2 || width == 1 || height == 1) {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GWL_EXSTYLE, GetWindowLongPtrW, SWP_NOACTIVATE, SWP_NOZORDER,
                    SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
                };
                kinakaze_libdisplay::ui::decorations::set(hwnd_val, false);
                unsafe {
                    let hwnd = hwnd_val as HWND;
                    let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                    if _class != 2 {
                        kinakaze_libdisplay::ui::presentation::small_window(hwnd_val, style);
                    }
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_EXSTYLE,
                        (style | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize),
                    );
                    SetWindowPos(
                        hwnd,
                        core::ptr::null_mut(),
                        x,
                        y,
                        w,
                        h,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    );
                }
            }
            if _class == 2 {
                kinakaze_libdisplay::ui::input_only::set(hwnd_val);
            }
            if !shared::recipients(1, 1 << 20).is_empty() {
                kinakaze_libdisplay::ui::decorations::set(hwnd_val, false);
            }
            shared::structure(hwnd_val, 16);
            return hwnd_val;
        }
        return id as usize;
    }
    if trace_enabled() {
        crate::diagnostic!("[libX11] window creation failed");
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateSimpleWindow")]
pub unsafe extern "sysv64" fn XCreateSimpleWindow(
    dpy: *mut Display,
    parent: Window,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
    border_width: c_uint,
    border: c_ulong,
    background: c_ulong,
) -> Window {
    let mut attr: XSetWindowAttributes = unsafe { core::mem::zeroed() };
    attr.border_pixel = border;
    attr.background_pixel = background;
    unsafe {
        XCreateWindow(
            dpy,
            parent,
            x,
            y,
            width,
            height,
            border_width,
            24,
            1,
            core::ptr::null_mut(),
            (1 << 1) | (1 << 3),
            &mut attr,
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDestroyWindow")]
pub unsafe extern "sysv64" fn XDestroyWindow(_dpy: *mut Display, w: Window) -> c_int {
    unsafe { issue_request(_dpy) };
    focus::destroying(w);
    destroy_extensions(w);
    property::forget_window(w);
    let hwnd_val = w;
    if hwnd_val != 0 {
        graphics::forget_window(hwnd_val);
        let id = if let Ok(mut s) = state().lock() {
            s.reported_geometry.remove(&hwnd_val);
            let id = s.hwnd_to_guest.remove(&hwnd_val);
            if let Some(id) = id {
                s.guest_to_hwnd.remove(&id);
            }
            id
        } else {
            Option::None
        };
        if let Some(id) = id {
            let _ = kinakaze_libdisplay::window::destroy(id);
        }
    }
    shared::structure(w, 17);
    shared::forget(w);
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XMapWindow")]
pub unsafe extern "sysv64" fn XMapWindow(dpy: *mut Display, w: Window) -> c_int {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, IsIconic, IsWindow, SW_RESTORE, ShowWindow, WS_VISIBLE,
    };
    unsafe { issue_request(dpy) };
    if w == 1 {
        return 1;
    }
    let mut map_request = [0u8; 32];
    map_request[0] = 20;
    if shared::redirect_request(dpy, w, map_request) {
        return 1;
    }
    let hwnd = w as HWND;
    if unsafe { IsWindow(hwnd) } == 0 {
        unsafe {
            errors::report(dpy, 3, 8, 0, w);
        }
        return 0;
    }
    // Mapping an iconified window asks the window manager to restore it; the
    // resulting MapNotify arrives through the native size change.
    let iconic = unsafe { IsIconic(hwnd) } != 0;
    if trace_enabled() {
        crate::diagnostic!(
            "[libX11] XMapWindow {w:#x} visible={} iconic={iconic}",
            unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } & WS_VISIBLE as isize != 0
        );
    }
    if shared::mapped(w) {
        if iconic {
            unsafe { ShowWindow(hwnd, SW_RESTORE) };
        }
        return 1;
    }
    // Presentation may already have exposed the native HWND. X mapping and
    // MapNotify still transition exactly once, independently of WS_VISIBLE.
    shared::set_mapped(w, true);
    if !hwnd.is_null() {
        if unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } & WS_VISIBLE as isize == 0 {
            if let Some(id) = kinakaze_libdisplay::window::logical_for_native(w) {
                kinakaze_libdisplay::window::set_visible(id, true);
            } else if shared::valid(w) {
                kinakaze_libdisplay::ui::set_visible(w, true);
            }
        }
        if iconic {
            unsafe { ShowWindow(hwnd, SW_RESTORE) };
            return 1;
        }
        // The window can be destroyed by its owner while this runs.
        let Some((x, y)) = window_tree::origin(w) else {
            unsafe { errors::report(dpy, 3, 8, 0, w) };
            return 0;
        };
        if let Ok(mut s) = state().lock() {
            let mut rect: RECT = unsafe { core::mem::zeroed() };
            unsafe { GetClientRect(hwnd, &mut rect) };
            let width = (rect.right - rect.left).max(1);
            let height = (rect.bottom - rect.top).max(1);

            s.queue_map_state(dpy, w, true);
            s.report_geometry(dpy, w, x, y, width, height);
            set_display_queue_len(dpy, s.count_for(dpy));
        }
        property::set_wm_state(dpy, w, 1);
        shared::structure(w, 19);
        unsafe {
            notify_x11_event();
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XMapRaised")]
pub unsafe extern "sysv64" fn XMapRaised(dpy: *mut Display, w: Window) -> c_int {
    if unsafe { XMapWindow(dpy, w) } == 0 {
        return 0;
    }
    unsafe { wm::XRaiseWindow(dpy, w) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUnmapWindow")]
pub unsafe extern "sysv64" fn XUnmapWindow(_dpy: *mut Display, w: Window) -> c_int {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, IsWindow, WS_VISIBLE,
    };
    unsafe { issue_request(_dpy) };
    if w == 1 {
        return 1;
    }
    let hwnd = w as HWND;
    if unsafe { IsWindow(hwnd) } == 0 {
        unsafe {
            errors::report(_dpy, 3, 10, 0, w);
        }
        return 0;
    }
    if trace_enabled() {
        crate::diagnostic!(
            "[libX11] XUnmapWindow {w:#x} visible={}",
            unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } & WS_VISIBLE as isize != 0
        );
    }
    if !shared::mapped(w) {
        return 1;
    }
    shared::set_mapped(w, false);
    if !hwnd.is_null() {
        if let Some(id) = kinakaze_libdisplay::window::logical_for_native(w) {
            kinakaze_libdisplay::window::set_visible(id, false);
        } else if shared::valid(w) {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, 0);
            }
        }
        if let Ok(mut s) = state().lock() {
            s.queue_map_state(_dpy, w, false);
            set_display_queue_len(_dpy, s.count_for(_dpy));
        }
        property::set_wm_state(_dpy, w, 0);
        shared::structure(w, 18);
        unsafe {
            notify_x11_event();
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XStoreName")]
pub unsafe extern "sysv64" fn XStoreName(
    dpy: *mut Display,
    w: Window,
    name: *const c_char,
) -> c_int {
    let bytes = if name.is_null() {
        &[][..]
    } else {
        unsafe { core::ffi::CStr::from_ptr(name) }.to_bytes()
    };
    let Ok(length) = c_int::try_from(bytes.len()) else {
        return 0;
    };
    unsafe { XChangeProperty(dpy, w, 39, 31, 8, 0, bytes.as_ptr(), length) }
}

fn set_window_title(w: Window, title: &str) -> bool {
    if let Some(logical) = kinakaze_libdisplay::window::logical_for_native(w) {
        // Updating the logical registry as well as the HWND preserves the title
        // when a Linux-style fork reconstructs the window in another process.
        kinakaze_libdisplay::window::set_title(logical, title)
    } else {
        kinakaze_libdisplay::ui::set_title(w, title)
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetStandardProperties")]
pub unsafe extern "sysv64" fn XSetStandardProperties(
    dpy: *mut Display,
    w: Window,
    name: *const c_char,
    _icon_string: *const c_char,
    _icon_pixmap: Pixmap,
    _argv: *mut *mut c_char,
    _argc: c_int,
    _hints: *mut XSizeHints,
) -> c_int {
    unsafe { XStoreName(dpy, w, name) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetWMProtocols")]
pub unsafe extern "sysv64" fn XSetWMProtocols(
    dpy: *mut Display,
    w: Window,
    protocols: *mut Atom,
    count: c_int,
) -> Status {
    if count < 0 || (count != 0 && protocols.is_null()) {
        return 0;
    }
    let property = unsafe { XInternAtom(dpy, c"WM_PROTOCOLS".as_ptr(), 0) };
    unsafe {
        XChangeProperty(dpy, w, property, 4, 32, 0, protocols.cast(), count);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetNormalHints")]
pub unsafe extern "sysv64" fn XSetNormalHints(
    dpy: *mut Display,
    w: Window,
    hints: *mut XSizeHints,
) -> c_int {
    if hints.is_null() {
        return 0;
    }
    unsafe {
        wm::XSetWMNormalHints(dpy, w, hints);
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetClassHint")]
pub unsafe extern "sysv64" fn XSetClassHint(
    dpy: *mut Display,
    w: Window,
    hint: *mut c_void,
) -> c_int {
    if hint.is_null() {
        return 0;
    }
    unsafe {
        wm_properties::set_hints(
            dpy,
            w,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            hint.cast(),
        );
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFree")]
pub unsafe extern "sysv64" fn XFree(pointer: *mut c_void) -> c_int {
    // Guest-owned query results share the libc allocator. Legacy native result
    // producers still require migration; do not feed their Rust storage to it.
    if kinakaze_alloc::guest::contains(pointer as usize) {
        unsafe { kinakaze_alloc::guest::free(pointer.cast()) };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFlush")]
pub unsafe extern "sysv64" fn XFlush(_dpy: *mut Display) -> c_int {
    unsafe { xext::protocol::flush(_dpy) };
    if trace_enabled() {
        crate::diagnostic!("[libX11] XFlush");
    }
    graphics::flush_all();
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSync")]
pub unsafe extern "sysv64" fn XSync(_dpy: *mut Display, _discard: Bool) -> c_int {
    unsafe { issue_request(_dpy) };
    unsafe { xext::protocol::flush(_dpy) };
    graphics::flush_all();
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSelectInput")]
pub unsafe extern "sysv64" fn XSelectInput(
    _dpy: *mut Display,
    _w: Window,
    _event_mask: c_long,
) -> c_int {
    unsafe { issue_request(_dpy) };
    unsafe { property::notify::select(_dpy, _w, _event_mask) }
}

/// The modifier and button state a core pointer event reports.
pub fn pointer_state() -> c_uint {
    keyboard::pointer_mask()
}

/// Client coordinates of `window` in root coordinates.
fn root_position(window: Window, x: c_int, y: c_int) -> (c_int, c_int) {
    let mut point = windows_sys::Win32::Foundation::POINT { x, y };
    if window != 1 {
        unsafe {
            windows_sys::Win32::Graphics::Gdi::ClientToScreen(window as HWND, &raw mut point)
        };
    }
    (point.x, point.y)
}

/// Dispatch native events to each subscribing display, regardless of which
/// frontend happens to pump first. XCB then consumes only its display's queue.
pub fn drain_native_events(dpy: *mut Display) {
    // Consume the wake before inspecting producer state. A counter update
    // arriving during the extension poll must leave the connection readable
    // for the next iteration, rather than having its wake erased afterward.
    let xcb_owner = connection::xcb_events_for(dpy);
    if !xcb_owner {
        connection::drain_display(dpy);
    }
    xext::protocol::poll(dpy, false);
    if !xcb_owner {
        xext::protocol::drain_events(dpy);
    }
    let mut modifiers = Option::None;
    while let Some(ev) = kinakaze_libdisplay::event::queue()
        .lock()
        .ok()
        .and_then(|mut q| q.pop())
    {
        if ev.kind == kinakaze_libdisplay::event::EVENT_MODIFIERS {
            modifiers = Some(ev.state);
            continue;
        }
        let _modifiers = keyboard::EventModifiers::enter(modifiers);
        if ev.kind == kinakaze_libdisplay::event::EVENT_SCROLL {
            queue_scroll_events(dpy, &ev);
            continue;
        }
        if ev.kind == kinakaze_libdisplay::event::EVENT_FOCUS {
            // XI2 toolkits discard ordinary core focus events. Deliver their
            // keyboard focus cookie while preserving core subscribers below.
            input_bridge::dispatch(&ev);
        }
        if matches!(
            ev.kind,
            kinakaze_libdisplay::event::EVENT_KEY
                | kinakaze_libdisplay::event::EVENT_POINTER_BUTTON
                | kinakaze_libdisplay::event::EVENT_POINTER_MOTION
                | kinakaze_libdisplay::event::EVENT_TOUCH_BEGIN
                | kinakaze_libdisplay::event::EVENT_TOUCH_UPDATE
                | kinakaze_libdisplay::event::EVENT_TOUCH_END
                | kinakaze_libdisplay::event::EVENT_POINTER_ENTER
                | kinakaze_libdisplay::event::EVENT_POINTER_LEAVE
        ) {
            if input_bridge::dispatch(&ev) {
                continue;
            }
        }
        if matches!(
            ev.kind,
            kinakaze_libdisplay::event::EVENT_RAW_KEY
                | kinakaze_libdisplay::event::EVENT_RAW_BUTTON
                | kinakaze_libdisplay::event::EVENT_RAW_MOTION
        ) {
            input_bridge::dispatch(&ev);
            continue;
        }

        if let Ok(mut s) = state().lock() {
            // A window destroyed after the native message was queued has no
            // X11 id left; the X server likewise reports nothing for it.
            let Some(win) = s.guest_to_hwnd.get(&ev.window).copied() else {
                if trace_enabled() {
                    crate::diagnostic!(
                        "[libX11] native event kind={} for destroyed logical={}",
                        ev.kind,
                        ev.window
                    );
                }
                continue;
            };
            if trace_enabled() {
                crate::diagnostic!(
                    "[libX11] native event kind={} logical={} window={win:#x} x={} y={}",
                    ev.kind,
                    ev.window,
                    ev.x,
                    ev.y
                );
            }
            match ev.kind {
                kinakaze_libdisplay::event::EVENT_KEY => {
                    if !keyboard::accept_key(u32::from(ev.keycode) + 8, ev.state) {
                        continue;
                    }
                    let mut xev: XEvent = unsafe { core::mem::zeroed() };
                    let is_press = ev.state != kinakaze_libdisplay::event::STATE_RELEASED;
                    xev.xkey.r#type = if is_press { KeyPress } else { KeyRelease };
                    xev.xkey.display = dpy;
                    let Some(target) = focus::key_target(win) else {
                        continue;
                    };
                    xev.xkey.window = target;
                    xev.xkey.time = ev.time_ms as Time;
                    // Core X11 keycodes reserve 0..7; evdev codes start at zero.
                    // The conventional evdev XKB mapping is therefore code + 8.
                    let x11_code = u32::from(ev.keycode) + 8;
                    xev.xkey.keycode = x11_code;
                    xev.xkey.state = keyboard::pointer_mask();
                    if input_trace_enabled() {
                        crate::diagnostic!(
                            "[input] core key state={} evdev={} x11={} keysym={:#x} window={win:#x}",
                            ev.state,
                            ev.keycode,
                            x11_code,
                            linux_keycode_to_keysym(x11_code)
                        );
                    }
                    s.push(dpy, xev);
                }
                kinakaze_libdisplay::event::EVENT_POINTER_BUTTON => {
                    let btn = match ev.button {
                        kinakaze_libdisplay::event::BTN_LEFT => 1,
                        kinakaze_libdisplay::event::BTN_MIDDLE => 2,
                        kinakaze_libdisplay::event::BTN_RIGHT => 3,
                        kinakaze_libdisplay::event::BTN_SIDE => 8,
                        kinakaze_libdisplay::event::BTN_EXTRA => 9,
                        _ => 0,
                    };
                    if btn == 0 {
                        continue;
                    }
                    let is_press = ev.state != kinakaze_libdisplay::event::STATE_RELEASED;
                    let mut xev: XEvent = unsafe { core::mem::zeroed() };
                    xev.xbutton.r#type = if is_press { ButtonPress } else { ButtonRelease };
                    xev.xbutton.display = dpy;
                    xev.xbutton.window = win;
                    xev.xbutton.root = 1;
                    xev.xbutton.time = ev.time_ms as Time;
                    xev.xbutton.x = ev.x;
                    xev.xbutton.y = ev.y;
                    (xev.xbutton.x_root, xev.xbutton.y_root) = root_position(win, ev.x, ev.y);
                    // `state` reports the buttons held before this one changed.
                    let bit = if (1..=5).contains(&btn) {
                        1 << (btn + 7)
                    } else {
                        0
                    };
                    xev.xbutton.state = if is_press {
                        pointer_state() & !bit
                    } else {
                        pointer_state() | bit
                    };
                    xev.xbutton.button = btn;
                    xev.xbutton.same_screen = True;
                    s.push(dpy, xev);
                }
                kinakaze_libdisplay::event::EVENT_POINTER_MOTION => {
                    let mut xev: XEvent = unsafe { core::mem::zeroed() };
                    xev.xmotion.r#type = MotionNotify;
                    xev.xmotion.display = dpy;
                    xev.xmotion.window = win;
                    xev.xmotion.root = 1;
                    xev.xmotion.time = ev.time_ms as Time;
                    xev.xmotion.x = ev.x;
                    xev.xmotion.y = ev.y;
                    (xev.xmotion.x_root, xev.xmotion.y_root) = root_position(win, ev.x, ev.y);
                    xev.xmotion.state = pointer_state();
                    xev.xmotion.same_screen = True;
                    s.push(dpy, xev);
                }
                kinakaze_libdisplay::event::EVENT_POINTER_ENTER
                | kinakaze_libdisplay::event::EVENT_POINTER_LEAVE => {
                    let entered = ev.kind == kinakaze_libdisplay::event::EVENT_POINTER_ENTER;
                    // EnterWindowMask and LeaveWindowMask select these.
                    if property::notify::selected(win) & (1 << if entered { 4 } else { 5 }) == 0 {
                        continue;
                    }
                    let mut xev: XEvent = unsafe { core::mem::zeroed() };
                    xev.xcrossing.r#type = if entered { EnterNotify } else { LeaveNotify };
                    xev.xcrossing.display = dpy;
                    xev.xcrossing.window = win;
                    xev.xcrossing.root = 1;
                    xev.xcrossing.time = ev.time_ms as Time;
                    xev.xcrossing.x = ev.x;
                    xev.xcrossing.y = ev.y;
                    (xev.xcrossing.x_root, xev.xcrossing.y_root) = root_position(win, ev.x, ev.y);
                    // The window the pointer came from or went to is not part of
                    // this client's hierarchy as seen here: NotifyNonlinear.
                    xev.xcrossing.mode = ev.state.min(2) as c_int;
                    xev.xcrossing.detail = 3;
                    xev.xcrossing.same_screen = True;
                    xev.xcrossing.focus = Bool::from(focus::key_target(win) == Some(win));
                    xev.xcrossing.state = pointer_state();
                    s.push(dpy, xev);
                }
                kinakaze_libdisplay::event::EVENT_SCROLL => {
                    let (button, count) = if ev.scroll_y > 0 {
                        (4, ev.scroll_y)
                    } else if ev.scroll_y < 0 {
                        (5, -ev.scroll_y)
                    } else if ev.scroll_x > 0 {
                        (6, ev.scroll_x)
                    } else {
                        (7, -ev.scroll_x)
                    };
                    for _ in 0..count.clamp(0, 32) {
                        for event_type in [ButtonPress, ButtonRelease] {
                            let mut xev: XEvent = unsafe { core::mem::zeroed() };
                            xev.xbutton.r#type = event_type;
                            xev.xbutton.display = dpy;
                            xev.xbutton.window = win;
                            xev.xbutton.time = ev.time_ms as Time;
                            xev.xbutton.button = button;
                            s.push(dpy, xev);
                        }
                    }
                }
                kinakaze_libdisplay::event::EVENT_FOCUS => {
                    let mut xev: XEvent = unsafe { core::mem::zeroed() };
                    xev.xfocus.r#type = if ev.state != 0 { FocusIn } else { FocusOut };
                    xev.xfocus.display = dpy;
                    xev.xfocus.window = win;
                    s.push(dpy, xev);
                }
                kinakaze_libdisplay::event::EVENT_EXPOSE => {
                    if ev.state == 0 {
                        graphics::expose(win, ev.x, ev.y, ev.scroll_x, ev.scroll_y);
                    }
                    if property::notify::selected(win) & EXPOSURE != 0 {
                        let mut event: XEvent = unsafe { core::mem::zeroed() };
                        event.xexpose.r#type = Expose;
                        event.xexpose.display = dpy;
                        event.xexpose.window = win;
                        event.xexpose.x = ev.x;
                        event.xexpose.y = ev.y;
                        event.xexpose.width = ev.scroll_x;
                        event.xexpose.height = ev.scroll_y;
                        s.push(dpy, event);
                    }
                }
                kinakaze_libdisplay::event::EVENT_RESIZE
                | kinakaze_libdisplay::event::EVENT_MOVE
                | kinakaze_libdisplay::event::EVENT_STACK => {
                    // A window destroyed after the native message was queued
                    // has no geometry left to report.
                    let size = if ev.kind != kinakaze_libdisplay::event::EVENT_RESIZE {
                        kinakaze_libdisplay::ui::client_size(win)
                    } else {
                        Some((ev.x, ev.y))
                    };
                    let (Some((width, height)), Some((x, y))) = (size, window_tree::origin(win))
                    else {
                        continue;
                    };
                    if width > 0 && height > 0 {
                        s.report_geometry(dpy, win, x, y, width, height);
                    }
                }
                kinakaze_libdisplay::event::EVENT_ICONIC => {
                    // ICCCM: the window manager unmaps an iconified window and
                    // maps it again on restore; WM_STATE records which.
                    let iconic = ev.state != 0;
                    s.queue_map_state(dpy, win, !iconic);
                    set_display_queue_len(dpy, s.count_for(dpy));
                    drop(s);
                    property::set_wm_state(dpy, win, if iconic { 3 } else { 1 });
                    property::wm_support::set_state(dpy, win, b"_NET_WM_STATE_HIDDEN", iconic);
                    continue;
                }
                kinakaze_libdisplay::event::EVENT_MAXIMIZED => {
                    drop(s);
                    for name in [
                        b"_NET_WM_STATE_MAXIMIZED_VERT".as_slice(),
                        b"_NET_WM_STATE_MAXIMIZED_HORZ",
                    ] {
                        property::wm_support::set_state(dpy, win, name, ev.state != 0);
                    }
                    continue;
                }
                kinakaze_libdisplay::event::EVENT_CLOSE => {
                    let proto = unsafe { XInternAtom(dpy, c"WM_PROTOCOLS".as_ptr(), 0) };
                    let del = unsafe { XInternAtom(dpy, c"WM_DELETE_WINDOW".as_ptr(), 0) };
                    let mut client: XEvent = unsafe { core::mem::zeroed() };
                    unsafe {
                        client.xclient.r#type = ClientMessage;
                        client.xclient.display = dpy;
                        client.xclient.send_event = 1;
                        client.xclient.window = win;
                        client.xclient.message_type = proto;
                        client.xclient.format = 32;
                        client.xclient.data[0] = del as c_long;
                        client.xclient.data[1] = ev.time_ms as c_long;
                    }
                    s.push(dpy, client);
                }
                _ => {}
            }
            set_display_queue_len(dpy, s.count_for(dpy));
        }
    }
}

fn queue_scroll_events(dpy: *mut Display, source: &kinakaze_libdisplay::event::Event) {
    use kinakaze_libdisplay::event::{STATE_PRESSED, STATE_RELEASED};
    let Some(window) = kinakaze_libdisplay::window::native_handle(source.window) else {
        return;
    };
    if input_trace_enabled() {
        crate::diagnostic!(
            "[input] scroll window={window:#x} at {},{} delta={},{}",
            source.x,
            source.y,
            source.scroll_x,
            source.scroll_y
        );
    }
    for (amount, positive, negative) in [(source.scroll_y, 4, 5), (source.scroll_x, 7, 6)] {
        let button = if amount > 0 { positive } else { negative };
        for _ in 0..amount.unsigned_abs().min(32) {
            for pressed in [true, false] {
                let mut wheel = *source;
                wheel.button = button;
                wheel.state = if pressed {
                    STATE_PRESSED
                } else {
                    STATE_RELEASED
                };
                if input_bridge::dispatch(&wheel) {
                    continue;
                }
                let mut event: XEvent = unsafe { core::mem::zeroed() };
                event.xbutton.r#type = if pressed { ButtonPress } else { ButtonRelease };
                event.xbutton.display = dpy;
                event.xbutton.window = window;
                event.xbutton.root = 1;
                event.xbutton.time = source.time_ms as Time;
                event.xbutton.button = u32::from(button);
                event.xbutton.x = source.x;
                event.xbutton.y = source.y;
                (event.xbutton.x_root, event.xbutton.y_root) =
                    root_position(window, source.x, source.y);
                event.xbutton.state = pointer_state();
                event.xbutton.same_screen = True;
                if let Ok(mut state) = state().lock() {
                    state.push(dpy, event);
                }
            }
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XPending")]
pub unsafe extern "sysv64" fn XPending(dpy: *mut Display) -> c_int {
    unsafe { xext::protocol::flush(dpy) };
    graphics::flush_all();
    drain_native_events(dpy);
    if let Ok(s) = state().lock() {
        let len = s.count_for(dpy);
        set_display_queue_len(dpy, len);
        len.min(c_int::MAX as usize) as c_int
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XNextEvent")]
pub unsafe extern "sysv64" fn XNextEvent(dpy: *mut Display, event_return: *mut XEvent) -> c_int {
    unsafe { event_select::next(dpy, event_return, true) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XPeekEvent")]
pub unsafe extern "sysv64" fn XPeekEvent(dpy: *mut Display, event_return: *mut XEvent) -> c_int {
    unsafe { event_select::next(dpy, event_return, false) }
}

fn linux_keycode_to_keysym(x11_code: u32) -> KeySym {
    let Some(code) = x11_code.checked_sub(8) else {
        return None;
    };
    match code {
        1 => XK_Escape,
        2 => b'1' as KeySym,
        3 => b'2' as KeySym,
        4 => b'3' as KeySym,
        5 => b'4' as KeySym,
        6 => b'5' as KeySym,
        7 => b'6' as KeySym,
        8 => b'7' as KeySym,
        9 => b'8' as KeySym,
        10 => b'9' as KeySym,
        11 => b'0' as KeySym,
        12 => b'-' as KeySym,
        13 => b'=' as KeySym,
        14 => XK_BackSpace,
        15 => XK_Tab,
        16 => b'q' as KeySym,
        17 => b'w' as KeySym,
        18 => b'e' as KeySym,
        19 => b'r' as KeySym,
        20 => b't' as KeySym,
        21 => b'y' as KeySym,
        22 => b'u' as KeySym,
        23 => b'i' as KeySym,
        24 => b'o' as KeySym,
        25 => b'p' as KeySym,
        26 => b'[' as KeySym,
        27 => b']' as KeySym,
        28 => XK_Return,
        29 => XK_Control_L,
        30 => b'a' as KeySym,
        31 => b's' as KeySym,
        32 => b'd' as KeySym,
        33 => b'f' as KeySym,
        34 => b'g' as KeySym,
        35 => b'h' as KeySym,
        36 => b'j' as KeySym,
        37 => b'k' as KeySym,
        38 => b'l' as KeySym,
        39 => b';' as KeySym,
        40 => b'\'' as KeySym,
        41 => b'`' as KeySym,
        42 => XK_Shift_L,
        43 => b'\\' as KeySym,
        44 => b'z' as KeySym,
        45 => b'x' as KeySym,
        46 => b'c' as KeySym,
        47 => b'v' as KeySym,
        48 => b'b' as KeySym,
        49 => b'n' as KeySym,
        50 => b'm' as KeySym,
        51 => b',' as KeySym,
        52 => b'.' as KeySym,
        53 => b'/' as KeySym,
        54 => XK_Shift_R,
        56 => XK_Alt_L,
        57 => XK_space,
        58 => XK_Caps_Lock,
        59 => XK_F1,
        60 => XK_F2,
        61 => XK_F3,
        62 => XK_F4,
        63 => XK_F5,
        64 => XK_F6,
        65 => XK_F7,
        66 => XK_F8,
        67 => XK_F9,
        68 => XK_F10,
        69 => 0xff7f, // Num_Lock
        70 => 0xff14, // Scroll_Lock
        87 => XK_F11,
        88 => XK_F12,
        97 => XK_Control_R,
        100 => XK_Alt_R,
        102 => XK_Home,
        103 => XK_Up,
        104 => XK_Page_Up,
        105 => XK_Left,
        106 => XK_Right,
        107 => XK_End,
        108 => XK_Down,
        109 => XK_Page_Down,
        110 => XK_Insert,
        111 => XK_Delete,
        125 => XK_Super_L,
        126 => XK_Super_R,
        127 => XK_Menu,
        _ => None,
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XLookupKeysym")]
pub unsafe extern "sysv64" fn XLookupKeysym(event: *mut XKeyEvent, _index: c_int) -> KeySym {
    if event.is_null() {
        return None;
    }
    let code = unsafe { (*event).keycode };
    linux_keycode_to_keysym(code)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XLookupString")]
pub unsafe extern "sysv64" fn XLookupString(
    event: *mut XKeyEvent,
    buffer_return: *mut c_char,
    bytes_buffer: c_int,
    keysym_return: *mut KeySym,
    _status_in_out: *mut c_void,
) -> c_int {
    if event.is_null() {
        return 0;
    }
    let keysym = unsafe { XLookupKeysym(event, 0) };
    if !keysym_return.is_null() {
        unsafe { *keysym_return = keysym };
    }
    if !buffer_return.is_null() && bytes_buffer > 0 {
        if keysym == XK_Escape {
            unsafe { *buffer_return = 27 };
            return 1;
        } else if keysym == XK_Return {
            unsafe { *buffer_return = 13 };
            return 1;
        } else if (32..=126).contains(&keysym) {
            unsafe { *buffer_return = keysym as u8 as c_char };
            return 1;
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XParseGeometry")]
pub unsafe extern "sysv64" fn XParseGeometry(
    string: *const c_char,
    x_return: *mut c_int,
    y_return: *mut c_int,
    width_return: *mut c_uint,
    height_return: *mut c_uint,
) -> c_int {
    if string.is_null() {
        return 0;
    }
    let s = match unsafe { core::ffi::CStr::from_ptr(string) }.to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut mask = 0;
    let s = s.trim_start_matches('=');
    let mut parts = s.split(|c| c == '+' || c == '-');
    if let Some(dim) = parts.next() {
        if let Some((w_str, h_str)) = dim.split_once(|c| c == 'x' || c == 'X') {
            if let Ok(w) = w_str.parse::<u32>() {
                if !width_return.is_null() {
                    unsafe { *width_return = w };
                    mask |= 1;
                }
            }
            if let Ok(h) = h_str.parse::<u32>() {
                if !height_return.is_null() {
                    unsafe { *height_return = h };
                    mask |= 2;
                }
            }
        }
    }
    if !x_return.is_null() {
        unsafe { *x_return = 0 };
    }
    if !y_return.is_null() {
        unsafe { *y_return = 0 };
    }
    mask
}

/// Geometry requests share XConfigureWindow's client-area semantics: the
/// size names the drawable, the position the client origin, and an invalid
/// window or size is an X error rather than a silent success.
unsafe fn configure_geometry(
    dpy: *mut Display,
    w: Window,
    mask: c_uint,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
) -> c_int {
    if width > i32::MAX as c_uint || height > i32::MAX as c_uint {
        return unsafe { errors::report(dpy, 2, 12, 0, w) };
    }
    let mut values: wm::XWindowChanges = unsafe { core::mem::zeroed() };
    values.x = x;
    values.y = y;
    values.width = width as c_int;
    values.height = height as c_int;
    unsafe { wm::XConfigureWindow(dpy, w, mask, &raw mut values) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XMoveWindow")]
pub unsafe extern "sysv64" fn XMoveWindow(
    dpy: *mut Display,
    w: Window,
    x: c_int,
    y: c_int,
) -> c_int {
    unsafe { configure_geometry(dpy, w, 1 | 2, x, y, 0, 0) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XResizeWindow")]
pub unsafe extern "sysv64" fn XResizeWindow(
    dpy: *mut Display,
    w: Window,
    width: c_uint,
    height: c_uint,
) -> c_int {
    unsafe { configure_geometry(dpy, w, 4 | 8, 0, 0, width, height) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XMoveResizeWindow")]
pub unsafe extern "sysv64" fn XMoveResizeWindow(
    dpy: *mut Display,
    w: Window,
    x: c_int,
    y: c_int,
    width: c_uint,
    height: c_uint,
) -> c_int {
    unsafe { configure_geometry(dpy, w, 1 | 2 | 4 | 8, x, y, width, height) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetGeometry")]
pub unsafe extern "sysv64" fn XGetGeometry(
    _dpy: *mut Display,
    w: Window,
    root_return: *mut Window,
    x_return: *mut c_int,
    y_return: *mut c_int,
    width_return: *mut c_uint,
    height_return: *mut c_uint,
    border_width_return: *mut c_uint,
    depth_return: *mut c_uint,
) -> Status {
    // GDK scopes error traps using NextRequest; even a failed query must own
    // a fresh serial so BadDrawable is caught by the trap around this call.
    unsafe { issue_request(_dpy) };
    let dimensions = if w == 1 {
        Some((
            unsafe { GetSystemMetrics(SM_CXSCREEN) },
            unsafe { GetSystemMetrics(SM_CYSCREEN) },
            24,
        ))
    } else {
        graphics::drawable_dimensions(w)
    };
    let Some((width, height, depth)) = dimensions else {
        if trace_enabled() {
            crate::diagnostic!(
                "[libX11] XGetGeometry BadDrawable drawable={w:#x} is_window={} serial={}",
                unsafe { IsWindow(w as HWND) },
                current_serial(_dpy)
            );
        }
        unsafe {
            errors::report(_dpy, 9, 14, 0, w);
        }
        return 0;
    };
    if !root_return.is_null() {
        unsafe { *root_return = 1 };
    }
    // Pixmaps have no position; a window reports its origin in its parent.
    let (x, y) = match window_tree::origin(w) {
        Some(origin) if w == 1 || unsafe { IsWindow(w as HWND) } != 0 => origin,
        _ => (0, 0),
    };
    if !x_return.is_null() {
        unsafe { *x_return = x };
    }
    if !y_return.is_null() {
        unsafe { *y_return = y };
    }
    if !width_return.is_null() {
        unsafe { *width_return = width as u32 };
    }
    if !height_return.is_null() {
        unsafe { *height_return = height as u32 };
    }
    if !border_width_return.is_null() {
        unsafe { *border_width_return = 0 };
    }
    if !depth_return.is_null() {
        unsafe { *depth_return = depth };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetWindowAttributes")]
pub unsafe extern "sysv64" fn XGetWindowAttributes(
    _dpy: *mut Display,
    w: Window,
    window_attributes_return: *mut XWindowAttributes,
) -> Status {
    let w = window_tree::native_window(w);
    unsafe { issue_request(_dpy) };
    if window_attributes_return.is_null() {
        return 0;
    }
    let hwnd = w as HWND;
    let mut rect: RECT = unsafe { core::mem::zeroed() };
    if w == 1 {
        rect.right = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        rect.bottom = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    } else if unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
        unsafe {
            errors::report(_dpy, 3, 3, 0, w);
        }
        return 0;
    }
    let Some((x, y)) = window_tree::origin(w) else {
        unsafe {
            errors::report(_dpy, 3, 3, 0, w);
        }
        return 0;
    };
    let attr = unsafe { &mut *window_attributes_return };
    *attr = unsafe { core::mem::zeroed() };
    (attr.x, attr.y) = (x, y);
    attr.width = (rect.right - rect.left).max(1);
    attr.height = (rect.bottom - rect.top).max(1);
    attr.border_width = 0;
    attr.depth = 24;
    attr.visual = unsafe { &raw mut GLOBAL_VISUAL };
    attr.root = 1;
    attr.class = if kinakaze_libdisplay::ui::input_only::is_input_only(w) {
        2
    } else {
        1
    };
    if attr.class == 2 {
        attr.depth = 0;
        attr.visual = core::ptr::null_mut();
    }
    attr.all_event_masks = shared::selected_masks(w) as c_long;
    attr.your_event_mask = connection::selected(_dpy, w) as c_long;
    attr.colormap = colormap::window_map(_dpy, w);
    attr.map_installed = True;
    attr.override_redirect = shared::override_redirect(w) as Bool;
    attr.map_state = if !shared::mapped(w) {
        0
    } else {
        let mut parent = shared::parent(w);
        let mut viewable = 2;
        for _ in 0..1024 {
            if parent == 1 {
                break;
            }
            if !shared::mapped(parent) {
                viewable = 1;
                break;
            }
            parent = shared::parent(parent);
        }
        viewable
    };
    attr.screen = unsafe { (*_dpy).screens };
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XInitThreads")]
pub unsafe extern "sysv64" fn XInitThreads() -> Status {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XLockDisplay")]
pub unsafe extern "sysv64" fn XLockDisplay(_dpy: *mut Display) {}

#[unsafe(export_name = "kinakaze_engine_libX11_XUnlockDisplay")]
pub unsafe extern "sysv64" fn XUnlockDisplay(_dpy: *mut Display) {}

pub mod composite;
pub mod context;
pub mod damage;
pub mod errors;
pub mod event_select;
pub mod event_wire;
pub mod shape;
pub mod xfixes;
pub use event_select::*;
pub mod focus;
pub mod font_metrics;
pub mod frame_sync;
pub mod graphics;
pub mod image;
pub mod input_bridge;
pub mod input_method;
pub mod keyboard;
pub mod shared;
pub use focus::{XGetInputFocus, XSetInputFocus};
pub mod property;
pub use property::*;
pub mod text;
pub use input_method::*;
pub use keyboard::*;
pub use text::*;
mod accessors;
mod colormap;
mod pointer_control;
pub mod region;
pub mod render;
mod text_draw;
pub use colormap::*;
mod screensaver;
pub mod window_tree;
pub use window_tree::{XQueryTree, XReparentWindow, XTranslateCoordinates};
pub mod wm;
mod wm_properties;
pub mod xcursor;
pub mod xext;
pub mod xf86vmode;
pub mod xinerama;
pub mod xkb;
pub mod xrm;

pub use context::*;
pub use graphics::*;
pub use region::*;
pub use render::*;
pub use wm::*;
pub use xcursor::*;
pub use xext::*;
pub use xf86vmode::*;
pub use xinerama::*;
pub use xkb::*;
pub use xrm::*;

pub type KeyCode = u8;

#[unsafe(export_name = "kinakaze_engine_libX11_XDisplayKeycodes")]
pub unsafe extern "sysv64" fn XDisplayKeycodes(
    _dpy: *mut Display,
    min_keycodes_return: *mut c_int,
    max_keycodes_return: *mut c_int,
) -> Status {
    if !min_keycodes_return.is_null() {
        unsafe { *min_keycodes_return = 8 };
    }
    if !max_keycodes_return.is_null() {
        unsafe { *max_keycodes_return = 255 };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetKeyboardMapping")]
pub unsafe extern "sysv64" fn XGetKeyboardMapping(
    _dpy: *mut Display,
    first_keycode: KeyCode,
    keycode_count: c_int,
    keysyms_per_keycode_return: *mut c_int,
) -> *mut KeySym {
    if !keysyms_per_keycode_return.is_null() {
        unsafe { *keysyms_per_keycode_return = 2 };
    }
    if keycode_count < 0 || first_keycode < 8 || first_keycode as i32 + keycode_count > 256 {
        unsafe { errors::report(_dpy, 2, 101, 0, first_keycode as usize) };
        return core::ptr::null_mut();
    }
    let total = keycode_count as usize;
    let ptr =
        unsafe { kinakaze_alloc::guest::malloc(total * 2 * size_of::<KeySym>()) }.cast::<KeySym>();
    if ptr.is_null() {
        return ptr;
    }
    for i in 0..total {
        let kc = (first_keycode as usize) + i;
        for (level, sym) in xkb::server::symbols(kc as u8).into_iter().enumerate() {
            unsafe {
                ptr.add(i * 2 + level).write(sym as usize);
            }
        }
    }
    ptr
}

#[repr(C)]
pub struct XModifierKeymap {
    pub max_keypermod: c_int,
    pub modifiermap: *mut KeyCode,
}

#[unsafe(export_name = "kinakaze_engine_libX11_XGetModifierMapping")]
pub unsafe extern "sysv64" fn XGetModifierMapping(_dpy: *mut Display) -> *mut XModifierKeymap {
    let map =
        unsafe { kinakaze_alloc::guest::malloc(core::mem::size_of::<XModifierKeymap>() + 16) }
            .cast::<XModifierKeymap>();
    if !map.is_null() {
        unsafe {
            let keys = map.add(1).cast::<KeyCode>();
            core::ptr::copy_nonoverlapping(keyboard::MODIFIERS.as_ptr(), keys, 16);
            map.write(XModifierKeymap {
                max_keypermod: 2,
                modifiermap: keys,
            });
        }
    }
    map
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeModifiermap")]
pub unsafe extern "sysv64" fn XFreeModifiermap(modmap: *mut XModifierKeymap) -> c_int {
    unsafe {
        kinakaze_alloc::guest::free(modmap.cast());
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XKeycodeToKeysym")]
pub unsafe extern "sysv64" fn XKeycodeToKeysym(
    _dpy: *mut Display,
    keycode: KeyCode,
    _index: c_int,
) -> KeySym {
    if !(0..2).contains(&_index) {
        return 0;
    }
    xkb::server::symbols(keycode)[_index as usize] as usize
}

#[unsafe(export_name = "kinakaze_engine_libX11_XKeysymToKeycode")]
pub unsafe extern "sysv64" fn XKeysymToKeycode(_dpy: *mut Display, keysym: KeySym) -> KeyCode {
    (8u16..=255)
        .find(|keycode| xkb::server::symbols(*keycode as u8).contains(&(keysym as u32)))
        .map_or(0, |keycode| keycode as KeyCode)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XKeysymToString")]
pub unsafe extern "sysv64" fn XKeysymToString(_keysym: KeySym) -> *const c_char {
    c"".as_ptr()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XStringToKeysym")]
pub unsafe extern "sysv64" fn XStringToKeysym(_string: *const c_char) -> KeySym {
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbGetMap")]
pub unsafe extern "sysv64" fn XkbGetMap(
    _dpy: *mut Display,
    _which: c_uint,
    _device_spec: c_uint,
) -> *mut c_void {
    unsafe { xkb::client::get_map(_dpy, _which, _device_spec).cast() }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbFreeClientMap")]
pub unsafe extern "sysv64" fn XkbFreeClientMap(xkb: *mut c_void, which: c_uint, free_all: Bool) {
    unsafe { xkb::data::free_client_map(xkb.cast(), which, free_all) };
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbFreeKeyboard")]
pub unsafe extern "sysv64" fn XkbFreeKeyboard(xkb: *mut c_void, which: c_uint, free_all: Bool) {
    unsafe { xkb::data::free_keyboard(xkb.cast(), which, free_all) };
}

#[unsafe(export_name = "kinakaze_engine_libX11_XkbQueryExtension")]
pub unsafe extern "sysv64" fn XkbQueryExtension(
    _dpy: *mut Display,
    _opcode_rtrn: *mut c_int,
    _event_rtrn: *mut c_int,
    _error_rtrn: *mut c_int,
    _major_in_out: *mut c_int,
    _minor_in_out: *mut c_int,
) -> Bool {
    if !_major_in_out.is_null() && unsafe { *_major_in_out } > 1 {
        return 0;
    }
    for (out, value) in [
        (_opcode_rtrn, xkb::server::OPCODE as i32),
        (_event_rtrn, xkb::server::EVENT as i32),
        (_error_rtrn, xkb::server::ERROR as i32),
        (_major_in_out, 1),
        (_minor_in_out, 0),
    ] {
        if !out.is_null() {
            unsafe { out.write(value) };
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSupportsLocale")]
pub unsafe extern "sysv64" fn XSupportsLocale() -> Bool {
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetLocaleModifiers")]
pub unsafe extern "sysv64" fn XSetLocaleModifiers(_modifier_list: *const c_char) -> *mut c_char {
    c"".as_ptr() as *mut c_char
}

#[unsafe(export_name = "kinakaze_engine_libX11_XCreateFontSet")]
pub unsafe extern "sysv64" fn XCreateFontSet(
    _dpy: *mut Display,
    _base_font_name_list: *const c_char,
    missing_charset_list_return: *mut *mut *mut c_char,
    missing_charset_count_return: *mut c_int,
    def_string_return: *mut *mut c_char,
) -> *mut c_void {
    if !missing_charset_count_return.is_null() {
        unsafe { *missing_charset_count_return = 0 };
    }
    if !missing_charset_list_return.is_null() {
        unsafe { *missing_charset_list_return = core::ptr::null_mut() };
    }
    if !def_string_return.is_null() {
        unsafe { *def_string_return = core::ptr::null_mut() };
    }
    static mut DUMMY_FONTSET: usize = 1;
    &raw mut DUMMY_FONTSET as *mut c_void
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeFontSet")]
pub unsafe extern "sysv64" fn XFreeFontSet(_dpy: *mut Display, _font_set: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xstore_name_preserves_utf8_titles() {
        let window = kinakaze_libdisplay::ui::create(160, 120, "initial").expect("window");
        let title = std::ffi::CString::new("Minecraft - 单人游戏").unwrap();
        assert_eq!(
            unsafe { XStoreName(core::ptr::null_mut(), window, title.as_ptr()) },
            1
        );
        let mut text = [0_u16; 128];
        let length =
            unsafe { GetWindowTextW(window as HWND, text.as_mut_ptr(), text.len() as i32) };
        assert!(length > 0);
        assert_eq!(
            String::from_utf16_lossy(&text[..length as usize]),
            "Minecraft - 单人游戏"
        );
        assert!(kinakaze_libdisplay::ui::destroy(window));
    }

    #[test]
    fn override_redirect_changes_policy_without_inventing_fullscreen() {
        let window = kinakaze_libdisplay::ui::create(320, 240, "override").expect("window");
        let drawable = kinakaze_libdisplay::ui::client_size(window);
        let mut attributes: XSetWindowAttributes = unsafe { core::mem::zeroed() };
        attributes.override_redirect = True;
        assert_eq!(
            unsafe {
                crate::wm::XChangeWindowAttributes(
                    core::ptr::null_mut(),
                    window,
                    1 << 9,
                    &raw mut attributes,
                )
            },
            1
        );
        assert!(kinakaze_libdisplay::ui::is_override_redirect(window));
        assert!(!kinakaze_libdisplay::ui::is_fullscreen(window));
        assert_eq!(kinakaze_libdisplay::ui::client_size(window), drawable);
        attributes.override_redirect = False;
        unsafe {
            crate::wm::XChangeWindowAttributes(
                core::ptr::null_mut(),
                window,
                1 << 9,
                &raw mut attributes,
            );
        }
        assert!(!kinakaze_libdisplay::ui::is_override_redirect(window));
        assert!(!kinakaze_libdisplay::ui::is_fullscreen(window));
        assert_eq!(kinakaze_libdisplay::ui::client_size(window), drawable);
        assert!(kinakaze_libdisplay::ui::destroy(window));
    }

    #[test]
    fn event_layout_matches_linux_x86_64_xlib() {
        assert_eq!(core::mem::size_of::<c_long>(), 8);
        assert_eq!(core::mem::size_of::<c_ulong>(), 8);
        assert_eq!(core::mem::size_of::<XConfigureEvent>(), 88);
        assert_eq!(core::mem::offset_of!(XConfigureEvent, width), 56);
        assert_eq!(core::mem::offset_of!(XConfigureEvent, height), 60);
        assert_eq!(core::mem::size_of::<XClientMessageEvent>(), 96);
        assert_eq!(core::mem::offset_of!(XClientMessageEvent, data), 56);
        assert_eq!(core::mem::size_of::<XGenericEventCookie>(), 56);
        assert_eq!(core::mem::offset_of!(XGenericEventCookie, data), 48);
        assert_eq!(core::mem::size_of::<XEvent>(), 192);
    }

    #[test]
    fn core_keycodes_use_the_standard_evdev_offset_and_keysyms() {
        // Linux KEY_W is 17 and the Xorg evdev mapping exposes it as X keycode 25.
        assert_eq!(linux_keycode_to_keysym(17 + 8), b'w' as KeySym);
        assert_eq!(linux_keycode_to_keysym(30 + 8), b'a' as KeySym);
        assert_eq!(linux_keycode_to_keysym(31 + 8), b's' as KeySym);
        assert_eq!(linux_keycode_to_keysym(32 + 8), b'd' as KeySym);
        assert_eq!(
            unsafe { XKeysymToKeycode(core::ptr::null_mut(), b'w' as KeySym) },
            25
        );
    }

    #[test]
    fn render_is_advertised_only_with_real_formats_and_shape_stays_absent() {
        let mut event_base = -1;
        let mut error_base = -1;
        assert_eq!(
            unsafe {
                crate::render::XRenderQueryExtension(
                    core::ptr::null_mut(),
                    &raw mut event_base,
                    &raw mut error_base,
                )
            },
            1
        );
        assert_eq!((event_base, error_base), (68, 132));
        let render =
            unsafe { crate::xext::XInitExtension(core::ptr::null_mut(), c"RENDER".as_ptr()) };
        assert!(!render.is_null());
        assert_eq!(unsafe { (*render).major_opcode }, 129);
        unsafe { drop(Box::from_raw(render)) };
        assert!(
            !unsafe { crate::render::XRenderFindStandardFormat(core::ptr::null_mut(), 0) }
                .is_null()
        );
        assert!(
            !unsafe { crate::render::XRenderFindStandardFormat(core::ptr::null_mut(), 2) }
                .is_null()
        );

        assert!(
            unsafe { crate::xext::XInitExtension(core::ptr::null_mut(), c"SHAPE".as_ptr()) }
                .is_null()
        );

        let xinput = unsafe {
            crate::xext::XInitExtension(core::ptr::null_mut(), c"XInputExtension".as_ptr())
        };
        assert!(!xinput.is_null());
        assert_eq!(unsafe { (*xinput).major_opcode }, 128);
        unsafe { drop(Box::from_raw(xinput)) };
    }

    #[test]
    fn render_solid_polygon_changes_real_back_buffer_pixels() {
        let display = unsafe { XOpenDisplay(core::ptr::null()) };
        assert!(!display.is_null());
        let window = unsafe { XCreateSimpleWindow(display, 1, 0, 0, 100, 100, 0, 0, 0) };
        assert_ne!(window, 0);
        unsafe { crate::graphics::XClearWindow(display, window) };

        let format = unsafe { crate::render::XRenderFindStandardFormat(display, 1) };
        assert!(!format.is_null());
        let destination = unsafe {
            crate::render::XRenderCreatePicture(display, window, format, 0, core::ptr::null())
        };
        let red = crate::render::XRenderColor {
            red: u16::MAX,
            green: 0,
            blue: 0,
            alpha: u16::MAX,
        };
        let source = unsafe { crate::render::XRenderCreateSolidFill(display, &red) };
        assert_ne!(destination, 0);
        assert_ne!(source, 0);

        let points = [
            crate::render::XPointDouble { x: 10.0, y: 10.0 },
            crate::render::XPointDouble { x: 90.0, y: 10.0 },
            crate::render::XPointDouble { x: 90.0, y: 90.0 },
            crate::render::XPointDouble { x: 10.0, y: 90.0 },
        ];
        unsafe {
            crate::render::XRenderCompositeDoublePoly(
                display,
                3,
                source,
                destination,
                core::ptr::null(),
                0,
                0,
                0,
                0,
                points.as_ptr(),
                points.len() as c_int,
                0,
            );
        }
        let pixel = crate::graphics::back_buffer_pixel(window, 50, 50).expect("back-buffer pixel");
        assert_eq!(pixel & 0x00ff_ffff, 0x0000_00ff);

        unsafe {
            crate::render::XRenderFreePicture(display, source);
            crate::render::XRenderFreePicture(display, destination);
            XDestroyWindow(display, window);
            XCloseDisplay(display);
        }
    }

    #[test]
    fn native_events_are_mapped_back_to_the_x11_window() {
        let display = unsafe { XOpenDisplay(core::ptr::null()) };
        assert!(!display.is_null());
        let window = unsafe { XCreateSimpleWindow(display, 1, 0, 0, 320, 240, 0, 0, 0) };
        assert_ne!(window, 0);
        // ConfigureNotify and Expose reach only clients that selected them.
        unsafe { XSelectInput(display, window, (1 << 15) | (1 << 17)) };

        let logical = state()
            .lock()
            .expect("X11 state")
            .hwnd_to_guest
            .get(&window)
            .copied()
            .expect("HWND to stable guest id mapping");
        assert_ne!(logical as usize, window);

        if let Ok(mut queue) = kinakaze_libdisplay::event::queue().lock() {
            while queue.pop().is_some() {}
        }
        state().lock().expect("X11 state").pending_events.clear();

        kinakaze_libdisplay::event::publish(kinakaze_libdisplay::event::Event {
            kind: kinakaze_libdisplay::event::EVENT_POINTER_MOTION,
            x: 17,
            y: 29,
            window: logical,
            ..Default::default()
        });
        kinakaze_libdisplay::event::publish(kinakaze_libdisplay::event::Event {
            kind: kinakaze_libdisplay::event::EVENT_FOCUS,
            state: 1,
            window: logical,
            ..Default::default()
        });
        kinakaze_libdisplay::event::publish(kinakaze_libdisplay::event::Event {
            kind: kinakaze_libdisplay::event::EVENT_SCROLL,
            scroll_y: 1,
            window: logical,
            ..Default::default()
        });
        kinakaze_libdisplay::event::publish(kinakaze_libdisplay::event::Event {
            kind: kinakaze_libdisplay::event::EVENT_RESIZE,
            x: 800,
            y: 600,
            window: logical,
            ..Default::default()
        });
        kinakaze_libdisplay::event::publish(kinakaze_libdisplay::event::Event {
            kind: kinakaze_libdisplay::event::EVENT_CLOSE,
            window: logical,
            ..Default::default()
        });

        assert_eq!(unsafe { XPending(display) }, 7);
        // GLFW calls XPending to transfer native events, then reads this field
        // through Xlib's QLength macro instead of using XPending's return value.
        assert_eq!(unsafe { (*display).qlen }, 7);
        let mut event: XEvent = unsafe { core::mem::zeroed() };

        assert_eq!(unsafe { XNextEvent(display, &raw mut event) }, 0);
        assert_eq!(unsafe { (*display).qlen }, 6);
        let motion = unsafe { event.xmotion };
        assert_eq!(
            (motion.r#type, motion.window, motion.x, motion.y),
            (MotionNotify, window, 17, 29)
        );

        assert_eq!(unsafe { XNextEvent(display, &raw mut event) }, 0);
        assert_eq!(unsafe { (*display).qlen }, 5);
        let focus = unsafe { event.xfocus };
        assert_eq!((focus.r#type, focus.window), (FocusIn, window));

        for expected_type in [ButtonPress, ButtonRelease] {
            assert_eq!(unsafe { XNextEvent(display, &raw mut event) }, 0);
            let wheel = unsafe { event.xbutton };
            assert_eq!(
                (wheel.r#type, wheel.window, wheel.button),
                (expected_type, window, 4)
            );
        }

        assert_eq!(unsafe { XNextEvent(display, &raw mut event) }, 0);
        let configure = unsafe { event.xconfigure };
        assert_eq!(
            (
                configure.r#type,
                configure.event,
                configure.window,
                configure.width,
                configure.height
            ),
            (ConfigureNotify, window, window, 800, 600)
        );
        assert_eq!(unsafe { event.xany.window }, window);

        assert_eq!(unsafe { XNextEvent(display, &raw mut event) }, 0);
        let expose = unsafe { event.xexpose };
        assert_eq!(
            (expose.r#type, expose.window, expose.width, expose.height),
            (Expose, window, 800, 600)
        );

        assert_eq!(unsafe { XNextEvent(display, &raw mut event) }, 0);
        let close = unsafe { event.xclient };
        assert_eq!(
            (close.r#type, close.window, close.message_type),
            (ClientMessage, window, unsafe {
                XInternAtom(display, c"WM_PROTOCOLS".as_ptr(), 0)
            })
        );
        assert_eq!(
            close.data[0],
            unsafe { XInternAtom(display, c"WM_DELETE_WINDOW".as_ptr(), 0) } as i64
        );
        assert_eq!(unsafe { (*display).qlen }, 0);

        assert_eq!(unsafe { XDestroyWindow(display, window) }, 0);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFontsOfFontSet")]
pub unsafe extern "sysv64" fn XFontsOfFontSet(
    _font_set: *mut c_void,
    font_struct_list_return: *mut *mut *mut c_void,
    font_name_list_return: *mut *mut *mut c_char,
) -> c_int {
    if !font_struct_list_return.is_null() {
        unsafe { *font_struct_list_return = core::ptr::null_mut() };
    }
    if !font_name_list_return.is_null() {
        unsafe { *font_name_list_return = core::ptr::null_mut() };
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XBaseFontNameListOfFontSet")]
pub unsafe extern "sysv64" fn XBaseFontNameListOfFontSet(_font_set: *mut c_void) -> *mut c_char {
    c"fixed".as_ptr() as *mut c_char
}

#[unsafe(export_name = "kinakaze_engine_libX11_XLocaleOfFontSet")]
pub unsafe extern "sysv64" fn XLocaleOfFontSet(_font_set: *mut c_void) -> *mut c_char {
    c"C".as_ptr() as *mut c_char
}

#[unsafe(export_name = "kinakaze_engine_libX11_XExtentsOfFontSet")]
pub unsafe extern "sysv64" fn XExtentsOfFontSet(_font_set: *mut c_void) -> *mut c_void {
    static mut DUMMY_EXTENTS: [c_int; 4] = [0, 0, 10, 10];
    &raw mut DUMMY_EXTENTS as *mut c_void
}

#[unsafe(export_name = "kinakaze_engine_libX11_XmbTextExtents")]
pub unsafe extern "sysv64" fn XmbTextExtents(
    _font_set: *mut c_void,
    _text: *const c_char,
    text_len: c_int,
    _overall_ink_return: *mut c_void,
    _overall_logical_return: *mut c_void,
) -> c_int {
    unsafe {
        text_draw::mb_extents(
            _text,
            text_len,
            _overall_ink_return.cast(),
            _overall_logical_return.cast(),
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_Xutf8TextExtents")]
pub unsafe extern "sysv64" fn Xutf8TextExtents(
    _font_set: *mut c_void,
    _text: *const c_char,
    text_len: c_int,
    _overall_ink_return: *mut c_void,
    _overall_logical_return: *mut c_void,
) -> c_int {
    unsafe {
        text_draw::mb_extents(
            _text,
            text_len,
            _overall_ink_return.cast(),
            _overall_logical_return.cast(),
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XmbDrawString")]
pub unsafe extern "sysv64" fn XmbDrawString(
    _dpy: *mut Display,
    _d: Drawable,
    _font_set: *mut c_void,
    _gc: GC,
    _x: c_int,
    _y: c_int,
    _text: *const c_char,
    _text_len: c_int,
) {
    unsafe {
        text_draw::mb_draw(_d, _gc, _x, _y, _text, _text_len);
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_Xutf8DrawString")]
pub unsafe extern "sysv64" fn Xutf8DrawString(
    _dpy: *mut Display,
    _d: Drawable,
    _font_set: *mut c_void,
    _gc: GC,
    _x: c_int,
    _y: c_int,
    _text: *const c_char,
    _text_len: c_int,
) {
    unsafe {
        text_draw::mb_draw(_d, _gc, _x, _y, _text, _text_len);
    }
}

mod object_layout;

/// Module-owned display resources are released before the event connection.
/// Callbacks register at module initialization; they are not guest heap state.
type DisplayClose = unsafe fn(*mut Display);
static DISPLAY_CLOSE: Mutex<Vec<DisplayClose>> = Mutex::new(Vec::new());
pub fn register_display_close(callback: DisplayClose) {
    let mut callbacks = DISPLAY_CLOSE.lock().unwrap_or_else(|e| e.into_inner());
    if !callbacks
        .iter()
        .any(|entry| *entry as usize == callback as usize)
    {
        callbacks.push(callback);
    }
}
/// Extension state tied to a window (XI grabs and selections, for one) ends
/// with the window, as the server releases a grab whose window is destroyed.
type WindowDestroy = fn(Window);
static WINDOW_DESTROY: Mutex<Vec<WindowDestroy>> = Mutex::new(Vec::new());
pub fn register_window_destroy(callback: WindowDestroy) {
    let mut callbacks = WINDOW_DESTROY.lock().unwrap_or_else(|e| e.into_inner());
    if !callbacks
        .iter()
        .any(|entry| *entry as usize == callback as usize)
    {
        callbacks.push(callback);
    }
}
fn destroy_extensions(window: Window) {
    let callbacks = WINDOW_DESTROY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    for callback in callbacks {
        callback(window);
    }
}
fn close_extensions(display: *mut Display) {
    let callbacks = DISPLAY_CLOSE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    for callback in callbacks {
        unsafe {
            callback(display);
        }
    }
}
/// Append an extension event and wake clients waiting on ConnectionNumber.
pub fn queue_extension_event(display: *mut Display, event: XEvent) {
    if let Ok(mut state) = state().lock() {
        state.push_to(display, event);
        set_display_queue_len(display, state.count_for(display));
    }
    unsafe {
        notify_x11_event();
    }
}

/// Queues a structure event for `window` subject to StructureNotify and the
/// parent's SubstructureNotify selection.
pub(crate) fn queue_structure_event(display: *mut Display, window: Window, event: XEvent) {
    if let Ok(mut state) = state().lock() {
        state.queue_structure(window, event);
        set_display_queue_len(display, state.count_for(display));
    }
    unsafe {
        notify_x11_event();
    }
}

/// Transfer a core event to the frontend which owns the shared connection.
pub fn take_wire_event() -> Option<XEvent> {
    let display = connection::xcb_display();
    let mut state = state().lock().unwrap();
    let index = state
        .pending_events
        .iter()
        .position(|event| unsafe { event.xany.display == display } && matches!(unsafe{event.r#type},2..=10|12|18|19|21|22|28..=31|33))?;
    let event = state.pending_events.remove(index);
    set_display_queue_len(display, state.count_for(display));
    event
}
pub fn try_take_wire_event() -> Result<Option<XEvent>, i32> {
    let display = connection::try_xcb_display()?;
    let mut state = state().try_lock().map_err(|_| 11)?;
    let Some(index) = state
        .pending_events
        .iter()
        .position(|event| unsafe { event.xany.display == display } && matches!(unsafe{event.r#type},2..=10|12|18|19|21|22|28..=31|33))
    else {
        return Ok(Option::None);
    };
    let event = state.pending_events.remove(index);
    set_display_queue_len(display, state.count_for(display));
    Ok(event)
}
