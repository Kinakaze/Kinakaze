//! Windows resources owned by the V2 runtime infrastructure.
//!
//! Native handles never cross the RPC or provider ABI. Every owning object
//! releases its resource on drop, including partially completed operations.

#![cfg(windows)]

mod diagnostics;
mod file_map;
mod library;
mod memory;
mod pipes;
mod process;
mod security;
mod startup_profile;

pub use diagnostics::{ProcessMetrics, process_metrics};
pub use file_map::ReadOnlyFile;
pub use library::{Library, LoadedModule};
pub use memory::{ExecutableMemory, MemoryProtection, page_size};
pub use pipes::{PipeConnection, PipeListener};
pub use process::{
    Job, ProcessHandle, background_creation_flags, inherited_process_priority,
    set_current_process_priority,
};
pub use startup_profile::StartupSpan;

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "embedded NUL"));
    }
    result.push(0);
    Ok(result)
}

// SAFETY: The caller transfers an exclusively owned, CloseHandle-compatible
// handle, or a null/INVALID_HANDLE_VALUE failure sentinel.
unsafe fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: The ownership precondition is forwarded from the caller.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

/// Generate a 256-bit opaque capability using the Windows system RNG.
/// Callers must not log the returned token.
pub fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    // SAFETY: The supplied buffer is writable for its declared length.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(io::Error::other(format!(
            "BCryptGenRandom failed: {status:#x}"
        )));
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(64);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(result)
}

/// Create a credential file with a protected current-user DACL from its first byte.
pub fn private_file(path: &std::path::Path) -> io::Result<std::fs::File> {
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
    };
    let name = wide(path.as_os_str())?;
    let security = security::UserSecurity::new()?;
    let attributes = security.attributes();
    // SAFETY: All inputs remain live during CreateFileW; CREATE_NEW never opens
    // an existing file or link, and this returned handle has one owner.
    let handle = unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_WRITE,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        ))?
    };
    Ok(std::fs::File::from(handle))
}
