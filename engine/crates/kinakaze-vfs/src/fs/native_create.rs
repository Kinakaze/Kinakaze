//! Exclusive creation relative to one freshly pinned native parent directory.
//! Only ordinary paths with DAC override bypass the component walker. The
//! parent and its live setgid record stay local to this single operation.
use super::{
    O_ACCMODE, O_APPEND, O_CLOEXEC, O_CREAT, O_EXCL, O_NOFOLLOW, O_NONBLOCK, O_RDWR, O_TRUNC,
    O_WRONLY, S_IFDIR, S_IFMT, S_IFREG, Stat, inode, object::Object,
};
use crate::{FdFlags, FdKind};
use std::{ffi::OsStr, path::Path, sync::OnceLock};
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};

fn eligible_flags(flags: i32) -> bool {
    flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL)
        && matches!(flags & O_ACCMODE, O_WRONLY | O_RDWR)
        && flags
            & !(O_ACCMODE
                | O_CREAT
                | O_EXCL
                | O_NOFOLLOW
                | O_TRUNC
                | O_CLOEXEC
                | O_NONBLOCK
                | O_APPEND
                | 0o100000)
            == 0
}

pub(super) fn try_open_at(
    dirfd: i32,
    original: &str,
    absolute: &str,
    flags: i32,
    mode: u32,
) -> Result<Option<i32>, i32> {
    if !eligible_flags(flags) {
        return Ok(None);
    }
    let Some(path) = super::native_read::candidate_path(dirfd, original, absolute, 0) else {
        return Ok(None);
    };
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !*ENABLED.get_or_init(|| {
        std::env::var_os("KINAKAZE_NATIVE_EXCLUSIVE_CREATE").as_deref()
            != Some(std::ffi::OsStr::new("0"))
    }) || !crate::user_namespace::capable(1, 1)
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
    let mut guest = namespace.trim_end_matches('/').to_owned();
    let mut native = crate::path::default_system_root().to_path_buf();
    for part in namespace.split('/').filter(|part| !part.is_empty()) {
        native.push(crate::path::escape_component(part).as_ref());
    }
    for part in path.split('/').filter(|part| !part.is_empty()) {
        guest.push('/');
        guest.push_str(part);
        native.push(crate::path::escape_component(part).as_ref());
    }
    if crate::mount::has_attachment(&guest)? {
        return Ok(None);
    }
    let canonical = format!(
        "/{}",
        path.split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    );
    let description = crate::mount::native::prepare_open_canonical(path, flags, &canonical)?;
    if description
        .as_ref()
        .is_some_and(|d| d.policy.id != crate::mount::ROOT_MOUNT_ID)
    {
        return Ok(None);
    }
    let Some(parent) = PreparedParent::open(native.parent().ok_or(crate::EINVAL)?)? else {
        return Ok(None);
    };
    let access = GENERIC_WRITE
        | if flags & O_ACCMODE == O_RDWR {
            GENERIC_READ
        } else {
            0
        };
    let object = parent.create(native.file_name().ok_or(crate::EINVAL)?, access, mode)?;
    let mut fd_flags = FdFlags::WRITE_ACCESS
        .union(FdFlags::OVERLAPPED)
        .union(FdFlags::SEEKABLE)
        .union(FdFlags::VERITY_WRITABLE);
    if flags & O_ACCMODE == O_RDWR {
        fd_flags = fd_flags.union(FdFlags::READ_ACCESS);
    }
    if flags & O_CLOEXEC != 0 {
        fd_flags = fd_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & O_NONBLOCK != 0 {
        fd_flags = fd_flags.union(FdFlags::NONBLOCK);
    }
    if flags & O_APPEND != 0 {
        fd_flags = fd_flags.union(FdFlags::APPEND);
    }
    let fd = crate::install_with(object.raw() as usize, FdKind::File, fd_flags, |_, entry| {
        if let Some(description) = description {
            crate::mount::native::register(entry, description)?;
        }
        Ok(())
    })?;
    object.into_raw();
    Ok(Some(fd))
}

struct PreparedParent {
    object: Object,
    mode: u32,
    gid: u32,
}
impl PreparedParent {
    fn open(path: &Path) -> Result<Option<Self>, i32> {
        let object = match crate::mount::overlay::native_open::directory(path) {
            Ok(object) => object,
            Err(_) => return Ok(None),
        };
        let record = inode::read_object(&object)?;
        if record.symlink.is_some()
            || record
                .mode
                .is_some_and(|mode| !matches!(mode & S_IFMT, 0 | S_IFDIR))
        {
            return Ok(None);
        }
        // Host ACL projection never adds setgid. An absent Linux record needs
        // no ACL, timestamps, size, volume id or fs-verity stat transaction.
        Ok(Some(Self {
            object,
            mode: record.mode.unwrap_or(0),
            gid: record.gid.unwrap_or(0),
        }))
    }
    fn create(&self, stored: &OsStr, access: u32, mode: u32) -> Result<Object, i32> {
        let parent = Stat {
            st_mode: self.mode,
            st_gid: self.gid,
            ..Stat::default()
        };
        let record = super::created_inode_record(&parent, S_IFREG | (mode & 0o7777));
        self.object
            .create_regular_child_with_inode(stored, access, &record)
    }
}

#[cfg(test)]
mod tests;
