//! Data writeback hints must not force a device-cache flush for every file.
//! fsync/fdatasync remain separate durability barriers. Windows flushes the
//! whole file and may wait, which is stronger than a requested range/write hint.

use super::{NativeIoStatus, object::Object};
use crate::{EACCES, EBADF, EINVAL, ESPIPE, FdEntry, FdFlags, FdKind, errno_from_win32};
use std::os::windows::io::AsRawHandle;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_WRITE_DATA};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtFlushBuffersFileEx(
        file: HANDLE,
        flags: u32,
        parameters: *const core::ffi::c_void,
        size: u32,
        io: *mut NativeIoStatus,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

fn validate(entry: FdEntry) -> Result<(), i32> {
    if entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    if !matches!(
        entry.kind,
        FdKind::File | FdKind::Directory | FdKind::TmpfsFile | FdKind::TmpfsDirectory
    ) {
        return Err(ESPIPE);
    }
    Ok(())
}

/// Hold the open inode and its mount policy through the durability barrier.
/// Descriptor reuse after lookup must not redirect the flush to another file.
pub fn sync_descriptor(fd: i32) -> Result<(), i32> {
    let entry = crate::get(fd)?;
    if entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    match entry.kind {
        FdKind::TmpfsFile
        | FdKind::TmpfsDirectory
        | FdKind::MessageQueue
        | FdKind::SysfsFile
        | FdKind::Synthetic
        | FdKind::SyntheticDirectory
        | FdKind::CgroupFile => return Ok(()),
        FdKind::File | FdKind::Directory => {}
        _ => return Err(EINVAL),
    }
    let _deferred = crate::signal::defer_delivery();
    let (entry, pin) = crate::pin_native_fd(fd, |entry| {
        if entry.flags.contains(FdFlags::PATH_ONLY) {
            return Err(EBADF);
        }
        if !matches!(entry.kind, FdKind::File | FdKind::Directory) {
            return Err(EINVAL);
        }
        Ok(())
    })?;
    if pin.sync_overlay()?.is_some() {
        return Ok(());
    }
    // Preserve the existing directory compatibility behavior. This is not
    // a promise of Linux directory-fsync crash durability on all hosts.
    if entry.kind == FdKind::Directory {
        return Ok(());
    }
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
    if unsafe { FlushFileBuffers(pin.as_raw_handle()) } != 0 {
        return Ok(());
    }
    let error = unsafe { GetLastError() };
    // Read-only native opens retain the established compatibility behavior.
    if error == ERROR_ACCESS_DENIED {
        Ok(())
    } else {
        Err(errno_from_win32(error))
    }
}

pub fn sync_file_range(fd: i32, offset: i64, nbytes: i64, flags: u32) -> Result<(), i32> {
    let entry = crate::get(fd)?;
    if flags & !7 != 0 || offset < 0 || nbytes < 0 || offset.checked_add(nbytes).is_none() {
        return Err(EINVAL);
    }
    validate(entry)?;
    if flags == 0 || matches!(entry.kind, FdKind::TmpfsFile | FdKind::TmpfsDirectory) {
        return Ok(());
    }
    let _deferred = crate::signal::defer_delivery();
    let (entry, pin) = crate::pin_native_fd(fd, validate)?;
    // Preserve overlay copy-up and volatile-mount error handling using the
    // exact open description captured by the pin, even during close/dup2.
    if pin.sync_overlay()?.is_some() || entry.kind == FdKind::Directory {
        return Ok(());
    }
    // The original open retains write access after chmod sets the
    // host readonly bit. Reopening here rejects that valid writer and
    // used to promote a data writeback hint into a device-cache flush.
    // A pin keeps the exact inode alive across concurrent close/dup2.
    match flush_data(pin.as_raw_handle()) {
        Err(EACCES) => {
            // Linux permits a readonly descriptor to request writeback.
            // Request additional access only for this exceptional case.
            match Object::reopen(pin.as_raw_handle(), FILE_WRITE_DATA | FILE_READ_ATTRIBUTES) {
                Ok(object) => flush_data(object.raw()),
                Err(EACCES) => Ok(()), // established readonly-host compatibility
                Err(error) => Err(error),
            }
        }
        result => result,
    }
}

/// A request on a pinned, potentially shared file object. NtFlushBuffersFileEx
/// normally completes synchronously. A filesystem/filter can return PENDING;
/// retain our status block until *our* request completes, without cancelling
/// unrelated reads/writes on dup/fork aliases or trusting their shared event.
fn flush_data(handle: HANDLE) -> Result<(), i32> {
    let mut io = NativeIoStatus {
        status: 0x103,
        information: 0,
    };
    const FLUSH_FLAGS_FILE_DATA_ONLY: u32 = 1;
    let mut status = unsafe {
        NtFlushBuffersFileEx(
            handle,
            FLUSH_FLAGS_FILE_DATA_ONLY,
            std::ptr::null(),
            0,
            &mut io,
        )
    };
    if status == 0x103 {
        loop {
            status = unsafe { std::ptr::read_volatile(&io.status) as i32 };
            if status != 0x103 {
                break;
            }
            // Bounded kernel sleep only on the exceptional pending path. A
            // shared file event may already be signalled by unrelated I/O.
            unsafe { windows_sys::Win32::System::Threading::Sleep(1) };
        }
    }
    match status as u32 {
        0 => Ok(()),
        // Keep a real durability operation on filesystems lacking data-only
        // support. I/O errors and dismounts must reach the Linux caller.
        0xc000_0002 | 0xc000_000d | 0xc000_0010 | 0xc000_00bb => {
            use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
            if unsafe { FlushFileBuffers(handle) } != 0 {
                Ok(())
            } else {
                Err(errno_from_win32(unsafe {
                    windows_sys::Win32::Foundation::GetLastError()
                }))
            }
        }
        _ => Err(errno_from_win32(unsafe { RtlNtStatusToDosError(status) })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::io::IntoRawHandle;

    #[test]
    fn writeback_preserves_open_inode_contents_offset_and_releases_handles() {
        let directory =
            std::env::temp_dir().join(format!("kinakaze-writeback-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("original");
        let moved = directory.join("moved");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let bytes = vec![0x5a; 128 * 1024];
        file.write_all(&bytes).unwrap();
        file.seek(SeekFrom::Start(17)).unwrap();
        let fd = crate::install(
            file.try_clone().unwrap().into_raw_handle() as usize,
            FdKind::File,
            FdFlags::READ_ACCESS
                .union(FdFlags::WRITE_ACCESS)
                .union(FdFlags::SEEKABLE),
        )
        .unwrap();
        let path_fd = crate::install(
            file.try_clone().unwrap().into_raw_handle() as usize,
            FdKind::File,
            FdFlags::PATH_ONLY,
        )
        .unwrap();
        assert_eq!(sync_descriptor(path_fd), Err(EBADF));
        crate::close(path_fd).unwrap();
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        for _ in 0..32 {
            assert_eq!(sync_descriptor(fd), Ok(()));
            for flags in 0..8 {
                assert_eq!(sync_file_range(fd, 1, 4097, flags), Ok(()));
                assert_eq!(file.stream_position().unwrap(), 17);
            }
        }
        for (offset, length, flags) in [(-1, 0, 2), (0, -1, 2), (i64::MAX, 1, 2), (0, 0, 8)] {
            assert_eq!(sync_file_range(fd, offset, length, flags), Err(EINVAL));
        }
        let permissions = std::fs::metadata(&moved).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&moved, readonly).unwrap();
        let readonly_result = sync_file_range(fd, 0, 0, 7);
        std::fs::set_permissions(&moved, permissions).unwrap();
        assert_eq!(readonly_result, Ok(()));
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        crate::close(fd).unwrap();
        assert_eq!(sync_file_range(fd, 0, 0, 2), Err(EBADF));
        assert_eq!(sync_descriptor(fd), Err(EBADF));
        drop(file);
        std::fs::remove_file(moved).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn writeback_rejects_invalid_and_nonfile_descriptors_even_for_noop() {
        assert_eq!(sync_file_range(-1, 0, 0, 0), Err(EBADF));
        let (reader, writer) = crate::create_pipe(FdFlags::NONE, 4096).unwrap();
        for flags in [0, 2, 7] {
            assert_eq!(sync_file_range(reader, 0, 0, flags), Err(ESPIPE));
        }
        assert_eq!(sync_descriptor(reader), Err(EINVAL));
        crate::close(reader).unwrap();
        crate::close(writer).unwrap();
    }
}
