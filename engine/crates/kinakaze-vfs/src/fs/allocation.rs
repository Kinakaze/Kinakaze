//! Dense file reservation owns a pinned native description throughout the call.
use super::*;
use crate::{ENODEV, EOPNOTSUPP};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALLOCATION_INFO, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_SPARSE_FILE,
    FILE_ATTRIBUTE_TAG_INFO, FILE_END_OF_FILE_INFO, FILE_STANDARD_INFO, FileAllocationInfo,
    FileAttributeTagInfo, FileEndOfFileInfo, FileStandardInfo, GetFileInformationByHandleEx,
};
const KEEP_SIZE: i32 = 1;
const EFBIG: i32 = 27;
fn last_errno() -> i32 {
    errno_from_win32(unsafe { GetLastError() })
}
fn query<T: Default>(handle: HANDLE, class: i32) -> Result<T, i32> {
    let mut value = T::default();
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            class,
            (&raw mut value).cast(),
            size_of::<T>() as u32,
        )
    } == 0
    {
        return Err(last_errno());
    }
    Ok(value)
}
fn set<T>(handle: HANDLE, class: i32, value: &T) -> Result<(), i32> {
    if unsafe {
        SetFileInformationByHandle(
            handle,
            class,
            ptr::from_ref(value).cast(),
            size_of::<T>() as u32,
        )
    } == 0
    {
        return Err(last_errno());
    }
    Ok(())
}
pub fn fallocate(fd: i32, mode: i32, offset: i64, length: i64) -> Result<(), i32> {
    if offset < 0 || length <= 0 {
        return Err(EINVAL);
    }
    let end = offset.checked_add(length).ok_or(EFBIG)?;
    let (object, _) = object::Object::from_fd_checked(fd, |entry| {
        if entry.flags.contains(FdFlags::PATH_ONLY) {
            return Err(EBADF);
        }
        if entry.kind == FdKind::Directory {
            return Err(EISDIR);
        }
        if !entry.flags.contains(FdFlags::SEEKABLE) {
            return Err(ESPIPE);
        }
        if entry.kind != FdKind::File || entry.raw == 0 {
            return Err(ENODEV);
        }
        Ok(())
    })?;
    let handle = object.raw();
    if object::Object::granted_access(handle)? & (FILE_WRITE_DATA | FILE_APPEND_DATA) == 0 {
        return Err(EBADF);
    }
    if mode & !KEEP_SIZE != 0 {
        return Err(EOPNOTSUPP);
    }
    let _inode = verity::lock(handle)?;
    verity::ensure_writable(handle)?;
    let attributes = query::<FILE_ATTRIBUTE_TAG_INFO>(handle, FileAttributeTagInfo)?;
    // The native allocation operation does not guarantee dense reservation for
    // sparse/compressed files. Never report a reservation that was not made.
    if attributes.FileAttributes & (FILE_ATTRIBUTE_SPARSE_FILE | FILE_ATTRIBUTE_COMPRESSED) != 0 {
        return Err(EOPNOTSUPP);
    }
    let info = query::<FILE_STANDARD_INFO>(handle, FileStandardInfo)?;
    let size = i64::try_from(verity::authoritative_size(handle)?).map_err(|_| EFBIG)?;
    if info.AllocationSize < 0 {
        return Err(EIO);
    }
    let requested = end.max(size);
    // Preserve earlier reservations and data even for an interior range.
    if requested > info.AllocationSize {
        set(
            handle,
            FileAllocationInfo,
            &FILE_ALLOCATION_INFO {
                AllocationSize: requested,
            },
        )?;
    }
    if mode & KEEP_SIZE == 0 && end > size {
        set(
            handle,
            FileEndOfFileInfo,
            &FILE_END_OF_FILE_INFO { EndOfFile: end },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::IntoRawHandle;
    struct Fixture {
        fd: i32,
        path: std::path::PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = crate::close(self.fd);
            let _ = std::fs::remove_file(&self.path);
        }
    }
    #[test]
    fn keep_size_reserves_native_blocks_without_moving_eof_or_releasing_an_earlier_reservation() {
        let path = std::env::temp_dir().join(format!(
            "kinakaze-reservation-{}-{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.set_len(9).unwrap();
        let fd = crate::install(
            file.into_raw_handle() as usize,
            FdKind::File,
            FdFlags::SEEKABLE,
        )
        .unwrap();
        let fixture = Fixture { fd, path };
        let pin = object::Object::from_fd(fd).unwrap();
        fallocate(fd, KEEP_SIZE, 0, 2 * 1024 * 1024).unwrap();
        let reserved = query::<FILE_STANDARD_INFO>(pin.raw(), FileStandardInfo).unwrap();
        assert!(reserved.AllocationSize >= 2 * 1024 * 1024);
        assert_eq!(reserved.EndOfFile, 9);
        fallocate(fd, 0, 0, 8192).unwrap();
        let extended = query::<FILE_STANDARD_INFO>(pin.raw(), FileStandardInfo).unwrap();
        assert_eq!(extended.EndOfFile, 8192);
        assert!(extended.AllocationSize >= reserved.AllocationSize);
        fallocate(fd, 0, 0, 1).unwrap();
        assert_eq!(
            query::<FILE_STANDARD_INFO>(pin.raw(), FileStandardInfo)
                .unwrap()
                .EndOfFile,
            8192
        );
        drop(pin);
        drop(fixture);
    }
}
