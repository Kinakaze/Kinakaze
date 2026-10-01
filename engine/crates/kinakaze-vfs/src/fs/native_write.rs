//! Existing regular-file write opens retain the selected inode and mount writer.
//! Creation, truncation, special inodes and complex names keep the full resolver.
use super::{O_ACCMODE, O_CLOEXEC, O_NOFOLLOW, O_NONBLOCK, O_RDWR, O_WRONLY};
use crate::{FdFlags, FdKind};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_EA};

fn eligible_flags(flags: i32) -> bool {
    matches!(flags & O_ACCMODE, O_WRONLY | O_RDWR)
        && flags & !(O_ACCMODE | O_CLOEXEC | O_NONBLOCK | O_NOFOLLOW | 0o100000) == 0
}

pub(super) fn try_open_at(
    dirfd: i32,
    original: &str,
    absolute: &str,
    flags: i32,
) -> Result<Option<i32>, i32> {
    if !eligible_flags(flags) {
        return Ok(None);
    }
    let Some(path) = super::native_read::candidate_path(dirfd, original, absolute, 0) else {
        return Ok(None);
    };
    if !crate::user_namespace::capable(1, 1)
        || crate::path::overlay_root().is_some()
        || crate::mount::api::tree_reference(path).is_some()
        || crate::tmpfs::owns(path)
        || crate::procfs::owns(path)
    {
        return Ok(None);
    }
    let Some(namespace) = crate::path::namespace_root_path()? else {
        return Ok(None);
    };
    let access = GENERIC_WRITE
        | FILE_READ_ATTRIBUTES
        | FILE_READ_EA
        | if flags & O_ACCMODE == O_RDWR {
            GENERIC_READ
        } else {
            0
        };
    let Some(object) = super::native_read::open_regular(
        &crate::path::default_system_root(),
        &namespace,
        path,
        access,
    )?
    else {
        return Ok(None);
    };
    let canonical = format!(
        "/{}",
        path.split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    );
    // Acquire the ordinary mount writer before publishing a writable handle.
    // This also preserves read-only mount and concurrent remount exclusion.
    let Some(description) = crate::mount::native::prepare_open_canonical(path, flags, &canonical)?
    else {
        return Ok(None);
    };
    super::permissions::check(object.raw(), flags)?;
    super::verity::ensure_writable(object.raw())?;
    let mut fd_flags = FdFlags::WRITE_ACCESS
        .union(FdFlags::VERITY_WRITABLE)
        .union(FdFlags::OVERLAPPED)
        .union(FdFlags::SEEKABLE);
    if flags & O_ACCMODE == O_RDWR {
        fd_flags = fd_flags.union(FdFlags::READ_ACCESS);
    }
    if flags & O_CLOEXEC != 0 {
        fd_flags = fd_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & O_NONBLOCK != 0 {
        fd_flags = fd_flags.union(FdFlags::NONBLOCK);
    }
    let fd = crate::install_with(object.raw() as usize, FdKind::File, fd_flags, |_, entry| {
        crate::mount::native::register(entry, description)
    })?;
    object.into_raw();
    Ok(Some(fd))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
fn try_open(path: &str, flags: i32) -> Result<Option<i32>, i32> {
    try_open_at(super::AT_FDCWD, path, path, flags)
}
