//! `libxcb.dll`: native Win32 windowing & event translation for Linux XCB applications.
//!
//! Provides `libxcb.so.1` ABI to Linux guest binaries without requiring an external
//! X server. XCB window creation (`xcb_create_window`) maps directly to real Win32
//! `HWND` windows via `kinakaze_libdisplay`.

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code
)]

use core::ffi::{c_char, c_int, c_uint, c_void};
use kinakaze_libX11 as x11;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

mod display_name;
mod property;
mod request;
pub use property::*;
mod query;
pub use query::*;
mod lifecycle;
mod window;
pub use window::*;
mod input;
pub use input::*;
mod resources;
pub use resources::*;
mod selection;
pub use selection::*;
mod drawing;
pub use drawing::*;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
};

pub type xcb_window_t = u32;
pub type xcb_colormap_t = u32;
pub type xcb_visualid_t = u32;
pub type xcb_atom_t = u32;
pub type xcb_keycode_t = u8;
pub type xcb_timestamp_t = u32;

#[repr(C)]
pub struct xcb_connection_t {
    pub dummy: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_generic_event_t {
    pub response_type: u8,
    pub pad0: u8,
    pub sequence: u16,
    pub pad: [u32; 7],
    pub full_sequence: u32,
}

unsafe impl Send for xcb_generic_event_t {}
unsafe impl Sync for xcb_generic_event_t {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_generic_error_t {
    pub response_type: u8,
    pub error_code: u8,
    pub sequence: u16,
    pub resource_id: u32,
    pub minor_code: u16,
    pub major_code: u8,
    pub pad0: u8,
    pub pad: [u32; 5],
    pub full_sequence: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_screen_t {
    pub root: xcb_window_t,
    pub default_colormap: xcb_colormap_t,
    pub white_pixel: u32,
    pub black_pixel: u32,
    pub current_input_masks: u32,
    pub width_in_pixels: u16,
    pub height_in_pixels: u16,
    pub width_in_millimeters: u16,
    pub height_in_millimeters: u16,
    pub min_installed_maps: u16,
    pub max_installed_maps: u16,
    pub root_visual: xcb_visualid_t,
    pub backing_stores: u8,
    pub save_unders: u8,
    pub root_depth: u8,
    pub allowed_depths_len: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_screen_iterator_t {
    pub data: *mut xcb_screen_t,
    pub rem: c_int,
    pub index: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_setup_t {
    pub status: u8,
    pub pad0: u8,
    pub protocol_major_version: u16,
    pub protocol_minor_version: u16,
    pub length: u16,
    pub release_number: u32,
    pub resource_id_base: u32,
    pub resource_id_mask: u32,
    pub motion_buffer_size: u32,
    pub vendor_len: u16,
    pub maximum_request_length: u16,
    pub roots_len: u8,
    pub pixmap_formats_len: u8,
    pub image_byte_order: u8,
    pub bitmap_format_bit_order: u8,
    pub bitmap_format_scanline_unit: u8,
    pub bitmap_format_scanline_pad: u8,
    pub min_keycode: xcb_keycode_t,
    pub max_keycode: xcb_keycode_t,
    pub pad1: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_void_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_intern_atom_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
pub struct xcb_intern_atom_reply_t {
    pub response_type: u8,
    pub pad0: u8,
    pub sequence: u16,
    pub length: u32,
    pub atom: xcb_atom_t,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_get_geometry_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_get_image_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_get_property_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_translate_coordinates_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_grab_pointer_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_query_pointer_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_get_input_focus_cookie_t {
    pub sequence: c_uint,
}

#[repr(C)]
pub struct xcb_get_geometry_reply_t {
    pub response_type: u8,
    pub depth: u8,
    pub sequence: u16,
    pub length: u32,
    pub root: xcb_window_t,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub border_width: u16,
    pub pad0: [u8; 2],
}

#[repr(C)]
pub struct xcb_get_image_reply_t {
    pub response_type: u8,
    pub depth: u8,
    pub sequence: u16,
    pub length: u32,
    pub visual: xcb_visualid_t,
    pub pad0: [u8; 20],
}

#[repr(C)]
pub struct xcb_get_property_reply_t {
    pub response_type: u8,
    pub format: u8,
    pub sequence: u16,
    pub length: u32,
    pub r#type: xcb_atom_t,
    pub bytes_after: u32,
    pub value_len: u32,
    pub pad0: [u8; 12],
}

#[repr(C)]
pub struct xcb_translate_coordinates_reply_t {
    pub response_type: u8,
    pub same_screen: u8,
    pub sequence: u16,
    pub length: u32,
    pub child: xcb_window_t,
    pub dst_x: i16,
    pub dst_y: i16,
}

#[repr(C)]
pub struct xcb_grab_pointer_reply_t {
    pub response_type: u8,
    pub status: u8,
    pub sequence: u16,
    pub length: u32,
}

#[repr(C)]
pub struct xcb_query_pointer_reply_t {
    pub response_type: u8,
    pub same_screen: u8,
    pub sequence: u16,
    pub length: u32,
    pub root: xcb_window_t,
    pub child: xcb_window_t,
    pub root_x: i16,
    pub root_y: i16,
    pub win_x: i16,
    pub win_y: i16,
    pub mask: u16,
    pub pad0: [u8; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_rectangle_t {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_format_t {
    pub depth: u8,
    pub bits_per_pixel: u8,
    pub scanline_pad: u8,
    pub pad0: [u8; 5],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_visualtype_t {
    pub visual_id: xcb_visualid_t,
    pub class: u8,
    pub bits_per_rgb_value: u8,
    pub colormap_entries: u16,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub pad0: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_visualtype_iterator_t {
    pub data: *mut xcb_visualtype_t,
    pub rem: c_int,
    pub index: c_int,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_depth_t {
    pub depth: u8,
    pub pad0: u8,
    pub visuals_len: u16,
    pub pad1: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_depth_iterator_t {
    pub data: *mut xcb_depth_t,
    pub rem: c_int,
    pub index: c_int,
}

#[derive(Clone)]
pub struct XcbGc {
    pub foreground: u32,
    pub background: u32,
    pub line_width: u16,
    pub depth: u8,
    pub graphics_exposures: bool,
    pub function: u8,
    pub plane_mask: u32,
    pub clip_origin: (i16, i16),
    pub clip: Option<std::sync::Arc<[xcb_rectangle_t]>>,
}
impl Default for XcbGc {
    fn default() -> Self {
        Self {
            foreground: 0,
            background: 1,
            line_width: 0,
            depth: 24,
            graphics_exposures: true,
            function: 3,
            plane_mask: u32::MAX,
            clip_origin: (0, 0),
            clip: None,
        }
    }
}

#[derive(Clone)]
pub struct XcbPixmap {
    pub width: u16,
    pub height: u16,
    pub depth: u8,
    pub data: Vec<u32>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_key_press_event_t {
    pub response_type: u8,
    pub detail: xcb_keycode_t,
    pub sequence: u16,
    pub time: xcb_timestamp_t,
    pub root: xcb_window_t,
    pub event: xcb_window_t,
    pub child: xcb_window_t,
    pub root_x: i16,
    pub root_y: i16,
    pub event_x: i16,
    pub event_y: i16,
    pub state: u16,
    pub same_screen: u8,
    pub pad0: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_map_notify_event_t {
    pub response_type: u8,
    pub pad0: u8,
    pub sequence: u16,
    pub event: xcb_window_t,
    pub window: xcb_window_t,
    /// `override_redirect` for MapNotify, `from_configure` for UnmapNotify.
    pub flag: u8,
    pub pad1: [u8; 3],
}

#[repr(C)]
pub struct xcb_configure_notify_event_t {
    pub response_type: u8,
    pub pad0: u8,
    pub sequence: u16,
    pub event: xcb_window_t,
    pub window: xcb_window_t,
    pub above_sibling: xcb_window_t,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub border_width: u16,
    pub override_redirect: u8,
    pub pad1: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct xcb_client_message_event_t {
    pub response_type: u8,
    pub format: u8,
    pub sequence: u16,
    pub window: xcb_window_t,
    pub r#type: xcb_atom_t,
    pub data: [u32; 5],
}

pub const XCB_KEY_PRESS: u8 = 2;
pub const XCB_KEY_RELEASE: u8 = 3;
pub const XCB_BUTTON_PRESS: u8 = 4;
pub const XCB_BUTTON_RELEASE: u8 = 5;
pub const XCB_MOTION_NOTIFY: u8 = 6;
pub const XCB_ENTER_NOTIFY: u8 = 7;
pub const XCB_LEAVE_NOTIFY: u8 = 8;
pub const XCB_EXPOSE: u8 = 12;
pub const XCB_UNMAP_NOTIFY: u8 = 18;
pub const XCB_MAP_NOTIFY: u8 = 19;
pub const XCB_CONFIGURE_NOTIFY: u8 = 22;
pub const XCB_CLIENT_MESSAGE: u8 = 33;

struct XcbState {
    connection_error: c_int,
    pending_events: VecDeque<xcb_generic_event_t>,
    next_id: u32,
    wid_to_hwnd: HashMap<xcb_window_t, usize>,
    hwnd_to_wid: HashMap<usize, xcb_window_t>,
    guest_to_wid: HashMap<u64, xcb_window_t>,
    /// Geometry last reported by ConfigureNotify; Win32 announces one change
    /// as separate move and size messages.
    reported_geometry: HashMap<xcb_window_t, (i32, i32, i32, i32)>,
    wid_to_guest: HashMap<xcb_window_t, u64>,
    gcs: HashMap<u32, XcbGc>,
    pixmaps: HashMap<u32, XcbPixmap>,
}

unsafe impl Send for XcbState {}
unsafe impl Sync for XcbState {}

impl Default for XcbState {
    fn default() -> Self {
        Self {
            connection_error: 0,
            pending_events: Default::default(),
            next_id: 0x1000_0000,
            wid_to_hwnd: Default::default(),
            hwnd_to_wid: Default::default(),
            guest_to_wid: Default::default(),
            reported_geometry: Default::default(),
            wid_to_guest: Default::default(),
            gcs: Default::default(),
            pixmaps: Default::default(),
        }
    }
}
fn xcb_state() -> &'static Mutex<XcbState> {
    static STATE: OnceLock<Mutex<XcbState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(XcbState::default()))
}

static PIXMAP_FORMATS: [xcb_format_t; 3] = [
    xcb_format_t {
        depth: 1,
        bits_per_pixel: 1,
        scanline_pad: 32,
        pad0: [0; 5],
    },
    xcb_format_t {
        depth: 24,
        bits_per_pixel: 32,
        scanline_pad: 32,
        pad0: [0; 5],
    },
    xcb_format_t {
        depth: 32,
        bits_per_pixel: 32,
        scanline_pad: 32,
        pad0: [0; 5],
    },
];

const SCREEN_VISUALS: [xcb_visualtype_t; 1] = [xcb_visualtype_t {
    visual_id: 1,
    class: 4, // TrueColor
    bits_per_rgb_value: 8,
    colormap_entries: 256,
    red_mask: 0x00ff0000,
    green_mask: 0x0000ff00,
    blue_mask: 0x000000ff,
    pad0: [0; 4],
}];

const SCREEN_DEPTHS: [xcb_depth_t; 2] = [
    xcb_depth_t {
        depth: 24,
        pad0: 0,
        visuals_len: 1,
        pad1: [0; 4],
    },
    xcb_depth_t {
        depth: 32,
        pad0: 0,
        visuals_len: 0,
        pad1: [0; 4],
    },
];

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_big_requests_id")]
pub static mut kinakaze_engine_libxcb_xcb_big_requests_id: xcb_extension_t = xcb_extension_t {
    name: b"BIG-REQUESTS\0".as_ptr().cast(),
    global_id: 0,
};

const GLOBAL_SCREEN: xcb_screen_t = xcb_screen_t {
    root: 1,
    default_colormap: 1,
    white_pixel: 0x00ffffff,
    black_pixel: 0x00000000,
    current_input_masks: 0,
    width_in_pixels: 1920,
    height_in_pixels: 1080,
    width_in_millimeters: 508,
    height_in_millimeters: 285,
    min_installed_maps: 1,
    max_installed_maps: 1,
    root_visual: 1,
    backing_stores: 0,
    save_unders: 0,
    root_depth: 24,
    allowed_depths_len: 2,
};

const GLOBAL_SETUP: xcb_setup_t = xcb_setup_t {
    status: 1,
    pad0: 0,
    protocol_major_version: 11,
    protocol_minor_version: 0,
    length: ((core::mem::size_of::<SetupReply>() - 8) / 4) as u16,
    release_number: 1,
    resource_id_base: 0x1000_0000,
    resource_id_mask: 0x0fff_ffff,
    motion_buffer_size: 256,
    vendor_len: 15,
    maximum_request_length: 65535,
    roots_len: 1,
    pixmap_formats_len: 3,
    image_byte_order: 0,
    bitmap_format_bit_order: 0,
    bitmap_format_scanline_unit: 32,
    bitmap_format_scanline_pad: 32,
    min_keycode: 8,
    max_keycode: 255,
    pad1: [0; 4],
};

// xcb_get_setup exposes the complete X11 wire reply. Chromium decodes this
// buffer directly instead of using XCB's iterator functions.
#[repr(C)]
struct SetupReply {
    header: xcb_setup_t,
    vendor: [u8; 16],
    formats: [xcb_format_t; 3],
    screen: xcb_screen_t,
    depth24: xcb_depth_t,
    visual24: xcb_visualtype_t,
    depth32: xcb_depth_t,
}

fn setup_reply() -> &'static SetupReply {
    static SETUP: OnceLock<SetupReply> = OnceLock::new();
    SETUP.get_or_init(|| {
        let mut screen = GLOBAL_SCREEN;
        let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if width > 0 && height > 0 {
            screen.width_in_pixels = width as u16;
            screen.height_in_pixels = height as u16;
        }
        SetupReply {
            header: GLOBAL_SETUP,
            vendor: *b"Kinakaze Native\0",
            formats: PIXMAP_FORMATS,
            screen,
            depth24: SCREEN_DEPTHS[0],
            visual24: SCREEN_VISUALS[0],
            depth32: SCREEN_DEPTHS[1],
        }
    })
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_connect")]
pub unsafe extern "sysv64" fn xcb_connect(
    _displayname: *const c_char,
    screenp: *mut c_int,
) -> *mut xcb_connection_t {
    if x11::connection::event_fd() < 0 {
        xcb_state().lock().unwrap().connection_error = 0;
    }
    if !screenp.is_null() {
        unsafe { *screenp = 0 };
    }
    let display = x11::connection::retain_shared();
    if display.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { x11::XSetEventQueueOwner(display, 1) };
    let _ = kinakaze_libdisplay::ui::available();
    let _ = setup_reply();
    unsafe { x11::XGetXCBConnection(x11::connection::xcb_display()).cast() }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_disconnect")]
pub unsafe extern "sysv64" fn xcb_disconnect(c: *mut xcb_connection_t) {
    let _request_display = x11::connection::scope_xcb(c.cast());
    if !c.is_null() {
        unsafe { x11::XCloseDisplay(x11::connection::xcb_display()) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_connection_has_error")]
pub unsafe extern "sysv64" fn xcb_connection_has_error(c: *mut xcb_connection_t) -> c_int {
    let _request_display = x11::connection::scope_xcb(c.cast());
    if c.is_null() || x11::connection::event_fd() < 0 {
        return 1;
    }
    xcb_state().lock().unwrap().connection_error
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_setup")]
pub unsafe extern "sysv64" fn xcb_get_setup(_c: *mut xcb_connection_t) -> *const xcb_setup_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    &setup_reply().header
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_setup_roots_iterator")]
pub unsafe extern "sysv64" fn xcb_setup_roots_iterator(
    _R: *const xcb_setup_t,
) -> xcb_screen_iterator_t {
    if _R.is_null() {
        return xcb_screen_iterator_t {
            data: core::ptr::null_mut(),
            rem: 0,
            index: 0,
        };
    }
    let formats = unsafe { xcb_setup_pixmap_formats(_R) };
    let data = unsafe {
        formats
            .add((*_R).pixmap_formats_len as usize)
            .cast::<xcb_screen_t>()
    };
    xcb_screen_iterator_t {
        data,
        rem: unsafe { (*_R).roots_len as c_int },
        index: (data as usize - _R as usize) as c_int,
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_screen_next")]
pub unsafe extern "sysv64" fn xcb_screen_next(i: *mut xcb_screen_iterator_t) {
    if !i.is_null() && unsafe { (*i).rem > 0 } {
        unsafe {
            let start = (*i).data;
            let mut depths = xcb_screen_allowed_depths_iterator(start);
            while depths.rem > 0 {
                xcb_depth_next(&mut depths);
            }
            (*i).data = depths.data.cast();
            (*i).index += (depths.data as usize - start as usize) as c_int;
            (*i).rem -= 1;
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_generate_id")]
pub unsafe extern "sysv64" fn xcb_generate_id(_c: *mut xcb_connection_t) -> u32 {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    if let Ok(mut s) = xcb_state().lock() {
        let id = s.next_id;
        s.next_id += 1;
        id
    } else {
        1
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_destroy_window")]
pub unsafe extern "sysv64" fn xcb_destroy_window(
    _c: *mut xcb_connection_t,
    wid: xcb_window_t,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    resources::forget_window(wid);
    let logical = if let Ok(mut s) = xcb_state().lock() {
        s.reported_geometry.remove(&wid);
        if let Some(hwnd_val) = s.wid_to_hwnd.remove(&wid) {
            s.hwnd_to_wid.remove(&hwnd_val);
        }
        let logical = s.wid_to_guest.remove(&wid);
        if let Some(logical) = logical {
            s.guest_to_wid.remove(&logical);
        }
        logical
    } else {
        None
    };
    if let Some(logical) = logical {
        if let Some(native) = kinakaze_libdisplay::window::native_handle(logical) {
            unsafe { x11::XDestroyWindow(x11::connection::xcb_display(), native) };
        }
    }
    xcb_void_cookie_t { sequence: wid }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_flush")]
pub unsafe extern "sysv64" fn xcb_flush(_c: *mut xcb_connection_t) -> c_int {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    if unsafe { xcb_connection_has_error(_c) } != 0 {
        return 0;
    }
    unsafe { x11::XFlush(x11::connection::xcb_display()) };
    1
}

/// Queues MapNotify or UnmapNotify for a window the guest knows by `wid`.
pub(crate) fn queue_map_notify(wid: xcb_window_t, mapped: bool, flag: u8) {
    let event = xcb_map_notify_event_t {
        response_type: if mapped {
            XCB_MAP_NOTIFY
        } else {
            XCB_UNMAP_NOTIFY
        },
        pad0: 0,
        sequence: 0,
        event: wid,
        window: wid,
        flag,
        pad1: [0; 3],
    };
    let mut generic: xcb_generic_event_t = unsafe { core::mem::zeroed() };
    unsafe {
        core::ptr::copy_nonoverlapping(
            (&raw const event).cast::<u8>(),
            (&raw mut generic).cast::<u8>(),
            core::mem::size_of_val(&event),
        );
    }
    if let Ok(mut s) = xcb_state().lock() {
        s.pending_events.push_back(generic);
    }
}

fn drain_xcb_events() {
    // Xlib and XCB share one native source, but each display has its own
    // subscribers. Converting here must not steal another connection's paint
    // or input events (CEF uses Xlib beside Qt's XCB reader).
    x11::connection::drain();
    // SAFETY: The caller owns the selected live display for this dispatch.
    unsafe { x11::drain_native_events(x11::connection::xcb_display()) };
    selection::drain_shared_events();
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_poll_for_event")]
pub unsafe extern "sysv64" fn xcb_poll_for_event(
    _c: *mut xcb_connection_t,
) -> *mut xcb_generic_event_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    let error = unsafe { request::event() };
    if !error.is_null() {
        return error;
    }
    // Route wire events to their subscribing displays before consuming this
    // frontend's queue. A Qt reader must not steal a CEF/Xlib timestamp reply.
    // SAFETY: The caller owns the selected live display for this dispatch.
    unsafe { x11::xext::protocol::drain_events(x11::connection::xcb_display()) };
    drain_xcb_events();
    if let Ok(mut s) = xcb_state().lock() {
        if let Some(ev) = s.pending_events.pop_front() {
            return unsafe {
                request::copy_guest(core::slice::from_raw_parts(
                    (&raw const ev).cast(),
                    size_of::<xcb_generic_event_t>(),
                ))
                .cast()
            };
        }
    }
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_wait_for_event")]
pub unsafe extern "sysv64" fn xcb_wait_for_event(
    c: *mut xcb_connection_t,
) -> *mut xcb_generic_event_t {
    let _request_display = x11::connection::scope_xcb(c.cast());
    let Some(waiter) = x11::event_select::Waiter::new() else {
        return core::ptr::null_mut();
    };
    loop {
        waiter.drain();
        let event = unsafe { xcb_poll_for_event(c) };
        if !event.is_null() {
            return event;
        }
        if unsafe { xcb_connection_has_error(c) } != 0 || !waiter.wait() {
            return core::ptr::null_mut();
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_request_check")]
pub unsafe extern "sysv64" fn xcb_request_check(
    _c: *mut xcb_connection_t,
    _cookie: xcb_void_cookie_t,
) -> *mut xcb_generic_error_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    let mut error = core::ptr::null_mut();
    let reply = unsafe { request::take(_cookie.sequence, &raw mut error) };
    unsafe { kinakaze_alloc::c::free(reply.cast()) };
    error
}

#[repr(C)]
pub struct xcb_extension_t {
    pub name: *const c_char,
    pub global_id: c_int,
}

#[repr(C)]
pub struct xcb_query_extension_reply_t {
    pub response_type: u8,
    pub pad0: u8,
    pub sequence: u16,
    pub length: u32,
    pub present: u8,
    pub major_opcode: u8,
    pub first_event: u8,
    pub first_error: u8,
}

#[repr(C)]
pub struct xcb_protocol_request_t {
    pub count: usize,
    pub ext: *mut xcb_extension_t,
    pub opcode: u8,
    pub isvoid: u8,
}

#[unsafe(export_name = "kinakaze_engine_libxcb_XGetXCBConnection")]
pub unsafe extern "sysv64" fn XGetXCBConnection(dpy: *mut c_void) -> *mut xcb_connection_t {
    unsafe { x11::XGetXCBConnection(dpy.cast()).cast() }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_send_request")]
pub unsafe extern "sysv64" fn xcb_send_request(
    _c: *mut xcb_connection_t,
    _flags: c_int,
    _vector: *mut c_void,
    _req: *const xcb_protocol_request_t,
) -> c_uint {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    if unsafe { xcb_connection_has_error(_c) } != 0 {
        return 0;
    }
    if !_req.is_null() && !unsafe { (*_req).ext }.is_null() {
        let ext = unsafe { &*(*_req).ext };
        if !ext.name.is_null()
            && unsafe { core::ffi::CStr::from_ptr(ext.name) }.to_bytes() == b"XKEYBOARD"
        {
            #[repr(C)]
            struct IoVec {
                base: *const u8,
                len: usize,
            }
            let req = unsafe { &*_req };
            return request::submit(_flags & 1 != 0, req.isvoid == 0, || {
                if _vector.is_null() || req.count > 4096 {
                    return Err(request::error(16, x11::xkb::server::OPCODE, 0));
                }
                let vectors =
                    unsafe { core::slice::from_raw_parts(_vector.cast::<IoVec>(), req.count) };
                let mut bytes = Vec::new();
                for v in vectors {
                    if v.len > 262140usize.saturating_sub(bytes.len()) {
                        return Err(request::error(16, x11::xkb::server::OPCODE, 0));
                    }
                    if v.base.is_null() {
                        bytes.resize(bytes.len() + v.len, 0);
                    } else {
                        bytes.extend_from_slice(unsafe {
                            core::slice::from_raw_parts(v.base, v.len)
                        });
                    }
                }
                if bytes.len() < 4 {
                    return Err(request::error(16, x11::xkb::server::OPCODE, 0));
                }
                bytes[0] = x11::xkb::server::OPCODE;
                bytes[1] = req.opcode;
                let words = bytes.len().div_ceil(4) as u16;
                request::put16(&mut bytes, 2, words);
                let result = x11::xkb::server::dispatch(req.opcode, &bytes);
                if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
                    eprintln!(
                        "[libxcb] XKEYBOARD request={} bytes={} reply={:?}",
                        req.opcode,
                        bytes.len(),
                        result
                            .as_ref()
                            .map(|r| r.as_ref().map(Vec::len))
                            .map_err(|e| e.code)
                    );
                }
                result
            });
        }
    }
    let (opcode, minor) = if _req.is_null() {
        (0, 0)
    } else if unsafe { (*_req).ext.is_null() } {
        (unsafe { (*_req).opcode }, 0)
    } else {
        (0, unsafe { (*_req).opcode })
    };
    request::submit(
        _flags & 1 != 0,
        !_req.is_null() && unsafe { (*_req).isvoid == 0 },
        || {
            let mut error = request::error(1, opcode, 0);
            error.minor = minor;
            Err(error)
        },
    )
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_wait_for_reply")]
pub unsafe extern "sysv64" fn xcb_wait_for_reply(
    _c: *mut xcb_connection_t,
    _request: c_uint,
    e: *mut *mut xcb_generic_error_t,
) -> *mut c_void {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { request::take(_request, e) }
}

static mut GLOBAL_EXT_REPLY: xcb_query_extension_reply_t = xcb_query_extension_reply_t {
    response_type: 1,
    pad0: 0,
    sequence: 1,
    length: 0,
    present: 0,
    major_opcode: 0,
    first_event: 0,
    first_error: 0,
};

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_extension_data")]
pub unsafe extern "sysv64" fn xcb_get_extension_data(
    _c: *mut xcb_connection_t,
    ext: *mut xcb_extension_t,
) -> *const xcb_query_extension_reply_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    if !ext.is_null()
        && !unsafe { (*ext).name }.is_null()
        && unsafe { core::ffi::CStr::from_ptr((*ext).name) }.to_bytes() == b"XKEYBOARD"
    {
        static XKB: xcb_query_extension_reply_t = xcb_query_extension_reply_t {
            response_type: 1,
            pad0: 0,
            sequence: 0,
            length: 0,
            present: 1,
            major_opcode: x11::xkb::server::OPCODE,
            first_event: x11::xkb::server::EVENT,
            first_error: x11::xkb::server::ERROR,
        };
        return &XKB;
    }
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() && !ext.is_null() {
        let name = unsafe { (*ext).name };
        if !name.is_null() {
            eprintln!(
                "[libxcb] extension {} -> unsupported",
                unsafe { core::ffi::CStr::from_ptr(name) }.to_string_lossy()
            );
        }
    }
    &raw const GLOBAL_EXT_REPLY
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_prefetch_extension_data")]
pub unsafe extern "sysv64" fn xcb_prefetch_extension_data(
    _c: *mut xcb_connection_t,
    _ext: *mut xcb_extension_t,
) {
    let _request_display = x11::connection::scope_xcb(_c.cast());
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_str_sizeof")]
pub unsafe extern "sysv64" fn xcb_str_sizeof(buffer: *const c_void) -> c_int {
    if buffer.is_null() {
        return 0;
    }
    let len = unsafe { *(buffer as *const u8) };
    1 + len as c_int
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_reply_fds")]
pub unsafe extern "sysv64" fn xcb_get_reply_fds(
    _c: *mut xcb_connection_t,
    _reply: *mut c_void,
    _reply_size: usize,
) -> *mut c_int {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_send_request_with_fds")]
pub unsafe extern "sysv64" fn xcb_send_request_with_fds(
    c: *mut xcb_connection_t,
    flags: c_int,
    vector: *mut c_void,
    req: *const xcb_protocol_request_t,
    num_fds: c_uint,
    fds: *const c_int,
) -> c_uint {
    let _request_display = x11::connection::scope_xcb(c.cast());
    if num_fds != 0 {
        if !fds.is_null() {
            for index in 0..num_fds as usize {
                unsafe {
                    xcb_send_fd(c, *fds.add(index));
                }
            }
        } else if !c.is_null() {
            xcb_state().lock().unwrap().connection_error = 7;
            x11::notify_event();
        }
        return 0;
    }
    unsafe { xcb_send_request(c, flags, vector, req) }
}

/// The native connection does not advertise DRI3/FD-based SHM. Reject an FD
/// request with XCB_CONN_CLOSED_FDPASSING_FAILED, consuming its descriptor as
/// required by xcbext.h. Never silently turn it into an ordinary request.
#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_send_fd")]
pub unsafe extern "sysv64" fn xcb_send_fd(c: *mut xcb_connection_t, fd: c_int) {
    let _request_display = x11::connection::scope_xcb(c.cast());
    libc::kinakaze_abi_close(fd);
    if !c.is_null() {
        xcb_state().lock().unwrap().connection_error = 7;
        x11::notify_event();
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_maximum_request_length")]
pub unsafe extern "sysv64" fn xcb_get_maximum_request_length(_c: *mut xcb_connection_t) -> u32 {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    65535
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_prefetch_maximum_request_length")]
pub unsafe extern "sysv64" fn xcb_prefetch_maximum_request_length(_c: *mut xcb_connection_t) {
    let _request_display = x11::connection::scope_xcb(_c.cast());
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_discard_reply")]
pub unsafe extern "sysv64" fn xcb_discard_reply(_c: *mut xcb_connection_t, _sequence: c_uint) {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    request::discard(_sequence);
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_poll_for_reply")]
pub unsafe extern "sysv64" fn xcb_poll_for_reply(
    _c: *mut xcb_connection_t,
    _request: c_uint,
    reply: *mut *mut c_void,
    error: *mut *mut xcb_generic_error_t,
) -> c_int {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    if !error.is_null() {
        unsafe { error.write(core::ptr::null_mut()) };
    }
    if !reply.is_null() {
        unsafe { reply.write(core::ptr::null_mut()) };
    }
    if !request::ready(_request) {
        return 0;
    }
    let value = unsafe { request::take(_request, error) };
    if !reply.is_null() {
        unsafe { reply.write(value) };
    } else {
        unsafe { kinakaze_alloc::c::free(value.cast()) };
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_screen_allowed_depths_iterator")]
pub unsafe extern "sysv64" fn xcb_screen_allowed_depths_iterator(
    _r: *const xcb_screen_t,
) -> xcb_depth_iterator_t {
    xcb_depth_iterator_t {
        data: if _r.is_null() {
            core::ptr::null_mut()
        } else {
            unsafe { _r.add(1).cast_mut().cast() }
        },
        rem: if _r.is_null() {
            0
        } else {
            unsafe { (*_r).allowed_depths_len as c_int }
        },
        index: size_of::<xcb_screen_t>() as c_int,
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_depth_next")]
pub unsafe extern "sysv64" fn xcb_depth_next(i: *mut xcb_depth_iterator_t) {
    if !i.is_null() && unsafe { (*i).rem } > 0 {
        unsafe {
            let size = size_of::<xcb_depth_t>()
                + (*(*i).data).visuals_len as usize * size_of::<xcb_visualtype_t>();
            (*i).index += size as c_int;
            (*i).rem -= 1;
            (*i).data = (*i).data.byte_add(size);
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_depth_visuals_iterator")]
pub unsafe extern "sysv64" fn xcb_depth_visuals_iterator(
    _r: *const xcb_depth_t,
) -> xcb_visualtype_iterator_t {
    let count = if _r.is_null() {
        0
    } else {
        unsafe { (*_r).visuals_len as c_int }
    };
    xcb_visualtype_iterator_t {
        data: if _r.is_null() {
            core::ptr::null_mut()
        } else {
            unsafe { _r.add(1).cast_mut().cast() }
        },
        rem: count,
        index: size_of::<xcb_depth_t>() as c_int,
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_visualtype_next")]
pub unsafe extern "sysv64" fn xcb_visualtype_next(i: *mut xcb_visualtype_iterator_t) {
    if !i.is_null() && unsafe { (*i).rem > 0 } {
        unsafe {
            (*i).rem -= 1;
            (*i).data = (*i).data.add(1);
            (*i).index += size_of::<xcb_visualtype_t>() as c_int;
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_setup_pixmap_formats")]
pub unsafe extern "sysv64" fn xcb_setup_pixmap_formats(
    _r: *const xcb_setup_t,
) -> *mut xcb_format_t {
    if _r.is_null() {
        return core::ptr::null_mut();
    }
    let offset =
        size_of::<xcb_setup_t>() + (unsafe { (*_r).vendor_len } as usize).next_multiple_of(4);
    unsafe { _r.byte_add(offset).cast_mut().cast() }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_setup_pixmap_formats_length")]
pub unsafe extern "sysv64" fn xcb_setup_pixmap_formats_length(_r: *const xcb_setup_t) -> c_int {
    if _r.is_null() {
        0
    } else {
        unsafe { (*_r).pixmap_formats_len as c_int }
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_query_pointer")]
pub unsafe extern "sysv64" fn xcb_query_pointer(
    _c: *mut xcb_connection_t,
    window: xcb_window_t,
) -> xcb_query_pointer_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    xcb_query_pointer_cookie_t { sequence: window }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_query_pointer_reply")]
pub unsafe extern "sysv64" fn xcb_query_pointer_reply(
    _c: *mut xcb_connection_t,
    cookie: xcb_query_pointer_cookie_t,
    _e: *mut *mut xcb_generic_error_t,
) -> *mut xcb_query_pointer_reply_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&raw mut pt) };

    let mut win_pt = pt;
    if let Some(hwnd) = get_native_hwnd(cookie.sequence) {
        unsafe { ScreenToClient(hwnd, &raw mut win_pt) };
    }

    let mut mask = 0u16;
    if (unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } as u16 & 0x8000) != 0 {
        mask |= 0x0100;
    }
    if (unsafe { GetAsyncKeyState(VK_MBUTTON as i32) } as u16 & 0x8000) != 0 {
        mask |= 0x0200;
    }
    if (unsafe { GetAsyncKeyState(VK_RBUTTON as i32) } as u16 & 0x8000) != 0 {
        mask |= 0x0400;
    }

    let raw =
        unsafe { kinakaze_alloc::c::malloc(core::mem::size_of::<xcb_query_pointer_reply_t>()) }
            as *mut xcb_query_pointer_reply_t;
    if raw.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        (*raw).response_type = 1;
        (*raw).same_screen = 1;
        (*raw).sequence = cookie.sequence as u16;
        (*raw).length = 0;
        (*raw).root = 1;
        (*raw).child = 0;
        (*raw).root_x = pt.x as i16;
        (*raw).root_y = pt.y as i16;
        (*raw).win_x = win_pt.x as i16;
        (*raw).win_y = win_pt.y as i16;
        (*raw).mask = mask;
        (*raw).pad0 = [0; 2];
    }
    raw
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_grab_pointer")]
pub unsafe extern "sysv64" fn xcb_grab_pointer(
    _c: *mut xcb_connection_t,
    _owner_events: u8,
    grab_window: xcb_window_t,
    _event_mask: u16,
    _pointer_mode: u8,
    _keyboard_mode: u8,
    _confine_to: xcb_window_t,
    _cursor: u32,
    _time: u32,
) -> xcb_grab_pointer_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    if let Some(hwnd) = get_native_hwnd(grab_window) {
        unsafe { SetCapture(hwnd) };
    }
    xcb_grab_pointer_cookie_t {
        sequence: grab_window,
    }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_grab_pointer_reply")]
pub unsafe extern "sysv64" fn xcb_grab_pointer_reply(
    _c: *mut xcb_connection_t,
    cookie: xcb_grab_pointer_cookie_t,
    _e: *mut *mut xcb_generic_error_t,
) -> *mut xcb_grab_pointer_reply_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    let raw = unsafe { kinakaze_alloc::c::malloc(core::mem::size_of::<xcb_grab_pointer_reply_t>()) }
        as *mut xcb_grab_pointer_reply_t;
    if raw.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        (*raw).response_type = 1;
        (*raw).status = 0;
        (*raw).sequence = cookie.sequence as u16;
        (*raw).length = 0;
    }
    raw
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_ungrab_pointer")]
pub unsafe extern "sysv64" fn xcb_ungrab_pointer(
    _c: *mut xcb_connection_t,
    _time: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    unsafe { ReleaseCapture() };
    xcb_void_cookie_t { sequence: 1 }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_property_value")]
pub unsafe extern "sysv64" fn xcb_get_property_value(
    r: *const xcb_get_property_reply_t,
) -> *mut c_void {
    if r.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { (r as *mut u8).add(core::mem::size_of::<xcb_get_property_reply_t>()) as *mut c_void }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_get_property_value_length")]
pub unsafe extern "sysv64" fn xcb_get_property_value_length(
    r: *const xcb_get_property_reply_t,
) -> c_int {
    if r.is_null() {
        return 0;
    }
    unsafe { ((*r).value_len as usize * ((*r).format as usize / 8)) as c_int }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_create_glyph_cursor")]
pub unsafe extern "sysv64" fn xcb_create_glyph_cursor(
    _c: *mut xcb_connection_t,
    cid: u32,
    _source_font: u32,
    _mask_font: u32,
    _source_char: u16,
    _mask_char: u16,
    _fore_red: u16,
    _fore_green: u16,
    _fore_blue: u16,
    _back_red: u16,
    _back_green: u16,
    _back_blue: u16,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    xcb_void_cookie_t { sequence: cid }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_open_font")]
pub unsafe extern "sysv64" fn xcb_open_font(
    _c: *mut xcb_connection_t,
    fid: u32,
    _name_len: u16,
    _name: *const c_char,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    xcb_void_cookie_t { sequence: fid }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_close_font")]
pub unsafe extern "sysv64" fn xcb_close_font(
    _c: *mut xcb_connection_t,
    fid: u32,
) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    xcb_void_cookie_t { sequence: fid }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_grab_server")]
pub unsafe extern "sysv64" fn xcb_grab_server(_c: *mut xcb_connection_t) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    xcb_void_cookie_t { sequence: 1 }
}

#[unsafe(export_name = "kinakaze_engine_libxcb_xcb_ungrab_server")]
pub unsafe extern "sysv64" fn xcb_ungrab_server(_c: *mut xcb_connection_t) -> xcb_void_cookie_t {
    let _request_display = x11::connection::scope_xcb(_c.cast());
    xcb_void_cookie_t { sequence: 1 }
}

/// Helper function to retrieve the native HWND for an XCB window ID
pub fn get_native_hwnd(wid: xcb_window_t) -> Option<HWND> {
    let local = xcb_state().lock().ok()?.wid_to_hwnd.get(&wid).copied();
    local
        .or_else(|| x11::shared::window_owner(wid as usize).map(|_| wid as usize))
        .map(|h| h as HWND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_extensions_are_not_advertised() {
        let mut extension = xcb_extension_t {
            name: c"Present".as_ptr(),
            global_id: 0,
        };
        let reply = unsafe {
            xcb_get_extension_data(core::ptr::null_mut(), &raw mut extension)
                .as_ref()
                .expect("static extension reply")
        };
        assert_eq!(reply.present, 0);
        assert_eq!(reply.major_opcode, 0);
        assert_eq!(reply.first_event, 0);
        assert_eq!(reply.first_error, 0);
    }

    struct EventBuffer(*mut xcb_generic_event_t);
    impl EventBuffer {
        fn as_ref(&self) -> &xcb_generic_event_t {
            unsafe { &*self.0 }
        }
    }
    impl Drop for EventBuffer {
        fn drop(&mut self) {
            unsafe { kinakaze_alloc::c::free(self.0.cast()) }
        }
    }
    unsafe fn take_event(connection: *mut xcb_connection_t) -> EventBuffer {
        let event = unsafe { xcb_poll_for_event(connection) };
        assert!(!event.is_null(), "expected an XCB event");
        EventBuffer(event)
    }

    #[test]
    fn window_mapping_input_resize_close_and_destroy_roundtrip() {
        let connection = unsafe { xcb_connect(core::ptr::null(), core::ptr::null_mut()) };
        let wid = unsafe { xcb_generate_id(connection) };
        // ConfigureNotify is delivered only to StructureNotify subscribers.
        let event_mask: u32 = 1 << 17;
        unsafe {
            xcb_create_window(
                connection,
                24,
                wid,
                1,
                0,
                0,
                320,
                240,
                0,
                1,
                1,
                1 << 11,
                (&raw const event_mask).cast(),
            )
        };

        let (logical, hwnd) = {
            let state = xcb_state().lock().expect("XCB state");
            (
                *state.wid_to_guest.get(&wid).expect("stable guest id"),
                *state.wid_to_hwnd.get(&wid).expect("native HWND"),
            )
        };
        assert_ne!(wid as usize, hwnd);
        assert_eq!(get_native_hwnd(wid), Some(hwnd as HWND));
        assert_eq!(
            kinakaze_libdisplay::window::native_handle(logical),
            Some(hwnd)
        );

        if let Ok(mut queue) = kinakaze_libdisplay::event::queue().lock() {
            while queue.pop().is_some() {}
        }
        xcb_state()
            .lock()
            .expect("XCB state")
            .pending_events
            .clear();

        for event in [
            kinakaze_libdisplay::event::Event {
                kind: kinakaze_libdisplay::event::EVENT_POINTER_MOTION,
                x: 23,
                y: 41,
                window: logical,
                ..Default::default()
            },
            kinakaze_libdisplay::event::Event {
                kind: kinakaze_libdisplay::event::EVENT_POINTER_BUTTON,
                button: kinakaze_libdisplay::event::BTN_LEFT,
                state: kinakaze_libdisplay::event::STATE_PRESSED,
                x: 23,
                y: 41,
                window: logical,
                ..Default::default()
            },
            kinakaze_libdisplay::event::Event {
                kind: kinakaze_libdisplay::event::EVENT_SCROLL,
                scroll_y: 1,
                window: logical,
                ..Default::default()
            },
            kinakaze_libdisplay::event::Event {
                kind: kinakaze_libdisplay::event::EVENT_RESIZE,
                x: 800,
                y: 600,
                window: logical,
                ..Default::default()
            },
            kinakaze_libdisplay::event::Event {
                kind: kinakaze_libdisplay::event::EVENT_CLOSE,
                window: logical,
                ..Default::default()
            },
        ] {
            kinakaze_libdisplay::event::publish(event);
        }

        let motion = unsafe { take_event(connection) };
        let motion = unsafe { &*(motion.as_ref() as *const _ as *const xcb_key_press_event_t) };
        assert_eq!(
            (
                motion.response_type,
                motion.event,
                motion.event_x,
                motion.event_y
            ),
            (XCB_MOTION_NOTIFY, wid, 23, 41)
        );

        let button = unsafe { take_event(connection) };
        let button = unsafe { &*(button.as_ref() as *const _ as *const xcb_key_press_event_t) };
        assert_eq!(
            (button.response_type, button.detail, button.event),
            (XCB_BUTTON_PRESS, 1, wid)
        );

        for expected in [XCB_BUTTON_PRESS, XCB_BUTTON_RELEASE] {
            let scroll = unsafe { take_event(connection) };
            let scroll = unsafe { &*(scroll.as_ref() as *const _ as *const xcb_key_press_event_t) };
            assert_eq!(
                (scroll.response_type, scroll.detail, scroll.event),
                (expected, 4, wid)
            );
        }

        let configure = unsafe { take_event(connection) };
        let configure =
            unsafe { &*(configure.as_ref() as *const _ as *const xcb_configure_notify_event_t) };
        assert_eq!(
            (
                configure.response_type,
                configure.window,
                configure.width,
                configure.height
            ),
            (XCB_CONFIGURE_NOTIFY, wid, 800, 600)
        );

        let close = unsafe { take_event(connection) };
        let close = unsafe { &*(close.as_ref() as *const _ as *const xcb_client_message_event_t) };
        assert_eq!(
            (
                close.response_type,
                close.window,
                close.r#type,
                close.data[0]
            ),
            (
                XCB_CLIENT_MESSAGE | 0x80,
                wid,
                x11::property::intern_bytes(b"WM_PROTOCOLS", false) as u32,
                x11::property::intern_bytes(b"WM_DELETE_WINDOW", false) as u32
            )
        );
        assert!(unsafe { xcb_poll_for_event(connection) }.is_null());

        unsafe { xcb_destroy_window(connection, wid) };
        assert!(get_native_hwnd(wid).is_none());
        assert!(kinakaze_libdisplay::window::native_handle(logical).is_none());
        unsafe { xcb_disconnect(connection) };
    }
}

mod object_layout;
