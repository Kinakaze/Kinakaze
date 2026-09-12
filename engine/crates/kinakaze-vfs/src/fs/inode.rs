//! Versioned Linux inode metadata in one native EA. Unlike newly opened ADS,
//! EAs remain accessible through an open inode after its last name is unlinked.
//! This is the sole live format: legacy streams are read only by the explicit
//! offline migration entry point, never by stat/open or as an error fallback.

use super::{ea, object::Object};
use crate::{EIO, ENOENT};
use std::path::Path;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA};

pub(super) const EA_NAME: &[u8] = b"KINAKAZE.LINUX.INODE";
const MAGIC: &[u8; 8] = b"CYINODE3";
pub(crate) mod migration;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub mode: Option<u32>,
    pub device: Option<u64>,
    pub symlink: Option<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

impl Record {
    fn encode(&self) -> Result<Vec<u8>, i32> {
        let mut bytes = vec![0u8; 40];
        bytes[..8].copy_from_slice(MAGIC);
        let flags = u32::from(self.mode.is_some())
            | (u32::from(self.device.is_some()) << 1)
            | (u32::from(self.symlink.is_some()) << 2)
            | (u32::from(self.uid.is_some()) << 3)
            | (u32::from(self.gid.is_some()) << 4);
        bytes[8..12].copy_from_slice(&flags.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.mode.unwrap_or(0).to_le_bytes());
        bytes[16..24].copy_from_slice(&self.device.unwrap_or(0).to_le_bytes());
        bytes[28..32].copy_from_slice(&self.uid.unwrap_or(0).to_le_bytes());
        bytes[32..36].copy_from_slice(&self.gid.unwrap_or(0).to_le_bytes());
        if let Some(target) = &self.symlink {
            if target.is_empty() || target.contains('\0') {
                return Err(crate::EINVAL);
            }
            let length = u32::try_from(target.len()).map_err(|_| crate::ENAMETOOLONG)?;
            bytes[24..28].copy_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(target.as_bytes());
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        let legacy = bytes.get(..8) == Some(b"CYINODE2");
        let header = if legacy { 32 } else { 40 };
        if bytes.len() < header
            || (!legacy && bytes.get(..8) != Some(MAGIC))
            || if legacy {
                bytes[28..32] != [0; 4]
            } else {
                bytes[36..40] != [0; 4]
            }
        {
            return Err(EIO);
        }
        let flags = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let mode = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let device = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let length = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
        let uid = if legacy {
            0
        } else {
            u32::from_le_bytes(bytes[28..32].try_into().unwrap())
        };
        let gid = if legacy {
            0
        } else {
            u32::from_le_bytes(bytes[32..36].try_into().unwrap())
        };
        if flags & !if legacy { 7 } else { 31 } != 0
            || length != bytes.len() - header
            || flags & 8 == 0 && uid != 0
            || flags & 16 == 0 && gid != 0
            || flags & 8 != 0 && uid == u32::MAX
            || flags & 16 != 0 && gid == u32::MAX
            || (flags & 1 == 0 && mode != 0)
            || mode & !(super::S_IFMT | 0o7777) != 0
            || (flags & 2 == 0 && device != 0)
            || (flags & 4 == 0 && length != 0)
        {
            return Err(EIO);
        }
        let symlink = if flags & 4 != 0 {
            let target = std::str::from_utf8(&bytes[header..]).map_err(|_| EIO)?;
            if target.is_empty() || target.contains('\0') {
                return Err(EIO);
            }
            Some(target.to_owned())
        } else {
            None
        };
        Ok(Self {
            mode: (flags & 1 != 0).then_some(mode),
            device: (flags & 2 != 0).then_some(device),
            symlink,
            uid: (flags & 8 != 0).then_some(uid),
            gid: (flags & 16 != 0).then_some(gid),
        })
    }
}

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
    let object = unsafe { Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_READ_EA)? };
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
    let object =
        unsafe { Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_WRITE_EA)? };
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    let mut record = read_object(&object)?;
    change(&mut record)?;
    ea::write(&object, EA_NAME, &record.encode()?)
}

/// Whole-record copy for an unpublished inode; does not copy guest xattrs.
pub(crate) fn replace(handle: HANDLE, record: &Record) -> Result<(), i32> {
    let object = unsafe { Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_WRITE_EA)? };
    ea::write(&object, EA_NAME, &record.encode()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_format_reads_legacy_and_preserves_full_width_ids() {
        let mut legacy = vec![0u8; 32];
        legacy[..8].copy_from_slice(b"CYINODE2");
        legacy[8] = 1;
        legacy[12..16].copy_from_slice(&(super::super::S_IFREG | 0o644).to_le_bytes());
        let mut record = Record::decode(&legacy).unwrap();
        assert_eq!(record.uid, None);
        record.uid = Some(4_000_000_000);
        record.gid = Some(65534);
        assert_eq!(Record::decode(&record.encode().unwrap()).unwrap(), record);
    }

    #[test]
    fn record_roundtrip_and_corrupt_fields() {
        let record = Record {
            mode: Some(super::super::S_IFLNK | 0o777),
            device: None,
            symlink: Some("../目标".into()),
            ..Record::default()
        };
        let bytes = record.encode().unwrap();
        assert_eq!(Record::decode(&bytes).unwrap(), record);
        for offset in [0, 11, 15, 28] {
            let mut bad = bytes.clone();
            bad[offset] = 0xff;
            assert_eq!(Record::decode(&bad), Err(EIO));
        }
        assert_eq!(Record::decode(&bytes[..31]), Err(EIO));
        let mut bad = bytes.clone();
        bad.push(0);
        assert_eq!(Record::decode(&bad), Err(EIO));
    }
}
