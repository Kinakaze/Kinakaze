use super::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA, GetFileInformationByHandle, GetLastError,
    HANDLE, Ownership, READ_CONTROL, S_IFMT, S_IFREG, Stat, ea, errno_from_win32, inode,
    object::Object,
};

pub(super) fn unchanged_descriptor(
    fd: i32,
    allow_path: bool,
    owner: &Ownership,
) -> Result<bool, i32> {
    use std::os::windows::io::AsRawHandle;
    let mut native = None;
    let pinned = crate::native_pin::pin_native_fd(fd, |entry| {
        if !allow_path && entry.flags.contains(crate::FdFlags::PATH_ONLY) {
            return Err(crate::EBADF);
        }
        if crate::mount::overlay::reference(entry)?.is_some() {
            return Err(crate::EOPNOTSUPP);
        }
        native = crate::mount::native::reference(entry)?;
        Ok(())
    });
    let (_, pinned) = match pinned {
        Ok(value) => value,
        Err(crate::EOPNOTSUPP) => return Ok(false),
        Err(error) => return Err(error),
    };
    let _writer = native
        .as_ref()
        .map(|description| description.policy.writer())
        .transpose()?;
    unchanged(pinned.as_raw_handle(), owner)
}

pub(super) fn unchanged(handle: HANDLE, owner: &Ownership) -> Result<bool, i32> {
    let required = FILE_READ_ATTRIBUTES | FILE_READ_EA | FILE_WRITE_EA | READ_CONTROL;
    if Object::granted_access(handle)? & required != required {
        return Ok(false);
    }
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut information) } == 0 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    if information.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
    {
        return Ok(false);
    }
    let _lock = crate::xattr::InodeKey::from_information(&information).acquire()?;
    let Some(record) = ea::read_shared_decoded(handle, inode::EA_NAME, inode::Record::decode)?
    else {
        return Ok(false);
    };
    let (Some(mode), Some(uid), Some(gid)) = (record.mode, record.uid, record.gid) else {
        return Ok(false);
    };
    if mode & S_IFMT != S_IFREG || record.symlink.is_some() {
        return Ok(false);
    }
    owner.check(&Stat {
        st_mode: mode,
        st_uid: uid,
        st_gid: gid,
        ..Stat::default()
    })?;
    if owner.uid != u32::MAX && owner.uid != uid || owner.gid != u32::MAX && owner.gid != gid {
        return Ok(false);
    }
    if owner.uid != u32::MAX || owner.gid != u32::MAX {
        let clear = 0o4000 | if mode & 0o10 != 0 { 0o2000 } else { 0 };
        if mode & clear != 0
            || crate::xattr::Attributes::contains_shared_locked(handle, b"security.capability")?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
