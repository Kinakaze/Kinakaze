//! Versioned Linux inode metadata in one native EA. Unlike newly opened ADS,
//! EAs remain accessible through an open inode after its last name is unlinked.
//! This is the sole live format: legacy streams are read only by the explicit
//! offline migration entry point, never by stat/open or as an error fallback.

use super::{ea, object::Object};
use crate::ENOENT;
use std::path::Path;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA};

pub(super) use kinakaze_v2_abi::inode::EA_NAME;
pub(crate) mod migration;

pub(crate) use kinakaze_v2_abi::inode::Record;

/// Query an independently opened metadata handle; no shared data I/O can be
/// cancelled by this query. Callers with a borrowed descriptor use `read`.
pub(super) fn read_object(object: &Object) -> Result<Record, i32> {
    ea::read(object, EA_NAME)?
        .map(|bytes| Record::decode(&bytes))
        .transpose()
        .map(Option::unwrap_or_default)
}

/// The caller keeps the borrowed inode live. An independent open owns each
/// native query, so cancellation cannot cancel another fd's data operation.
pub(crate) fn read(handle: HANDLE) -> Result<Record, i32> {
    let object = Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_READ_EA)?;
    read_object(&object)
}

pub(crate) fn read_path(path: &Path) -> Result<Record, i32> {
    let object = match Object::open(path, FILE_READ_ATTRIBUTES | FILE_READ_EA) {
        Ok(object) => object,
        Err(ENOENT) => return Ok(Record::default()),
        Err(error) => return Err(error),
    };
    read_object(&object)
}

pub(crate) fn update(
    handle: HANDLE,
    change: impl FnOnce(&mut Record) -> Result<(), i32>,
) -> Result<(), i32> {
    let object = Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_WRITE_EA)?;
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    let mut record = read_object(&object)?;
    change(&mut record)?;
    ea::write(&object, EA_NAME, &record.encode()?)
}

/// Whole-record copy for an unpublished inode; does not copy guest xattrs.
pub(crate) fn replace(handle: HANDLE, record: &Record) -> Result<(), i32> {
    let object = Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_WRITE_EA)?;
    ea::write(&object, EA_NAME, &record.encode()?)
}
