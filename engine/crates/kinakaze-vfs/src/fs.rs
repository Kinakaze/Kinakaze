//! Linux file operations mapped onto Windows handles.
//!
//! Everything here funnels through [`crate::path`] for name translation, then
//! opens handles that are compatible with the interruptible overlapped I/O path
//! in [`crate::platform`]: `FILE_FLAG_OVERLAPPED` for streams and
//! `FILE_SHARE_DELETE` everywhere so POSIX unlink-while-open keeps working.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_ACCESS_DENIED, FILETIME,
    GENERIC_EXECUTE, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, EqualSid, GROUP_SECURITY_INFORMATION,
    IsWellKnownSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    WinAuthenticatedUserSid, WinBuiltinAdministratorsSid, WinBuiltinUsersSid, WinCreatorOwnerSid,
    WinLocalSystemSid, WinWorldSid,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE,
    FILE_APPEND_DATA, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    FILE_DISPOSITION_INFO_EX, FILE_EXECUTE, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
    FILE_READ_EA, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FileDispositionInfoEx, FileRenameInfoEx,
    GetFileInformationByHandle, GetFinalPathNameByHandleW, OPEN_ALWAYS, OPEN_EXISTING,
    READ_CONTROL, RemoveDirectoryW, SYNCHRONIZE, SetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::Win32::System::WindowsProgramming::{
    FILE_RENAME_FLAG_POSIX_SEMANTICS, FILE_RENAME_FLAG_REPLACE_IF_EXISTS,
};

use crate::path::{
    PathError, emulated_symlink_target, resolve_linux_path_from, resolve_linux_path_from_no_follow,
};
use crate::{
    EACCES, EBADF, EINVAL, EIO, EISDIR, ELOOP, ENOENT, ENOTDIR, ESPIPE, FdFlags, FdKind,
    errno_from_win32, get, install, table,
};

mod allocation;
mod confined;
pub use allocation::fallocate;
pub(crate) mod cwd;
pub use cwd::fchdir;
mod ea;
pub(crate) mod inode;
pub(crate) mod object;
#[cfg(test)]
mod proc_fd_tests;
mod readahead;
mod reparse;
pub use readahead::readahead;
pub mod verity;
pub use inode::migration::{Report as InodeMigrationReport, run as migrate_inode_metadata};

// Linux x86_64 open flags. The low two bits carry the access mode.
pub const O_RDONLY: i32 = 0o0;
pub const O_WRONLY: i32 = 0o1;
pub const O_RDWR: i32 = 0o2;
pub const O_ACCMODE: i32 = 0o3;
pub const O_CREAT: i32 = 0o100;
pub const O_EXCL: i32 = 0o200;
pub const O_NOCTTY: i32 = 0o400;
pub const O_TRUNC: i32 = 0o1000;
pub const O_APPEND: i32 = 0o2000;
pub const O_NOATIME: i32 = 0o1000000;
pub const O_NONBLOCK: i32 = 0o4000;
pub const O_DIRECTORY: i32 = 0o200000;
pub const O_NOFOLLOW: i32 = 0o400000;
pub const O_CLOEXEC: i32 = 0o2000000;
pub const O_PATH: i32 = 0o10000000;

pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFSOCK: u32 = 0o140000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFBLK: u32 = 0o060000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFIFO: u32 = 0o010000;

pub const F_OK: i32 = 0;
pub const X_OK: i32 = 1;
pub const W_OK: i32 = 2;
pub const R_OK: i32 = 4;

/// `AT_FDCWD` from the Linux `*at` family.
pub const AT_FDCWD: i32 = -100;

/// The Linux x86_64 `struct stat`, 144 bytes with three reserved words.
///
/// Guest code reads these fields at fixed offsets, so the layout is ABI and
/// must not be reordered.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    pub __unused: [i64; 3],
}

/// Serializes the guest-visible current directory for fork.
pub(crate) fn serialize_cwd() -> Result<Vec<u8>, i32> {
    Ok(cwd::serialize()?.unwrap_or_else(|| getcwd().into_bytes()))
}

/// Restores the guest-visible current directory in a fresh child.
pub(crate) fn restore_cwd(payload: &[u8]) -> bool {
    match cwd::restore(payload) {
        Ok(true) => return true,
        Err(_) => return false,
        Ok(false) => {}
    }
    let Ok(value) = std::str::from_utf8(payload) else {
        return false;
    };
    if !value.starts_with('/') {
        return false;
    }
    if crate::tmpfs::stat(value, true)
        .ok()
        .flatten()
        .is_some_and(|s| s.st_mode & S_IFMT == S_IFDIR)
    {
        crate::fs_context::update(|s| s.cwd = Some(value.to_owned()));
        return true;
    }
    let Ok(resolved) = resolve(value) else {
        return false;
    };
    if std::env::set_current_dir(&resolved).is_err() {
        return false;
    }
    crate::fs_context::update(|s| s.cwd = Some(value.to_owned()));
    true
}

/// Returns the guest's current working directory.
pub fn getcwd() -> String {
    if let Some(path) = cwd::display() {
        return path;
    }
    if let Some(path) = crate::fs_context::read(|s| s.cwd.clone()) {
        return path;
    }
    let initial = std::env::current_dir()
        .map(|dir| crate::to_guest_path(&dir))
        .unwrap_or_else(|_| "/".into());
    crate::fs_context::update(|s| s.cwd.get_or_insert(initial).clone())
}

/// Collapses `.`, `..` and empty components in an absolute Linux path.
fn normalize_linux(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/').filter(|part| !part.is_empty()) {
        match component {
            "." => {}
            ".." => {
                let at_drive_root = parts.len() == 1 && parts[0].len() == 1;
                if !at_drive_root {
                    parts.pop();
                }
            }
            component => parts.push(component),
        }
    }
    if parts.is_empty() {
        String::from("/")
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Joins a possibly relative guest path against the current directory.
pub fn absolute_linux(path: &str) -> String {
    if path.starts_with('/') {
        normalize_linux(path)
    } else {
        let cwd = getcwd();
        if cwd == "/" {
            normalize_linux(&format!("/{path}"))
        } else {
            normalize_linux(&format!("{cwd}/{path}"))
        }
    }
}

/// Translates a guest path into a Windows path.
pub(crate) fn resolve(path: &str) -> Result<std::path::PathBuf, i32> {
    let root = crate::path::system_root().map_err(|_| ENOENT)?;
    resolve_linux_path_from(&root, &absolute_linux(path)).map_err(path_errno)
}

/// Resolves intermediate symlinks but retains a link in the last component.
fn resolve_no_follow(path: &str) -> Result<std::path::PathBuf, i32> {
    let root = crate::path::system_root().map_err(|_| ENOENT)?;
    resolve_linux_path_from_no_follow(&root, &absolute_linux(path)).map_err(path_errno)
}

fn path_errno(error: PathError) -> i32 {
    match error {
        PathError::TooManySymlinks => ELOOP,
        PathError::Filesystem(error) => error,
        _ => EINVAL,
    }
}

/// Resolves a path for the `*at` family.
fn resolve_at(dirfd: i32, path: &str) -> Result<std::path::PathBuf, i32> {
    resolve_at_with(dirfd, path, true)
}

fn resolve_at_no_follow(dirfd: i32, path: &str) -> Result<std::path::PathBuf, i32> {
    resolve_at_with(dirfd, path, false)
}

fn resolve_at_with(dirfd: i32, path: &str, follow_final: bool) -> Result<std::path::PathBuf, i32> {
    if path.starts_with('/') || dirfd == AT_FDCWD {
        return if follow_final {
            resolve(path)
        } else {
            resolve_no_follow(path)
        };
    }
    let entry = crate::get(dirfd)?;
    let virtual_base = if entry.kind == FdKind::MountTree {
        Some(crate::mount::api::tree_path(dirfd)?)
    } else if let Some(path) = crate::mount::overlay::descriptor_path(dirfd)? {
        Some(path)
    } else {
        crate::mount::native::descriptor_path(dirfd)?
    };
    if let Some(base) = virtual_base {
        let joined = format!("{}/{}", base.trim_end_matches('/'), path);
        return if follow_final {
            resolve(&joined)
        } else {
            resolve_no_follow(&joined)
        };
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
        };
        if entry.raw != 0 && entry.raw != usize::MAX {
            let mut buf = [0u16; 1024];
            let len = unsafe {
                GetFinalPathNameByHandleW(
                    entry.raw as windows_sys::Win32::Foundation::HANDLE,
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
                )
            };
            if len > 0 && (len as usize) < buf.len() {
                let win_path = String::from_utf16_lossy(&buf[..len as usize]);
                let clean_path = win_path.strip_prefix(r"\\?\").unwrap_or(&win_path);
                let dir_buf = std::path::PathBuf::from(clean_path);
                if path.is_empty() || path == "." {
                    return Ok(dir_buf);
                }
                let base = crate::to_guest_path(&dir_buf);
                let joined = if base.ends_with('/') {
                    format!("{base}{path}")
                } else {
                    format!("{base}/{path}")
                };
                return if follow_final {
                    resolve(&joined)
                } else {
                    resolve_no_follow(&joined)
                };
            }
        }
    }
    Err(crate::ENOSYS)
}

fn wide(path: &Path) -> Result<Vec<u16>, i32> {
    crate::path::wide_path(path)
}

/// Opens a guest path, returning a Linux descriptor.
pub fn open(path: &str, flags: i32, mode: u32) -> Result<i32, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    openat(AT_FDCWD, path, flags, mode)
}

/// Serves a `/proc` path by materializing its text into a descriptor.
fn open_procfs(path: &str, flags: i32) -> Result<i32, i32> {
    // O_PATH resolves an inode without opening it for data access. Linux ignores
    // the remaining status/access flags, including O_TRUNC and O_ACCMODE.
    let flags = if flags & O_PATH != 0 {
        flags & (O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
    } else {
        flags
    };
    if flags & O_NOFOLLOW != 0 {
        if let Some((identity, target)) = crate::procfs::alias_link(path)? {
            if flags & O_PATH == 0 {
                return Err(ELOOP);
            }
            if flags & O_DIRECTORY != 0 {
                return Err(ENOTDIR);
            }
            return crate::install_procfs_file(
                &identity,
                target.into_bytes(),
                special_fd_flags(flags)
                    .union(FdFlags::PATH_ONLY)
                    .union(FdFlags::PROC_SYMLINK),
            );
        }
        let metadata = crate::procfs::metadata(path)?;
        if metadata.kind == crate::procfs::ProcKind::Symlink {
            if flags & O_PATH == 0 {
                return Err(ELOOP);
            }
            if flags & O_DIRECTORY != 0 {
                return Err(ENOTDIR);
            }
            let target = metadata.target.ok_or(EIO)?;
            return crate::install_procfs_file(
                &crate::procfs::canonical_path(path),
                target.into_bytes(),
                special_fd_flags(flags)
                    .union(FdFlags::PATH_ONLY)
                    .union(FdFlags::PROC_SYMLINK),
            );
        }
    }
    // Validate visibility before changing a user pathname into a pinned
    // registry-ID path. An unresolved caller PID must never become a root PID.
    crate::procfs::metadata(path)?;
    let canonical = crate::procfs::canonical_path(path);
    crate::procfs::pinned(|| open_procfs_pinned(&canonical, flags))
}
fn open_procfs_pinned(path: &str, flags: i32) -> Result<i32, i32> {
    if let Some((pid, fd)) = crate::procfs::fd_magic_link(path) {
        if pid != crate::job::process_id() {
            // Opening another process's fd needs a ptrace-style permission
            // check and cross-process handle duplication. The published link
            // text is not a substitute for its open file description.
            return Err(crate::EOPNOTSUPP);
        }
        return reopen_local_fd(fd, flags);
    }
    if flags & O_ACCMODE != O_RDONLY {
        crate::procfs::instance::check_write(path)?;
    }
    let metadata = crate::procfs::metadata(path)?;
    if metadata.kind == crate::procfs::ProcKind::Directory {
        // Linux permits a directory to be opened read-only without O_DIRECTORY;
        // that flag only rejects a non-directory target.  A write-capable open
        // of a directory still fails with EISDIR.
        if flags & O_ACCMODE != O_RDONLY {
            return Err(EISDIR);
        }
        return crate::install_procfs_directory(path.to_string(), special_fd_flags(flags));
    }
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    if crate::procfs::writable(path) {
        if !matches!(flags & O_ACCMODE, O_RDONLY | O_WRONLY | O_RDWR) {
            return Err(EINVAL);
        }
        return crate::install_proc_sysctl_file(path.to_string(), special_fd_flags(flags));
    }
    if let Some(pid) = crate::procfs::user_namespace_process(path) {
        if flags & O_NOFOLLOW != 0 {
            return Err(ELOOP);
        }
        if flags & O_ACCMODE != O_RDONLY {
            return Err(EACCES);
        }
        return crate::user_namespace::open_process(pid, special_fd_flags(flags));
    }
    if let Some((pid, kind)) = crate::procfs::other_namespace_process(path) {
        if flags & O_NOFOLLOW != 0 {
            return Err(ELOOP);
        }
        if flags & O_ACCMODE != O_RDONLY {
            return Err(EACCES);
        }
        return crate::namespaces::open_process(pid, kind, special_fd_flags(flags));
    }
    if let Some((pid, children)) = crate::procfs::time_namespace_process(path) {
        if flags & O_NOFOLLOW != 0 {
            return Err(ELOOP);
        }
        if flags & O_ACCMODE != O_RDONLY {
            return Err(EACCES);
        }
        return crate::time_namespace::open_process(pid, children, special_fd_flags(flags));
    }
    if let Some(pid) = crate::procfs::mount_namespace_process(path) {
        if flags & O_NOFOLLOW != 0 {
            return Err(ELOOP);
        }
        if flags & O_ACCMODE != O_RDONLY {
            return Err(EACCES);
        }
        return crate::mount::open_process_namespace(pid, special_fd_flags(flags));
    }
    // Procfs files without a write handler remain read-only; permissions must
    // not turn an unsupported write into a successful no-op.
    if flags & O_ACCMODE != O_RDONLY {
        return Err(EACCES);
    }
    let contents = crate::procfs::read_file(path)?;
    let identity = metadata.target.as_deref().unwrap_or(path);
    crate::install_procfs_file(identity, contents, special_fd_flags(flags))
}

/// Reopens the object behind a local procfs fd magic link.
fn reopen_local_fd(fd: i32, flags: i32) -> Result<i32, i32> {
    if matches!(
        get(fd)?.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return crate::tmpfs::reopen(fd, flags);
    }
    let mut pinned = ProcFdReference::acquire(fd)?;
    let source = pinned.entry;
    if flags & O_NOATIME != 0 && matches!(source.kind, FdKind::File | FdKind::Directory) {
        check_noatime(pinned.handle)?;
    }
    if flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL) {
        return Err(crate::EEXIST);
    }
    if flags & O_NOFOLLOW != 0 {
        // O_PATH|O_NOFOLLOW needs a procfs symlink descriptor, not the inode
        // behind it. Do not silently follow while that representation is absent.
        return Err(if flags & O_PATH != 0 {
            crate::EOPNOTSUPP
        } else {
            ELOOP
        });
    }
    let path_only = flags & O_PATH != 0;
    let directory = matches!(source.kind, FdKind::Directory | FdKind::SyntheticDirectory);
    if flags & O_DIRECTORY != 0 && !directory {
        return Err(ENOTDIR);
    }
    if !path_only && flags & O_ACCMODE == O_ACCMODE {
        return Err(EINVAL);
    }
    if directory && !path_only && flags & O_ACCMODE != O_RDONLY {
        return Err(EISDIR);
    }
    if matches!(
        source.kind,
        FdKind::MountNamespace | FdKind::TimeNamespace | FdKind::Namespace | FdKind::UserNamespace
    ) {
        if !path_only && flags & O_ACCMODE != O_RDONLY {
            return Err(EACCES);
        }
        let installed = install(pinned.handle as usize, source.kind, special_fd_flags(flags))?;
        pinned.handle = ptr::null_mut();
        return Ok(installed);
    }
    if source.kind == FdKind::SyntheticDirectory {
        let path = pinned.synthetic_path.take().ok_or(EIO)?;
        let mut fd_flags = special_fd_flags(flags);
        if path_only {
            fd_flags = fd_flags.union(FdFlags::PATH_ONLY);
        }
        return crate::install_procfs_directory(path, fd_flags);
    }
    if source.kind == FdKind::CgroupFile {
        let path = crate::cgroup_file_path(fd)?;
        if get(fd)?.generation != source.generation {
            return Err(EBADF);
        }
        let mut fd_flags = special_fd_flags(flags);
        if path_only {
            fd_flags = fd_flags.union(FdFlags::PATH_ONLY);
        }
        return crate::install_cgroup_file(path, fd_flags);
    }
    if matches!(source.kind, FdKind::ProcSysctl | FdKind::Synthetic) {
        let path = pinned.synthetic_path.take().ok_or(EIO)?;
        // Retain the original proc view and process identity, but perform a new
        // open with its own access flags and offset. Resolving /proc/self again
        // here would select the wrong process after fork or a namespace change.
        return crate::procfs::pinned(|| open_procfs_pinned(&path, flags));
    }
    if source.kind == FdKind::Fifo {
        return crate::fifo::reopen_pinned(pinned.fifo.as_ref().ok_or(EIO)?, flags);
    }
    if matches!(
        source.kind,
        FdKind::Null | FdKind::Zero | FdKind::Random | FdKind::Full
    ) {
        let mut fd_flags = special_fd_flags(flags);
        if path_only {
            fd_flags = fd_flags.union(FdFlags::PATH_ONLY);
        }
        return crate::install_dev_special(source.kind, fd_flags);
    }
    if source.kind == FdKind::Pipe {
        let requested = match flags & O_ACCMODE {
            O_RDONLY => FdFlags::PIPE_READ_END,
            O_WRONLY => FdFlags::PIPE_WRITE_END,
            O_RDWR => FdFlags::PIPE_READ_END.union(FdFlags::PIPE_WRITE_END),
            _ => return Err(EINVAL),
        };
        if !path_only && !source.flags.contains(requested) {
            // The native endpoint cannot gain the opposite endpoint's access.
            // A shared inode/endpoint registry is required for that operation.
            return Err(crate::EOPNOTSUPP);
        }
        // A pipe has one shared byte queue, no seek position, and request-local
        // OVERLAPPED state. Retain its kernel endpoint, but create a NEW Linux
        // open description with independent status flags. This is not a failed
        // filesystem-reopen fallback and never preserves the old description ID.
        let endpoints = if path_only {
            FdFlags(source.flags.0 & (FdFlags::PIPE_READ_END.0 | FdFlags::PIPE_WRITE_END.0))
        } else {
            requested
        };
        let mut fd_flags = special_fd_flags(flags).union(endpoints);
        if source.flags.contains(FdFlags::OVERLAPPED) {
            fd_flags = fd_flags.union(FdFlags::OVERLAPPED);
        }
        if path_only {
            fd_flags = fd_flags.union(FdFlags::PATH_ONLY);
        }
        if flags & O_NOATIME != 0 {
            fd_flags = fd_flags.union(FdFlags::NOATIME);
        }
        if flags & O_APPEND != 0 {
            fd_flags = fd_flags.union(FdFlags::APPEND);
        }
        let installed = crate::install_with(
            pinned.handle as usize,
            FdKind::Pipe,
            fd_flags,
            |_, entry| {
                if let Some(inode) = &pinned.pipe {
                    crate::pipe_inode::register(entry, inode.clone())?;
                }
                Ok(())
            },
        )?;
        pinned.handle = ptr::null_mut();
        return Ok(installed);
    }
    if matches!(
        source.kind,
        FdKind::Socket
            | FdKind::UnixSocket
            | FdKind::NetlinkSocket
            | FdKind::Event
            | FdKind::EventFd
            | FdKind::Inotify
            | FdKind::BpfProgram
    ) {
        // Linux socket/anon-inode objects do not provide an ordinary open
        // operation through procfs. Duplicating their HANDLE would bypass it
        // and fail to carry their object-specific side-table state.
        return Err(crate::ENXIO);
    }
    if !matches!(source.kind, FdKind::File | FdKind::Directory) {
        return Err(crate::EOPNOTSUPP);
    }
    let native_description = pinned
        .native
        .as_ref()
        .map(|d| {
            let writer = if !path_only && (flags & O_ACCMODE != O_RDONLY || flags & O_TRUNC != 0) {
                Some(d.policy.writer()?)
            } else {
                None
            };
            Ok::<_, i32>(crate::mount::native::Description {
                policy: d.policy.clone(),
                path: d.path.clone(),
                writer,
            })
        })
        .transpose()?;
    let mode = stored_mode_handle(pinned.handle)?;
    if !path_only && mode.is_some_and(|mode| matches!(mode & S_IFMT, S_IFCHR | S_IFBLK)) {
        if pinned
            .overlay
            .as_ref()
            .is_some_and(|d| d.mount_flags() & 4 != 0)
        {
            return Err(crate::EACCES);
        }
        let device = inode::read(pinned.handle)?.device.ok_or(EIO)?;
        return open_device_value(device, mode.ok_or(EIO)?, flags);
    }
    if !path_only && mode.is_some_and(|mode| mode & S_IFMT == S_IFIFO) {
        return crate::fifo::open_marker(pinned.handle, flags);
    }
    let access = if path_only {
        FILE_READ_ATTRIBUTES | SYNCHRONIZE
    } else {
        match flags & O_ACCMODE {
            O_RDONLY => GENERIC_READ,
            O_WRONLY => GENERIC_WRITE,
            O_RDWR => GENERIC_READ | GENERIC_WRITE,
            _ => return Err(EINVAL),
        }
    };
    // Empty-name NtCreateFile works on pinned directories as well as files,
    // and obtains fresh access rights and native open-file-description state.
    let overlay_object = pinned
        .overlay
        .as_ref()
        .map(|description| {
            crate::mount::overlay::reopen_object(
                description,
                pinned.handle,
                !path_only && (flags & O_ACCMODE != O_RDONLY || flags & O_TRUNC != 0),
                path_only,
            )
        })
        .transpose()?;
    let actual = overlay_object
        .as_ref()
        .map_or(pinned.handle, |(object, _)| object.raw());
    let reopened = object::Object::reopen(actual, access)?;
    let handle = reopened.raw();
    if !directory && !path_only && flags & O_ACCMODE != O_RDONLY {
        // ensure_writable obtains its own metadata-capable query handle; a
        // legitimate write-only native handle need not grant READ_ATTRIBUTES.
        // Holding write access already excludes a concurrent verity enable.
        verity::ensure_writable(handle)?;
    }
    if flags & O_TRUNC != 0 && !path_only && !directory {
        truncate_handle(handle, 0)?;
    }

    let mut fd_flags = special_fd_flags(flags).union(FdFlags::OVERLAPPED);
    if path_only {
        fd_flags = fd_flags.union(FdFlags::PATH_ONLY);
    }
    if !directory && !path_only {
        fd_flags = fd_flags.union(FdFlags::SEEKABLE);
    }
    if flags & O_NOATIME != 0 {
        fd_flags = fd_flags.union(FdFlags::NOATIME);
    }
    if flags & O_APPEND != 0 {
        fd_flags = fd_flags.union(FdFlags::APPEND);
    }
    let overlay_description = overlay_object.map(|(_, description)| description);
    let installed = crate::install_with(handle as usize, source.kind, fd_flags, |_, entry| {
        if let Some(description) = overlay_description {
            crate::mount::overlay::register(entry, description)?;
        }
        if let Some(description) = native_description {
            crate::mount::native::register(entry, description)?;
        }
        Ok(())
    })?;
    reopened.into_raw();
    Ok(installed)
}

/// Operation-local reference, acquired before a concurrent close/dup2 can reuse
/// the source slot. No filesystem I/O or wait occurs under the descriptor lock.
struct ProcFdReference {
    entry: crate::FdEntry,
    handle: HANDLE,
    fifo: Option<crate::fifo::Pinned>,
    overlay: Option<crate::mount::overlay::Description>,
    native: Option<crate::mount::native::Description>,
    pipe: Option<std::sync::Arc<crate::pipe_inode::Inode>>,
    synthetic_path: Option<String>,
}

impl ProcFdReference {
    fn acquire(fd: i32) -> Result<Self, i32> {
        let table = crate::table().read().map_err(|_| EIO)?;
        let entry = usize::try_from(fd)
            .ok()
            .and_then(|fd| table.slots.get(fd))
            .and_then(|entry| *entry)
            .ok_or(ENOENT)?;
        let mut handle = ptr::null_mut();
        let overlay = crate::mount::overlay::reference(entry)?;
        let native = crate::mount::native::reference(entry)?;
        let pipe = crate::pipe_inode::reference(entry)?;
        if entry.raw != 0
            && matches!(
                entry.kind,
                FdKind::File
                    | FdKind::Directory
                    | FdKind::Pipe
                    | FdKind::MountNamespace
                    | FdKind::TimeNamespace
                    | FdKind::Namespace
                    | FdKind::UserNamespace
            )
        {
            if unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    entry.raw as HANDLE,
                    GetCurrentProcess(),
                    &mut handle,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            } == 0
            {
                return Err(errno_from_win32(unsafe { GetLastError() }));
            }
        }
        let fifo = if entry.kind == FdKind::Fifo {
            Some(crate::fifo::pin_entry_locked(entry)?)
        } else {
            None
        };
        // Synthetic publication takes its side-table lock before the fd table.
        // Copy outside this fd lock and validate the generation again; the copied
        // immutable identity then survives closing/reusing the source fd.
        drop(table);
        let synthetic_path = if matches!(
            entry.kind,
            FdKind::SyntheticDirectory | FdKind::ProcSysctl | FdKind::Synthetic
        ) {
            let path = crate::procfs::descriptor_path(fd)?;
            if crate::get(fd)?.generation != entry.generation {
                return Err(ENOENT);
            }
            Some(path)
        } else {
            None
        };
        Ok(Self {
            entry,
            handle,
            fifo,
            overlay,
            native,
            pipe,
            synthetic_path,
        })
    }
}

impl Drop for ProcFdReference {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

/// The pseudo-terminal number a `/dev/pts/N` path names.
fn pts_number(path: &str) -> Option<u32> {
    let rest = path.strip_prefix("/dev/pts/")?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

pub(crate) fn check_character_device_open(flags: i32, major: u32, minor: u32) -> Result<(), i32> {
    if flags & O_PATH != 0 {
        return Ok(());
    }
    let access = match flags & O_ACCMODE {
        O_RDONLY => crate::bpf::BPF_DEVCG_ACC_READ,
        O_WRONLY => crate::bpf::BPF_DEVCG_ACC_WRITE,
        O_RDWR => crate::bpf::BPF_DEVCG_ACC_READ | crate::bpf::BPF_DEVCG_ACC_WRITE,
        _ => return Err(EINVAL),
    };
    crate::bpf::check_current_device(crate::bpf::BPF_DEVCG_DEV_CHAR, major, minor, access)
}

pub(crate) fn device_numbers(device: u64) -> (u32, u32) {
    let major = (((device >> 8) & 0xfff) | ((device >> 32) & 0xffff_f000)) as u32;
    let minor = ((device & 0xff) | ((device >> 12) & 0xffff_ff00)) as u32;
    (major, minor)
}

pub(crate) fn special_fd_flags(flags: i32) -> FdFlags {
    let mut result = FdFlags::NONE;
    if flags & O_CLOEXEC != 0 {
        result = result.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & O_PATH != 0 {
        return result.union(FdFlags::PATH_ONLY);
    }
    if flags & O_NONBLOCK != 0 {
        result = result.union(FdFlags::NONBLOCK);
    }
    match flags & O_ACCMODE {
        O_RDONLY => result = result.union(FdFlags::READ_ACCESS),
        O_WRONLY => result = result.union(FdFlags::WRITE_ACCESS),
        O_RDWR => {
            result = result
                .union(FdFlags::READ_ACCESS)
                .union(FdFlags::WRITE_ACCESS);
        }
        _ => {}
    }
    result
}

pub(crate) fn check_noatime(handle: HANDLE) -> Result<(), i32> {
    let caller = crate::credentials::filesystem();
    if caller.uid == 0
        || caller.capabilities & (1 << 3) != 0
        || stat_handle(handle, false)?.st_uid == caller.uid
    {
        Ok(())
    } else {
        Err(crate::EPERM)
    }
}

fn open_device_node(path: &Path, mode: u32, flags: i32) -> Result<i32, i32> {
    let device = stored_device(path)?.ok_or(crate::EIO)?;
    if device == (5 << 8) | 2 {
        check_character_device_open(flags, 5, 2)?;
        return crate::devpts::open_sibling(&crate::to_guest_path(path), flags)?
            .ok_or(crate::ENODEV);
    }
    open_device_value(device, mode, flags)
}
pub(crate) fn open_device_value(device: u64, mode: u32, flags: i32) -> Result<i32, i32> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    let (major, minor) = device_numbers(device);
    if mode & S_IFMT == S_IFBLK {
        return Err(crate::ENXIO);
    }
    check_character_device_open(flags, major, minor)?;
    let fd_flags = special_fd_flags(flags);
    match (major, minor) {
        (1, 3) => crate::install_dev_special(crate::FdKind::Null, fd_flags),
        (1, 5) => crate::install_dev_special(crate::FdKind::Zero, fd_flags),
        (1, 7) => crate::install_dev_special(crate::FdKind::Full, fd_flags),
        (1, 8 | 9) => crate::install_dev_special(crate::FdKind::Random, fd_flags),
        (5, 0) => crate::tty::open_controlling(flags),
        (5, 2) => crate::tty::open_master(flags),
        (136, number) => crate::tty::open_slave(number, flags),
        // The inode is real, but no host driver implements this device.
        _ => Err(crate::ENXIO),
    }
}

/// The `openat` form. See [`resolve_at`] for the supported `dirfd` values.
pub fn openat(dirfd: i32, path: &str, flags: i32, mode: u32) -> Result<i32, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    if !path.starts_with("/")
        && dirfd != AT_FDCWD
        && crate::get(dirfd)?.kind == FdKind::TmpfsDirectory
    {
        return crate::tmpfs::openat(dirfd, path, flags, mode);
    }
    let absolute = if crate::mount::api::tree_reference(path).is_some() {
        path.to_owned()
    } else if !path.starts_with('/')
        && dirfd != AT_FDCWD
        && crate::get(dirfd)?.kind == FdKind::MountTree
    {
        format!("{}/{path}", crate::mount::api::tree_path(dirfd)?)
    } else if path.starts_with('/') || dirfd == AT_FDCWD {
        absolute_linux(path)
    } else if let Ok(base) = crate::synthetic_directory_path(dirfd) {
        if base.ends_with('/') {
            format!("{base}{path}")
        } else {
            format!("{base}/{path}")
        }
    } else if let Some(base) = crate::mount::overlay::descriptor_path(dirfd)? {
        format!("{base}/{path}")
    } else if let Some(base) = crate::mount::native::descriptor_path(dirfd)? {
        format!("{base}/{path}")
    } else {
        absolute_linux(path)
    };
    if !path.starts_with('/')
        && dirfd != AT_FDCWD
        && crate::synthetic_directory_path(dirfd).is_ok_and(|p| p.starts_with("/proc/.mount/"))
    {
        return crate::procfs::pinned(|| open_procfs(&absolute, flags));
    }
    if crate::mount::proc_location(&absolute)?.is_some() {
        return open_procfs(&absolute, flags);
    }
    if let Some(fd) = crate::tmpfs::open(&absolute, flags, mode)? {
        return Ok(fd);
    }
    if crate::procfs::owns(&absolute)
        && (crate::procfs::instance::lookup(&absolute)?.is_some()
            || crate::mount::api::tree_reference(&absolute).is_none()
            || flags & O_NOFOLLOW != 0)
    {
        if !path.starts_with('/')
            && dirfd != AT_FDCWD
            && crate::synthetic_directory_path(dirfd).is_ok()
        {
            return crate::procfs::pinned(|| open_procfs(&absolute, flags));
        }
        return open_procfs(&absolute, flags);
    }
    if crate::cgroup::owns(&absolute) {
        let metadata = crate::cgroup::metadata(&absolute)?;
        if metadata.kind == crate::procfs::ProcKind::Directory {
            if flags & O_DIRECTORY == 0 && flags & O_ACCMODE != O_RDONLY {
                return Err(EISDIR);
            }
            return crate::install_procfs_directory(absolute, special_fd_flags(flags));
        }
        return crate::install_cgroup_file(absolute, special_fd_flags(flags));
    }
    match absolute.as_str() {
        "/dev/null" => {
            check_character_device_open(flags, 1, 3)?;
            return crate::install_dev_special(crate::FdKind::Null, special_fd_flags(flags));
        }
        "/dev/zero" => {
            check_character_device_open(flags, 1, 5)?;
            return crate::install_dev_special(crate::FdKind::Zero, special_fd_flags(flags));
        }
        "/dev/urandom" => {
            check_character_device_open(flags, 1, 9)?;
            return crate::install_dev_special(crate::FdKind::Random, special_fd_flags(flags));
        }
        "/dev/random" => {
            check_character_device_open(flags, 1, 8)?;
            return crate::install_dev_special(crate::FdKind::Random, special_fd_flags(flags));
        }
        "/dev/full" => {
            check_character_device_open(flags, 1, 7)?;
            return crate::install_dev_special(crate::FdKind::Full, special_fd_flags(flags));
        }
        // Opening the multiplexer *creates* a terminal, which is the whole of
        // how a pty is allocated on Linux: there is no separate call.
        "/dev/ptmx" | "/dev/pts/ptmx" => {
            check_character_device_open(flags, 5, 2)?;
            if let Some(fd) = crate::devpts::open_sibling(&absolute, flags)? {
                return Ok(fd);
            }
            return crate::tty::open_master(flags);
        }
        // The caller's controlling terminal, whatever descriptor it arrived on.
        // This is how a program that has had its standard streams redirected —
        // `ssh` asking for a password, `sudo` asking for one — reaches the user.
        "/dev/tty" => {
            check_character_device_open(flags, 5, 0)?;
            return crate::tty::open_controlling(flags);
        }
        "/dev/stdin" => return openat(AT_FDCWD, "/proc/self/fd/0", flags, mode),
        "/dev/stdout" => return openat(AT_FDCWD, "/proc/self/fd/1", flags, mode),
        "/dev/stderr" => return openat(AT_FDCWD, "/proc/self/fd/2", flags, mode),
        "/dev/fd" => return openat(AT_FDCWD, "/proc/self/fd", flags, mode),
        path if path.starts_with("/dev/fd/") => {
            return openat(
                AT_FDCWD,
                &format!("/proc/self/fd/{}", &path[8..]),
                flags,
                mode,
            );
        }
        "/dev/core" => return openat(AT_FDCWD, "/proc/kcore", flags, mode),
        "/dev/shm" => {
            if flags & O_ACCMODE != O_RDONLY {
                return Err(EISDIR);
            }
            return crate::install_procfs_directory(
                String::from("/dev/shm"),
                special_fd_flags(flags),
            );
        }
        // A directory that really can be opened and listed, rather than one that
        // only exists in `stat`.
        "/dev/pts" => {
            if flags & O_ACCMODE != O_RDONLY {
                return Err(EISDIR);
            }
            return crate::install_procfs_directory(
                String::from("/dev/pts"),
                special_fd_flags(flags),
            );
        }
        _ => {}
    }
    // `/dev/pts/N` is the slave the master with that number handed out. The
    // number is parsed rather than matched because the set is open-ended.
    if let Some(number) = pts_number(&absolute) {
        check_character_device_open(flags, 136, number)?;
        return crate::tty::open_slave(number, flags);
    }
    let no_follow_final =
        flags & O_NOFOLLOW != 0 || flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL);
    let mut overlay_path = match crate::mount::overlay::open_path(&absolute, flags)? {
        crate::mount::overlay::OpenPath::Native(path) => path,
        crate::mount::overlay::OpenPath::Virtual => {
            // Continue through real backend descriptors. A symlink from an
            // image layer into /dev or /proc has no native Windows pathname.
            return confined::open(dirfd, path, flags, mode, 0);
        }
    };
    let native_description = if overlay_path.is_some() {
        None
    } else {
        crate::mount::native::prepare_open(&absolute, flags)?
    };
    let resolved = if let Some(overlay) = &overlay_path {
        overlay.to_path_buf()
    } else if no_follow_final {
        resolve_at_no_follow(dirfd, path)?
    } else {
        resolve_at(dirfd, path)?
    };
    let final_is_symlink = emulated_symlink_target(&resolved)?.is_some()
        || std::fs::symlink_metadata(&resolved)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false);
    if flags & O_NOFOLLOW != 0 && flags & O_PATH == 0 && final_is_symlink {
        return Err(ELOOP);
    }
    if absolute == "/etc/resolv.conf" && !resolved.exists() {
        return crate::install_procfs_file(
            &absolute,
            synthetic_resolv_conf(),
            special_fd_flags(flags),
        );
    }
    if absolute == "/etc/environment" && !resolved.exists() {
        return crate::install_procfs_file(
            &absolute,
            synthetic_environment(),
            special_fd_flags(flags),
        );
    }
    if absolute == "/etc/hosts" && !resolved.exists() {
        return crate::install_procfs_file(
            &absolute,
            b"127.0.0.1 localhost\n::1 localhost\n".to_vec(),
            special_fd_flags(flags),
        );
    }
    if absolute == "/etc/passwd" && !resolved.exists() {
        return crate::install_procfs_file(
            &absolute,
            b"root:x:0:0:root:/root:/bin/sh\nnobody:x:65534:65534:nobody:/nonexistent:/bin/false\n"
                .to_vec(),
            special_fd_flags(flags),
        );
    }
    if absolute == "/etc/group" && !resolved.exists() {
        return crate::install_procfs_file(
            &absolute,
            b"root:x:0:\nnogroup:x:65534:\n".to_vec(),
            special_fd_flags(flags),
        );
    }
    if absolute == "/etc/nsswitch.conf" && !resolved.exists() {
        return crate::install_procfs_file(
            &absolute,
            b"hosts: files dns\n".to_vec(),
            special_fd_flags(flags),
        );
    }

    let is_tmpfile = (flags & 0o20000000) != 0;
    let (resolved, wide_path) = if is_tmpfile && resolved.is_dir() {
        let random_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp_file = resolved.join(format!(".tmpfile_{}_{}", std::process::id(), random_nanos));
        let wide = wide(&tmp_file)?;
        (tmp_file, wide)
    } else {
        let wide = wide(&resolved)?;
        (resolved, wide)
    };

    let stored = if is_tmpfile {
        None
    } else {
        stored_mode(&resolved)?
    };
    if flags & O_PATH == 0 && stored.is_some_and(|mode| matches!(mode & S_IFMT, S_IFCHR | S_IFBLK))
    {
        if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
            return Err(crate::EEXIST);
        }
        return open_device_node(&resolved, stored.ok_or(crate::EIO)?, flags);
    }

    if !is_tmpfile && flags & O_PATH == 0 && stored.is_some_and(|stored| stored & S_IFMT == S_IFIFO)
    {
        if flags & O_DIRECTORY != 0 {
            return Err(ENOTDIR);
        }
        if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
            return Err(crate::EEXIST);
        }
        return crate::fifo::open_path(&resolved, flags);
    }

    let is_path = flags & O_PATH != 0;
    let accmode = flags & O_ACCMODE;
    let access = if is_tmpfile {
        GENERIC_READ | GENERIC_WRITE | DELETE
    } else if is_path {
        windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES | SYNCHRONIZE
    } else {
        match accmode {
            O_RDONLY => GENERIC_READ,
            O_WRONLY => GENERIC_WRITE,
            O_RDWR => GENERIC_READ | GENERIC_WRITE,
            _ => return Err(EINVAL),
        }
    };

    let disposition = if is_tmpfile {
        CREATE_NEW
    } else {
        match (
            flags & O_CREAT != 0,
            flags & O_EXCL != 0,
            flags & O_TRUNC != 0,
        ) {
            (true, true, _) => CREATE_NEW,
            // Never let the native open truncate before inode policy has been
            // checked. fs-verity protects the data even when mode permits write.
            (true, false, true) => OPEN_ALWAYS,
            (true, false, false) => OPEN_ALWAYS,
            (false, _, true) => OPEN_EXISTING,
            (false, _, false) => OPEN_EXISTING,
        }
    };

    let is_dir_target = !is_tmpfile && resolved.is_dir();
    if is_dir_target && flags & O_PATH == 0 && flags & O_ACCMODE != O_RDONLY {
        return Err(EISDIR);
    }
    let directory = !is_tmpfile && (flags & O_DIRECTORY != 0 || is_dir_target);
    let mut attributes = if directory {
        FILE_FLAG_BACKUP_SEMANTICS
    } else {
        FILE_FLAG_OVERLAPPED
    };
    if flags & O_NOFOLLOW != 0 {
        attributes |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    if is_tmpfile {
        attributes |= windows_sys::Win32::Storage::FileSystem::FILE_FLAG_DELETE_ON_CLOSE;
    }

    // SAFETY: `wide_path` is a null-terminated wide string that outlives the call.
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            access,
            // FILE_SHARE_DELETE is what lets another descriptor unlink this file
            // while it is still open, which POSIX requires.
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            disposition,
            attributes,
            ptr::null_mut(),
        )
    };
    let open_status = unsafe { GetLastError() };
    if handle.is_null() || handle as isize == -1 {
        // SAFETY: GetLastError has no preconditions.
        let error = unsafe { GetLastError() };
        // Opening a directory without FILE_FLAG_BACKUP_SEMANTICS fails with
        // ERROR_ACCESS_DENIED; report the POSIX errno for that case.
        if error == ERROR_ACCESS_DENIED && !directory && resolved.is_dir() {
            return Err(EISDIR);
        }
        return Err(errno_from_win32(error));
    }

    if !directory && !is_path && (accmode != O_RDONLY || flags & O_TRUNC != 0) {
        let policy = verity::ensure_writable(handle).and_then(|()| {
            if flags & O_TRUNC != 0 {
                truncate_handle(handle, 0)
            } else {
                Ok(())
            }
        });
        if let Err(error) = policy {
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
    }

    if !is_tmpfile && (flags & O_DIRECTORY != 0) && !resolved.is_dir() {
        // SAFETY: the handle was just opened and is owned here.
        unsafe { CloseHandle(handle) };
        return Err(ENOTDIR);
    }

    if is_tmpfile || flags & O_CREAT != 0 && open_status != 183 {
        let initialized = if let Some(overlay) = &overlay_path {
            overlay.initialize_created_handle(handle, S_IFREG | (mode & 0o7777))
        } else {
            initialize_created_handle(handle, &resolved, S_IFREG | (mode & 0o7777))
        };
        if let Err(error) = initialized {
            let _ = unlink_inode(handle);
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
    }
    if is_tmpfile {
        if let Err(error) = unlink_inode(handle) {
            unsafe {
                CloseHandle(handle);
            }
            return Err(error);
        }
    }

    let mut fd_flags = FdFlags::NONE;
    let kind = if resolved.is_dir() {
        FdKind::Directory
    } else {
        fd_flags = fd_flags.union(FdFlags::OVERLAPPED).union(FdFlags::SEEKABLE);
        FdKind::File
    };
    if flags & O_CLOEXEC != 0 {
        fd_flags = fd_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    if is_path {
        fd_flags = fd_flags.union(FdFlags::PATH_ONLY);
    } else {
        if accmode != O_WRONLY {
            fd_flags = fd_flags.union(FdFlags::READ_ACCESS);
        }
        if accmode != O_RDONLY {
            fd_flags = fd_flags.union(FdFlags::WRITE_ACCESS);
        }
    }
    if flags & O_NONBLOCK != 0 {
        fd_flags = fd_flags.union(FdFlags::NONBLOCK);
    }
    if flags & O_NOATIME != 0 {
        fd_flags = fd_flags.union(FdFlags::NOATIME);
    }
    if flags & O_APPEND != 0 {
        fd_flags = fd_flags.union(FdFlags::APPEND);
    }

    if flags & O_NOATIME != 0 {
        if let Err(error) = check_noatime(handle) {
            unsafe {
                CloseHandle(handle);
            }
            return Err(error);
        }
    }
    let overlay_description = match overlay_path
        .as_mut()
        .map(|overlay| {
            overlay.finish()?;
            overlay.description(handle, fd_flags.contains(FdFlags::WRITE_ACCESS))
        })
        .transpose()
    {
        Ok(value) => value.flatten(),
        Err(error) => {
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
    };
    let fd = crate::install_with(handle as usize, kind, fd_flags, |_, entry| {
        if let Some(description) = overlay_description {
            crate::mount::overlay::register(entry, description)?;
        }
        if let Some(description) = native_description {
            crate::mount::native::register(entry, description)?;
        }
        Ok(())
    })
    .inspect_err(|_| {
        // SAFETY: installation failed, so this function still owns the handle.
        unsafe { CloseHandle(handle) };
    })?;

    // O_APPEND affects each write, not the initial open-file position. Keep
    // native write rights too: F_SETFL can remove APPEND, and ftruncate remains
    // valid on an append-mode descriptor. The overlapped write offset supplies
    // the native atomic-append operation while the flag is set.
    Ok(fd)
}

/// Returns the size of an open file in bytes.
fn file_size(handle: HANDLE) -> Result<u64, i32> {
    verity::authoritative_size(handle)
}

/// Repositions a seekable descriptor.
///
/// Because overlapped handles carry the offset per request, this only has to
/// update the table's own position rather than call `SetFilePointerEx`.
pub fn lseek(fd: i32, offset: i64, whence: i32) -> Result<u64, i32> {
    crate::ofd::with(fd, || lseek_inner(fd, offset, whence))
}
fn lseek_inner(fd: i32, offset: i64, whence: i32) -> Result<u64, i32> {
    if matches!(
        get(fd)?.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return crate::tmpfs::seek(fd, offset, whence);
    }
    if let Some(position) = crate::mount::overlay::seek_directory(fd, offset, whence)? {
        return Ok(position);
    }
    let entry = get(fd)?;
    if matches!(
        entry.kind,
        FdKind::Null | FdKind::Zero | FdKind::Full | FdKind::Random
    ) {
        return Ok(0);
    }
    // Pipes, sockets and consoles have no file position.
    if !entry.flags.contains(FdFlags::SEEKABLE) {
        return Err(ESPIPE);
    }
    let base = match whence {
        SEEK_SET => 0,
        SEEK_CUR => entry.offset,
        // A synthetic file has no handle to query for its length.
        SEEK_END if entry.kind == FdKind::Synthetic => crate::synthetic_size(fd)?,
        SEEK_END => file_size(entry.raw as HANDLE)?,
        _ => return Err(EINVAL),
    };

    // A negative resulting offset is EINVAL; seeking past the end is legal and
    // creates a sparse hole on the next write.
    let target = if offset >= 0 {
        base.checked_add(offset as u64).ok_or(EINVAL)?
    } else {
        base.checked_sub(offset.unsigned_abs()).ok_or(EINVAL)?
    };

    let mut table = table().write().map_err(|_| crate::EIO)?;
    let slot = table.slots.get_mut(fd as usize).ok_or(EBADF)?;
    let stored = slot.as_mut().ok_or(EBADF)?;
    // Re-check the generation: the descriptor may have been recycled between
    // the read above and this write.
    if stored.generation != entry.generation {
        return Err(EBADF);
    }
    stored.offset = target;
    Ok(target)
}

/// Seconds between the Windows FILETIME epoch (1601) and the Unix epoch (1970).
const FILETIME_TO_UNIX_SECONDS: i64 = 11_644_473_600;

/// Splits a FILETIME into Unix seconds and nanoseconds.
fn filetime_to_unix(time: &FILETIME) -> (i64, i64) {
    let ticks = ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    if ticks == 0 {
        return (0, 0);
    }
    let ticks = ticks as i64;
    let seconds = ticks / 10_000_000 - FILETIME_TO_UNIX_SECONDS;
    // Each tick is 100ns, so the sub-second remainder scales by 100.
    let nanoseconds = (ticks % 10_000_000) * 100;
    (seconds, nanoseconds)
}

/// Builds a Linux `struct stat` from Windows file information.
fn stat_from_info(
    info: &BY_HANDLE_FILE_INFORMATION,
    symlink: bool,
    stored_mode: Option<u32>,
) -> Stat {
    let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let readonly = info.dwFileAttributes & FILE_ATTRIBUTE_READONLY != 0;

    // The owner bits describe the ordinary host access. Group/other bits come
    // from the DACL when it can be read, so an owner-only private key remains
    // owner-only instead of every regular file being reported as 0755.
    let stored_type = stored_mode.map_or(0, |mode| mode & S_IFMT);
    let permissions = stored_mode.map(|mode| mode & 0o7777).unwrap_or_else(|| {
        if directory {
            0o755
        } else if readonly {
            0o555
        } else {
            0o755
        }
    });
    let format = if symlink {
        S_IFLNK
    } else if directory {
        S_IFDIR
    } else if stored_type != 0 {
        stored_type
    } else {
        S_IFREG
    };

    let size = ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64;
    let (atime, atime_nsec) = filetime_to_unix(&info.ftLastAccessTime);
    let (mtime, mtime_nsec) = filetime_to_unix(&info.ftLastWriteTime);
    // Windows tracks creation time; Linux st_ctime is inode change time. Neither
    // maps cleanly, so creation time is the closest available approximation.
    let (ctime, ctime_nsec) = filetime_to_unix(&info.ftCreationTime);

    Stat {
        st_dev: info.dwVolumeSerialNumber as u64,
        st_ino: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        st_nlink: info.nNumberOfLinks as u64,
        st_mode: format | permissions,
        st_size: size as i64,
        st_blksize: 4096,
        // POSIX counts 512-byte blocks regardless of the real cluster size.
        st_blocks: size.div_ceil(512) as i64,
        st_atime: atime,
        st_atime_nsec: atime_nsec,
        st_mtime: mtime,
        st_mtime_nsec: mtime_nsec,
        st_ctime: ctime,
        st_ctime_nsec: ctime_nsec,
        ..Stat::default()
    }
}

/// Projects a file DACL onto Linux owner/group/other mode classes.
///
/// Windows ACLs can express principals that a three-class mode cannot. An allow
/// ACE for such a principal is therefore projected onto `other`, while the
/// descriptor's primary-group SID maps to `group`. LocalSystem, Administrators
/// and CREATOR OWNER are privileged metadata principals rather than ordinary
/// file users and do not make the Linux mode world-accessible. An allow ACE we
/// cannot decode is reported as shared, never as falsely private.
fn permissions_from_acl(handle: HANDLE, readonly: bool) -> Option<u32> {
    let mut owner: PSID = ptr::null_mut();
    let mut group: PSID = ptr::null_mut();
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: all output pointers are writable locals and `handle` is live.
    let result = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut owner,
            &raw mut group,
            &raw mut dacl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if result != 0 || descriptor.is_null() {
        return None;
    }

    let mut owner_bits = 0u32;
    let mut group_bits = 0u32;
    let mut other_bits = 0u32;
    if dacl.is_null() {
        // A null DACL grants full access to everyone.
        owner_bits = 0o7;
        group_bits = 0o7;
        other_bits = 0o7;
    } else {
        // SAFETY: GetSecurityInfo returned a descriptor containing this ACL.
        let count = unsafe { (*dacl).AceCount } as u32;
        for index in 0..count {
            let mut raw_ace = ptr::null_mut();
            // SAFETY: `index` is within AceCount and raw_ace is writable.
            if unsafe { windows_sys::Win32::Security::GetAce(dacl, index, &raw mut raw_ace) } == 0
                || raw_ace.is_null()
            {
                group_bits = 0o7;
                other_bits = 0o7;
                break;
            }
            // ACCESS_ALLOWED_ACE_TYPE is zero. Object/callback allow ACEs use
            // a different layout; until their object fields are decoded,
            // conservatively report the file as shared rather than private.
            let header = raw_ace.cast::<windows_sys::Win32::Security::ACE_HEADER>();
            let ace_type = unsafe { (*header).AceType };
            if matches!(ace_type, 5 | 9 | 11) {
                group_bits = 0o7;
                other_bits = 0o7;
                break;
            }
            // Deny and audit ACEs do not themselves grant access.
            if ace_type != 0 {
                continue;
            }
            let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
            // SAFETY: a basic ACCESS_ALLOWED_ACE stores its SID at SidStart.
            let sid = unsafe { (&raw const (*ace).SidStart).cast_mut().cast() };
            let owner_ace = !owner.is_null() && unsafe { EqualSid(sid, owner) } != 0;
            let group_ace = !group.is_null() && unsafe { EqualSid(sid, group) } != 0;
            let privileged = unsafe { IsWellKnownSid(sid, WinLocalSystemSid) } != 0
                || unsafe { IsWellKnownSid(sid, WinBuiltinAdministratorsSid) } != 0
                || unsafe { IsWellKnownSid(sid, WinCreatorOwnerSid) } != 0;
            if privileged {
                continue;
            }
            // SAFETY: `ace` is the allowed ACE just classified above.
            let mask = unsafe { (*ace).Mask };
            let mut class = 0u32;
            if mask & (GENERIC_READ | FILE_READ_DATA) != 0 {
                class |= 0o4;
            }
            if !readonly && mask & (GENERIC_WRITE | FILE_WRITE_DATA | FILE_APPEND_DATA) != 0 {
                class |= 0o2;
            }
            if mask & (GENERIC_EXECUTE | FILE_EXECUTE) != 0 {
                class |= 0o1;
            }
            let broad_group = unsafe { IsWellKnownSid(sid, WinAuthenticatedUserSid) } != 0
                || unsafe { IsWellKnownSid(sid, WinBuiltinUsersSid) } != 0;
            let is_world = unsafe { IsWellKnownSid(sid, WinWorldSid) } != 0;
            if owner_ace {
                owner_bits |= class;
            } else if group_ace || broad_group {
                group_bits |= class;
            } else if is_world {
                other_bits |= class;
            }
        }
    }

    // Windows allow ACEs are cumulative.  The owner is also an authenticated
    // user and a member of Everyone, while a group member is also in Everyone.
    // POSIX mode selection does not fall through from owner to group/other, so
    // fold broader grants into the narrower classes before publishing st_mode.
    owner_bits |= group_bits | other_bits;
    group_bits |= other_bits;

    if readonly {
        owner_bits &= !0o2;
        group_bits &= !0o2;
        other_bits &= !0o2;
    }

    // SAFETY: GetSecurityInfo allocated the returned descriptor with LocalAlloc.
    unsafe { LocalFree(descriptor) };
    Some((owner_bits << 6) | (group_bits << 3) | other_bits)
}

fn stored_device(path: &Path) -> Result<Option<u64>, i32> {
    Ok(inode::read_path(path)?.device)
}

fn stored_mode(path: &Path) -> Result<Option<u32>, i32> {
    Ok(inode::read_path(path)?.mode)
}

pub(crate) fn path_from_handle(handle: HANDLE) -> Option<std::path::PathBuf> {
    let mut buffer = vec![0u16; 512];
    loop {
        let written = unsafe {
            GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        } as usize;
        if written == 0 {
            return None;
        }
        if written < buffer.len() {
            buffer.truncate(written);
            return Some(std::path::PathBuf::from(OsString::from_wide(&buffer)));
        }
        buffer.resize(written.saturating_add(1), 0);
    }
}

/// Store mode on the inode itself. A native read-only attribute is only the
/// owner-write projection; it must not prevent changing Linux metadata.
pub fn set_mode_host_path(path: &Path, mode: u32) -> Result<(), i32> {
    let object = object::Object::open(path, FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)?;
    set_mode_handle(object.raw(), mode)
}

/// Initialize a newly allocated inode before publishing it in an overlay.
/// The parent is the visible guest directory, never the staging workdir.
pub(crate) fn initialize_inode(handle: HANDLE, parent: &Stat, mode: u32) -> Result<(), i32> {
    let caller = crate::credentials::filesystem();
    let inherited = parent.st_mode & 0o2000 != 0;
    let gid = if inherited { parent.st_gid } else { caller.gid };
    let mut mode = mode;
    if mode & S_IFMT == S_IFDIR && inherited {
        mode |= 0o2000;
    }
    if mode & S_IFMT != S_IFDIR
        && mode & 0o2000 != 0
        && caller.capabilities & (1 << 4) == 0
        && !crate::credentials::group_member(gid)
    {
        mode &= !0o2000;
    }
    set_mode_handle(handle, mode)?;
    inode::update(handle, |record| {
        record.uid = Some(caller.uid);
        record.gid = Some(gid);
        Ok(())
    })
}

pub(crate) fn initialize_created_handle(handle: HANDLE, path: &Path, mode: u32) -> Result<(), i32> {
    let parent = object::Object::open(
        path.parent().ok_or(EINVAL)?,
        FILE_READ_ATTRIBUTES | 0x00020000,
    )?;
    initialize_inode(handle, &stat_handle(parent.raw(), false)?, mode)
}

pub fn initialize_created_path(path: &Path, mode: u32) -> Result<(), i32> {
    let inode = object::Object::open(path, FILE_READ_ATTRIBUTES)?;
    initialize_created_handle(inode.raw(), path, mode)
}

/// Linux ownership is inode metadata; Windows ACLs remain the host access gate.
pub struct Ownership {
    pub uid: u32,
    pub gid: u32,
    pub caller: u32,
    pub group_member: bool,
}
impl Ownership {
    fn translated(&self, mapping: Option<&crate::user_namespace::Mapping>) -> Result<Self, i32> {
        let Some(map) = mapping else {
            return Ok(Self {
                uid: self.uid,
                gid: self.gid,
                caller: self.caller,
                group_member: self.group_member,
            });
        };
        Ok(Self {
            uid: if self.uid == u32::MAX {
                self.uid
            } else {
                map.up(self.uid, false).ok_or(crate::EOVERFLOW)?
            },
            gid: if self.gid == u32::MAX {
                self.gid
            } else {
                map.up(self.gid, true).ok_or(crate::EOVERFLOW)?
            },
            caller: 0,
            group_member: true,
        })
    }
    pub(crate) fn check(&self, stat: &Stat) -> Result<(), i32> {
        if self.caller != 0
            && (self.caller != stat.st_uid
                || self.uid != u32::MAX && self.uid != stat.st_uid
                || self.gid != u32::MAX && self.gid != stat.st_gid && !self.group_member)
        {
            return Err(crate::EPERM);
        }
        Ok(())
    }
}
fn set_ownership_handle(handle: HANDLE, owner: &Ownership) -> Result<(), i32> {
    let query = {
        object::Object::reopen(
            handle,
            FILE_READ_ATTRIBUTES
                | windows_sys::Win32::Storage::FileSystem::FILE_READ_EA
                | 0x00020000,
        )?
    };
    let handle = query.raw();
    let _lock = crate::xattr::InodeLock::acquire(handle)?;
    let stat = stat_handle(handle, false)?;
    owner.check(&stat)?;
    inode::update(handle, |record| {
        if owner.uid != u32::MAX {
            record.uid = Some(owner.uid);
        }
        if owner.gid != u32::MAX {
            record.gid = Some(owner.gid);
        }
        if stat.st_mode & S_IFMT == S_IFREG && (owner.uid != u32::MAX || owner.gid != u32::MAX) {
            let clear = 0o4000 | if stat.st_mode & 0o10 != 0 { 0o2000 } else { 0 };
            record.mode = Some(stat.st_mode & !clear);
        }
        Ok(())
    })?;
    if stat.st_mode & S_IFMT == S_IFREG && (owner.uid != u32::MAX || owner.gid != u32::MAX) {
        let attrs = unsafe { crate::xattr::Attributes::from_handle(handle, true)? };
        match attrs.remove(b"security.capability") {
            Ok(()) | Err(crate::xattr::ENODATA) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
pub fn chown(path: &str, follow: bool, owner: &Ownership) -> Result<(), i32> {
    if crate::tmpfs::chown(path, follow, owner)? {
        return Ok(());
    }
    if path.is_empty() {
        return Err(crate::ENOENT);
    }
    let absolute = absolute_linux(path);
    if let Some(number) = pts_number(&absolute) {
        if !crate::tty::terminal_exists(number) {
            return Err(crate::ENOENT);
        }
        owner.check(&stat(path)?)?;
        return crate::tty::set_terminal_ownership(number, owner.uid, owner.gid);
    }
    if follow && let Some((pid, fd)) = crate::procfs::fd_magic_link(&absolute) {
        if pid != crate::job::process_id() {
            return Err(crate::EOPNOTSUPP);
        }
        return fchown(fd, true, owner);
    }
    owner.check(&if follow { stat(path)? } else { lstat(path)? })?;
    if crate::procfs::owns(&absolute_linux(path)) || crate::cgroup::owns(&absolute_linux(path)) {
        return Err(crate::EOPNOTSUPP);
    }
    let translated =
        owner.translated(crate::mount::overlay::ownership_mapping(path, follow)?.as_ref())?;
    let owner = &translated;
    let prepared = crate::mount::overlay::prepare_write(path, follow, false)?;
    let object = object::Object::open(&prepared, FILE_READ_ATTRIBUTES)?;
    set_ownership_handle(object.raw(), owner)
}
pub fn fchown(fd: i32, allow_path: bool, owner: &Ownership) -> Result<(), i32> {
    if crate::devpts::fchown(fd, owner)? {
        return Ok(());
    }
    if matches!(get(fd)?.kind, FdKind::PtySlave) {
        owner.check(&fstat(fd)?)?;
        return crate::tty::set_pty_ownership(fd, owner.uid, owner.gid);
    }
    if matches!(
        get(fd)?.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        if !allow_path && get(fd)?.flags.contains(FdFlags::PATH_ONLY) {
            return Err(EBADF);
        }
        return crate::tmpfs::fchown(fd, owner);
    }
    use std::os::windows::io::AsRawHandle;
    owner.check(&fstat(fd)?)?;
    if crate::pipe_inode::chown(fd, allow_path, owner)? {
        return Ok(());
    }
    let translated = owner.translated(crate::mount::overlay::descriptor_mapping(fd)?.as_ref())?;
    if let Some(handle) = crate::mount::overlay::metadata_handle(fd, true, allow_path)? {
        return set_ownership_handle(handle.as_raw_handle(), &translated);
    }
    let (object, _) = object::Object::from_fd_checked(fd, |entry| {
        if !allow_path && entry.flags.contains(FdFlags::PATH_ONLY) {
            return Err(crate::EBADF);
        }
        Ok(())
    })?;
    set_ownership_handle(object.raw(), owner)
}

pub(crate) fn set_mode_handle(handle: HANDLE, mode: u32) -> Result<(), i32> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx,
    };
    let object = object::Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)?;
    let _lock = crate::xattr::InodeLock::acquire(object.raw())?;
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            object.raw(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            std::mem::size_of_val(&basic) as u32,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let mut projection = FILE_BASIC_INFO {
        FileAttributes: basic.FileAttributes & !FILE_ATTRIBUTE_READONLY,
        ..unsafe { std::mem::zeroed() }
    };
    if projection.FileAttributes == 0 {
        projection.FileAttributes = 0x80;
    }
    let set = |value: &FILE_BASIC_INFO| -> Result<(), i32> {
        if unsafe {
            SetFileInformationByHandle(
                object.raw(),
                FileBasicInfo,
                (value as *const FILE_BASIC_INFO).cast(),
                std::mem::size_of_val(value) as u32,
            )
        } == 0
        {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(())
        }
    };
    // FILE_WRITE_EA is valid even on a native read-only inode. Do not briefly
    // grant host data-write access merely to change Linux metadata.
    inode::update(object.raw(), |record| {
        let kind = if mode & S_IFMT == 0 {
            record.mode.unwrap_or(0) & S_IFMT
        } else {
            mode & S_IFMT
        };
        record.mode = Some(kind | (mode & 0o7777));
        Ok(())
    })?;
    if mode & 0o200 == 0 {
        projection.FileAttributes |= FILE_ATTRIBUTE_READONLY;
    }
    set(&projection)
}

pub fn set_mode(path: &str, mode: u32) -> Result<(), i32> {
    if crate::tmpfs::chmod(path, mode)? {
        return Ok(());
    }
    let absolute = absolute_linux(path);
    if let Some(number) = pts_number(&absolute) {
        if !crate::tty::terminal_exists(number) {
            return Err(crate::ENOENT);
        }
        let current_stat = stat(path)?;
        let caller = crate::credentials::filesystem().uid;
        if caller != 0 && caller != current_stat.st_uid {
            return Err(crate::EPERM);
        }
        return crate::tty::set_terminal_mode(number, mode & 0o7777);
    }
    let resolved = crate::mount::overlay::prepare_write(path, true, false)?;
    set_mode_host_path(&resolved, mode)
}

/// Change the inode selected by a procfs magic link, without resolving its
/// display name. O_PATH references remain valid after rename, unlink or detach.
pub fn chmod_fd_link(path: &str, mode: u32) -> Result<bool, i32> {
    let path = absolute_linux(path);
    if crate::tmpfs::owns(&path) {
        return Ok(false);
    }
    let Some((pid, fd)) = crate::procfs::fd_magic_link(&path) else {
        return Ok(false);
    };
    let (canonical, _scope) = crate::procfs::instance::enter(&path)?;
    crate::procfs::metadata(&canonical)?;
    if pid != crate::job::process_id() {
        return Err(crate::EOPNOTSUPP);
    }
    chmod_descriptor(fd, mode, true)?;
    Ok(true)
}

/// Metadata-only descriptor operation, also used by fchmodat2(AT_EMPTY_PATH).
/// `allow_path` is false for fchmod, which must reject O_PATH descriptors.
pub fn chmod_descriptor(fd: i32, mode: u32, allow_path: bool) -> Result<(), i32> {
    use std::os::windows::io::AsRawHandle;
    let entry = get(fd)?;
    if !allow_path && entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(crate::EBADF);
    }
    if entry.flags.contains(FdFlags::PROC_SYMLINK) {
        return Err(crate::EOPNOTSUPP);
    }
    if matches!(
        entry.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return crate::tmpfs::chmod_descriptor(fd, mode, allow_path);
    }
    if crate::devpts::fchmod(fd, mode)?
        || crate::pipe_inode::chmod_descriptor(fd, mode, allow_path)?
    {
        return Ok(());
    }
    if matches!(entry.kind, FdKind::PtySlave) {
        let current_stat = fstat(fd)?;
        let caller = crate::credentials::filesystem().uid;
        if caller != 0 && caller != current_stat.st_uid {
            return Err(crate::EPERM);
        }
        return crate::tty::set_pty_mode(fd, mode & 0o7777);
    }
    let stat = fstat(fd)?;
    if stat.st_mode & S_IFMT == S_IFLNK {
        return Err(crate::EOPNOTSUPP);
    }
    let caller = crate::credentials::filesystem().uid;
    if caller != 0 && caller != stat.st_uid {
        return Err(crate::EPERM);
    }
    if let Some(handle) = crate::mount::overlay::metadata_handle(fd, true, allow_path)? {
        return set_mode_handle(handle.as_raw_handle(), mode);
    }
    let (object, _) = object::Object::from_fd_checked(fd, |entry| {
        if !allow_path && entry.flags.contains(FdFlags::PATH_ONLY) {
            return Err(crate::EBADF);
        }
        Ok(())
    })?;
    set_mode_handle(object.raw(), mode)
}

/// Creates a filesystem FIFO inode without opening either data endpoint.
pub fn create_fifo(path: &str, mode: u32) -> Result<(), i32> {
    if crate::tmpfs::create(path, S_IFIFO | (mode & 0o7777), 0, "")? {
        return Ok(());
    }
    let mut resolved = crate::mount::overlay::prepare_create(path)?;
    let wide_path = wide(&resolved)?;
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            GENERIC_WRITE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            CREATE_NEW,
            0,
            ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let initialized = resolved.initialize_created_handle(handle, S_IFIFO | (mode & 0o7777));
    unsafe { CloseHandle(handle) };
    if let Err(error) = initialized {
        // Creation is transactional: a failed type record must not leave a
        // regular file at a name the caller requested as a FIFO.
        let _ = std::fs::remove_file(&resolved);
        return Err(error);
    }
    resolved.finish()
}

/// Creates a block or character device inode and preserves its Linux `dev_t`.
pub fn create_device(path: &str, mode: u32, device: u64) -> Result<(), i32> {
    if crate::tmpfs::create(path, mode, device, "")? {
        return Ok(());
    }
    let file_type = mode & S_IFMT;
    if !matches!(file_type, S_IFCHR | S_IFBLK) {
        return Err(EINVAL);
    }
    let mut resolved = crate::mount::overlay::prepare_create(path)?;
    let wide_path = wide(&resolved)?;
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            GENERIC_WRITE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            CREATE_NEW,
            0,
            ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    unsafe { CloseHandle(handle) };
    let result = (|| {
        let object = object::Object::open(&resolved, FILE_READ_ATTRIBUTES)?;
        inode::replace(
            object.raw(),
            &inode::Record {
                mode: Some(file_type | (mode & 0o7777)),
                device: Some(device),
                symlink: None,
                ..inode::Record::default()
            },
        )?;
        resolved.initialize_created_handle(object.raw(), file_type | (mode & 0o7777))
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&resolved);
        return Err(error);
    }
    resolved.finish()
}

/// Collects file information from an open handle.
pub(crate) fn stat_handle(handle: HANDLE, symlink: bool) -> Result<Stat, i32> {
    let query = object::Object::reopen(handle, FILE_READ_ATTRIBUTES | FILE_READ_EA)?;
    stat_with_query(handle, &query, symlink)
}

// `query` is a private asynchronous metadata open of the same inode as
// `handle`. Reuse it for both EAs, keeping the ACL rights of the original open.
fn stat_with_query(handle: HANDLE, query: &object::Object, symlink: bool) -> Result<Stat, i32> {
    // SAFETY: `info` is a writable local and the handle is live.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle carries FILE_READ_ATTRIBUTES access.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let readonly = info.dwFileAttributes & FILE_ATTRIBUTE_READONLY != 0;
    let record = inode::read_object(query)?;
    let mode = record
        .mode
        .or_else(|| permissions_from_acl(handle, readonly));
    let target = match record.symlink {
        Some(target) => Some(target),
        None if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
            native_symlink_target_handle(handle)?
        }
        None => None,
    };
    let symlink = symlink || target.is_some();
    let mut stat = stat_from_info(&info, symlink, mode);
    stat.st_uid = record.uid.unwrap_or(0);
    stat.st_gid = record.gid.unwrap_or(0);
    if stat.st_mode & S_IFMT == S_IFREG && !symlink {
        stat.st_size = verity::authoritative_size_object(query)? as i64;
    }
    if let Some(target) = target {
        stat.st_mode = S_IFLNK | 0o777;
        stat.st_size = target.len() as i64;
    }
    if mode.is_some_and(|mode| matches!(mode & S_IFMT, S_IFCHR | S_IFBLK)) {
        stat.st_rdev = record.device.ok_or(EIO)?;
        stat.st_size = 0;
        stat.st_blocks = 0;
    }
    Ok(stat)
}

fn stored_mode_handle(handle: HANDLE) -> Result<Option<u32>, i32> {
    Ok(inode::read(handle)?.mode)
}

pub(crate) fn symlink_target_handle(handle: HANDLE) -> Result<Option<String>, i32> {
    Ok(inode::read(handle)?.symlink)
}

pub(crate) fn readlink_handle(handle: HANDLE) -> Result<String, i32> {
    if let Some(target) = symlink_target_handle(handle)? {
        return Ok(target);
    }
    native_symlink_target_handle(handle)?.ok_or(EINVAL)
}

fn native_symlink_target_handle(handle: HANDLE) -> Result<Option<String>, i32> {
    reparse::read(handle)?
        .map(|target| target.guest(&crate::path::system_root().map_err(path_errno)?))
        .transpose()
}

/// Read a final link from its opened inode, including native Windows links.
pub fn read_link_host_path(path: &Path) -> Result<String, i32> {
    let object = object::Object::open(path, FILE_READ_ATTRIBUTES)?;
    readlink_handle(object.raw())
}

/// The empty-path form of Linux readlinkat. O_PATH is valid here. A valid
/// non-link (including AT_FDCWD's current directory) returns ENOENT, whereas a
/// bad descriptor returns EBADF. No /proc or current-name reconstruction occurs.
pub fn read_link_fd(fd: i32) -> Result<String, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    if fd == AT_FDCWD {
        return Err(ENOENT);
    }
    if matches!(
        get(fd)?.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return crate::tmpfs::read_link_fd(fd);
    }
    let entry = crate::get(fd)?;
    if entry.flags.contains(FdFlags::PROC_SYMLINK) {
        let table = crate::synthetic().read().map_err(|_| EIO)?;
        let (_, bytes) = table
            .get(&fd)
            .filter(|(generation, _)| *generation == entry.generation)
            .ok_or(crate::EBADF)?;
        return String::from_utf8(bytes.clone()).map_err(|_| EIO);
    }
    let object = match object::Object::from_fd(fd) {
        Err(crate::EOPNOTSUPP) => return Err(ENOENT),
        result => result?,
    };
    readlink_handle(object.raw()).map_err(|error| if error == EINVAL { ENOENT } else { error })
}

/// Opens a path purely to read its metadata.
///
/// `FILE_READ_ATTRIBUTES` with backup semantics works for both files and
/// directories, and never fails because the file is in use for writing.
/// Builds a `Stat` for a synthetic `/proc` path.
fn stat_procfs(path: &str, follow_symlinks: bool) -> Result<Stat, i32> {
    use crate::procfs::ProcKind;
    let (view_path, _scope) = crate::procfs::instance::enter(path)?;
    let path = view_path.as_str();
    let metadata = crate::procfs::metadata(path)?;
    if follow_symlinks && let Some((pid, fd)) = crate::procfs::fd_magic_link(path) {
        if pid == crate::job::process_id() {
            return fstat(fd);
        }
    }
    // Following a symlink means reporting the target instead.
    if follow_symlinks && metadata.kind == ProcKind::Symlink {
        if let Some(inode) = crate::procfs::namespace_target_inode(path) {
            // Namespace entries are procfs magic links. Following one yields an
            // nsfs file descriptor object; the `kind:[inode]` text returned by
            // readlink is only a display name and is not a resolvable pathname.
            return Ok(Stat {
                st_dev: 4,
                st_ino: inode,
                st_nlink: 1,
                st_mode: S_IFREG | 0o444,
                st_blksize: 4096,
                ..Stat::default()
            });
        }
        if let Some(target) = metadata.target.as_deref() {
            // `/proc/self/exe` and `/proc/self/fd/*` point at ordinary guest
            // paths, so they retain the usual symlink-following behaviour.
            return stat_path(target, true);
        }
    }
    let (format, permissions) = match metadata.kind {
        ProcKind::Directory => (S_IFDIR, 0o555),
        ProcKind::Symlink => (S_IFLNK, 0o777),
        ProcKind::File => (
            S_IFREG,
            if crate::procfs::writable(path) {
                0o644
            } else {
                0o444
            },
        ),
    };
    Ok(Stat {
        // A distinct synthetic device number keeps procfs inodes from colliding
        // with real ones in callers that key on (st_dev, st_ino).
        st_dev: crate::procfs::instance::current().map_or(0x70726f63, |v| v.device()),
        st_ino: procfs_inode(path),
        st_nlink: crate::procfs::directory_links(path).unwrap_or(1),
        st_mode: format | permissions,
        st_size: metadata.size as i64,
        st_blksize: 4096,
        st_blocks: 0,
        ..Stat::default()
    })
}

/// Derives a stable inode number for a `/proc` path.
///
/// Real procfs inodes come from the kernel's own numbering, which has no
/// equivalent here. Hashing the path gives a value that is at least stable
/// across calls, which is what callers caching by inode need.
fn procfs_inode(path: &str) -> u64 {
    let canonical = crate::procfs::canonical_path(path);
    let path = canonical.as_str();
    // PROC_ROOT_INO is an ABI invariant of the live procfs root.
    if path.trim_end_matches('/') == "/proc"
        || crate::procfs::instance::enter(path).is_ok_and(|(p, _)| p == "/proc")
    {
        return 1;
    }
    // FNV-1a: small, deterministic, and adequate for identity here.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Zero is reserved for "no inode" by some callers.
    hash | (1u64 << 63)
}

fn stat_path(path: &str, follow_symlinks: bool) -> Result<Stat, i32> {
    stat_path_resolved(path, follow_symlinks, None)
}

fn stat_path_resolved(
    path: &str,
    follow_symlinks: bool,
    resolved: Option<std::path::PathBuf>,
) -> Result<Stat, i32> {
    if let Some(value) = crate::tmpfs::stat(path, follow_symlinks)? {
        return Ok(value);
    }
    let absolute = absolute_linux(path);
    if crate::procfs::owns(&absolute) {
        return stat_procfs(&absolute, follow_symlinks);
    }
    if crate::cgroup::owns(&absolute) {
        let metadata = crate::cgroup::metadata(&absolute)?;
        let (format, permissions) = match metadata.kind {
            crate::procfs::ProcKind::Directory => (S_IFDIR, 0o755),
            crate::procfs::ProcKind::Symlink => (S_IFLNK, 0o777),
            crate::procfs::ProcKind::File => (S_IFREG, 0o644),
        };
        return Ok(Stat {
            st_dev: 0x63677270,
            st_ino: procfs_inode(&absolute),
            st_nlink: 1,
            st_mode: format | permissions,
            st_size: metadata.size as i64,
            st_blksize: 4096,
            st_blocks: 0,
            ..Stat::default()
        });
    }
    match absolute.as_str() {
        "/dev/stdin" => return stat_procfs("/proc/self/fd/0", follow_symlinks),
        "/dev/stdout" => return stat_procfs("/proc/self/fd/1", follow_symlinks),
        "/dev/stderr" => return stat_procfs("/proc/self/fd/2", follow_symlinks),
        "/dev/fd" => return stat_procfs("/proc/self/fd", follow_symlinks),
        path if path.starts_with("/dev/fd/") => {
            return stat_procfs(&format!("/proc/self/fd/{}", &path[8..]), follow_symlinks);
        }
        "/dev/core" => return stat_procfs("/proc/kcore", follow_symlinks),
        "/dev/null" => {
            return Ok(Stat {
                st_mode: S_IFCHR | 0o666,
                st_rdev: (1 << 8) | 3,
                st_blksize: 4096,
                st_nlink: 1,
                ..Stat::default()
            });
        }
        "/dev/zero" | "/dev/full" => {
            return Ok(Stat {
                st_mode: S_IFCHR | 0o666,
                st_rdev: (1 << 8) | 5,
                st_blksize: 4096,
                st_nlink: 1,
                ..Stat::default()
            });
        }
        "/dev/urandom" | "/dev/random" => {
            return Ok(Stat {
                st_mode: S_IFCHR | 0o666,
                st_rdev: (1 << 8) | 9,
                st_blksize: 4096,
                st_nlink: 1,
                ..Stat::default()
            });
        }
        "/dev" | "/dev/shm" | "/dev/pts" | "/dev/snd" => {
            return Ok(Stat {
                st_mode: S_IFDIR | 0o755,
                st_blksize: 4096,
                st_nlink: 2,
                ..Stat::default()
            });
        }
        // The pty multiplexer, at the device number Linux gives it. A program
        // that stats the path before opening it — and several do, to decide
        // between `/dev/ptmx` and `/dev/ptc` — must see a character device.
        "/dev/ptmx" | "/dev/pts/ptmx" => {
            return Ok(Stat {
                st_mode: S_IFCHR | 0o666,
                st_rdev: (5 << 8) | 2,
                st_blksize: 1024,
                st_nlink: 1,
                ..Stat::default()
            });
        }
        "/dev/tty" => {
            return Ok(Stat {
                st_mode: S_IFCHR | 0o666,
                st_rdev: (5 << 8),
                st_blksize: 1024,
                st_nlink: 1,
                ..Stat::default()
            });
        }
        _ => {}
    }
    // A pty slave exists only while its terminal does, so this is a real
    // existence check rather than a fabricated entry: `stat("/dev/pts/7")` on a
    // terminal nobody opened reports ENOENT, as it would on Linux.
    if let Some(number) = pts_number(&absolute) {
        return if crate::tty::terminal_exists(number) {
            let (uid, gid, mode) = crate::tty::terminal_stat(number).unwrap_or((0, 0, 0o620));
            Ok(Stat {
                st_mode: S_IFCHR | (mode & 0o7777),
                st_uid: uid,
                st_gid: gid,
                // Linux numbers pty slaves in the UNIX98 major, 136.
                st_rdev: (136 << 8) | u64::from(number),
                st_blksize: 1024,
                st_nlink: 1,
                st_ino: 3 + u64::from(number),
                ..Stat::default()
            })
        } else {
            Err(ENOENT)
        };
    }
    let resolved = match resolved {
        Some(path) => path,
        None if follow_symlinks => resolve(path)?,
        None => resolve_no_follow(path)?,
    };
    if absolute == "/etc/resolv.conf" && !resolved.exists() {
        let content = synthetic_resolv_conf();
        return Ok(Stat {
            st_mode: S_IFREG | 0o644,
            st_size: content.len() as i64,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        });
    }
    if absolute == "/etc/environment" && !resolved.exists() {
        let content = synthetic_environment();
        return Ok(Stat {
            st_mode: S_IFREG | 0o644,
            st_size: content.len() as i64,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        });
    }
    if absolute == "/etc/hosts" && !resolved.exists() {
        return Ok(Stat {
            st_mode: S_IFREG | 0o644,
            st_size: 34,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        });
    }
    let wide_path = wide(&resolved)?;
    let mut attributes = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED;
    if !follow_symlinks {
        attributes |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    // Ask for READ_CONTROL so mode bits can be projected from the DACL. Linux
    // `stat` itself does not require permission to read an ACL, though, so retry
    // with metadata-only access when the host denies that extra right.
    // SAFETY: `wide_path` is null-terminated and outlives both calls.
    let mut handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES | FILE_READ_EA | SYNCHRONIZE | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            attributes,
            ptr::null_mut(),
        )
    };
    if (handle.is_null() || handle as isize == -1)
        && unsafe { GetLastError() } == ERROR_ACCESS_DENIED
    {
        handle = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                FILE_READ_ATTRIBUTES | FILE_READ_EA | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                attributes,
                ptr::null_mut(),
            )
        };
    }
    if handle.is_null() || handle as isize == -1 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }

    // Type and target size come from this inode, not a preceding path query
    // that another thread could replace before CreateFileW succeeds.
    let query = object::Object::owned(handle)?;
    stat_with_query(query.raw(), &query, false)
}

/// `stat`: follows symlinks.
pub fn stat(path: &str) -> Result<Stat, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    if let Some(stat) = crate::tmpfs::stat(path, true)? {
        return Ok(stat);
    }
    if crate::mount::proc_location(path)?.is_some() {
        return stat_path(path, true);
    }
    if let Some(stat) = cwd::stat_dot(path) {
        return stat;
    }
    match crate::mount::overlay::stat_resolution(path, true)? {
        Some(crate::mount::overlay::StatResolution::Overlay(stat)) => return Ok(stat),
        Some(crate::mount::overlay::StatResolution::Native(native)) => {
            return stat_path_resolved(path, true, Some(native));
        }
        None => {}
    }
    stat_path(path, true)
}

/// `lstat`: reports the symlink itself.
pub fn lstat(path: &str) -> Result<Stat, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    if let Some(stat) = crate::tmpfs::stat(path, false)? {
        return Ok(stat);
    }
    if crate::mount::proc_location(path)?.is_some() {
        return stat_path(path, false);
    }
    match crate::mount::overlay::stat_resolution(path, false)? {
        Some(crate::mount::overlay::StatResolution::Overlay(stat)) => return Ok(stat),
        Some(crate::mount::overlay::StatResolution::Native(native)) => {
            return stat_path_resolved(path, false, Some(native));
        }
        None => {}
    }
    stat_path(path, false)
}

/// `fstat`: metadata for an open descriptor.
pub fn fstat(fd: i32) -> Result<Stat, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    let entry = get(fd)?;
    if let Some(stat) = crate::devpts::fstat(fd)? {
        return Ok(stat);
    }
    if matches!(
        entry.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return crate::tmpfs::fstat(fd);
    }
    if let Some(stat) = crate::mount::overlay::fstat(entry)? {
        return Ok(stat);
    }
    match entry.kind {
        FdKind::FsContext | FdKind::MountTree => crate::mount::api::stat(fd),
        // Pipes, consoles and sockets have no file information to query, so
        // synthesize the character-device or FIFO shape the guest expects.
        FdKind::Console => Ok(Stat {
            st_mode: S_IFCHR | 0o620,
            st_blksize: 1024,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::Null => Ok(Stat {
            st_mode: S_IFCHR | 0o666,
            st_rdev: (1 << 8) | 3,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::Zero => Ok(Stat {
            st_mode: S_IFCHR | 0o666,
            st_rdev: (1 << 8) | 5,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::Full => Ok(Stat {
            st_mode: S_IFCHR | 0o666,
            st_rdev: (1 << 8) | 7,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::Random => Ok(Stat {
            st_mode: S_IFCHR | 0o666,
            st_rdev: (1 << 8) | 9,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::Fifo => crate::fifo::metadata(fd, entry),
        FdKind::Pipe => Ok(crate::pipe_inode::metadata(fd)?.unwrap_or(Stat {
            st_mode: S_IFIFO | 0o600,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        })),
        // Both ends of a pty are character devices, at the numbers Linux uses:
        // 5:2 for the multiplexer, 136:N for a UNIX98 slave. Falling through to
        // `stat_handle` would report the readiness event's file information and
        // tell a caller its terminal was a regular file.
        FdKind::PtyMaster => Ok(Stat {
            st_mode: S_IFCHR | 0o666,
            st_rdev: (5 << 8) | 2,
            st_blksize: 1024,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::PtySlave => {
            let number = crate::tty::pty_number(fd).unwrap_or(0);
            let (uid, gid, mode) = crate::tty::pty_stat(fd).unwrap_or((0, 0, 0o620));
            Ok(Stat {
                st_mode: S_IFCHR | (mode & 0o7777),
                st_uid: uid,
                st_gid: gid,
                st_rdev: (136 << 8) | u64::from(number),
                st_blksize: 1024,
                st_nlink: 1,
                st_ino: 3 + u64::from(number),
                ..Stat::default()
            })
        }
        FdKind::Socket | FdKind::UnixSocket | FdKind::NetlinkSocket => Ok(Stat {
            st_mode: S_IFSOCK | 0o600,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::BpfProgram => Ok(Stat {
            // Linux exposes BPF objects as anonymous inodes through procfs.
            st_mode: S_IFREG | 0o600,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        // A synthetic file's size lives in the side table, not in a handle.
        FdKind::TimerFd => Ok(Stat {
            st_mode: 0o600,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::Namespace => Ok(Stat {
            st_dev: 4,
            st_ino: crate::namespaces::descriptor_inode(fd)?,
            st_mode: S_IFREG | 0o444,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::UserNamespace => Ok(Stat {
            st_dev: 4,
            st_ino: crate::user_namespace::descriptor_inode(fd)?,
            st_mode: S_IFREG | 0o444,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::TimeNamespace => Ok(Stat {
            st_dev: 4,
            st_ino: crate::time_namespace::descriptor_inode(fd)?,
            st_mode: S_IFREG | 0o444,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::MountNamespace => Ok(Stat {
            st_dev: 4,
            st_ino: crate::mount::namespace_descriptor_inode(fd)?,
            st_mode: S_IFREG | 0o444,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::Synthetic
            if crate::synthetic_file_path(fd)
                .is_ok_and(|p| crate::procfs::pinned(|| crate::procfs::owns(&p))) =>
        {
            crate::procfs::pinned(|| {
                let mut stat = stat_procfs(
                    &crate::synthetic_file_path(fd)?,
                    !entry.flags.contains(FdFlags::PROC_SYMLINK),
                )?;
                if entry.flags.contains(FdFlags::PROC_SYMLINK) {
                    stat.st_mode = S_IFLNK | 0o777;
                    stat.st_size = crate::synthetic_size(fd)? as i64;
                }
                Ok(stat)
            })
        }
        FdKind::ProcSysctl
            if crate::proc_sysctl_file_path(fd)
                .is_ok_and(|p| crate::procfs::pinned(|| crate::procfs::owns(&p))) =>
        {
            crate::procfs::pinned(|| stat_procfs(&crate::proc_sysctl_file_path(fd)?, true))
        }
        FdKind::SyntheticDirectory
            if crate::synthetic_directory_path(fd)
                .is_ok_and(|p| crate::procfs::pinned(|| crate::procfs::owns(&p))) =>
        {
            crate::procfs::pinned(|| stat_procfs(&crate::synthetic_directory_path(fd)?, true))
        }
        FdKind::CgroupFile => stat_path(&crate::cgroup_file_path(fd)?, true),
        FdKind::SyntheticDirectory
            if crate::synthetic_directory_path(fd).is_ok_and(|p| crate::cgroup::owns(&p)) =>
        {
            stat_path(&crate::synthetic_directory_path(fd)?, true)
        }
        FdKind::Synthetic => Ok(Stat {
            st_mode: S_IFREG
                | if entry.kind == FdKind::CgroupFile {
                    0o644
                } else {
                    0o444
                },
            st_size: crate::synthetic_size(fd)? as i64,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::ProcSysctl => Ok(Stat {
            st_mode: S_IFREG | 0o644,
            // Linux proc-sysctl inodes report zero, even though reads produce
            // the current textual value.
            st_size: 0,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        FdKind::SyntheticDirectory => Ok(Stat {
            st_mode: S_IFDIR | 0o555,
            st_blksize: 4096,
            st_nlink: 1,
            ..Stat::default()
        }),
        _ => stat_handle(entry.raw as HANDLE, false),
    }
}

/// Removes a file, detaching it from the namespace even while it stays open.
///
/// `FILE_DISPOSITION_FLAG_POSIX_SEMANTICS` is what makes this behave like
/// POSIX: the name disappears immediately and the data lives until the last
/// descriptor closes. Native support for these semantics is required; a failed
/// operation never changes to delayed deletion or changes the file attributes.
pub fn unlink(path: &str) -> Result<(), i32> {
    if crate::tmpfs::unlink(path, false)? {
        return Ok(());
    }
    if crate::mount::overlay::remove(path, false)?.is_some() {
        return Ok(());
    }
    let _mount_writer = crate::mount::native::write_path(path, false)?;
    let resolved = resolve_no_follow(path)?;
    if resolved.is_dir() {
        return Err(EISDIR);
    }
    let wide_path = wide(&resolved)?;

    // DELETE access plus FILE_SHARE_DELETE so other open descriptors survive.
    // SAFETY: `wide_path` is null-terminated and outlives the call.
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle.is_null() || handle as isize == -1 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }

    let result = unlink_inode(handle);

    // SAFETY: this function owns the handle it opened.
    unsafe { CloseHandle(handle) };
    result
}

/// Creates the placeholder file that marks a bound `AF_UNIX` socket path.
///
/// The emulated socket lives in the named pipe namespace, so this file carries no
/// data and is never opened for I/O. It exists so the guest can `stat` the path,
/// see it in a directory listing, and `unlink` it the way it would on Linux.
///
/// Reports `EEXIST` when something is already there, leaving the caller to decide
/// whether that is a live socket or a leftover.
pub fn create_socket_placeholder(path: &str) -> Result<(), i32> {
    create_socket_inode(path).map(drop)
}

/// Return a pin to exactly the created inode. Retaining a FILE_SHARE_DELETE
/// handle allows unlink while preventing file-id reuse by another listener.
pub(crate) fn create_socket_inode(path: &str) -> Result<std::os::windows::io::OwnedHandle, i32> {
    use std::os::windows::io::FromRawHandle;
    let mut resolved = crate::mount::overlay::prepare_create(path)?;
    let wide_path = wide(&resolved)?;
    // CREATE_NEW is what makes the EEXIST distinction, and sharing delete lets
    // another process unlink the path while this socket is still bound.
    // SAFETY: `wide_path` is null-terminated and outlives the call.
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            CREATE_NEW,
            0,
            ptr::null_mut(),
        )
    };
    if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        // SAFETY: GetLastError has no preconditions.
        return Err(match unsafe { GetLastError() } {
            // ERROR_FILE_EXISTS and ERROR_ALREADY_EXISTS both mean occupied.
            80 | 183 => crate::EEXIST,
            other => errno_from_win32(other),
        });
    }
    let owned = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(handle.cast()) };
    // The inode type is persistent filesystem metadata, not process-local socket
    // state. Recording it in the same NTFS stream used by chmod makes `stat` in
    // every process report S_IFSOCK, including a parent waiting for a child
    // daemon to publish its endpoint.
    if let Err(error) = resolved.initialize_created_handle(handle, S_IFSOCK | 0o777) {
        // Creation is transactional: never leave a regular file behind for a
        // bind that failed to establish a socket inode.
        unlink_inode(handle)?;
        return Err(error);
    }
    resolved.finish()?;
    Ok(owned)
}

/// Remove this name without changing inode attributes. Linux unlink depends
/// on directory/mount permissions, not the target's write bits. Existing mount
/// write guards and native DELETE access still apply.
pub(crate) fn unlink_inode(handle: HANDLE) -> Result<(), i32> {
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE
            | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
            | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    if unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfoEx,
            (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
            size_of_val(&disposition) as u32,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(())
}

/// Creates a directory. The Linux `mode` has no Windows equivalent.
pub fn mkdir(path: &str, mode: u32) -> Result<(), i32> {
    if crate::tmpfs::create(
        path,
        S_IFDIR | (mode & 0o7777 & !crate::fs_context::umask()),
        0,
        "",
    )? {
        return Ok(());
    }
    let absolute = absolute_linux(path);
    if crate::cgroup::owns(&absolute) {
        return crate::cgroup::create_directory(&absolute);
    }
    let mut resolved = crate::mount::overlay::prepare_create(path)?;
    let wide_path = wide(&resolved)?;
    // SAFETY: `wide_path` is null-terminated and outlives the call.
    if unsafe { CreateDirectoryW(wide_path.as_ptr(), ptr::null()) } == 0 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    if let Err(error) = resolved.initialize_created(S_IFDIR | (mode & 0o7777)) {
        let _ = std::fs::remove_dir(&*resolved);
        return Err(error);
    }
    resolved.finish()
}

/// Removes an empty directory.
pub fn rmdir(path: &str) -> Result<(), i32> {
    if crate::tmpfs::unlink(path, true)? {
        return Ok(());
    }
    if crate::mount::overlay::remove(path, true)?.is_some() {
        return Ok(());
    }
    let absolute = absolute_linux(path);
    if crate::cgroup::owns(&absolute) {
        return crate::cgroup::remove_directory(&absolute);
    }
    let _mount_writer = crate::mount::native::write_path(path, false)?;
    let resolved = resolve_no_follow(path)?;
    if resolved.is_file() {
        return Err(ENOTDIR);
    }
    let wide_path = wide(&resolved)?;
    // SAFETY: `wide_path` is null-terminated and outlives the call.
    if unsafe { RemoveDirectoryW(wide_path.as_ptr()) } == 0 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(())
}

/// Renames a path with Linux replacement semantics.
pub fn rename(from: &str, to: &str) -> Result<(), i32> {
    rename_with_flags(from, to, 0)
}

pub fn rename_with_flags(from: &str, to: &str, flags: u32) -> Result<(), i32> {
    if crate::tmpfs::rename(from, to, flags)? {
        return Ok(());
    }
    if flags & !7 != 0 || flags & 2 != 0 && flags & 5 != 0 {
        return Err(EINVAL);
    }
    if crate::mount::overlay::rename_with_flags(from, to, flags)?.is_some() {
        return Ok(());
    }
    if flags & 6 != 0 {
        return Err(crate::EOPNOTSUPP);
    }
    let _source_writer = crate::mount::native::write_path(from, false)?;
    let _target_writer = crate::mount::native::write_path(to, false)?;
    let source = wide(&resolve_no_follow(from)?)?;
    let target = resolve_no_follow(to)?;

    // Package managers keep the old cache memory-mapped while atomically
    // publishing its replacement. FILE_RENAME_INFO_EX with POSIX semantics
    // permits that Linux pattern; MoveFileExW alone rejects it with EACCES.
    let source_handle = unsafe {
        CreateFileW(
            source.as_ptr(),
            DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if source_handle.is_null() || source_handle as isize == -1 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    // SAFETY: this function owns the DELETE-capable source handle.
    let result = unsafe { rename_host_handle(source_handle, &target, flags & 1 == 0) };
    unsafe { CloseHandle(source_handle) };
    result
}

/// Rename the same open inode, optionally refusing to replace any destination.
/// Internal filesystems use no-replace publication for fully staged objects.
///
/// # Safety
/// `source_handle` must remain live and carry DELETE access for the call.
pub(crate) unsafe fn rename_host_handle(
    source_handle: HANDLE,
    target: &Path,
    replace: bool,
) -> Result<(), i32> {
    let (mut storage, bytes) = rename_information(&wide(target)?, replace)?;
    // SAFETY: the aligned buffer includes the target's terminating WCHAR.
    let renamed = unsafe {
        SetFileInformationByHandle(
            source_handle,
            FileRenameInfoEx,
            storage.as_mut_ptr().cast(),
            bytes,
        )
    };
    // CloseHandle must not overwrite the rename's actual failure. An
    // unsupported POSIX rename is an error, not a different replacement mode.
    let error = if renamed == 0 {
        Some(unsafe { GetLastError() })
    } else {
        None
    };
    match error {
        Some(error) => Err(errno_from_win32(error)),
        None => Ok(()),
    }
}

/// Rename into the same open parent directory, even if its pathname changes.
/// Unlike the Win32 DOS-path wrapper, the NT request honors RootDirectory.
///
/// # Safety
/// Source and parent must stay live. The source is a private DELETE/SYNCHRONIZE
/// handle with no other pending I/O; the parent permits traversal. Cancellation
/// belongs to this operation, and guest handlers must run only after unwinding.
pub(crate) unsafe fn rename_host_handle_relative(
    source: HANDLE,
    parent: HANDLE,
    name: &OsStr,
    replace: bool,
) -> Result<(), i32> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetInformationFile(
            file: HANDLE,
            io: *mut NativeIoStatus,
            buffer: *const u8,
            length: u32,
            class: u32,
        ) -> i32;
    }
    let name: Vec<u16> = name.encode_wide().chain(Some(0)).collect();
    if name[..name.len() - 1]
        .iter()
        .any(|&c| matches!(c, 47 | 92 | 58))
        || name == [46, 0]
        || name == [46, 46, 0]
    {
        return Err(EINVAL);
    }
    let interrupt = crate::interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    let (mut buffer, length) = rename_information(&name, replace)?;
    unsafe { (*buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>()).RootDirectory = parent };
    let mut io = NativeIoStatus::default();
    let result =
        unsafe { NtSetInformationFile(source, &mut io, buffer.as_ptr().cast(), length, 65) };
    unsafe { complete_native_io(source, &mut io, result) }
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct NativeIoStatus {
    pub(crate) status: usize,
    pub(crate) information: usize,
}

/// Retire a native file request before its caller releases any kernel buffers.
/// # Safety
/// `file` is a private, live SYNCHRONIZE-capable handle with no other pending
/// I/O. `io` and every request buffer must stay live until this function returns.
/// Signal handlers belong to the outer syscall boundary, after unwinding.
pub(crate) unsafe fn complete_native_io(
    file: HANDLE,
    io: &mut NativeIoStatus,
    result: i32,
) -> Result<(), i32> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }
    let result = unsafe { complete_native_status(file, io, result)? };
    if result < 0 {
        Err(errno_from_win32(unsafe { RtlNtStatusToDosError(result) }))
    } else {
        Ok(())
    }
}

/// Retire a request while preserving native statuses needed by record parsers.
/// # Safety
/// Same handle and buffer lifetime requirements as `complete_native_io`.
pub(crate) unsafe fn complete_native_status(
    file: HANDLE,
    io: &mut NativeIoStatus,
    mut result: i32,
) -> Result<i32, i32> {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::IO::CancelIoEx;
    use windows_sys::Win32::System::Threading::{
        INFINITE, WaitForMultipleObjects, WaitForSingleObject,
    };
    if result == 0x103 {
        // STATUS_PENDING
        let interrupt = crate::interrupt::current();
        let handles = [file, interrupt];
        let unavailable = interrupt.is_null();
        let mut cancelling = unavailable;
        if unavailable {
            unsafe { CancelIoEx(file, ptr::null()) };
        }
        loop {
            crate::signal::register_waiter();
            if !cancelling && crate::signal::interrupt_pending() {
                cancelling = true;
                unsafe { CancelIoEx(file, ptr::null()) };
            }
            let waited = unsafe {
                WaitForMultipleObjects(
                    if cancelling { 1 } else { 2 },
                    handles.as_ptr(),
                    0,
                    INFINITE,
                )
            };
            crate::signal::unregister_waiter();
            if waited == WAIT_OBJECT_0 {
                result = unsafe { ptr::read_volatile(&io.status) } as i32;
                if unavailable {
                    return Err(EIO);
                }
                break;
            }
            if waited == WAIT_OBJECT_0 + 1 {
                // An auto-reset wake for a blocked/stale signal is not EINTR.
                continue;
            }
            unsafe { CancelIoEx(file, ptr::null()) };
            let retired = unsafe { WaitForSingleObject(file, INFINITE) };
            if retired != WAIT_OBJECT_0 {
                // Both handles are privately owned and cannot legally become
                // invalid. Never unwind a buffer still owned by the kernel.
                std::process::abort();
            }
            return Err(EIO);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod native_io_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtReadFile(
            file: HANDLE,
            event: HANDLE,
            apc: *const u8,
            context: *const u8,
            io: *mut NativeIoStatus,
            buffer: *mut u8,
            length: u32,
            offset: *const i64,
            key: *const u32,
        ) -> i32;
    }
    struct Pipe(usize, usize);
    impl Pipe {
        fn new() -> Self {
            let (read, write) = crate::platform::create_overlapped_pipe_pair(4096).unwrap();
            Self(read, write)
        }
        fn write(&self) -> std::thread::JoinHandle<()> {
            let write = self.1;
            std::thread::spawn(move || {
                let entry = crate::FdEntry {
                    raw: write,
                    kind: FdKind::Pipe,
                    flags: FdFlags::OVERLAPPED,
                    generation: 0,
                    description_id: 0,
                    offset: 0,
                };
                let mut data = [b'q'];
                assert_eq!(
                    unsafe { crate::platform::transfer_once(&entry, data.as_mut_ptr(), 1, false) },
                    Ok(1)
                );
            })
        }
    }
    impl Drop for Pipe {
        fn drop(&mut self) {
            crate::platform::close_raw(self.0);
            crate::platform::close_raw(self.1);
        }
    }
    unsafe fn read(pipe: &Pipe, io: &mut NativeIoStatus, byte: &mut u8) -> i32 {
        unsafe {
            NtReadFile(
                pipe.0 as HANDLE,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                io,
                byte,
                1,
                ptr::null(),
                ptr::null(),
            )
        }
    }
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "sysv64" fn handler(_: i32) {
        CALLS.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn pending_native_request_retires_on_the_file_handle() {
        let pipe = Pipe::new();
        let mut io = NativeIoStatus::default();
        let mut byte = 0;
        let status = unsafe { read(&pipe, &mut io, &mut byte) };
        assert_eq!(status, 0x103);
        let writer = pipe.write();
        assert_eq!(
            unsafe { complete_native_io(pipe.0 as HANDLE, &mut io, status) },
            Ok(())
        );
        writer.join().unwrap();
        assert_eq!(io.information, 1);
        assert_eq!(byte, b'q');
    }

    fn signal_case(blocked: bool) {
        let _lock = crate::signal::test_lock();
        CALLS.store(0, Ordering::SeqCst);
        assert!(!crate::interrupt::current().is_null());
        let old = crate::signal::sigaction(
            12,
            Some(crate::signal::Action {
                disposition: crate::signal::Disposition::Handle(handler, 0),
                ..crate::signal::Action::default()
            }),
        )
        .unwrap();
        let old_mask = crate::signal::swap_blocked_mask(if blocked { 1 << 11 } else { 0 });
        let pipe = Pipe::new();
        let mut io = NativeIoStatus::default();
        let mut byte = 0;
        let status = unsafe { read(&pipe, &mut io, &mut byte) };
        assert_eq!(status, 0x103);
        crate::signal::raise_thread_signal(crate::interrupt::current_thread_id(), 12).unwrap();
        let writer = blocked.then(|| pipe.write());
        let result = unsafe { complete_native_io(pipe.0 as HANDLE, &mut io, status) };
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);
        if let Some(writer) = writer {
            writer.join().unwrap();
            assert_eq!(result, Ok(()));
            assert_eq!(byte, b'q');
            assert_eq!(
                crate::signal::deliver_pending(),
                crate::signal::Delivery::None
            );
        } else {
            assert_eq!(result, Err(crate::EINTR));
            assert_eq!(io.status as u32, 0xc000_0120); // STATUS_CANCELLED, not merely requested
            assert_eq!(io.information, 0);
        }
        crate::signal::swap_blocked_mask(0);
        assert_eq!(
            crate::signal::deliver_pending(),
            crate::signal::Delivery::Interrupted
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        crate::signal::sigaction(12, Some(old)).unwrap();
        crate::signal::swap_blocked_mask(old_mask);
    }

    #[test]
    fn native_cancellation_retires_before_signal_delivery() {
        signal_case(false);
    }

    #[test]
    fn blocked_signal_does_not_cancel_a_pending_native_request() {
        signal_case(true);
    }
}

/// Builds the Win32 rename buffer, not a native NT counted-string request.
///
/// SetFileInformationByHandle reads FileName as a terminated DOS pathname while
/// translating it. A counted name alone can succeed with adjacent heap bytes
/// appended to the destination. Include the NUL in both allocation and supplied
/// buffer size; FileNameLength itself still excludes that terminating WCHAR.
fn rename_information(target: &[u16], replace: bool) -> Result<(Vec<usize>, u32), i32> {
    if target.len() < 2 || target.last() != Some(&0) || target[..target.len() - 1].contains(&0) {
        return Err(EINVAL);
    }
    let name_bytes = (target.len() - 1)
        .checked_mul(size_of::<u16>())
        .ok_or(crate::ENAMETOOLONG)?;
    let bytes = core::mem::offset_of!(FILE_RENAME_INFO, FileName)
        .checked_add(name_bytes)
        .and_then(|bytes| bytes.checked_add(size_of::<u16>()))
        .and_then(|bytes| u32::try_from(bytes).ok())
        .ok_or(crate::ENAMETOOLONG)?;
    // Align the flexible structure for its pointer-sized RootDirectory field.
    let mut storage = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: the allocation contains every fixed field and the full UTF-16
    // string, including the NUL. Both source and destination are nonoverlapping.
    unsafe {
        (*info).Anonymous.Flags = FILE_RENAME_FLAG_POSIX_SEMANTICS
            | if replace {
                FILE_RENAME_FLAG_REPLACE_IF_EXISTS
            } else {
                0
            };
        (*info).RootDirectory = ptr::null_mut();
        (*info).FileNameLength = name_bytes as u32;
        ptr::copy_nonoverlapping(target.as_ptr(), (*info).FileName.as_mut_ptr(), target.len());
    }
    Ok((storage, bytes))
}

/// Checks path accessibility.
///
/// Windows ACLs do not reduce to POSIX rwx bits, so this reports existence plus
/// the read-only attribute, which is what portable guest code actually tests.
pub fn access(path: &str, mode: i32) -> Result<(), i32> {
    let _observation = crate::tmpfs::Observation::enter();
    if crate::tmpfs::access(path, mode)? {
        return Ok(());
    }
    let info = stat(path)?;
    if mode == F_OK {
        return Ok(());
    }
    if mode & W_OK != 0 {
        let resolved = resolve(path)?;
        if let Some(stored) = stored_mode(&resolved)? {
            if stored & 0o222 == 0 {
                return Err(EACCES);
            }
        } else {
            let wide_path = wide(&resolved)?;
            let directory = info.st_mode & S_IFMT == S_IFDIR;
            // For directories FILE_WRITE_DATA/FILE_APPEND_DATA mean
            // FILE_ADD_FILE/FILE_ADD_SUBDIRECTORY. Opening a handle with those
            // rights asks Windows about this process's effective token instead
            // of guessing it from the owner/group/other ACL projection.
            let desired = if directory {
                FILE_WRITE_DATA | FILE_APPEND_DATA
            } else {
                FILE_WRITE_DATA
            };
            let attributes = if directory {
                FILE_FLAG_BACKUP_SEMANTICS
            } else {
                0
            };
            let handle = unsafe {
                CreateFileW(
                    wide_path.as_ptr(),
                    desired,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    ptr::null(),
                    OPEN_EXISTING,
                    attributes,
                    ptr::null_mut(),
                )
            };
            if handle.is_null() || handle as isize == -1 {
                return Err(errno_from_win32(unsafe { GetLastError() }));
            }
            unsafe { CloseHandle(handle) };
        }
    }
    // In POSIX on Windows hosting, existing files and directories are allowed for X_OK.
    if mode & X_OK != 0
        && (info.st_mode & S_IFMT != S_IFDIR
            && info.st_mode & S_IFMT != S_IFREG
            && info.st_mode & S_IFMT != S_IFLNK)
    {
        return Err(EACCES);
    }
    Ok(())
}

/// Changes the guest's current directory after checking it is a directory.
pub fn chdir(path: &str) -> Result<(), i32> {
    let target = absolute_linux(path);
    let info = stat(&target)?;
    if info.st_mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    if !crate::tmpfs::owns(&target)
        && !crate::procfs::owns(&target)
        && !crate::cgroup::owns(&target)
    {
        let fd = open(&target, O_PATH | O_DIRECTORY, 0)?;
        let result = fchdir(fd);
        let _ = crate::close(fd);
        return result;
    }
    crate::fs_context::update(|s| {
        s.cwd = Some(target);
        s.cwd_object = None;
    });
    Ok(())
}

/// Changes the root directory for this process.
pub fn chroot(path: &str) -> Result<(), i32> {
    if crate::mount::overlay::chroot(path)? {
        return Ok(());
    }
    let target = absolute_linux(path);
    let info = stat(&target)?;
    if info.st_mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    let resolved = resolve(&target)?;
    crate::path::set_system_root(resolved);
    Ok(())
}

/// One entry from a directory listing.
pub struct DirectoryEntry {
    pub name: String,
    pub is_directory: bool,
    pub is_symlink: bool,
}

/// Batched native names for readdir/getdents callers that accept DT_UNKNOWN.
/// Looking up Linux inode EAs is deferred until a caller actually needs a type.
/// Enumeration remains attached to the open inode across rename and bind mounts.
pub fn read_directory_names_fd(fd: i32) -> Result<Option<Vec<String>>, i32> {
    if get(fd)?.kind != FdKind::Directory {
        return Ok(None);
    }
    let (directory, entry) = object::Object::from_fd_with_entry(fd)?;
    if entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    if entry.kind != FdKind::Directory {
        return Err(ENOTDIR);
    }
    let mut names = vec![".".into(), "..".into()];
    names.extend(
        directory
            .entries()?
            .into_iter()
            .map(|name| crate::path::unescape_path(&name.to_string_lossy()).into_owned()),
    );
    Ok(Some(names))
}

/// Lists a directory's contents, excluding nothing.
///
/// `.` and `..` are synthesized rather than taken from the host, because Windows
/// omits them for drive roots while POSIX callers expect them everywhere.
pub fn read_directory_fd(fd: i32) -> Result<Vec<DirectoryEntry>, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    if get(fd)?.kind == FdKind::TmpfsDirectory {
        return crate::tmpfs::read_directory_fd(fd);
    }
    let entry = crate::get(fd)?;
    match entry.kind {
        crate::FdKind::SyntheticDirectory => {
            let path = crate::synthetic_directory_path(fd)?;
            crate::procfs::pinned(|| read_directory(&path))
        }
        crate::FdKind::Directory => {
            // The native inode may back a bind mount outside the caller's
            // chroot. Re-resolving its Windows pathname as a guest path loses
            // that attachment (and can enumerate a replacement after rename).
            let (directory, entry) = object::Object::from_fd_with_entry(fd)?;
            if entry.flags.contains(FdFlags::PATH_ONLY) {
                return Err(EBADF);
            }
            if entry.kind != FdKind::Directory {
                return Err(ENOTDIR);
            }
            let mut entries = vec![
                DirectoryEntry {
                    name: ".".into(),
                    is_directory: true,
                    is_symlink: false,
                },
                DirectoryEntry {
                    name: "..".into(),
                    is_directory: true,
                    is_symlink: false,
                },
            ];
            for stored in directory.entries()? {
                let child = match directory.child(&stored, FILE_READ_ATTRIBUTES | FILE_READ_EA) {
                    Ok(child) => child,
                    Err(ENOENT) => continue, // Removed concurrently with enumeration.
                    Err(error) => return Err(error),
                };
                // readdir only needs the entry's type. A full stat also queries
                // ACLs, timestamps and the verity-protected file size, acquiring
                // an inode mutex and reopening every icon just to discard them.
                // `child` already owns an independent metadata query handle.
                let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
                if unsafe { GetFileInformationByHandle(child.raw(), &mut info) } == 0 {
                    return Err(errno_from_win32(unsafe { GetLastError() }));
                }
                let record = inode::read_object(&child)?;
                let is_symlink = record.symlink.is_some()
                    || (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                        && native_symlink_target_handle(child.raw())?.is_some());
                let kind = if is_symlink {
                    S_IFLNK
                } else if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    S_IFDIR
                } else {
                    record.mode.map_or(S_IFREG, |mode| mode & S_IFMT)
                };
                entries.push(DirectoryEntry {
                    name: crate::path::unescape_path(&stored.to_string_lossy()).into_owned(),
                    is_directory: kind == S_IFDIR,
                    is_symlink: kind == S_IFLNK,
                });
            }
            Ok(entries)
        }
        _ => Err(crate::ENOTDIR),
    }
}

pub fn read_directory(path: &str) -> Result<Vec<DirectoryEntry>, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    if let Some(entries) = crate::tmpfs::read_directory(path)? {
        return Ok(entries);
    }
    if let Some(entries) = crate::mount::overlay::read_directory(path)? {
        return Ok(entries);
    }
    let absolute = absolute_linux(path);
    // `/dev/pts` lists the terminals that exist right now, which is what makes
    // `ls /dev/pts` agree with what `ptsname` hands out.
    if absolute == "/dev/pts" {
        let mut entries = vec![
            DirectoryEntry {
                name: String::from("."),
                is_directory: true,
                is_symlink: false,
            },
            DirectoryEntry {
                name: String::from(".."),
                is_directory: true,
                is_symlink: false,
            },
            // A real devpts carries the multiplexer inside the directory too.
            DirectoryEntry {
                name: String::from("ptmx"),
                is_directory: false,
                is_symlink: false,
            },
        ];
        for number in crate::tty::live_terminals() {
            entries.push(DirectoryEntry {
                name: number.to_string(),
                is_directory: false,
                is_symlink: false,
            });
        }
        return Ok(entries);
    }
    if crate::procfs::owns(&absolute) {
        // procfs already supplies `.` and `..`, so the names are used verbatim.
        return crate::procfs::list_directory(&absolute)?
            .into_iter()
            .map(|name| {
                let child = if absolute.ends_with('/') {
                    format!("{absolute}{name}")
                } else {
                    format!("{absolute}/{name}")
                };
                let kind = crate::procfs::metadata(&child)
                    .map(|metadata| metadata.kind)
                    .ok();
                let is_directory = matches!(name.as_str(), "." | "..")
                    || kind == Some(crate::procfs::ProcKind::Directory);
                let is_symlink = kind == Some(crate::procfs::ProcKind::Symlink);
                Ok(DirectoryEntry {
                    name,
                    is_directory,
                    is_symlink,
                })
            })
            .collect();
    }
    if crate::cgroup::owns(&absolute) {
        let items = crate::cgroup::list_directory(&absolute)?;
        let mut entries = vec![
            DirectoryEntry {
                name: String::from("."),
                is_directory: true,
                is_symlink: false,
            },
            DirectoryEntry {
                name: String::from(".."),
                is_directory: true,
                is_symlink: false,
            },
        ];
        for name in items {
            let child = if absolute.ends_with('/') {
                format!("{absolute}{name}")
            } else {
                format!("{absolute}/{name}")
            };
            let is_dir = crate::cgroup::metadata(&child)
                .map(|m| m.kind == crate::procfs::ProcKind::Directory)
                .unwrap_or(false);
            entries.push(DirectoryEntry {
                name,
                is_directory: is_dir,
                is_symlink: false,
            });
        }
        return Ok(entries);
    }
    let resolved = resolve(path)?;
    if !resolved.is_dir() {
        return Err(ENOTDIR);
    }
    let mut entries = vec![
        DirectoryEntry {
            name: String::from("."),
            is_directory: true,
            is_symlink: false,
        },
        DirectoryEntry {
            name: String::from(".."),
            is_directory: true,
            is_symlink: false,
        },
    ];
    let listing = std::fs::read_dir(&resolved).map_err(|error| {
        error
            .raw_os_error()
            .map_or(EACCES, |code| errno_from_win32(code as u32))
    })?;
    for entry in listing.flatten() {
        let file_type = entry.file_type().ok();
        let is_directory = file_type.map(|kind| kind.is_dir()).unwrap_or(false);
        let is_symlink = file_type.map(|kind| kind.is_symlink()).unwrap_or(false)
            || emulated_symlink_target(&entry.path())?.is_some();
        entries.push(DirectoryEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_directory,
            is_symlink,
        });
    }
    Ok(entries)
}

/// Truncates or extends a file to an exact length.
pub fn ftruncate(fd: i32, length: i64) -> Result<(), i32> {
    if length < 0 {
        return Err(EINVAL);
    }
    if matches!(
        get(fd)?.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return crate::tmpfs::truncate(fd, length);
    }
    let mut mount_pin = None;
    let (object, entry) = object::Object::from_fd_checked(fd, |entry| {
        mount_pin = crate::mount::native::reference(entry)?;
        Ok(())
    })
    .map_err(|error| {
        if error == crate::EOPNOTSUPP {
            EINVAL
        } else {
            error
        }
    })?;
    if entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    if entry.kind != FdKind::File {
        return Err(EINVAL);
    }
    let rights = object::Object::granted_access(object.raw())?;
    if (entry.flags.contains(FdFlags::READ_ACCESS) && !entry.flags.contains(FdFlags::WRITE_ACCESS))
        || rights & (FILE_WRITE_DATA | FILE_APPEND_DATA) == 0
    {
        // Linux checks FMODE_WRITE before inode policy, and uses EINVAL here.
        return Err(EINVAL);
    }
    crate::limits::truncate(length as u64)?;
    truncate_handle(object.raw(), length)
}

fn truncate_handle(handle: HANDLE, length: i64) -> Result<(), i32> {
    // A private asynchronous inode handle gives EOF updates their own request
    // status without changing the caller's native or Linux file position.
    // The writer also excludes a concurrent verity enable through native share
    // access. O_TRUNC may request this temporary right on an O_RDONLY open;
    // ftruncate validates the original descriptor's write rights above.
    let writer = object::Object::reopen(handle, FILE_WRITE_DATA | FILE_READ_ATTRIBUTES)?;
    verity::ensure_writable(writer.raw())?;
    writer.set_length(length as u64)
}

#[cfg(windows)]
pub fn active_dns_servers() -> Vec<std::net::IpAddr> {
    const GAA_FLAG_SKIP_ANYCAST: u32 = 0x0002;
    const GAA_FLAG_SKIP_MULTICAST: u32 = 0x0004;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const ERROR_SUCCESS: u32 = 0;

    #[repr(C)]
    struct IpAdapterAddresses {
        length: u32,
        if_index: u32,
        next: *mut IpAdapterAddresses,
        adapter_name: *mut core::ffi::c_char,
        first_unicast_address: *mut core::ffi::c_void,
        first_anycast_address: *mut core::ffi::c_void,
        first_multicast_address: *mut core::ffi::c_void,
        first_dns_server_address: *mut IpAdapterDnsServerAddress,
        dns_suffix: *mut u16,
        description: *mut u16,
        friendly_name: *mut u16,
        physical_address: [u8; 8],
        physical_address_length: u32,
        flags: u32,
        mtu: u32,
        if_type: u32,
        oper_status: i32,
        ipv6_if_index: u32,
        zone_indices: [u32; 16],
        first_prefix: *mut core::ffi::c_void,
    }

    #[repr(C)]
    struct IpAdapterDnsServerAddress {
        length: u32,
        reserved: u32,
        next: *mut IpAdapterDnsServerAddress,
        address: SocketAddress,
    }

    #[repr(C)]
    struct SocketAddress {
        lp_sockaddr: *mut SockAddr,
        i_sockaddr_length: i32,
    }

    #[repr(C)]
    struct SockAddr {
        sa_family: u16,
        sa_data: [u8; 14],
    }

    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetAdaptersAddresses(
            family: u32,
            flags: u32,
            reserved: *mut core::ffi::c_void,
            addresses: *mut core::ffi::c_void,
            size: *mut u32,
        ) -> u32;
    }

    let mut size = 15 * 1024u32;
    let mut buffer = vec![0u8; size as usize];
    let mut result_code = unsafe {
        GetAdaptersAddresses(
            0,
            GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST,
            core::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    if result_code == ERROR_BUFFER_OVERFLOW {
        buffer.resize(size as usize, 0);
        result_code = unsafe {
            GetAdaptersAddresses(
                0,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST,
                core::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &raw mut size,
            )
        };
    }
    if result_code != ERROR_SUCCESS {
        return vec!["1.1.1.1".parse().unwrap(), "8.8.8.8".parse().unwrap()];
    }

    let mut ipv4_servers = Vec::new();
    let mut ipv6_servers = Vec::new();
    let mut cursor = buffer.as_ptr().cast::<IpAdapterAddresses>();
    while !cursor.is_null() {
        let adapter = unsafe { &*cursor };
        if adapter.oper_status == 1 && adapter.if_type != 24 {
            let mut dns_cursor = adapter.first_dns_server_address;
            while !dns_cursor.is_null() {
                let dns_entry = unsafe { &*dns_cursor };
                let sockaddr_ptr = dns_entry.address.lp_sockaddr;
                if !sockaddr_ptr.is_null() {
                    let family = unsafe { (*sockaddr_ptr).sa_family };
                    if family == 2 && dns_entry.address.i_sockaddr_length >= 8 {
                        let octets: [u8; 4] = unsafe {
                            let data_ptr = sockaddr_ptr.cast::<u8>().add(4);
                            [
                                *data_ptr,
                                *data_ptr.add(1),
                                *data_ptr.add(2),
                                *data_ptr.add(3),
                            ]
                        };
                        let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::from(octets));
                        if !ip.is_unspecified() && !ip.is_loopback() && !ipv4_servers.contains(&ip)
                        {
                            ipv4_servers.push(ip);
                        }
                    } else if family == 23 && dns_entry.address.i_sockaddr_length >= 24 {
                        let octets: [u8; 16] = unsafe {
                            let data_ptr = sockaddr_ptr.cast::<u8>().add(8);
                            let mut buf = [0u8; 16];
                            core::ptr::copy_nonoverlapping(data_ptr, buf.as_mut_ptr(), 16);
                            buf
                        };
                        let is_site_local = octets[0] == 0xfe && (octets[1] & 0xc0) == 0xc0;
                        let is_link_local = octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80;
                        let ip = std::net::IpAddr::V6(std::net::Ipv6Addr::from(octets));
                        if !ip.is_unspecified()
                            && !ip.is_loopback()
                            && !is_site_local
                            && !is_link_local
                            && !ipv6_servers.contains(&ip)
                        {
                            ipv6_servers.push(ip);
                        }
                    }
                }
                dns_cursor = dns_entry.next;
            }
        }
        cursor = adapter.next;
    }

    let mut dns_servers = ipv4_servers;
    dns_servers.extend(ipv6_servers);

    if dns_servers.is_empty() {
        vec!["1.1.1.1".parse().unwrap(), "8.8.8.8".parse().unwrap()]
    } else {
        dns_servers
    }
}

#[cfg(not(windows))]
pub fn active_dns_servers() -> Vec<std::net::IpAddr> {
    vec!["1.1.1.1".parse().unwrap(), "8.8.8.8".parse().unwrap()]
}

pub fn synthetic_resolv_conf() -> Vec<u8> {
    let servers = active_dns_servers();
    let mut text = String::new();
    for server in servers {
        text.push_str(&format!("nameserver {server}\n"));
    }
    text.into_bytes()
}

pub fn synthetic_environment() -> Vec<u8> {
    b"PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin\nLANG=C.UTF-8\n".to_vec()
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    fn directory(label: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-rename-{label}-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn rename_buffer_includes_nul_at_every_alignment() {
        for length in 1..=16 {
            let mut name = vec![b'x' as u16; length];
            name.push(0);
            let (storage, bytes) = rename_information(&name, true).unwrap();
            let info = storage.as_ptr().cast::<FILE_RENAME_INFO>();
            // SAFETY: storage is aligned, and the builder allocated all name
            // elements including the terminator tested below.
            unsafe {
                assert_eq!((*info).FileNameLength as usize, length * 2);
                assert_eq!(
                    bytes as usize,
                    core::mem::offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2
                );
                assert_eq!(
                    core::slice::from_raw_parts((*info).FileName.as_ptr(), name.len()),
                    name
                );
            }
        }
    }

    #[test]
    fn rename_buffer_rejects_invalid_termination() {
        for name in [&[][..], &[0], &[120], &[120, 0, 121, 0]] {
            assert_eq!(rename_information(name, true), Err(EINVAL));
        }
    }

    #[test]
    fn live_rename_preserves_exact_unicode_names_across_alignments() {
        let root = directory("names");
        let source = root.join("source");
        let mut destinations = Vec::new();
        for length in 1..=16 {
            let target = root.join(format!("目标🦀-{}", "x".repeat(length)));
            let content = format!("length {length}\n");
            std::fs::write(&source, content.as_bytes()).unwrap();
            rename(
                &crate::to_guest_path(&source),
                &crate::to_guest_path(&target),
            )
            .unwrap();
            assert!(!source.exists(), "source survived rename");
            assert_eq!(std::fs::read(&target).unwrap(), content.as_bytes());
            destinations.push(target);
        }
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            destinations.len()
        );
        // Remove only the exact test files we created; unexpected names remain
        // available for diagnosis if an assertion above fails.
        for target in destinations {
            std::fs::remove_file(target).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn live_rename_replaces_a_file_without_invalidating_its_open_handle() {
        use std::io::Read;
        use std::os::windows::fs::OpenOptionsExt;
        let root = directory("replace");
        let source = root.join("source");
        let target = root.join("target");
        std::fs::write(&source, b"new inode").unwrap();
        std::fs::write(&target, b"old inode").unwrap();
        let mut old = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&target)
            .unwrap();
        rename(
            &crate::to_guest_path(&source),
            &crate::to_guest_path(&target),
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new inode");
        let mut contents = Vec::new();
        old.read_to_end(&mut contents).unwrap();
        assert_eq!(contents, b"old inode");
        assert_eq!(
            rename(
                &crate::to_guest_path(&source),
                &crate::to_guest_path(&target)
            ),
            Err(crate::ENOENT)
        );
        drop(old);
        std::fs::remove_file(target).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}

/// Resolve openat2 paths with retained directory objects and explicit link and
/// mount traversal policy. Cached-only requests may fail without doing I/O.
pub fn openat_resolved(
    dirfd: i32,
    path: &str,
    flags: i32,
    mode: u32,
    resolve_flags: u64,
) -> Result<i32, i32> {
    let _observation = crate::tmpfs::Observation::enter();
    const TMPFILE: i32 = 0o20000000;
    const KNOWN_FLAGS: i32 = O_ACCMODE
        | O_CREAT
        | O_EXCL
        | O_NOCTTY
        | O_TRUNC
        | O_APPEND
        | O_NONBLOCK
        | 0o10000
        | 0o20000
        | 0o40000
        | 0o100000
        | O_DIRECTORY
        | O_NOFOLLOW
        | O_NOATIME
        | O_CLOEXEC
        | 0o4000000
        | O_PATH
        | TMPFILE;
    if resolve_flags & !0x3f != 0
        || resolve_flags & 0x18 == 0x18
        || flags & !KNOWN_FLAGS != 0
        || mode & !0o7777 != 0
        || mode != 0 && flags & (O_CREAT | TMPFILE) == 0
        || flags & O_PATH != 0 && flags & !(O_PATH | O_CLOEXEC | O_DIRECTORY | O_NOFOLLOW) != 0
        || flags & (O_CREAT | O_DIRECTORY) == O_CREAT | O_DIRECTORY
        || flags & TMPFILE != 0 && (flags & O_DIRECTORY == 0 || flags & O_ACCMODE == O_RDONLY)
    {
        return Err(EINVAL);
    }
    if resolve_flags == 0 {
        return openat(dirfd, path, flags, mode);
    }
    if resolve_flags & 0x20 != 0 {
        return Err(crate::EAGAIN);
    }
    if path.starts_with('/') && resolve_flags & 8 != 0 {
        return Err(crate::EXDEV);
    }
    if (!path.starts_with('/') || resolve_flags & 16 != 0)
        && dirfd != AT_FDCWD
        && get(dirfd)?.kind == FdKind::TmpfsDirectory
    {
        return crate::tmpfs::openat_resolved(dirfd, path, flags, mode, resolve_flags);
    }
    confined::open(dirfd, path, flags, mode, resolve_flags)
}
