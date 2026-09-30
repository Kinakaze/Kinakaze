//! Open ordinary native files once, then classify and retain that same inode.
//! Complex names and special objects continue through the component walker.
use super::{O_CLOEXEC, O_NOFOLLOW, O_NONBLOCK, S_IFMT, S_IFREG, inode, object::Object};
use crate::{FdFlags, FdKind};
use std::path::Path;
use std::sync::OnceLock;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FileAttributeTagInfo,
    GetFileInformationByHandleEx,
};

fn eligible(path: &str, flags: i32) -> bool {
    // O_LARGEFILE is accepted by openat but needs no descriptor flag on x86_64.
    flags & !(O_CLOEXEC | O_NONBLOCK | O_NOFOLLOW | 0o100000) == 0
        && path.starts_with('/')
        && !path.ends_with('/')
        && !path.contains('\0')
        && !path.split('/').any(|part| part == "." || part == "..")
}

pub(super) fn candidate_path<'a>(
    dirfd: i32,
    path: &'a str,
    absolute: &'a str,
    flags: i32,
) -> Option<&'a str> {
    if path.starts_with('/') {
        return eligible(path, flags).then_some(path);
    }
    // Preserve retained-dirfd, dot-component and trailing-slash semantics.
    // Following a symlink before '..' requires the component walker.
    if dirfd != super::AT_FDCWD
        || path.is_empty()
        || path.ends_with('/')
        || path.split('/').any(|part| part == "." || part == "..")
    {
        return None;
    }
    eligible(absolute, flags).then_some(absolute)
}

pub(super) fn try_open_at(
    dirfd: i32,
    original: &str,
    absolute: &str,
    flags: i32,
) -> Result<Option<i32>, i32> {
    let Some(path) = candidate_path(dirfd, original, absolute, flags) else {
        return Ok(None);
    };
    static RELATIVE_ENABLED: OnceLock<bool> = OnceLock::new();
    if !original.starts_with('/')
        && !*RELATIVE_ENABLED.get_or_init(|| {
            std::env::var_os("KINAKAZE_NATIVE_READ_RELATIVE").as_deref()
                != Some(std::ffi::OsStr::new("0"))
        })
    {
        return Ok(None);
    }
    static ENABLED: OnceLock<bool> = OnceLock::new();
    if !eligible(path, flags)
        || !*ENABLED.get_or_init(|| {
            std::env::var_os("KINAKAZE_NATIVE_READ_OPEN").as_deref()
                != Some(std::ffi::OsStr::new("0"))
        })
        || !crate::user_namespace::capable(1, 1)
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
    let Some(object) = open(&crate::path::default_system_root(), &namespace, path)? else {
        return Ok(None);
    };
    // Check the live credentials on the retained inode before publishing it.
    super::permissions::check(object.raw(), flags)?;
    let canonical = format!(
        "/{}",
        path.split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    );
    let description = crate::mount::native::prepare_open_canonical(path, flags, &canonical)?;
    let mut fd_flags = FdFlags::READ_ACCESS
        .union(FdFlags::OVERLAPPED)
        .union(FdFlags::SEEKABLE);
    if flags & O_CLOEXEC != 0 {
        fd_flags = fd_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & O_NONBLOCK != 0 {
        fd_flags = fd_flags.union(FdFlags::NONBLOCK);
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

fn open(root: &Path, namespace: &str, absolute: &str) -> Result<Option<Object>, i32> {
    open_regular(
        root,
        namespace,
        absolute,
        windows_sys::Win32::Foundation::GENERIC_READ,
    )
}

/// The returned private handle owns the inode classified by its live attributes
/// and guest metadata; no later path lookup selects the data object.
pub(super) fn open_regular(
    root: &Path,
    namespace: &str,
    absolute: &str,
    access: u32,
) -> Result<Option<Object>, i32> {
    if !eligible(absolute, 0) {
        return Ok(None);
    }
    let mut guest = namespace.trim_end_matches('/').to_owned();
    let mut native = root.to_path_buf();
    for part in namespace.split('/').filter(|part| !part.is_empty()) {
        native.push(crate::path::escape_component(part).as_ref());
    }
    for part in absolute.split('/').filter(|part| !part.is_empty()) {
        guest.push('/');
        guest.push_str(part);
        native.push(crate::path::escape_component(part).as_ref());
    }
    if crate::mount::has_attachment(&guest)? {
        return Ok(None);
    }
    // OBJ_DONT_REPARSE rejects junctions in all ancestors atomically. A hosted
    // link is a regular file, so it cannot be traversed as a native directory.
    let object = match crate::mount::overlay::native_open::open_file(&native, access) {
        Ok(object) => object,
        Err(_) => return Ok(None),
    };
    let mut attributes: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            object.raw(),
            FileAttributeTagInfo,
            (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of_val(&attributes) as u32,
        )
    } == 0
        || attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Ok(None);
    }
    let record = inode::read_object(&object)?;
    if record.symlink.is_some()
        || record
            .mode
            .is_some_and(|mode| !matches!(mode & S_IFMT, 0 | S_IFREG))
    {
        return Ok(None);
    }
    Ok(Some(object))
}

#[cfg(test)]
mod tests;
