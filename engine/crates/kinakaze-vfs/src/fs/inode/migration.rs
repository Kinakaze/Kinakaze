//! Explicit, offline, inode-by-inode conversion. Re-running a partially finished
//! migration is safe: a valid current record is never overwritten. Legacy ADS
//! are retained as backups; no live metadata operation consults them.

use super::{EA_NAME, Object, Record, ea};
use crate::{EINVAL, EIO, errno_from_win32};
use std::path::Path;
use windows_sys::Win32::{
    Foundation::GetLastError,
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_BASIC_INFO,
        FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_ATTRIBUTES, FILE_WRITE_EA, FileBasicInfo,
        GetFileInformationByHandleEx, SetFileInformationByHandle,
    },
};

#[derive(Debug, Default)]
pub struct Report {
    pub visited: usize,
    pub legacy: usize,
    pub converted: usize,
    pub current: usize,
    pub unmanaged: usize,
    pub reparse_points: usize,
}

fn legacy(object: &Object) -> Result<Record, i32> {
    let mode = unsafe { crate::fs::object::metadata_stream(object.raw(), ":kinakaze.mode", 8)? }
        .map(|bytes| {
            if bytes.len() != 8 || &bytes[..4] != b"CYMD" {
                return Err(EIO);
            }
            let mode = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
            if mode & !(crate::fs::S_IFMT | 0o7777) != 0 {
                return Err(EIO);
            }
            Ok(mode)
        })
        .transpose()?;
    let device =
        unsafe { crate::fs::object::metadata_stream(object.raw(), ":kinakaze.device", 12)? }
            .map(|bytes| {
                if bytes.len() != 12 || &bytes[..4] != b"CYDV" {
                    return Err(EIO);
                }
                Ok(u64::from_le_bytes(bytes[4..12].try_into().unwrap()))
            })
            .transpose()?;
    let symlink =
        unsafe { crate::fs::object::metadata_stream(object.raw(), ":kinakaze.symlink", 65_536)? }
            .map(|bytes| {
                let target = bytes.strip_prefix(b"CYSL\0\x01").ok_or(EIO)?;
                String::from_utf8(target.to_vec()).map_err(|_| EIO)
            })
            .transpose()?;
    let record = Record {
        mode,
        device,
        symlink,
        ..Record::default()
    };
    // Validate exactly the record the runtime will read, including target and
    // native EA size, before allowing any writes in either check/apply mode.
    let encoded = record.encode()?;
    if encoded.len() + 9 + EA_NAME.len() > 65_535 {
        return Err(crate::ENOSPC);
    }
    Record::decode(&encoded)?;
    Ok(record)
}

fn convert(object: &Object, apply: bool, report: &mut Report) -> Result<(), i32> {
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    report.visited += 1;
    if let Some(bytes) = ea::read(object, EA_NAME)? {
        Record::decode(&bytes)?;
        report.current += 1;
        return Ok(());
    }
    let record = legacy(object)?;
    if record == Record::default() {
        report.unmanaged += 1;
        return Ok(());
    }
    report.legacy += 1;
    if apply {
        let writer = unsafe { Object::reopen(object.raw(), FILE_READ_ATTRIBUTES | FILE_WRITE_EA)? };
        ea::write(&writer, EA_NAME, &record.encode()?)?;
        if super::read_object(object)? != record {
            return Err(EIO);
        }
        report.converted += 1;
    }
    Ok(())
}

fn basic(object: &Object) -> Result<FILE_BASIC_INFO, i32> {
    let mut basic = unsafe { std::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            object.raw(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            std::mem::size_of_val(&basic) as u32,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(basic)
}

struct Entry {
    object: Object,
    basic: FILE_BASIC_INFO,
    restore: bool,
}
impl Entry {
    fn finish(&mut self) -> Result<(), i32> {
        if self.restore {
            // Preserve timestamps on successful migration and on partial-failure
            // unwind. This is maintenance, not a guest chmod/write operation.
            if unsafe {
                SetFileInformationByHandle(
                    self.object.raw(),
                    FileBasicInfo,
                    (&self.basic as *const FILE_BASIC_INFO).cast(),
                    std::mem::size_of_val(&self.basic) as u32,
                )
            } == 0
            {
                return Err(errno_from_win32(unsafe { GetLastError() }));
            }
            self.restore = false;
        }
        Ok(())
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("kinakaze: migration timestamp restoration failed: Linux errno {error}");
        }
    }
}

/// Caller must quiesce every writer and must not deploy the new runtime until
/// this entire pass succeeds. The tool never stops processes or follows host
/// reparse points. Paths are diagnostic only after the initial root open.
pub fn run(root: &Path, apply: bool) -> Result<Report, i32> {
    if !root.is_absolute() || root.parent().is_none() {
        return Err(EINVAL);
    }
    let access = FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_WRITE_ATTRIBUTES;
    let mut next = Object::open(root, access)?;
    let root_info = basic(&next)?;
    if root_info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(crate::ELOOP);
    }
    if root_info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(crate::ENOTDIR);
    }
    let mut parents: Vec<(Entry, std::vec::IntoIter<std::ffi::OsString>)> = Vec::new();
    let mut report = Report::default();
    loop {
        let info = basic(&next)?;
        let mut entry = Entry {
            object: next,
            basic: info,
            restore: apply,
        };
        if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            report.reparse_points += 1;
        } else {
            convert(&entry.object, apply, &mut report).map_err(|error| {
                eprintln!(
                    "kinakaze: migration failed at {:?}: Linux errno {error}",
                    entry.object.path()
                );
                error
            })?;
            if info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                let names = entry.object.maintenance_entries()?;
                parents.push((entry, names.into_iter()));
                // The current entry now belongs to the traversal stack.
                next = match advance(&mut parents, access)? {
                    Some(object) => object,
                    None => break,
                };
                continue;
            }
        }
        entry.finish()?;
        next = match advance(&mut parents, access)? {
            Some(object) => object,
            None => break,
        };
    }
    Ok(report)
}

fn advance(
    parents: &mut Vec<(Entry, std::vec::IntoIter<std::ffi::OsString>)>,
    access: u32,
) -> Result<Option<Object>, i32> {
    while let Some((parent, names)) = parents.last_mut() {
        if let Some(name) = names.next() {
            return parent.object.child(&name, access).map(Some);
        }
        parent.finish()?;
        parents.pop();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_migration_preserves_unlinked_modes_and_is_idempotent() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-inode-migration-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("file");
        std::fs::write(&path, b"original").unwrap();
        let mut mode = b"CYMD".to_vec();
        mode.extend_from_slice(&0o640u32.to_le_bytes());
        std::fs::write(root.join("file:kinakaze.mode"), &mode).unwrap();
        let check = run(&root, false).unwrap();
        assert_eq!((check.legacy, check.converted), (1, 0));
        let object = Object::open(&path, FILE_READ_ATTRIBUTES | FILE_READ_EA).unwrap();
        assert_eq!(super::super::read(object.raw()).unwrap(), Record::default());
        assert_eq!(run(&root, true).unwrap().converted, 1);
        assert_eq!(run(&root, true).unwrap().converted, 0);
        crate::fs::set_mode_handle(object.raw(), 0o600).unwrap();
        assert_eq!(run(&root, true).unwrap().converted, 0);
        assert_eq!(super::super::read(object.raw()).unwrap().mode, Some(0o600));
        crate::fs::set_mode_handle(object.raw(), 0o640).unwrap();
        assert_eq!(
            std::fs::read(root.join("file:kinakaze.mode")).unwrap(),
            mode
        );
        crate::fs::unlink(&crate::path::to_guest_path(&path)).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        assert_eq!(
            crate::fs::stat_handle(object.raw(), false).unwrap().st_mode & 0o7777,
            0o640
        );
        drop(object);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn invalid_legacy_record_is_not_published_and_symlinks_convert_without_following() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-inode-invalid-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("link");
        std::fs::write(&path, b"").unwrap();
        std::fs::write(root.join("link:kinakaze.mode"), b"bad").unwrap();
        assert!(matches!(run(&root, true), Err(EIO)));
        let object = Object::open(&path, FILE_READ_ATTRIBUTES | FILE_READ_EA).unwrap();
        assert_eq!(ea::read(&object, EA_NAME).unwrap(), None);
        let mut mode = b"CYMD".to_vec();
        mode.extend_from_slice(&0o777u32.to_le_bytes());
        std::fs::write(root.join("link:kinakaze.mode"), mode).unwrap();
        std::fs::write(
            root.join("link:kinakaze.symlink"),
            b"CYSL\0\x01../../outside",
        )
        .unwrap();
        assert_eq!(run(&root, true).unwrap().converted, 1);
        assert_eq!(
            crate::fs::symlink_target_handle(object.raw())
                .unwrap()
                .as_deref(),
            Some("../../outside")
        );
        assert_eq!(
            crate::fs::stat_handle(object.raw(), false).unwrap().st_mode & crate::fs::S_IFMT,
            crate::fs::S_IFLNK
        );
        drop(object);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
