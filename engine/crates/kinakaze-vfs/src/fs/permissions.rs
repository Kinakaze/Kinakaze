//! Linux access checks for native inodes carrying guest metadata.
use super::*;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx,
};

pub(super) fn check(handle: HANDLE, flags: i32) -> Result<(), i32> {
    if flags & O_PATH != 0 || crate::user_namespace::capable(1, 1) {
        return Ok(());
    }
    let record = inode::read(handle)?;
    let Some(mode) = record.mode else {
        // Ordinary host files retain the host ACL policy.
        return Ok(());
    };
    let caller = crate::credentials::filesystem();
    let available = if caller.uid == record.uid.unwrap_or(0) {
        mode >> 6
    } else if crate::credentials::group_member(record.gid.unwrap_or(0)) {
        mode >> 3
    } else {
        mode
    } & 7;
    let requested = match flags & O_ACCMODE {
        O_RDONLY => 4,
        O_WRONLY => 2,
        O_RDWR => 6,
        _ => return Err(EINVAL),
    } | if flags & O_TRUNC != 0 { 2 } else { 0 };
    if requested & !available == 0 || requested & 2 == 0 && crate::user_namespace::capable(1, 2) {
        Ok(())
    } else {
        Err(EACCES)
    }
}

/// Windows READONLY is a projection of Linux mode, not a second permission
/// check. Obtain the authorized open on the pinned inode, then restore its host
/// attribute before returning. An unrelated host readonly file is never changed.
pub(super) fn reopen_readonly(
    handle: HANDLE,
    access: u32,
    flags: i32,
) -> Result<object::Object, i32> {
    let object = object::Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)?;
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    let record = inode::read(object.raw())?;
    if record.mode.is_none() || flags & O_PATH != 0 || flags & O_ACCMODE == O_RDONLY {
        return Err(EACCES);
    }
    check(object.raw(), flags)?;
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            object.raw(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of_val(&basic) as u32,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    if basic.FileAttributes & FILE_ATTRIBUTE_READONLY == 0
        || basic.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
    {
        return Err(EACCES);
    }
    let set = |attributes| {
        let info = FILE_BASIC_INFO {
            FileAttributes: attributes,
            ..unsafe { std::mem::zeroed() }
        };
        if unsafe {
            SetFileInformationByHandle(
                object.raw(),
                FileBasicInfo,
                (&info as *const FILE_BASIC_INFO).cast(),
                size_of_val(&info) as u32,
            )
        } == 0
        {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(())
        }
    };
    let writable = basic.FileAttributes & !FILE_ATTRIBUTE_READONLY;
    set(if writable == 0 { 0x80 } else { writable })?;
    let result = object::Object::reopen(object.raw(), access);
    set(basic.FileAttributes)?;
    result
}

pub(super) fn reopen(handle: HANDLE, access: u32, flags: i32) -> Result<object::Object, i32> {
    check(handle, flags)?;
    match object::Object::reopen(handle, access) {
        Err(EACCES) => reopen_readonly(handle, access, flags),
        result => result,
    }
}
