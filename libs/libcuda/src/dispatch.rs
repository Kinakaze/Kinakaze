//! Version and stream-aware proc lookup. Unknown host code never escapes to ELF.

use crate::{CuResult, INVALID_VALUE, NOT_FOUND, SUCCESS, api, host};
use core::ffi::{CStr, c_char, c_void};
use core::ptr;

pub(crate) struct Entry {
    pub host: &'static host::Symbol,
    pub thunk: fn() -> usize,
}

static QUERY_V1: host::Symbol =
    host::Symbol::new(b"cuGetProcAddress\0", b"cuGetProcAddress\0", 11030, 0);
static QUERY_V2: host::Symbol =
    host::Symbol::new(b"cuGetProcAddress_v2\0", b"cuGetProcAddress\0", 12000, 0);

fn bridge(name: &[u8], address: usize) -> Option<usize> {
    if name == b"cuGetProcAddress\0" {
        if QUERY_V1.canonical() == Some(address) {
            return Some(cuGetProcAddress as *const () as usize);
        }
        if QUERY_V2.canonical() == Some(address) {
            return Some(cuGetProcAddress_v2 as *const () as usize);
        }
    }
    if name == b"cuModuleLoad\0" && crate::module::LOAD.canonical() == Some(address) {
        return Some(crate::cuModuleLoad as *const () as usize);
    }
    api::ENTRIES.iter().find_map(|entry| {
        (entry.host.base == name && entry.host.canonical() == Some(address))
            .then(|| (entry.thunk)())
    })
}

#[unsafe(export_name = "kinakaze_engine_libcuda_cuGetProcAddress_v2")]
pub unsafe extern "sysv64" fn cuGetProcAddress_v2(
    name: *const c_char,
    output: *mut *mut c_void,
    version: i32,
    flags: u64,
    status: *mut i32,
) -> CuResult {
    let _tls = host::RestoreTls;
    if output.is_null() || name.is_null() || flags > 2 {
        return INVALID_VALUE;
    }
    unsafe { output.write(ptr::null_mut()) };
    let answer = unsafe { host::query(name, version, flags) };
    if answer.result != SUCCESS {
        return answer.result;
    }
    let mut query_status = answer.status;
    if !answer.address.is_null() && query_status == 0 {
        let name = unsafe { CStr::from_ptr(name) }.to_bytes_with_nul();
        if let Some(thunk) = bridge(name, answer.address as usize) {
            unsafe { output.write(thunk as *mut c_void) };
        } else {
            query_status = 1; // Host API exists, but no guest ABI bridge exists.
        }
    }
    if !status.is_null() {
        unsafe { status.write(query_status) };
    }
    SUCCESS
}

#[unsafe(export_name = "kinakaze_engine_libcuda_cuGetProcAddress")]
pub unsafe extern "sysv64" fn cuGetProcAddress(
    name: *const c_char,
    output: *mut *mut c_void,
    version: i32,
    flags: u64,
) -> CuResult {
    let mut status = 1;
    let result = unsafe { cuGetProcAddress_v2(name, output, version, flags, &mut status) };
    if result == SUCCESS && status != 0 {
        NOT_FOUND
    } else {
        result
    }
}
