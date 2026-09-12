//! X Context Manager implementation.
//!
//! Provides thread-safe association of caller data with (display, XID, context_type).

use std::collections::HashMap;
use std::os::raw::{c_char, c_int};
use std::sync::{Mutex, OnceLock};

pub type XContext = c_int;
pub type XPointer = *mut c_char;
pub type XID = usize;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ContextKey {
    display: usize,
    xid: XID,
    context: XContext,
}

fn context_map() -> &'static Mutex<HashMap<ContextKey, usize>> {
    static MAP: OnceLock<Mutex<HashMap<ContextKey, usize>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[unsafe(export_name = "kinakaze_engine_libX11_XUniqueContext")]
pub unsafe extern "sysv64" fn XUniqueContext() -> XContext {
    crate::xrm::XrmUniqueQuark()
}

#[unsafe(export_name = "kinakaze_engine_libX11_XSaveContext")]
pub unsafe extern "sysv64" fn XSaveContext(
    dpy: *mut crate::Display,
    xid: XID,
    context: XContext,
    data: XPointer,
) -> c_int {
    let key = ContextKey {
        display: dpy as usize,
        xid,
        context,
    };
    if let Ok(mut map) = context_map().lock() {
        map.insert(key, data as usize);
        0
    } else {
        1 // XCNOENT
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFindContext")]
pub unsafe extern "sysv64" fn XFindContext(
    dpy: *mut crate::Display,
    xid: XID,
    context: XContext,
    data_return: *mut XPointer,
) -> c_int {
    let key = ContextKey {
        display: dpy as usize,
        xid,
        context,
    };
    if let Ok(map) = context_map().lock() {
        if let Some(&val) = map.get(&key) {
            if !data_return.is_null() {
                unsafe { *data_return = val as XPointer };
            }
            return 0;
        }
    }
    1 // XCNOENT
}

#[unsafe(export_name = "kinakaze_engine_libX11_XDeleteContext")]
pub unsafe extern "sysv64" fn XDeleteContext(
    dpy: *mut crate::Display,
    xid: XID,
    context: XContext,
) -> c_int {
    let key = ContextKey {
        display: dpy as usize,
        xid,
        context,
    };
    if let Ok(mut map) = context_map().lock() {
        if map.remove(&key).is_some() { 0 } else { 1 }
    } else {
        1
    }
}
