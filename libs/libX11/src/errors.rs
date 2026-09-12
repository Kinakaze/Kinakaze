//! Xlib error callbacks. Unsupported protocol operations deliver an X error.
use crate::{Display, c_ulong};
use core::ffi::{c_char, c_int};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
mod lifecycle;
pub type XErrorHandler = Option<unsafe extern "sysv64" fn(*mut Display, *mut XErrorEvent) -> c_int>;
pub type XIOErrorHandler = Option<unsafe extern "sysv64" fn(*mut Display) -> c_int>;
#[repr(C)]
pub struct XErrorEvent {
    pub kind: c_int,
    pub display: *mut Display,
    pub resourceid: c_ulong,
    pub serial: c_ulong,
    pub error_code: u8,
    pub request_code: u8,
    pub minor_code: u8,
}
static ERROR: AtomicUsize = AtomicUsize::new(0);
static IO_ERROR: AtomicUsize = AtomicUsize::new(0);
static IO_EXIT: AtomicUsize = AtomicUsize::new(0);
static IO_EXIT_DATA: AtomicUsize = AtomicUsize::new(0);
static MUTATION: Mutex<()> = Mutex::new(());

#[unsafe(export_name = "kinakaze_engine_libX11_XSetIOErrorExitHandler")]
pub unsafe extern "sysv64" fn XSetIOErrorExitHandler(
    display: *mut Display,
    handler: Option<unsafe extern "sysv64" fn(*mut Display, *mut core::ffi::c_void)>,
    data: *mut core::ffi::c_void,
) {
    if display.is_null() {
        return;
    }
    // XOpenDisplay references share the same hosted connection.
    let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
    IO_EXIT.store(
        handler.map_or(0, |callback| callback as usize),
        Ordering::Release,
    );
    IO_EXIT_DATA.store(data as usize, Ordering::Release);
}

/// Deliver a terminal connection failure. Callbacks run outside the lock.
pub unsafe fn io_failure(display: *mut Display) {
    let (handler, exit, data) = {
        let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
        (
            IO_ERROR.load(Ordering::Acquire),
            IO_EXIT.load(Ordering::Acquire),
            IO_EXIT_DATA.load(Ordering::Acquire),
        )
    };
    if handler != 0 {
        let callback: unsafe extern "sysv64" fn(*mut Display) -> c_int =
            unsafe { core::mem::transmute(handler) };
        unsafe {
            callback(display);
        }
    }
    if exit != 0 {
        let callback: unsafe extern "sysv64" fn(*mut Display, *mut core::ffi::c_void) =
            unsafe { core::mem::transmute(exit) };
        unsafe {
            callback(display, data as *mut core::ffi::c_void);
        }
    } else {
        libc::process::kinakaze_abi_exit(1);
    }
}

/// The XCB frontend returns protocol errors with its cookies instead of
/// invoking an Xlib application's process-wide error handler.
#[derive(Clone, Copy)]
pub struct ProtocolError {
    pub code: u8,
    pub request: u8,
    pub minor: u8,
    pub resource: usize,
}
thread_local! {
    static CAPTURE: core::cell::Cell<Option<Option<ProtocolError>>> = const { core::cell::Cell::new(None) };
}
pub fn capture<T>(operation: impl FnOnce() -> T) -> Result<T, ProtocolError> {
    struct Restore(Option<Option<ProtocolError>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CAPTURE.set(self.0);
        }
    }
    let restore = Restore(CAPTURE.replace(Some(None)));
    let value = operation();
    let error = CAPTURE.get().flatten();
    drop(restore);
    error.map_or(Ok(value), Err)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSetErrorHandler")]
pub unsafe extern "sysv64" fn XSetErrorHandler(handler: XErrorHandler) -> XErrorHandler {
    let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
    let old = ERROR.swap(handler.map_or(0, |f| f as usize), Ordering::AcqRel);
    if old == 0 {
        None
    } else {
        Some(unsafe { core::mem::transmute(old) })
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetIOErrorHandler")]
pub unsafe extern "sysv64" fn XSetIOErrorHandler(handler: XIOErrorHandler) -> XIOErrorHandler {
    let _guard = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
    let old = IO_ERROR.swap(handler.map_or(0, |f| f as usize), Ordering::AcqRel);
    if old == 0 {
        None
    } else {
        Some(unsafe { core::mem::transmute(old) })
    }
}
pub unsafe fn report(
    display: *mut Display,
    code: u8,
    request: u8,
    minor: u8,
    resource: usize,
) -> c_int {
    let serial = if display.is_null() {
        0
    } else {
        unsafe { (*display).request as u64 }
    };
    unsafe { report_serial(display, code, request, minor, resource, serial) }
}

/// Reports an error for a specific request serial; raw protocol requests are
/// answered after later requests may already have advanced the counter.
pub unsafe fn report_serial(
    display: *mut Display,
    code: u8,
    request: u8,
    minor: u8,
    resource: usize,
    serial: u64,
) -> c_int {
    if crate::trace_enabled() {
        crate::diagnostic!(
            "[libX11] error display={display:p} code={code} request={request} minor={minor} resource={resource:#x} serial={serial}"
        );
    }
    if CAPTURE.get().is_some() {
        CAPTURE.set(Some(Some(ProtocolError {
            code,
            request,
            minor,
            resource,
        })));
        return code as c_int;
    }
    let mut event = XErrorEvent {
        kind: 0,
        display,
        resourceid: resource as u64,
        serial,
        error_code: code,
        request_code: request,
        minor_code: minor,
    };
    let handler = ERROR.load(Ordering::Acquire);
    if handler != 0 {
        let callback: unsafe extern "sysv64" fn(*mut Display, *mut XErrorEvent) -> c_int =
            unsafe { core::mem::transmute(handler) };
        unsafe { callback(display, &raw mut event) };
    } else {
        crate::diagnostic!(
            "X error: {} (request {request}.{minor}, resource {resource:#x})",
            text(code as i32)
        );
        libc::process::kinakaze_abi_exit(1);
    }
    code as c_int
}
fn text(code: c_int) -> &'static str {
    match code {
        0 => "Success",
        1 => "BadRequest",
        2 => "BadValue",
        3 => "BadWindow",
        4 => "BadPixmap",
        5 => "BadAtom",
        6 => "BadCursor",
        7 => "BadFont",
        8 => "BadMatch",
        9 => "BadDrawable",
        10 => "BadAccess",
        11 => "BadAlloc",
        12 => "BadColor",
        13 => "BadGC",
        14 => "BadIDChoice",
        15 => "BadName",
        16 => "BadLength",
        17 => "BadImplementation",
        _ => "Extension error",
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetErrorText")]
pub unsafe extern "sysv64" fn XGetErrorText(
    _display: *mut Display,
    code: c_int,
    buffer: *mut c_char,
    length: c_int,
) -> c_int {
    if !buffer.is_null() && length > 0 {
        let bytes = text(code).as_bytes();
        let count = bytes.len().min(length as usize - 1);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast(), count);
            *buffer.add(count) = 0;
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11__XDefaultIOError")]
pub unsafe extern "sysv64" fn _XDefaultIOError(_display: *mut Display) -> c_int {
    crate::diagnostic!("X connection: unrecoverable I/O error");
    libc::process::kinakaze_abi_exit(1);
}
