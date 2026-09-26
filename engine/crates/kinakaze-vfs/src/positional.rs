//! One operation owns one native inode pin. Explicit offsets never touch the
//! open-description position or an inherited synchronous handle's seek pointer.
//! No signal callback runs with these operation-local handles alive.

use crate::fs::object::Object;
use crate::native_pin::{NativePin, pin_native_fd};
use crate::{EBADF, EISDIR, ESPIPE, FdEntry, FdFlags, FdKind};

pub struct File {
    entry: FdEntry,
    pin: NativePin,
    _reopened: Option<Object>,
    writing: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::io::IntoRawHandle;

    #[test]
    fn synchronous_position_and_pinned_identity_survive_close_and_reuse() {
        let path =
            std::env::temp_dir().join(format!("kinakaze-positional-{}.tmp", std::process::id()));
        let other_path = path.with_extension("other");
        let mut original = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        original.write_all(b"abcdefgh").unwrap();
        original.seek(SeekFrom::Start(5)).unwrap();
        let flags = FdFlags::SEEKABLE
            .union(FdFlags::READ_ACCESS)
            .union(FdFlags::WRITE_ACCESS);
        let fd = crate::install(
            original.try_clone().unwrap().into_raw_handle() as usize,
            FdKind::File,
            flags,
        )
        .unwrap();
        let writer = File::open(fd, true).unwrap();
        let reader = File::open(fd, false).unwrap();
        assert_eq!(unsafe { writer.write_once(1, b"XYZ".as_ptr(), 3) }, Ok(3));
        assert_eq!(original.stream_position().unwrap(), 5);
        crate::close(fd).unwrap();
        let mut other = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&other_path)
            .unwrap();
        other.write_all(b"untouched").unwrap();
        crate::install_exact(
            other.try_clone().unwrap().into_raw_handle() as usize,
            FdKind::File,
            flags,
            fd,
        )
        .unwrap();
        assert_eq!(unsafe { writer.write_once(4, b"123".as_ptr(), 3) }, Ok(3));
        let mut bytes = [0u8; 8];
        assert_eq!(
            unsafe { reader.read_once(0, bytes.as_mut_ptr(), bytes.len()) },
            Ok(8)
        );
        assert_eq!(&bytes, b"aXYZ123h");
        assert_eq!(original.stream_position().unwrap(), 5);
        other.seek(SeekFrom::Start(0)).unwrap();
        let mut other_bytes = String::new();
        other.read_to_string(&mut other_bytes).unwrap();
        assert_eq!(other_bytes, "untouched");
        crate::close(fd).unwrap();
        drop((writer, reader, original, other));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(other_path).unwrap();
    }
}

impl File {
    pub fn open(fd: i32, writing: bool) -> Result<Self, i32> {
        let (mut entry, pin) = pin_native_fd(fd, |entry| {
            if entry.flags.contains(FdFlags::PATH_ONLY)
                || (writing
                    && entry.flags.contains(FdFlags::READ_ACCESS)
                    && !entry.flags.contains(FdFlags::WRITE_ACCESS))
                || (!writing
                    && entry.flags.contains(FdFlags::WRITE_ACCESS)
                    && !entry.flags.contains(FdFlags::READ_ACCESS))
            {
                return Err(EBADF);
            }
            if matches!(entry.kind, FdKind::Directory | FdKind::SyntheticDirectory) {
                return Err(EISDIR);
            }
            if entry.kind != FdKind::File || !entry.flags.contains(FdFlags::SEEKABLE) {
                return Err(ESPIPE);
            }
            Ok(())
        })?;
        let mut reopened = None;
        if writing {
            crate::platform::check_file_write(&entry)?;
            if !entry.flags.contains(FdFlags::OVERLAPPED) {
                // DuplicateHandle shares a synchronous file position. Reopen
                // the same pinned inode with no synchronous-open flags instead.
                let access = Object::granted_access(entry.raw as _)?;
                let object = Object::reopen(entry.raw as _, access)?;
                entry.raw = object.raw() as usize;
                entry.flags = entry.flags.union(FdFlags::OVERLAPPED);
                reopened = Some(object);
            }
        }
        Ok(Self {
            entry,
            pin,
            _reopened: reopened,
            writing,
        })
    }

    /// # Safety
    /// The buffer holds `length` writable bytes. Drop this file before calling
    /// guest signal handlers, including after EINTR.
    pub unsafe fn read_once(
        &self,
        offset: u64,
        buffer: *mut u8,
        length: usize,
    ) -> Result<usize, i32> {
        if self.writing {
            return Err(EBADF);
        }
        unsafe {
            self.pin.read_once(
                FdEntry {
                    offset,
                    ..self.entry
                },
                buffer,
                length,
            )
        }
    }

    /// # Safety
    /// The buffer holds `length` readable bytes. Drop this file before calling
    /// guest signal handlers. O_APPEND is honored without changing its status.
    pub unsafe fn write_once(
        &self,
        offset: u64,
        buffer: *const u8,
        length: usize,
    ) -> Result<usize, i32> {
        if !self.writing {
            return Err(EBADF);
        }
        // Recheck verity protection at the write boundary, as ordinary write
        // does. The owned pin keeps both the inode and mount policy alive.
        crate::platform::check_file_write(&self.entry)?;
        unsafe {
            crate::platform::transfer_once(
                &FdEntry {
                    offset,
                    ..self.entry
                },
                buffer.cast_mut(),
                length,
                false,
            )
        }
    }
}
