//! Set the shared inode format before publishing a new root. No VFS DLL is
//! needed to install the files which will supply those DLLs on first startup.
use std::{io, path::Path};

#[cfg(windows)]
pub(super) fn case_sensitive(path: &Path) -> io::Result<()> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES, FileCaseSensitiveInfo,
        SetFileInformationByHandle,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?;
    let flags: u32 = 1;
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileCaseSensitiveInfo,
            (&flags as *const u32).cast(),
            4,
        )
    } == 0
    {
        return Err(io::Error::other(format!(
            "rootfs requires filesystem support for case-sensitive directories: {}: {}",
            path.display(),
            io::Error::last_os_error()
        )));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn case_sensitive(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
pub(super) fn initialize(path: &Path, mode: u32) -> io::Result<()> {
    use kinakaze_v2_abi::inode::Record;
    let kind = if path.is_dir() { 0o040000 } else { 0o100000 };
    write_record(
        path,
        Record {
            mode: Some(kind | mode),
            uid: Some(0),
            gid: Some(0),
            ..Record::default()
        },
    )
}

#[cfg(windows)]
pub(super) fn symlink(path: &Path, target: &str) -> io::Result<()> {
    use kinakaze_v2_abi::inode::Record;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    write_record(
        path,
        Record {
            mode: Some(0o120777),
            uid: Some(0),
            gid: Some(0),
            symlink: Some(target.to_owned()),
            ..Record::default()
        },
    )
}

#[cfg(windows)]
fn write_record(path: &Path, record: kinakaze_v2_abi::inode::Record) -> io::Result<()> {
    use kinakaze_v2_abi::inode::EA_NAME;
    use std::{
        ffi::c_void,
        fs::OpenOptions,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    };
    use windows_sys::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_WRITE_EA,
        },
    };
    #[repr(C)]
    #[derive(Default)]
    struct IoStatus {
        status: usize,
        information: usize,
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetEaFile(file: HANDLE, io: *mut IoStatus, buffer: *const c_void, length: u32) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }
    let value = record
        .encode()
        .map_err(|_| io::Error::other("invalid rootfs inode metadata"))?;
    let length = 9 + EA_NAME.len() + value.len();
    // A synchronous file handle guarantees NtSetEaFile has completed before
    // these aligned request buffers are dropped.
    let mut storage = vec![0u32; length.div_ceil(4)];
    let bytes =
        unsafe { std::slice::from_raw_parts_mut(storage.as_mut_ptr().cast::<u8>(), length) };
    bytes[5] = EA_NAME.len() as u8;
    bytes[6..8].copy_from_slice(&(value.len() as u16).to_le_bytes());
    bytes[8..8 + EA_NAME.len()].copy_from_slice(EA_NAME);
    bytes[9 + EA_NAME.len()..].copy_from_slice(&value);
    let file = OpenOptions::new()
        .access_mode(FILE_WRITE_EA)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut io_status = IoStatus::default();
    let status = unsafe {
        NtSetEaFile(
            file.as_raw_handle(),
            &mut io_status,
            bytes.as_ptr().cast(),
            length as u32,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(status) } as i32,
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn initialize(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(unix)]
pub(super) fn symlink(path: &Path, target: &str) -> io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}
