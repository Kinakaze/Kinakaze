//! Xlib internal private flow and extension infrastructure.
//!
//! Core `_X*` entry points and extension discovery. Xext owns its public ABI.

use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::os::raw::{c_char, c_int, c_uchar};
use std::sync::{Mutex, OnceLock};

use crate::{Display, Visual, XEvent};

type c_long = i64;
type c_ulong = u64;
type Bool = c_int;
type Status = c_int;

// ---------------------------------------------------------------------------
// Internal private memory & thread hooks
// ---------------------------------------------------------------------------

mod locking;
pub mod protocol;

#[unsafe(export_name = "kinakaze_engine_libX11__XAllocTemp")]
pub unsafe extern "sysv64" fn _XAllocTemp(_dpy: *mut Display, nbytes: c_ulong) -> *mut c_void {
    unsafe { libc::kinakaze_abi_calloc(1, nbytes as usize) }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XFreeTemp")]
pub unsafe extern "sysv64" fn _XFreeTemp(_dpy: *mut Display, buf: *mut c_void) {
    unsafe {
        kinakaze_alloc::guest::free(buf.cast());
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XGetRequest")]
pub unsafe extern "sysv64" fn _XGetRequest(
    _dpy: *mut Display,
    _type_: c_uchar,
    _len: usize,
) -> *mut c_void {
    unsafe { protocol::allocate(_dpy, _type_, _len) }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XSend")]
pub unsafe extern "sysv64" fn _XSend(_dpy: *mut Display, _data: *const c_char, _size: c_long) {
    if _size > 0 && !_data.is_null() {
        unsafe {
            protocol::append(
                _dpy,
                core::slice::from_raw_parts(_data.cast(), _size as usize),
            );
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XFlush")]
pub unsafe extern "sysv64" fn _XFlush(_dpy: *mut Display) {
    unsafe { protocol::flush(_dpy) };
    if std::env::var_os("KINAKAZE_DISPLAY_TRACE").is_some() {
        crate::diagnostic!("[libX11] _XFlush");
    }
    crate::graphics::flush_all();
}

#[unsafe(export_name = "kinakaze_engine_libX11__XReply")]
pub unsafe extern "sysv64" fn _XReply(
    _dpy: *mut Display,
    rep: *mut c_void,
    _extra: c_int,
    _discard: Bool,
) -> Status {
    unsafe { protocol::receive(_dpy, rep, _extra, _discard != 0) }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XGetAsyncReply")]
pub unsafe extern "sysv64" fn _XGetAsyncReply(
    _dpy: *mut Display,
    rep: *mut c_void,
    _extra: *mut c_void,
) -> Status {
    if !rep.is_null() {
        unsafe {
            let slice = core::slice::from_raw_parts_mut(rep as *mut u8, 32);
            slice.fill(0);
            slice[0] = 1;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11__XSetLastRequestRead")]
pub unsafe extern "sysv64" fn _XSetLastRequestRead(_dpy: *mut Display, _rep: *mut c_void) {}

#[unsafe(export_name = "kinakaze_engine_libX11__XData32")]
pub unsafe extern "sysv64" fn _XData32(_dpy: *mut Display, _data: *const c_long, _len: c_long) {
    if _data.is_null() || _len <= 0 {
        return;
    }
    let mut bytes = Vec::with_capacity(_len as usize);
    for i in 0.._len as usize / 4 {
        bytes.extend_from_slice(&(unsafe { *_data.add(i) } as u32).to_ne_bytes());
    }
    unsafe { protocol::append(_dpy, &bytes) };
}

#[unsafe(export_name = "kinakaze_engine_libX11__XRead")]
pub unsafe extern "sysv64" fn _XRead(_dpy: *mut Display, data: *mut c_char, size: c_long) {
    if !data.is_null() && size > 0 {
        unsafe { protocol::read(_dpy, data.cast(), size as usize, false) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XRead32")]
pub unsafe extern "sysv64" fn _XRead32(_dpy: *mut Display, data: *mut c_long, size: c_long) {
    if !data.is_null() && size > 0 {
        for i in 0..size as usize / 4 {
            let mut bytes = [0; 4];
            unsafe {
                protocol::read(_dpy, bytes.as_mut_ptr(), 4, false);
                data.add(i).write(u32::from_ne_bytes(bytes) as i64);
            }
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XReadPad")]
pub unsafe extern "sysv64" fn _XReadPad(_dpy: *mut Display, data: *mut c_char, size: c_long) {
    if !data.is_null() && size > 0 {
        unsafe { protocol::read(_dpy, data.cast(), size as usize, true) };
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11__XEatData")]
pub unsafe extern "sysv64" fn _XEatData(_dpy: *mut Display, _nbytes: c_ulong) {
    unsafe { protocol::read(_dpy, core::ptr::null_mut(), _nbytes as usize, false) };
}

#[unsafe(export_name = "kinakaze_engine_libX11__XEatDataWords")]
pub unsafe extern "sysv64" fn _XEatDataWords(_dpy: *mut Display, _nwords: c_ulong) {
    unsafe {
        protocol::read(
            _dpy,
            core::ptr::null_mut(),
            (_nwords as usize).saturating_mul(4),
            false,
        )
    };
}

#[unsafe(export_name = "kinakaze_engine_libX11__XUnknownNativeEvent")]
pub unsafe extern "sysv64" fn _XUnknownNativeEvent(
    _dpy: *mut Display,
    _re: *mut XEvent,
    _event: *mut c_void,
) -> Bool {
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11__XDeqAsyncHandler")]
pub unsafe extern "sysv64" fn _XDeqAsyncHandler(_dpy: *mut Display, _handler: *mut c_void) {}

#[unsafe(export_name = "kinakaze_engine_libX11__XVIDtoVisual")]
pub unsafe extern "sysv64" fn _XVIDtoVisual(_dpy: *mut Display, _id: usize) -> *mut Visual {
    unsafe { crate::XDefaultVisual(_dpy, 0) }
}

// ---------------------------------------------------------------------------
// Xext standard extension infrastructure
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct XExtData {
    pub number: c_int,
    pub next: *mut XExtData,
    pub free_private: *mut c_void,
    pub private_data: *mut c_char,
}

#[repr(C)]
pub struct XExtensionCodes {
    pub extension: c_int,
    pub major_opcode: c_int,
    pub first_event: c_int,
    pub first_error: c_int,
}

#[derive(Default)]
struct ExtensionRegistry {
    extensions: HashMap<String, c_int>,
}

impl ExtensionRegistry {
    fn new() -> Self {
        let mut reg = Self {
            extensions: HashMap::new(),
        };
        reg.extensions.insert("XInputExtension".to_string(), 128);
        reg.extensions.insert("MIT-SHM".to_string(), 130);
        reg.extensions.insert("DOUBLE-BUFFER".to_string(), 131);
        reg.extensions
            .insert("XKEYBOARD".to_string(), crate::xkb::server::OPCODE as i32);
        reg.extensions
            .insert("Composite".to_string(), crate::composite::OPCODE as i32);
        reg.extensions
            .insert("DAMAGE".to_string(), crate::damage::OPCODE as i32);
        reg.extensions
            .insert("XFIXES".to_string(), crate::xfixes::OPCODE as i32);
        reg.extensions.insert("RANDR".to_string(), 140);
        reg.extensions.insert("SYNC".to_string(), 134);
        if crate::render::available() {
            reg.extensions.insert("RENDER".to_string(), 129);
        }
        reg.extensions
            .insert("SHAPE".to_string(), crate::shape::OPCODE as i32);
        reg
    }
}

fn extension_registry() -> &'static Mutex<ExtensionRegistry> {
    static REG: OnceLock<Mutex<ExtensionRegistry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(ExtensionRegistry::new()))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XListExtensions")]
pub unsafe extern "sysv64" fn XListExtensions(
    _display: *mut Display,
    count: *mut c_int,
) -> *mut *mut c_char {
    if count.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        *count = 0;
    }
    let registry = extension_registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut names: Vec<_> = registry.extensions.keys().collect();
    names.sort_unstable();
    let pointer_bytes = (names.len() + 1) * core::mem::size_of::<*mut c_char>();
    let string_bytes: usize = names.iter().map(|name| name.len() + 1).sum();
    let allocation = unsafe { kinakaze_alloc::c::malloc(pointer_bytes + string_bytes) };
    if allocation.is_null() {
        return core::ptr::null_mut();
    }
    let result = allocation.cast::<*mut c_char>();
    unsafe {
        let mut cursor = allocation.add(pointer_bytes);
        for (index, name) in names.iter().enumerate() {
            result.add(index).write(cursor.cast());
            core::ptr::copy_nonoverlapping(name.as_ptr(), cursor, name.len());
            cursor.add(name.len()).write(0);
            cursor = cursor.add(name.len() + 1);
        }
        result.add(names.len()).write(core::ptr::null_mut());
        *count = names.len() as c_int;
    }
    result
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeExtensionList")]
pub unsafe extern "sysv64" fn XFreeExtensionList(list: *mut *mut c_char) -> c_int {
    unsafe {
        kinakaze_alloc::c::free(list.cast());
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XAddExtension")]
pub unsafe extern "sysv64" fn XAddExtension(dpy: *mut Display) -> *mut XExtData {
    let data = Box::into_raw(Box::new(XExtData {
        number: 1,
        next: core::ptr::null_mut(),
        free_private: core::ptr::null_mut(),
        private_data: core::ptr::null_mut(),
    }));
    if !dpy.is_null() {
        unsafe { (*dpy).ext_data = data as *mut c_void };
    }
    data
}

#[unsafe(export_name = "kinakaze_engine_libX11_XInitExtension")]
pub unsafe extern "sysv64" fn XInitExtension(
    _dpy: *mut Display,
    name: *const c_char,
) -> *mut XExtensionCodes {
    let name_str = if !name.is_null() {
        unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("")
    } else {
        ""
    };
    let Some(code) = extension_registry()
        .lock()
        .ok()
        .and_then(|reg| reg.extensions.get(name_str).copied())
    else {
        return core::ptr::null_mut();
    };

    Box::into_raw(Box::new(XExtensionCodes {
        extension: code,
        major_opcode: code,
        first_event: if matches!(code, 131 | 133) {
            0
        } else {
            64 + (code - 128) * 4
        },
        first_error: 128 + (code - 128) * 4,
    }))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XQueryExtension")]
pub unsafe extern "sysv64" fn XQueryExtension(
    _dpy: *mut Display,
    name: *const c_char,
    major_opcode_return: *mut c_int,
    first_event_return: *mut c_int,
    first_error_return: *mut c_int,
) -> Bool {
    let name_str = if !name.is_null() {
        unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("")
    } else {
        ""
    };
    if let Ok(reg) = extension_registry().lock() {
        if let Some(&code) = reg.extensions.get(name_str) {
            if !major_opcode_return.is_null() {
                unsafe { *major_opcode_return = code };
            }
            if !first_event_return.is_null() {
                unsafe {
                    *first_event_return = if matches!(code, 131 | 133) {
                        0
                    } else {
                        64 + (code - 128) * 4
                    }
                };
            }
            if !first_error_return.is_null() {
                unsafe { *first_error_return = 128 + (code - 128) * 4 };
            }
            return 1;
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XMissingExtension")]
pub unsafe extern "sysv64" fn XMissingExtension(
    _dpy: *mut Display,
    _ext_name: *const c_char,
) -> c_int {
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XESetCloseDisplay")]
pub unsafe extern "sysv64" fn XESetCloseDisplay(
    _dpy: *mut Display,
    _extension: c_int,
    _proc: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XESetCopyEventCookie")]
pub unsafe extern "sysv64" fn XESetCopyEventCookie(
    _dpy: *mut Display,
    _extension: c_int,
    _proc: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XESetWireToEventCookie")]
pub unsafe extern "sysv64" fn XESetWireToEventCookie(
    _dpy: *mut Display,
    _extension: c_int,
    _proc: *mut c_void,
) -> *mut c_void {
    core::ptr::null_mut()
}

/// Native replies are complete buffers. A truncated reply cannot be filled from
/// an external X wire stream because this display has no such transport.
#[unsafe(export_name = "kinakaze_engine_libX11__XGetAsyncData")]
pub unsafe extern "sysv64" fn _XGetAsyncData(
    display: *mut Display,
    output: *mut c_char,
    buffer: *const c_char,
    length: c_int,
    skip: c_int,
    count: c_int,
    discard: c_int,
) {
    if skip < 0 || count < 0 || length < skip || count > length - skip || discard > length - skip {
        unsafe {
            crate::errors::_XDefaultIOError(display);
        }
        return;
    }
    if !output.is_null() && count != 0 {
        if buffer.is_null() {
            unsafe {
                crate::errors::_XDefaultIOError(display);
            }
            return;
        }
        unsafe {
            core::ptr::copy(buffer.add(skip as usize), output, count as usize);
        }
    }
}
