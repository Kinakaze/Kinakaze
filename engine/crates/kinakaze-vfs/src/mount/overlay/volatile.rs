//! Volatile overlay durability policy and upper-volume I/O error epochs.
use super::super::policy::{self, Policy};
use crate::fs::{self, object::Object};
use crate::{EINVAL, EIO, ENOENT};
use std::ffi::OsStr;
use std::sync::Arc;
const ERROR_DOMAIN: u64 = u64::MAX;

pub(crate) fn errors(volume: u64) -> Result<Arc<Policy>, i32> {
    policy::get(ERROR_DOMAIN, volume, 0)
}
pub(crate) fn sample(upper: &Object) -> Result<u64, i32> {
    Ok(errors(fs::stat_handle(upper.raw(), false)?.st_dev)?.flags())
}
pub(crate) fn check(volume: u64, since: u64) -> Result<(), i32> {
    if errors(volume)?.flags() != since {
        Err(EIO)
    } else {
        Ok(())
    }
}
/// Only actual storage failures advance the epoch; permission, validation and
/// signal interruptions are not writeback failures.
pub(crate) fn record(handle: windows_sys::Win32::Foundation::HANDLE, error: i32) {
    if !matches!(error, crate::EIO | crate::ENOSPC | 122) {
        return;
    }
    if let Ok(stat) = fs::stat_handle(handle, false) {
        if let Ok(epoch) = errors(stat.st_dev) {
            epoch.advance_error_epoch();
        }
    }
}
pub(crate) fn check_marker(work: &Object) -> Result<(), i32> {
    let find = || {
        work.child(OsStr::new("work"), 0x80)?
            .child(OsStr::new("incompat"), 0x80)?
            .child(OsStr::new("volatile"), 0x80)
    };
    match find() {
        Ok(_) => Err(EINVAL),
        Err(ENOENT) => Ok(()),
        Err(e) => Err(e),
    }
}
pub(crate) fn create_marker(work: &Object) -> Result<(), i32> {
    let dir = work.create_directory_child(OsStr::new("work"), true)?;
    let incompat = dir.create_directory_child(OsStr::new("incompat"), true)?;
    incompat.create_directory_child(OsStr::new("volatile"), false)?;
    // Intentionally persistent on clean unmount, matching overlayfs. Reusing
    // this workdir requires explicitly discarding/recovering the volatile data.
    Ok(())
}
