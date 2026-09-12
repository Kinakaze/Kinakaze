//! File-based module loading follows the guest VFS, including mounted files.

use crate::{
    CuModule, CuResult, FILE_NOT_FOUND, INVALID_VALUE, NOT_SUPPORTED, OUT_OF_MEMORY, UNKNOWN, host,
};
use core::ffi::{CStr, c_char};

pub(crate) static LOAD: host::Symbol =
    host::Symbol::new(b"cuModuleLoad\0", b"cuModuleLoad\0", 2000, 0);

struct File(i32);
impl Drop for File {
    fn drop(&mut self) {
        let _ = kinakaze_vfs::close(self.0);
    }
}

#[unsafe(export_name = "kinakaze_engine_libcuda_cuModuleLoad")]
pub unsafe extern "sysv64" fn cuModuleLoad(output: *mut CuModule, path: *const c_char) -> CuResult {
    let _tls = host::RestoreTls;
    if crate::lifecycle::unusable() {
        return NOT_SUPPORTED;
    }
    if output.is_null() || path.is_null() {
        return INVALID_VALUE;
    }
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        return INVALID_VALUE;
    };
    let fd = match kinakaze_vfs::fs::open(
        path,
        kinakaze_vfs::fs::O_RDONLY | kinakaze_vfs::fs::O_CLOEXEC,
        0,
    ) {
        Ok(fd) => File(fd),
        Err(kinakaze_vfs::ENOENT) => return FILE_NOT_FOUND,
        Err(_) => return UNKNOWN,
    };
    let mut bytes = Vec::new();
    if let Ok(metadata) = kinakaze_vfs::fs::fstat(fd.0) {
        let Some(size) = usize::try_from(metadata.st_size)
            .ok()
            .and_then(|size| size.checked_add(1))
        else {
            return OUT_OF_MEMORY;
        };
        // Fatbins can be large: reserve known file size once instead of growing
        // through multiple allocations while copying the same image again.
        if bytes.try_reserve_exact(size).is_err() {
            return OUT_OF_MEMORY;
        }
    }
    let mut block = [0u8; 16 * 1024];
    loop {
        let count = match kinakaze_vfs::read(fd.0, &mut block) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => return UNKNOWN,
        };
        if bytes.try_reserve(count + 1).is_err() {
            return OUT_OF_MEMORY;
        }
        bytes.extend_from_slice(&block[..count]);
    }
    if bytes.is_empty() {
        return crate::INVALID_IMAGE;
    }
    if bytes.try_reserve(1).is_err() {
        return OUT_OF_MEMORY;
    }
    bytes.push(0); // Required by PTX; ignored by self-describing cubin/fatbin data.
    unsafe { crate::cuModuleLoadData(output, bytes.as_ptr().cast()) }
}
