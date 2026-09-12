//! Filesystem FIFOs backed by one shared byte queue per physical inode.
//!
//! The guest directory entry remains a zero-length `S_IFIFO` marker. Its full
//! Windows file identity selects the shared queue, and native delete-on-close
//! tokens track open descriptions across dup, fork, exec, and process death.
//! Every reader, writer, and O_RDWR open uses that same queue; there is no broker
//! and no private-pipe fallback.

pub(crate) mod lifecycle;
mod runtime;
mod shared;

pub use runtime::{
    AuxiliaryHandle, auxiliary_handles, capacity, close_entry, duplicate_descriptor, link_target,
    metadata, prepare_wait, queued_bytes, read, restore_fork_state, serialize_matching,
    set_nonblocking, set_status_flags, status_flags, write,
};
pub(crate) use runtime::{
    Pinned, disable_inheritance_locked, import_rights, open_shared_marker as open_marker,
    pin_entry_locked, reopen_pinned, rights_reference,
};
pub use shared::{Readiness, WaitRegistration};

use std::path::Path;
use std::ptr;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, SYNCHRONIZE,
};

/// Opens a marker for identity and metadata, not for file-content I/O.
pub(crate) fn open_path(path: &Path, flags: i32) -> Result<i32, i32> {
    let path = crate::path::wide_path(path)?;
    let marker = lifecycle::Owned::checked(unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_READ_ATTRIBUTES | FILE_READ_EA | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    })?;
    open_marker(marker.0, flags)
}
