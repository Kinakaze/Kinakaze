//! openat2's component walker. Every ordinary child is opened without following
//! links; retained ancestors implement `..`, including across mount points.
use super::*;
use std::collections::VecDeque;

struct Descriptor(i32);
impl Drop for Descriptor {
    fn drop(&mut self) {
        let _ = crate::close(self.0);
    }
}
impl Descriptor {
    fn take(self) -> i32 {
        let fd = self.0;
        std::mem::forget(self);
        fd
    }
}

fn path(fd: i32) -> Result<String, i32> {
    match get(fd)?.kind {
        FdKind::TmpfsFile | FdKind::TmpfsDirectory => crate::tmpfs::descriptor_path(fd),
        FdKind::SyntheticDirectory => crate::synthetic_directory_path(fd),
        FdKind::MountTree => crate::mount::api::tree_path(fd),
        _ => {
            if let Some(path) = crate::mount::native::descriptor_path(fd)? {
                return Ok(path);
            }
            Ok(crate::to_guest_path(&object::Object::from_fd(fd)?.path()?))
        }
    }
}

/// Metadata-only leaf lookup relative to the retained native directory. The
/// caller has already dispatched virtual paths. Mount crossings still use the
/// full resolver, and no pathname or metadata is cached across operations.
pub(super) fn open_path_child(
    parent: i32,
    original: Option<crate::FdEntry>,
    name: &str,
    guest: &str,
    flags: i32,
) -> Result<Option<i32>, i32> {
    if parent == AT_FDCWD
        || flags & (O_PATH | O_NOFOLLOW) != O_PATH | O_NOFOLLOW
        || name.is_empty()
        || name.contains('/')
        || matches!(name, "." | "..")
    {
        return Ok(None);
    }
    let Some(entry) = original else {
        return Ok(None);
    };
    if get(parent)?.generation != entry.generation {
        return Err(EBADF);
    }
    if entry.kind != FdKind::Directory || crate::mount::overlay::reference(entry)?.is_some() {
        return Ok(None);
    }
    // A native attachment provides namespace coordinates for `guest`. Other
    // descriptor backends retain their own resolver and policy checks.
    let Some(description) = crate::mount::native::reference(entry)? else {
        return Ok(None);
    };
    if description.policy.namespace != crate::mount::namespace_id()? {
        return Ok(None);
    }
    let base = guest
        .rsplit_once('/')
        .map_or("/", |(base, _)| if base.is_empty() { "/" } else { base });
    if crate::mount::query::id_for_path(base)? != crate::mount::query::id_for_path(guest)? {
        return Ok(None);
    }
    let (object, current) = object::Object::from_fd_with_entry(parent)?;
    if current.generation != entry.generation {
        return Err(EBADF);
    }
    install_native_child(
        object,
        entry,
        name,
        guest,
        flags & (O_PATH | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC),
    )
    .map(|descriptor| Some(descriptor.take()))
}

fn install_native_child(
    parent: object::Object,
    entry: crate::FdEntry,
    name: &str,
    guest: &str,
    flags: i32,
) -> Result<Descriptor, i32> {
    let stored = crate::path::escape_component(name);
    let object = parent.child(
        OsStr::new(stored.as_ref()),
        FILE_READ_ATTRIBUTES | FILE_READ_EA,
    )?;
    let native = crate::mount::native::reference(entry)?
        .map(|mut d| {
            d.path = crate::mount::namespace_path(guest)?;
            d.writer = None;
            Ok::<_, i32>(d)
        })
        .transpose()?;
    install_native_object(object, flags, native).map(Descriptor)
}

/// The full resolver has already followed guest links and selected the mount.
/// Pin that object once, without repeating pathname metadata/ACL/verity reads
/// needed only for a data open. Its metadata is still read afresh by fstat.
pub(super) fn open_native_path(
    path: &Path,
    flags: i32,
    native: Option<crate::mount::native::Description>,
) -> Result<i32, i32> {
    let access = FILE_READ_ATTRIBUTES | FILE_READ_EA;
    let object = if flags & O_NOFOLLOW != 0 {
        object::Object::open(path, access)?
    } else {
        object::Object::open_follow(path, access)?
    };
    install_native_object(object, flags, native)
}

fn install_native_object(
    object: object::Object,
    flags: i32,
    native: Option<crate::mount::native::Description>,
) -> Result<i32, i32> {
    // Installation only needs the directory/file distinction. Linux link and
    // device markers are native files; querying their ownership, ACL and verity
    // size here duplicates the caller's later fstat without affecting O_PATH.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(object.raw(), &mut info) } == 0 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
        && (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
            || native_symlink_target_handle(object.raw())?.is_none());
    if flags & O_DIRECTORY != 0 && !directory {
        return Err(ENOTDIR);
    }
    let fd = crate::install_with(
        object.raw() as usize,
        if directory {
            FdKind::Directory
        } else {
            FdKind::File
        },
        special_fd_flags(flags)
            .union(FdFlags::PATH_ONLY)
            .union(FdFlags::OVERLAPPED),
        |_, entry| {
            if let Some(d) = native {
                crate::mount::native::register(entry, d)?;
            }
            Ok(())
        },
    )?;
    object.into_raw();
    Ok(fd)
}

fn child(parent: i32, name: &str, no_xdev: bool) -> Result<Descriptor, i32> {
    if let Some(fd) = crate::mount::overlay::confined_child(parent, name, no_xdev)? {
        return Ok(Descriptor(fd));
    }
    let entry = get(parent)?;
    let base = path(parent)?;
    let guest = format!("{}/{}", base.trim_end_matches('/'), name);
    // Descriptor paths are relative to the caller's root. Mount-table keys
    // retain namespace coordinates across chroot/pivot_root; compare through
    // the path API so a container's /sys still resolves its actual mount.
    let crossing =
        crate::mount::query::id_for_path(&base)? != crate::mount::query::id_for_path(&guest)?;
    if crossing && no_xdev {
        return Err(crate::EXDEV);
    }
    let flags = O_PATH | O_NOFOLLOW | O_CLOEXEC;
    if crossing {
        let namespace = crate::mount::namespace_path(&guest)?;
        if let Some(backing) = crate::mount::native_translation(&namespace)? {
            // The parent descriptor already proves this component's ancestry.
            // Rewalking the absolute path would encounter the virtual parent
            // again and recursively reenter this same mount crossing.
            let native = crate::path::resolve_unmounted(
                &crate::path::default_system_root(),
                &backing,
                false,
            )
            .map_err(path_errno)?;
            let description = crate::mount::native::prepare_open_canonical(&guest, flags, &guest)?;
            return open_native_path(&native, flags, description).map(Descriptor);
        }
    }
    if !crossing && entry.kind == FdKind::Directory && !crate::procfs::owns(&guest) {
        let (object, current) = object::Object::from_fd_with_entry(parent)?;
        if current.generation != entry.generation {
            return Err(EBADF);
        }
        return install_native_child(object, entry, name, &guest, flags);
    }
    let fd = Descriptor(openat(parent, name, flags, 0)?);
    if no_xdev && fstat(parent)?.st_dev != fstat(fd.0)?.st_dev {
        return Err(crate::EXDEV);
    }
    Ok(fd)
}

fn create(parent: i32, name: &str, flags: i32, mode: u32) -> Result<i32, i32> {
    if flags & O_DIRECTORY != 0 {
        return Err(EINVAL);
    }
    if let Some(fd) = crate::mount::overlay::confined_create(parent, Some(name), mode)? {
        let target = Descriptor(fd);
        return reopen_local_fd(target.0, flags & !(O_CREAT | O_EXCL | O_NOFOLLOW));
    }
    let entry = get(parent)?;
    if entry.kind == FdKind::TmpfsDirectory {
        return crate::tmpfs::openat(parent, name, flags | O_NOFOLLOW | O_EXCL, mode);
    }
    if entry.kind != FdKind::Directory {
        return Err(crate::EROFS);
    }
    let description = crate::mount::native::reference(entry)?
        .map(|mut d| {
            d.writer = Some(d.policy.writer()?);
            d.path = format!("{}/{}", d.path.trim_end_matches('/'), name);
            Ok::<_, i32>(d)
        })
        .transpose()?;
    let parent = object::Object::from_fd(parent)?;
    let stored = crate::path::escape_component(name);
    let object = parent.create_regular_child(
        OsStr::new(stored.as_ref()),
        GENERIC_READ | GENERIC_WRITE | DELETE | FILE_READ_ATTRIBUTES | FILE_READ_EA,
    )?;
    if let Err(e) =
        initialize_created_handle(object.raw(), &object.path()?, S_IFREG | mode & 0o7777)
    {
        let _ = unlink_inode(object.raw());
        return Err(e);
    }
    let fd = crate::install_with(
        object.raw() as usize,
        FdKind::File,
        FdFlags::PATH_ONLY
            .union(FdFlags::OVERLAPPED)
            .union(FdFlags::CLOSE_ON_EXEC),
        |_, entry| {
            if let Some(d) = description {
                crate::mount::native::register(entry, d)?;
            }
            Ok(())
        },
    )?;
    object.into_raw();
    let target = Descriptor(fd);
    reopen_local_fd(target.0, flags & !(O_CREAT | O_EXCL | O_NOFOLLOW))
}

pub(super) fn stat(path: &str, follow: bool) -> Result<Stat, i32> {
    let flags = O_PATH | O_CLOEXEC | if follow { 0 } else { O_NOFOLLOW };
    let descriptor = Descriptor(open(AT_FDCWD, path, flags, 0, 0)?);
    fstat(descriptor.0)
}

pub(super) fn canonical_guest(name: &str, follow: bool) -> Result<String, i32> {
    let flags = O_PATH | O_CLOEXEC | if follow { 0 } else { O_NOFOLLOW };
    let descriptor = Descriptor(if crate::procfs::owns(name) {
        open_procfs(name, flags, 0)?
    } else {
        open(AT_FDCWD, name, flags, 0, 0)?
    });
    path(descriptor.0)
}

pub(super) fn open_below_proc(prefix: &str, tail: &str, flags: i32, mode: u32) -> Result<i32, i32> {
    let parent = Descriptor(open_procfs(prefix, O_PATH | O_DIRECTORY | O_CLOEXEC, 0)?);
    open(parent.0, tail, flags, mode, 0)
}

pub(super) fn stat_below_proc(prefix: &str, tail: &str, follow: bool) -> Result<Stat, i32> {
    let flags = O_PATH | O_CLOEXEC | if follow { 0 } else { O_NOFOLLOW };
    let descriptor = Descriptor(open_below_proc(prefix, tail, flags, 0)?);
    fstat(descriptor.0)
}

pub(super) fn mkdir_proc(pathname: &str, mode: u32) -> Result<(), i32> {
    let (parent, name) = pathname
        .trim_end_matches('/')
        .rsplit_once('/')
        .ok_or(crate::EEXIST)?;
    let parent = Descriptor(super::open(
        if parent.is_empty() { "/" } else { parent },
        O_PATH | O_DIRECTORY | O_CLOEXEC,
        0,
    )?);
    if matches!(name, "" | "." | "..") {
        return Err(crate::EEXIST);
    }
    if get(parent.0)?.kind == FdKind::TmpfsDirectory {
        return crate::tmpfs::mkdirat(parent.0, name, mode);
    }
    let parent = path(parent.0)?;
    let target = format!("{}/{name}", parent.trim_end_matches('/'));
    if target == pathname.trim_end_matches('/') {
        return Err(if super::lstat(pathname).is_ok() {
            crate::EEXIST
        } else {
            crate::EROFS
        });
    }
    super::mkdir(&target, mode)
}

pub(super) fn mutation_path(pathname: &str) -> Result<String, i32> {
    let (parent, name) = pathname.rsplit_once('/').ok_or(EINVAL)?;
    let parent = canonical_guest(if parent.is_empty() { "/" } else { parent }, true)?;
    Ok(format!("{}/{name}", parent.trim_end_matches('/')))
}

pub(super) fn open(
    dirfd: i32,
    path: &str,
    flags: i32,
    mode: u32,
    resolve: u64,
) -> Result<i32, i32> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    if path.contains('\0') {
        return Err(EINVAL);
    }
    let beneath = resolve & 8 != 0;
    let in_root = resolve & 16 != 0;
    let no_xdev = resolve & 1 != 0;
    // Unconfined host sessions expose drive roots as /c, /d, ... . They are
    // namespace roots, not physical children of the configured guest root.
    // RESOLVE_IN_ROOT must never activate that host-drive spelling.
    let drive = if path.starts_with('/') && !in_root && !crate::fs_context::read(|s| s.confined) {
        path.split('/')
            .find(|p| !p.is_empty())
            .filter(|p| p.len() == 1 && p.as_bytes()[0].is_ascii_alphabetic())
    } else {
        None
    };
    let root = if path.starts_with('/') && !in_root {
        Descriptor(super::open(
            &drive.map_or_else(|| "/".into(), |d| format!("/{d}/")),
            O_PATH | O_DIRECTORY | O_CLOEXEC,
            0,
        )?)
    } else if dirfd == AT_FDCWD || get(dirfd)?.kind == FdKind::MountTree {
        Descriptor(openat(dirfd, ".", O_PATH | O_DIRECTORY | O_CLOEXEC, 0)?)
    } else {
        Descriptor(reopen_local_fd(dirfd, O_PATH | O_DIRECTORY | O_CLOEXEC)?)
    };
    let mut ancestors = vec![root];
    let mut pending: VecDeque<String> = path
        .split('/')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    if drive.is_some() {
        pending.pop_front();
    }
    // A trailing slash requires following the final link and a directory.
    if path.ends_with('/') {
        pending.push_back(".".into());
    }
    let mut links = 0;
    while let Some(name) = pending.pop_front() {
        let parent = ancestors.last().ok_or(EIO)?.0;
        if fstat(parent)?.st_mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
        if name == "." {
            continue;
        }
        if name == ".." {
            if ancestors.len() > 1 {
                ancestors.pop();
            } else if beneath {
                return Err(crate::EXDEV);
            } else if !in_root {
                let next = Descriptor(openat(parent, "..", O_PATH | O_DIRECTORY | O_CLOEXEC, 0)?);
                if no_xdev && fstat(next.0)?.st_dev != fstat(parent)?.st_dev {
                    return Err(crate::EXDEV);
                }
                ancestors[0] = next;
            }
            continue;
        }
        let final_part = pending.is_empty();
        let next = match child(parent, &name, no_xdev) {
            Err(ENOENT) if final_part && flags & O_CREAT != 0 && flags & O_PATH == 0 => {
                match create(parent, &name, flags, mode) {
                    Err(crate::EEXIST) if flags & O_EXCL == 0 => {
                        // A creator won the race. Re-run the component's link
                        // checks; do not follow the winner through an open call.
                        return Err(crate::EAGAIN);
                    }
                    result => return result,
                }
            }
            result => result?,
        };
        let stat = fstat(next.0)?;
        if final_part && flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL) {
            return Err(crate::EEXIST);
        }
        if stat.st_mode & S_IFMT == S_IFLNK {
            if final_part && flags & O_NOFOLLOW != 0 {
                if flags & O_PATH == 0 {
                    return Err(ELOOP);
                }
                ancestors.push(next);
                break;
            }
            let entry = get(next.0)?;
            let magic = entry.flags.contains(FdFlags::PROC_SYMLINK)
                && crate::procfs::descriptor_path(next.0).is_ok_and(|p| {
                    crate::procfs::pinned(|| {
                        crate::procfs::fd_magic_link(&p).is_some()
                            || crate::procfs::directory_magic_link(&p)
                            || crate::procfs::namespace_target_inode(&p).is_some()
                    })
                });
            if resolve & 4 != 0
                || magic && resolve & (2 | 8 | 16) != 0
                || crate::mount::overlay::reference(entry)?
                    .is_some_and(|d| d.mount_flags() & 256 != 0)
                || crate::mount::native::reference(entry)?
                    .is_some_and(|d| d.policy.flags() & 256 != 0)
            {
                return Err(ELOOP);
            }
            links += 1;
            if links > 40 {
                return Err(ELOOP);
            }
            if magic {
                let pinned = crate::procfs::descriptor_path(next.0)?;
                let open_flags = if final_part {
                    flags & !O_NOFOLLOW
                } else {
                    O_PATH | O_DIRECTORY | O_CLOEXEC
                };
                // Proc fd links select an open object, not their diagnostic
                // readlink text (e.g. pipe:[123]). Preserve the pinned PID and
                // proc view, and let its backend enforce access/open semantics.
                let fd = crate::procfs::pinned(|| open_procfs(&pinned, open_flags, mode))?;
                if final_part {
                    return Ok(fd);
                }
                ancestors.push(Descriptor(fd));
                continue;
            }
            let target = read_link_fd(next.0)?;
            if target.is_empty() {
                return Err(ENOENT);
            }
            let target_drive = if target.starts_with('/')
                && !in_root
                && !crate::fs_context::read(|s| s.confined)
            {
                target
                    .split('/')
                    .find(|part| !part.is_empty())
                    .filter(|part| part.len() == 1 && part.as_bytes()[0].is_ascii_alphabetic())
            } else {
                None
            };
            if target.starts_with('/') {
                if beneath {
                    return Err(crate::EXDEV);
                }
                if in_root {
                    ancestors.truncate(1);
                } else {
                    if no_xdev {
                        return Err(crate::EXDEV);
                    }
                    ancestors = vec![Descriptor(super::open(
                        &target_drive.map_or_else(|| "/".into(), |drive| format!("/{drive}/")),
                        O_PATH | O_DIRECTORY | O_CLOEXEC,
                        0,
                    )?)];
                }
            }
            if target.ends_with('/') && pending.is_empty() {
                pending.push_front(".".into());
            }
            for part in target.split('/').filter(|p| !p.is_empty()).rev() {
                pending.push_front(part.into());
            }
            if target_drive.is_some() {
                pending.pop_front();
            }
        } else {
            ancestors.push(next);
        }
    }
    let target = ancestors.pop().ok_or(EIO)?;
    if flags & O_DIRECTORY != 0 && fstat(target.0)?.st_mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    if flags & O_PATH != 0 {
        if flags & O_CLOEXEC == 0 {
            crate::set_close_on_exec(target.0, false)?;
        }
        return Ok(target.take());
    }
    if flags & 0o20000000 != 0 {
        if flags & O_DIRECTORY == 0 || flags & O_ACCMODE == O_RDONLY {
            return Err(EINVAL);
        }
        if let Some(fd) = crate::mount::overlay::confined_create(target.0, None, mode)? {
            let file = Descriptor(fd);
            return reopen_local_fd(
                file.0,
                flags & !(0o20000000 | O_DIRECTORY | O_EXCL | O_NOFOLLOW),
            );
        }
        return openat(target.0, ".", flags, mode);
    }
    // openat ignores mode when it does not create an inode. openat2 validates
    // its stricter mode contract before entering this shared walker.
    reopen_local_fd(target.0, flags & !O_NOFOLLOW)
}
