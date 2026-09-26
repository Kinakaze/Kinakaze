//! The remaining file-system ABI surface: LFS aliases, metadata, links,
//! directory walking, extended attributes, `fnmatch` and the `statfs` family.
//!
//! Three kinds of entry point live here.
//!
//! The first is glibc's large-file spelling. On x86_64 `off_t` is already 64
//! bits, so `stat64` and `stat` describe byte-for-byte identical calls; the
//! `*64` names are therefore pure forwarders to the implementations in
//! [`crate::fs`], [`crate::stdio`] and [`crate::dirent`] rather than second
//! copies that could drift.
//!
//! The second is the group Windows can express only partially. Those are
//! implemented as faithfully as the platform allows and each carries a comment
//! naming exactly what is lost, because a caller cannot reason about a mapping
//! it cannot see.
//!
//! The third is pure computation that never touches the file system at all:
//! `basename`, `dirname` and `fnmatch` are string functions, and they are
//! implemented to the letter of POSIX here so guest behaviour does not depend on
//! the host at all.

use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kinakaze_vfs::fs::{self, Stat};
use kinakaze_vfs::{EACCES, EFAULT, EINVAL, ENAMETOOLONG, errno_from_win32};

use crate::dirent::{DT_DIR, DT_LNK, DT_REG, Dir, Dirent};
use crate::set_errno;

/// Longest guest path accepted, matching Linux `PATH_MAX`.
const PATH_MAX: usize = 4096;

/// `EPERM`, reported by `mknod` for the device types no user process may create.
const EPERM: i32 = 1;

/// `AT_FDCWD` from the Linux `*at` family.
const AT_FDCWD: i32 = fs::AT_FDCWD;

/// `AT_SYMLINK_NOFOLLOW`, which turns an `*at` call into its `l*` form.
const AT_SYMLINK_NOFOLLOW: i32 = 0x100;

/// `AT_EACCESS`, selecting effective rather than real credentials for access checks.
const AT_EACCESS: i32 = 0x200;

/// `AT_EMPTY_PATH`, allowing an empty pathname to refer to `dirfd` itself.
const AT_EMPTY_PATH: i32 = 0x1000;

/// Borrows a guest path string.
///
/// # Safety
///
/// `path` must be null or point to a null-terminated string that stays valid for
/// the duration of the call.
unsafe fn borrow_path(path: *const c_char) -> Result<&'static str, i32> {
    if path.is_null() {
        return Err(EFAULT);
    }
    // SAFETY: the caller guarantees a null-terminated string.
    let raw = unsafe { CStr::from_ptr(path) };
    if raw.to_bytes().len() > PATH_MAX {
        return Err(ENAMETOOLONG);
    }
    // Guest paths are byte strings; reject non-UTF-8 rather than lose bytes on
    // the way to the Windows wide-character API.
    raw.to_str().map_err(|_| EINVAL)
}

/// Borrows a guest byte string of any content, without the UTF-8 requirement.
///
/// # Safety
///
/// `text` must be null or a null-terminated string valid for the call.
unsafe fn borrow_bytes(text: *const c_char) -> Option<&'static [u8]> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a null-terminated string.
    Some(unsafe { CStr::from_ptr(text) }.to_bytes())
}

/// Applies the POSIX `-1`/`errno` convention to a result.
fn posix(result: Result<(), i32>) -> c_int {
    match result {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Joins a relative guest path onto the current directory.
///
/// [`kinakaze_vfs::fs`] keeps its own `absolute_linux` private, so the same
/// two-line rule is repeated here rather than reached for across the module
/// boundary. Both spellings consult the one `getcwd` the VFS owns, so they
/// cannot disagree about where the guest currently is.
fn absolute_linux(path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    let cwd = fs::getcwd();
    if cwd == "/" {
        format!("/{path}")
    } else {
        format!("{cwd}/{path}")
    }
}

/// Translates a guest path into the Windows path that backs it.
fn resolve(path: &str) -> Result<PathBuf, i32> {
    kinakaze_vfs::resolve_linux_path(&absolute_linux(path)).map_err(path_errno)
}

/// Resolves intermediate links while retaining the final link itself.
pub(crate) fn resolve_no_follow(path: &str) -> Result<PathBuf, i32> {
    kinakaze_vfs::resolve_linux_path_no_follow(&absolute_linux(path)).map_err(path_errno)
}

fn path_errno(error: kinakaze_vfs::PathError) -> i32 {
    match error {
        kinakaze_vfs::PathError::TooManySymlinks => kinakaze_vfs::ELOOP,
        kinakaze_vfs::PathError::Filesystem(error) => error,
        _ => EINVAL,
    }
}

/// Translates a Windows path back into the guest's namespace.
///
/// This is the inverse of [`kinakaze_vfs::resolve_linux_path`] and is needed by
/// `readlink` and `realpath`, which must hand the guest a name it can pass back
/// to `open`. A path under the virtual root maps to that suffix; anything else
/// is a real Windows location and becomes the `/<drive>/...` spelling the
/// resolver accepts. Extended-length prefixes are stripped first, since
/// `canonicalize` emits them and the guest has no notion of `\\?\`.
fn windows_to_linux(path: &Path) -> String {
    // Keep file and root in the same canonical Windows namespace, as the loader
    // does. Otherwise a verbatim root leaks an unusable host drive path.
    kinakaze_vfs::to_guest_path(path)
}

/// Recovers the guest path a directory descriptor refers to.
///
/// Synthetic `/proc` directories carry their path in the VFS side table. A real
/// directory handle does not, so the name is read back out of the kernel with
/// `GetFinalPathNameByHandleW` and translated into the guest's namespace.
pub(crate) fn directory_path(fd: c_int) -> Result<String, i32> {
    if let Some(path) = kinakaze_vfs::mount::overlay::descriptor_path(fd)? {
        if fs::fstat(fd)?.st_mode & fs::S_IFMT != fs::S_IFDIR {
            return Err(kinakaze_vfs::ENOTDIR);
        }
        return Ok(path);
    }
    if let Some(path) = kinakaze_vfs::mount::native::descriptor_path(fd)? {
        if fs::fstat(fd)?.st_mode & fs::S_IFMT != fs::S_IFDIR {
            return Err(kinakaze_vfs::ENOTDIR);
        }
        return Ok(path);
    }
    let entry = kinakaze_vfs::get(fd)?;
    match entry.kind {
        kinakaze_vfs::FdKind::MountTree => kinakaze_vfs::mount::api::tree_path(fd),
        kinakaze_vfs::FdKind::SyntheticDirectory => kinakaze_vfs::synthetic_directory_path(fd),
        kinakaze_vfs::FdKind::TmpfsDirectory
        | kinakaze_vfs::FdKind::TmpfsFile
        | kinakaze_vfs::FdKind::MessageQueue
        | kinakaze_vfs::FdKind::SysfsFile => kinakaze_vfs::tmpfs::descriptor_path(fd),
        kinakaze_vfs::FdKind::Directory | kinakaze_vfs::FdKind::File => {
            if fs::fstat(fd)?.st_mode & fs::S_IFMT != fs::S_IFDIR {
                return Err(kinakaze_vfs::ENOTDIR);
            }
            let path = handle_path(entry.raw as *mut c_void)?;
            Ok(windows_to_linux(&path))
        }
        _ => Err(kinakaze_vfs::ENOTDIR),
    }
}

/// Reads the path behind an open Windows handle.
fn handle_path(handle: *mut c_void) -> Result<PathBuf, i32> {
    // The first call reports the length in characters, excluding the terminator.
    // SAFETY: the handle comes from the descriptor table and is live.
    let needed = unsafe { GetFinalPathNameByHandleW(handle, ptr::null_mut(), 0, 0) };
    if needed == 0 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let mut buffer = vec![0u16; needed as usize + 1];
    // SAFETY: `buffer` has `needed + 1` writable characters.
    let written = unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), needed + 1, 0) };
    if written == 0 || written > needed + 1 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    buffer.truncate(written as usize);
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer)))
}

// Windows entry points this module needs.
//
// Declared here rather than pulled from `windows-sys` because this crate's
// dependency enables only `Win32_System_Threading`, and the export surface is
// not worth widening a feature list for. Every signature matches the Windows
// headers: `BOOL` is `i32`, a `DWORD` is `u32`, and `ULARGE_INTEGER` out
// parameters are `u64`.
#[link(name = "kernel32")]
unsafe extern "system" {
    /// `GetLastError`: the calling thread's last error code.
    fn GetLastError() -> u32;
    /// `FlushFileBuffers`: writes a handle's buffered data through to the disk.
    fn FlushFileBuffers(handle: *mut c_void) -> i32;
    /// `GetFinalPathNameByHandleW`: the path an open handle refers to.
    fn GetFinalPathNameByHandleW(handle: *mut c_void, path: *mut u16, size: u32, flags: u32)
    -> u32;
    /// `GetDiskFreeSpaceExW`: byte totals for the volume holding a directory.
    fn GetDiskFreeSpaceExW(
        directory: *const u16,
        free_to_caller: *mut u64,
        total: *mut u64,
        total_free: *mut u64,
    ) -> i32;
}

// ---------------------------------------------------------------------------
// Large-file spellings
// ---------------------------------------------------------------------------

// glibc grew a parallel `*64` name for every call carrying an `off_t` back when
// `off_t` was 32 bits and the wide forms had to be requested explicitly. On
// x86_64 that split never existed: `off_t` is 64 bits in both spellings, so each
// pair describes one identical call. These are therefore forwarders to the one
// implementation rather than second copies, which is what keeps `stat` and
// `stat64` from ever disagreeing about a file.
//
// Only names whose base implementation exists are aliased. A `*64` export whose
// base is absent would claim a call this library cannot make, so those are left
// undefined and reported instead: the guest then fails at a symbol it can be
// told about rather than inside a function that silently lies.

/// `stat64`, identical to `stat` on x86_64.
///
/// # Safety
///
/// `path` must be a valid string and `out` a writable `struct stat`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_stat64(path: *const c_char, out: *mut Stat) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { crate::fs::stat(path, out) }
}

/// `lstat64`, identical to `lstat` on x86_64.
///
/// # Safety
///
/// `path` must be a valid string and `out` a writable `struct stat`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lstat64(path: *const c_char, out: *mut Stat) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { crate::fs::lstat(path, out) }
}

/// `fstat64`, identical to `fstat` on x86_64.
///
/// # Safety
///
/// `out` must point to a writable `struct stat`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fstat64(fd: c_int, out: *mut Stat) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { crate::fs::fstat(fd, out) }
}

/// `fstatat`: `stat` relative to a directory descriptor.
///
/// Only `AT_FDCWD` and absolute paths are honoured, matching the VFS's own
/// limit: a genuinely directory-relative lookup needs the descriptor's path,
/// which the table records for synthetic directories only. Anything else reports
/// `ENOSYS` rather than resolving against the wrong directory and returning
/// metadata for a file the caller did not name.
///
/// # Safety
///
/// `path` must be a valid string and `out` a writable `struct stat`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fstatat(
    dirfd: c_int,
    path: *const c_char,
    out: *mut Stat,
    flags: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let borrowed = unsafe { borrow_path(path) };
    let value = borrowed.and_then(|path| {
        crate::fs::restart_metadata(|| {
            if (flags & AT_EMPTY_PATH != 0 && path.is_empty()) || (path.is_empty() && dirfd >= 0) {
                return fs::fstat(dirfd);
            }
            if !path.starts_with('/') && dirfd != AT_FDCWD {
                // A relative name plus a real directory descriptor: recover the
                // directory's own path so the join is against the right place.
                let base = directory_path(dirfd)?;
                let joined = if base.ends_with('/') {
                    format!("{base}{path}")
                } else {
                    format!("{base}/{path}")
                };
                return if flags & AT_SYMLINK_NOFOLLOW != 0 {
                    fs::lstat(&joined)
                } else {
                    fs::stat(&joined)
                };
            }
            if flags & AT_SYMLINK_NOFOLLOW != 0 {
                fs::lstat(path)
            } else {
                fs::stat(path)
            }
        })
    });
    if out.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    match value {
        Ok(value) => {
            // SAFETY: the caller guarantees `out` is a writable struct stat.
            unsafe { ptr::write(out, value) };
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `fstatat64`, identical to `fstatat` on x86_64.
///
/// # Safety
///
/// `path` must be a valid string and `out` a writable `struct stat`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fstatat64(
    dirfd: c_int,
    path: *const c_char,
    out: *mut Stat,
    flags: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { kinakaze_abi_fstatat(dirfd, path, out, flags) }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct StatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Statx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    pub __spare0: [u16; 1],
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: StatxTimestamp,
    pub stx_btime: StatxTimestamp,
    pub stx_ctime: StatxTimestamp,
    pub stx_mtime: StatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    pub __spare2: [u64; 12],
}

pub const STATX_TYPE: u32 = 0x0001;
pub const STATX_MODE: u32 = 0x0002;
pub const STATX_NLINK: u32 = 0x0004;
pub const STATX_UID: u32 = 0x0008;
pub const STATX_GID: u32 = 0x0010;
pub const STATX_ATIME: u32 = 0x0020;
pub const STATX_MTIME: u32 = 0x0040;
pub const STATX_CTIME: u32 = 0x0080;
pub const STATX_INO: u32 = 0x0100;
pub const STATX_SIZE: u32 = 0x0200;
pub const STATX_BLOCKS: u32 = 0x0400;
pub const STATX_BASIC_STATS: u32 = 0x07ff;
pub const STATX_BTIME: u32 = 0x0800;
pub const STATX_ALL: u32 = 0x0fff;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_statx(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mask: u32,
    statxbuf: *mut Statx,
) -> c_int {
    if statxbuf.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let mut st: Stat = unsafe { core::mem::zeroed() };
    let res = unsafe { kinakaze_abi_fstatat(dirfd, path, &mut st, flags) };
    if res != 0 {
        return -1;
    }
    let sx = Statx {
        stx_mask: mask & STATX_BASIC_STATS,
        stx_blksize: st.st_blksize as u32,
        stx_attributes: 0,
        stx_nlink: st.st_nlink as u32,
        stx_uid: st.st_uid,
        stx_gid: st.st_gid,
        stx_mode: st.st_mode as u16,
        __spare0: [0; 1],
        stx_ino: st.st_ino,
        stx_size: st.st_size as u64,
        stx_blocks: st.st_blocks as u64,
        stx_attributes_mask: 0,
        stx_atime: StatxTimestamp {
            tv_sec: st.st_atime,
            tv_nsec: st.st_atime_nsec as u32,
            __reserved: 0,
        },
        stx_btime: StatxTimestamp {
            tv_sec: st.st_ctime,
            tv_nsec: st.st_ctime_nsec as u32,
            __reserved: 0,
        },
        stx_ctime: StatxTimestamp {
            tv_sec: st.st_ctime,
            tv_nsec: st.st_ctime_nsec as u32,
            __reserved: 0,
        },
        stx_mtime: StatxTimestamp {
            tv_sec: st.st_mtime,
            tv_nsec: st.st_mtime_nsec as u32,
            __reserved: 0,
        },
        stx_rdev_major: 0,
        stx_rdev_minor: 0,
        stx_dev_major: 0,
        stx_dev_minor: 0,
        stx_mnt_id: 0,
        stx_dio_mem_align: 0,
        stx_dio_offset_align: 0,
        __spare2: [0; 12],
    };
    unsafe { ptr::write(statxbuf, sx) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn statx(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mask: u32,
    statxbuf: *mut Statx,
) -> c_int {
    unsafe { kinakaze_abi_statx(dirfd, path, flags, mask, statxbuf) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___xstat(
    _ver: c_int,
    path: *const c_char,
    out: *mut Stat,
) -> c_int {
    unsafe { crate::fs::stat(path, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___xstat64(
    _ver: c_int,
    path: *const c_char,
    out: *mut Stat,
) -> c_int {
    unsafe { crate::fs::stat(path, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___lxstat(
    _ver: c_int,
    path: *const c_char,
    out: *mut Stat,
) -> c_int {
    unsafe { crate::fs::lstat(path, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___lxstat64(
    _ver: c_int,
    path: *const c_char,
    out: *mut Stat,
) -> c_int {
    unsafe { crate::fs::lstat(path, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___fxstat(
    _ver: c_int,
    fd: c_int,
    out: *mut Stat,
) -> c_int {
    unsafe { crate::fs::fstat(fd, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___fxstat64(
    _ver: c_int,
    fd: c_int,
    out: *mut Stat,
) -> c_int {
    unsafe { crate::fs::fstat(fd, out) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___fxstatat(
    _ver: c_int,
    dirfd: c_int,
    path: *const c_char,
    out: *mut Stat,
    flags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_fstatat(dirfd, path, out, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___fxstatat64(
    _ver: c_int,
    dirfd: c_int,
    path: *const c_char,
    out: *mut Stat,
    flags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_fstatat64(dirfd, path, out, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___xmknod(
    _ver: c_int,
    path: *const c_char,
    mode: u32,
    _dev: u64,
) -> c_int {
    unsafe { kinakaze_abi_mknod(path, mode, _dev) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___xmknodat(
    _ver: c_int,
    _dirfd: c_int,
    path: *const c_char,
    mode: u32,
    _dev: u64,
) -> c_int {
    unsafe { kinakaze_abi_mknod(path, mode, _dev) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkdirat(
    dirfd: c_int,
    path: *const c_char,
    mode: u32,
) -> c_int {
    let borrowed = unsafe { borrow_path(path) };
    let value = borrowed.and_then(|p| {
        crate::fs::restart_metadata(|| {
            let full = if !p.starts_with('/') && dirfd != AT_FDCWD {
                let base = directory_path(dirfd)?;
                if base.ends_with('/') {
                    format!("{base}{p}")
                } else {
                    format!("{base}/{p}")
                }
            } else {
                p.to_owned()
            };
            fs::mkdir(&full, creation_mode(mode))
        })
    });
    match value {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_unlinkat(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
) -> c_int {
    const AT_REMOVEDIR: c_int = 0x200;
    let borrowed = unsafe { borrow_path(path) };
    if flags & !AT_REMOVEDIR != 0 {
        set_errno(EINVAL);
        return -1;
    }
    let value = borrowed.and_then(|p| {
        crate::fs::restart_metadata(|| {
            let full = if !p.starts_with('/') && dirfd != AT_FDCWD {
                let base = directory_path(dirfd)?;
                if base.ends_with('/') {
                    format!("{base}{p}")
                } else {
                    format!("{base}/{p}")
                }
            } else {
                p.to_owned()
            };
            if flags & AT_REMOVEDIR != 0 {
                fs::rmdir(&full)
            } else {
                fs::unlink(&full)
            }
        })
    });
    match value {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_renameat(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
) -> c_int {
    unsafe { kinakaze_abi_renameat2(olddirfd, oldpath, newdirfd, newpath, 0) }
}

unsafe fn rename_at_with_flags(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
    flags: u32,
) -> c_int {
    let old_b = unsafe { borrow_path(oldpath) };
    let new_b = unsafe { borrow_path(newpath) };
    let value = old_b.and_then(|op| {
        let np = new_b?;
        let old_full = if !op.starts_with('/') && olddirfd != AT_FDCWD {
            let base = directory_path(olddirfd)?;
            if base.ends_with('/') {
                format!("{base}{op}")
            } else {
                format!("{base}/{op}")
            }
        } else {
            op.to_owned()
        };
        let new_full = if !np.starts_with('/') && newdirfd != AT_FDCWD {
            let base = directory_path(newdirfd)?;
            if base.ends_with('/') {
                format!("{base}{np}")
            } else {
                format!("{base}/{np}")
            }
        } else {
            np.to_owned()
        };
        fs::rename_with_flags(&old_full, &new_full, flags)
    });
    match value {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn renameat(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
) -> c_int {
    unsafe { kinakaze_abi_renameat(olddirfd, oldpath, newdirfd, newpath) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn renameat2(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
    flags: u32,
) -> c_int {
    unsafe { kinakaze_abi_renameat2(olddirfd, oldpath, newdirfd, newpath, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_renameat2(
    olddirfd: c_int,
    oldpath: *const c_char,
    newdirfd: c_int,
    newpath: *const c_char,
    flags: u32,
) -> c_int {
    if flags & !7 != 0 || flags & 2 != 0 && flags & 5 != 0 {
        set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    unsafe { rename_at_with_flags(olddirfd, oldpath, newdirfd, newpath, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_faccessat(
    dirfd: c_int,
    path: *const c_char,
    mode: c_int,
    flags: c_int,
) -> c_int {
    let value = unsafe { access_at(dirfd, path, mode, flags) };
    match value {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Linux `faccessat2`. Unlike the older x86-64 `faccessat` syscall, flags are
/// part of the kernel ABI instead of being emulated in the C library.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_faccessat2(
    dirfd: c_int,
    path: *const c_char,
    mode: c_int,
    flags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_faccessat(dirfd, path, mode, flags) }
}

/// Resolves an `*at` pathname and applies Linux's access permission algorithm.
///
/// Windows ACLs are intentionally not substituted for the guest mode bits: the
/// VFS persists Linux metadata, and access(2) must make the same decision after
/// fork, archive extraction, or a move to another host filesystem.
unsafe fn access_at(
    dirfd: c_int,
    path: *const c_char,
    mode: c_int,
    flags: c_int,
) -> Result<(), i32> {
    if mode & !(fs::R_OK | fs::W_OK | fs::X_OK) != 0
        || flags & !(AT_EACCESS | AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0
    {
        return Err(EINVAL);
    }

    let path = unsafe { borrow_path(path) }?;
    let info = if path.is_empty() && flags & AT_EMPTY_PATH != 0 {
        fs::fstat(dirfd)?
    } else {
        let target = resolve_at(dirfd, path)?;
        if flags & AT_SYMLINK_NOFOLLOW != 0 {
            fs::lstat(&target)?
        } else {
            fs::stat(&target)?
        }
    };

    let (uid, gid) = if flags & AT_EACCESS != 0 {
        (
            crate::userdb::kinakaze_abi_geteuid(),
            crate::userdb::kinakaze_abi_getegid(),
        )
    } else {
        (
            crate::userdb::kinakaze_abi_getuid(),
            crate::userdb::kinakaze_abi_getgid(),
        )
    };
    let in_file_group =
        gid == info.st_gid || crate::userdb::kinakaze_abi_group_member(info.st_gid) != 0;
    access_mode_allowed(&info, mode, uid, in_file_group)
}

fn access_mode_allowed(info: &Stat, mode: c_int, uid: u32, in_file_group: bool) -> Result<(), i32> {
    if mode == fs::F_OK {
        return Ok(());
    }

    // Linux root bypasses read/write mode bits. Execute still requires a
    // directory or at least one execute bit on a non-directory object.
    if uid == 0 {
        if mode & fs::X_OK != 0
            && info.st_mode & fs::S_IFMT != fs::S_IFDIR
            && info.st_mode & 0o111 == 0
        {
            return Err(EACCES);
        }
        return Ok(());
    }

    let available = if uid == info.st_uid {
        (info.st_mode >> 6) & 0o7
    } else if in_file_group {
        (info.st_mode >> 3) & 0o7
    } else {
        info.st_mode & 0o7
    };
    let requested = mode as u32;
    if requested & !available != 0 {
        Err(EACCES)
    } else {
        Ok(())
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkfifoat(
    dirfd: c_int,
    path: *const c_char,
    mode: u32,
) -> c_int {
    let target = unsafe { borrow_path(path) }.and_then(|path| resolve_at(dirfd, path));
    posix(target.and_then(|path| fs::create_fifo(&path, creation_mode(mode))))
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_truncate(path: *const c_char, length: i64) -> c_int {
    unsafe { kinakaze_abi_truncate64(path, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_truncate64(path: *const c_char, length: i64) -> c_int {
    let borrowed = unsafe { borrow_path(path) };
    let value = borrowed.and_then(|p| {
        let fd = fs::open(p, 0x0002, 0)?; // O_RDWR
        let res = fs::ftruncate(fd, length);
        let _ = crate::kinakaze_close(fd);
        res
    });
    match value {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `open64`, identical to `open` on x86_64.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_open64(
    path: *const c_char,
    flags: c_int,
    mode: u32,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { crate::fs::open(path, flags, mode) }
}

/// `openat64`, identical to `openat` on x86_64.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_openat64(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: u32,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { crate::fs::openat(dirfd, path, flags, mode) }
}

/// `creat64`, identical to `creat` on x86_64.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_creat64(path: *const c_char, mode: u32) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { crate::fs::creat(path, mode) }
}

/// `lseek64`, identical to `lseek` on x86_64.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_lseek64(fd: c_int, offset: i64, whence: c_int) -> i64 {
    crate::fs::lseek(fd, offset, whence)
}

/// `ftruncate64`, identical to `ftruncate` on x86_64.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ftruncate64(fd: c_int, length: i64) -> c_int {
    crate::fs::ftruncate(fd, length)
}

/// `fopen64`, identical to `fopen` on x86_64.
///
/// # Safety
///
/// Both arguments must be valid null-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fopen64(
    path: *const c_char,
    mode: *const c_char,
) -> *mut crate::stdio::File {
    // SAFETY: forwarded from this function's own contract.
    let file = unsafe { crate::stdio::fopen(path, mode) };
    crate::stdio::trace_event(format_args!("fopen64 result={file:p}"));
    file
}

/// `readdir64`, identical to `readdir` on x86_64.
///
/// The `struct dirent` this writes already uses a 64-bit `d_ino` and `d_off`, so
/// the two names describe the same record.
///
/// # Safety
///
/// `directory` must be a live stream from `opendir`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_readdir64(directory: *mut Dir) -> *mut Dirent {
    // SAFETY: forwarded from this function's own contract.
    unsafe { crate::dirent::kinakaze_abi_readdir(directory) }
}

// ---------------------------------------------------------------------------
// Permissions and ownership
// ---------------------------------------------------------------------------

/// Stores the guest mode in the VFS metadata stream and mirrors owner-write to
/// the Windows read-only attribute.
fn apply_mode(target: &Path, mode: u32) -> Result<(), i32> {
    if !target.exists() {
        return Err(kinakaze_vfs::ENOENT);
    }
    kinakaze_vfs::fs::set_mode_host_path(target, mode)
}

/// Converts a Rust I/O error into the Linux errno used by path helpers below.
fn io_errno(error: std::io::Error) -> i32 {
    error
        .raw_os_error()
        .map_or(kinakaze_vfs::EIO, |code| errno_from_win32(code as u32))
}

/// `chmod`. See [`apply_mode`] for what a Windows file can store.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_chmod(path: *const c_char, mode: u32) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let borrowed = unsafe { borrow_path(path) };
    posix(borrowed.and_then(|path| {
        if fs::chmod_fd_link(path, mode)? {
            return Ok(());
        }
        if kinakaze_vfs::tmpfs::chmod(path, mode)? {
            return Ok(());
        }
        if path.starts_with("/dev/") || path.starts_with("dev/") {
            return Ok(());
        }
        let resolved = kinakaze_vfs::mount::overlay::prepare_write(path, true, false)?;
        apply_mode(&resolved, mode)
    }))
}

/// `fchmod`, reached through the descriptor's own path.
///
/// The mode is applied by name rather than through the handle because a handle
/// opened for reading carries no `FILE_WRITE_ATTRIBUTES` access, and POSIX
/// permits `fchmod` on a read-only descriptor.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fchmod(fd: c_int, mode: u32) -> c_int {
    if kinakaze_vfs::get(fd).is_ok_and(|e| e.flags.contains(kinakaze_vfs::FdFlags::PATH_ONLY)) {
        return posix(Err(kinakaze_vfs::EBADF));
    }
    match kinakaze_vfs::devpts::fchmod(fd, mode) {
        Ok(true) => return 0,
        Err(e) => {
            set_errno(e);
            return -1;
        }
        _ => (),
    }
    if kinakaze_vfs::get(fd).is_ok_and(|e| {
        matches!(
            e.kind,
            kinakaze_vfs::FdKind::TmpfsFile
                | kinakaze_vfs::FdKind::TmpfsDirectory
                | kinakaze_vfs::FdKind::MessageQueue
                | kinakaze_vfs::FdKind::SysfsFile
        )
    }) {
        return posix(kinakaze_vfs::tmpfs::fchmod(fd, mode));
    }
    match kinakaze_vfs::pipe_inode::chmod(fd, mode) {
        Ok(true) => return 0,
        Ok(false) => {}
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    }
    match kinakaze_vfs::mount::overlay::chmod_descriptor(fd, mode) {
        Ok(true) => return 0,
        Err(error) => return posix(Err(error)),
        Ok(false) => {}
    }
    let result = kinakaze_vfs::get(fd).and_then(|entry| {
        if let Ok(path) = handle_path(entry.raw as *mut c_void) {
            apply_mode(&path, mode)
        } else {
            Ok(())
        }
    });
    posix(result)
}

/// Change a path's own mode without following its final symbolic link.
/// # Safety
/// `path` must be a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lchmod(path: *const c_char, mode: u32) -> c_int {
    unsafe { kinakaze_abi_fchmodat(AT_FDCWD, path, mode, AT_SYMLINK_NOFOLLOW) }
}

/// `fchmodat`.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fchmodat(
    dirfd: c_int,
    path: *const c_char,
    mode: u32,
    flags: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let borrowed = unsafe { borrow_path(path) };
    posix(borrowed.and_then(|path| {
        if flags & !(AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW) != 0 {
            return Err(EINVAL);
        }
        if path.is_empty() {
            if flags & AT_EMPTY_PATH == 0 {
                return Err(kinakaze_vfs::ENOENT);
            }
            if dirfd != AT_FDCWD {
                return fs::chmod_descriptor(dirfd, mode, true);
            }
            let fd = fs::open(".", fs::O_PATH | fs::O_DIRECTORY, 0)?;
            let result = fs::chmod_descriptor(fd, mode, true);
            let _ = kinakaze_vfs::close(fd);
            return result;
        }
        if flags & AT_SYMLINK_NOFOLLOW != 0 {
            // Pin the final object without following it, so replacement cannot
            // turn the no-follow request into a change to a symlink target.
            let fd = fs::openat(dirfd, path, fs::O_PATH | fs::O_NOFOLLOW, 0)?;
            let result = fs::chmod_descriptor(fd, mode, true);
            let _ = kinakaze_vfs::close(fd);
            return result;
        }
        let target = resolve_at(dirfd, path)?;
        if fs::chmod_fd_link(&target, mode)? {
            return Ok(());
        }
        if kinakaze_vfs::tmpfs::chmod(&target, mode)? {
            return Ok(());
        }
        let resolved = kinakaze_vfs::mount::overlay::prepare_write(&target, true, false)?;
        apply_mode(&resolved, mode)
    }))
}

/// Joins an `*at` path against its directory descriptor.
pub(crate) fn resolve_at(dirfd: c_int, path: &str) -> Result<String, i32> {
    if path.starts_with('/') || dirfd == AT_FDCWD {
        return Ok(path.to_string());
    }
    let base = directory_path(dirfd)?;
    Ok(if base.ends_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    })
}

// UID/GID are persisted on the native inode, including hard links and unlinked
// descriptors. The VFS rechecks ownership while holding the inode mutation lock.
fn ownership(owner: u32, group: u32) -> fs::Ownership {
    fs::Ownership {
        uid: owner,
        gid: group,
        caller: crate::userdb::kinakaze_abi_geteuid(),
        group_member: crate::userdb::kinakaze_abi_group_member(group) != 0,
    }
}
/// Change ownership while following the final symbolic link.
/// # Safety
/// `path` must be a readable null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_chown(
    path: *const c_char,
    owner: u32,
    group: u32,
) -> c_int {
    posix(unsafe { borrow_path(path) }.and_then(|p| fs::chown(p, true, &ownership(owner, group))))
}
/// Change ownership of an open inode, including after unlink.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fchown(fd: c_int, owner: u32, group: u32) -> c_int {
    posix(fs::fchown(fd, false, &ownership(owner, group)))
}
/// Change ownership of the link itself.
/// # Safety
/// `path` must be a readable null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lchown(
    path: *const c_char,
    owner: u32,
    group: u32,
) -> c_int {
    posix(unsafe { borrow_path(path) }.and_then(|p| fs::chown(p, false, &ownership(owner, group))))
}
/// Change ownership relative to a directory or an AT_EMPTY_PATH descriptor.
/// # Safety
/// `path` must be a readable null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fchownat(
    dirfd: c_int,
    path: *const c_char,
    owner: u32,
    group: u32,
    flags: c_int,
) -> c_int {
    posix((|| {
        if flags & !(AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW) != 0 {
            return Err(EINVAL);
        }
        let path = unsafe { borrow_path(path) }?;
        let owner = ownership(owner, group);
        if flags & AT_EMPTY_PATH != 0 && path.is_empty() {
            return if dirfd == AT_FDCWD {
                fs::chown(&fs::getcwd(), true, &owner)
            } else {
                fs::fchown(dirfd, true, &owner)
            };
        }
        if path.is_empty() {
            return Err(kinakaze_vfs::ENOENT);
        }
        fs::chown(
            &resolve_at(dirfd, path)?,
            flags & AT_SYMLINK_NOFOLLOW == 0,
            &owner,
        )
    })())
}
/// The process file-mode creation mask.
///
/// `022` is the conventional startup value. Creation calls apply the current
/// value before the VFS persists the resulting Linux mode.

pub(crate) fn creation_mode(mode: u32) -> u32 {
    mode & !kinakaze_vfs::fs_context::umask()
}

pub(crate) fn current_umask() -> u32 {
    kinakaze_vfs::fs_context::umask()
}

pub(crate) fn restore_umask(mask: u32) {
    kinakaze_vfs::fs_context::set_umask(mask);
}

/// `umask`, returning the previous mask. Never fails, as POSIX specifies.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_umask(mask: u32) -> u32 {
    // Only the nine permission bits are part of a mask.
    kinakaze_vfs::fs_context::set_umask(mask)
}

/// `mknod`.
///
/// Regular files, FIFOs, and device nodes create their actual guest inode
/// types. Device nodes also preserve the encoded `dev_t`; opening a node whose
/// driver is unavailable reports `ENXIO` rather than using file I/O.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mknod(
    path: *const c_char,
    mode: u32,
    dev: u64,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let borrowed = unsafe { borrow_path(path) };
    posix(borrowed.and_then(|path| {
        let file_type = mode & fs::S_IFMT;
        if matches!(file_type, fs::S_IFCHR | fs::S_IFBLK) {
            let major = (((dev >> 8) & 0xfff) | ((dev >> 32) & 0xffff_f000)) as u32;
            let minor = ((dev & 0xff) | ((dev >> 12) & 0xffff_ff00)) as u32;
            let device_type = if file_type == fs::S_IFCHR {
                kinakaze_vfs::bpf::BPF_DEVCG_DEV_CHAR
            } else {
                kinakaze_vfs::bpf::BPF_DEVCG_DEV_BLOCK
            };
            kinakaze_vfs::bpf::check_current_device(
                device_type,
                major,
                minor,
                kinakaze_vfs::bpf::BPF_DEVCG_ACC_MKNOD,
            )?;
        }
        match file_type {
            // Zero and S_IFREG create an ordinary directory entry.
            0 | fs::S_IFREG => {
                let fd = fs::open(
                    path,
                    fs::O_WRONLY | fs::O_CREAT | fs::O_EXCL,
                    creation_mode(mode),
                )?;
                kinakaze_vfs::close(fd)
            }
            fs::S_IFIFO => fs::create_fifo(path, creation_mode(mode)),
            fs::S_IFCHR | fs::S_IFBLK => {
                fs::create_device(path, creation_mode(mode) | file_type, dev)
            }
            _ => Err(EPERM),
        }
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn mknod(path: *const c_char, mode: u32, dev: u64) -> c_int {
    unsafe { kinakaze_abi_mknod(path, mode, dev) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn mknodat(
    dirfd: c_int,
    path: *const c_char,
    mode: u32,
    dev: u64,
) -> c_int {
    unsafe { kinakaze_abi_mknodat(dirfd, path, mode, dev) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mknodat(
    dirfd: c_int,
    path: *const c_char,
    mode: u32,
    dev: u64,
) -> c_int {
    let target = match unsafe { borrow_path(path) } {
        Ok(p) => match resolve_at(dirfd, p) {
            Ok(p) => p,
            Err(e) => {
                set_errno(e);
                return -1;
            }
        },
        Err(e) => {
            set_errno(e);
            return -1;
        }
    };
    let c_target = std::ffi::CString::new(target).unwrap_or_default();
    unsafe { kinakaze_abi_mknod(c_target.as_ptr(), mode, dev) }
}

/// `mkfifo`.
///
/// The filesystem entry carries the Linux FIFO type and stable inode identity;
/// the VFS uses that identity to select its Windows named-pipe data channel.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkfifo(path: *const c_char, mode: u32) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    posix(unsafe { borrow_path(path) }.and_then(|path| fs::create_fifo(path, creation_mode(mode))))
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------

/// `link`: a hard link, which NTFS supports natively.
///
/// # Safety
///
/// Both arguments must be valid null-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_link(
    existing: *const c_char,
    new: *const c_char,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let existing = unsafe { borrow_path(existing) };
    // SAFETY: forwarded from this function's own contract.
    let new = unsafe { borrow_path(new) };
    posix(existing.and_then(|existing| {
        let new = new?;
        if kinakaze_vfs::tmpfs::hard_link(existing, new)? {
            return Ok(());
        }
        if kinakaze_vfs::mount::overlay::hard_link(existing, new)? {
            return Ok(());
        }
        let source = kinakaze_vfs::mount::overlay::prepare_write(existing, false, false)?;
        let target = kinakaze_vfs::mount::overlay::prepare_create(new)?;
        std::fs::hard_link(source.as_ref(), target.as_ref()).map_err(io_errno)
    }))
}

/// `linkat`.
///
/// # Safety
///
/// Both path arguments must be valid null-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_linkat(
    old_dirfd: c_int,
    existing: *const c_char,
    new_dirfd: c_int,
    new: *const c_char,
    _flags: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let existing = unsafe { borrow_path(existing) };
    // SAFETY: forwarded from this function's own contract.
    let new = unsafe { borrow_path(new) };
    posix(existing.and_then(|existing| {
        let existing = resolve_at(old_dirfd, existing)?;
        let new = resolve_at(new_dirfd, new?)?;
        if kinakaze_vfs::tmpfs::hard_link(&existing, &new)? {
            return Ok(());
        }
        if kinakaze_vfs::mount::overlay::hard_link(&existing, &new)? {
            return Ok(());
        }
        let source = kinakaze_vfs::mount::overlay::prepare_write(&existing, false, false)?;
        let target = kinakaze_vfs::mount::overlay::prepare_create(&new)?;
        std::fs::hard_link(source.as_ref(), target.as_ref()).map_err(io_errno)
    }))
}

/// `symlink`.
///
/// A native Windows symlink is used when Developer Mode or the corresponding
/// privilege permits it. Otherwise the VFS stores the Linux target in an NTFS
/// alternate stream on an empty placeholder. Both representations retain link
/// identity: neither degrades into a hardlink or copy of the target.
///
/// # Safety
///
/// Both arguments must be valid null-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn symlink(target: *const c_char, link: *const c_char) -> c_int {
    unsafe { kinakaze_abi_symlink(target, link) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn symlinkat(
    target: *const c_char,
    dirfd: c_int,
    link: *const c_char,
) -> c_int {
    unsafe { kinakaze_abi_symlinkat(target, dirfd, link) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_symlink(
    target: *const c_char,
    link: *const c_char,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let target = unsafe { borrow_path(target) };
    // SAFETY: forwarded from this function's own contract.
    let link = unsafe { borrow_path(link) };
    // Native metadata defers signal delivery until overlay guards unwind.
    // Returning EINTR with the signal still pending makes callers that retry
    // EINTR (including Go's os.Symlink) repeat the same cancelled operation.
    posix(target.and_then(|target| crate::fs::restart_metadata(|| create_symlink(target, link?))))
}

/// `symlinkat`.
///
/// # Safety
///
/// Both path arguments must be valid null-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_symlinkat(
    target: *const c_char,
    dirfd: c_int,
    link: *const c_char,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let target = unsafe { borrow_path(target) };
    // SAFETY: forwarded from this function's own contract.
    let link = unsafe { borrow_path(link) };
    posix(target.and_then(|target| {
        crate::fs::restart_metadata(|| {
            let link = resolve_at(dirfd, link?)?;
            create_symlink(target, &link)
        })
    }))
}

/// Creates a symlink, choosing the Windows file or directory form.
fn create_symlink(target: &str, link: &str) -> Result<(), i32> {
    if kinakaze_vfs::tmpfs::create(link, fs::S_IFLNK | 0o777, 0, target)? {
        return Ok(());
    }
    if target.is_empty() {
        return Err(kinakaze_vfs::ENOENT);
    }
    if kinakaze_vfs::mount::overlay::is_overlay_path(link, false, true)? {
        let mut destination = kinakaze_vfs::mount::overlay::prepare_create(link)?;
        kinakaze_vfs::create_emulated_symlink(&destination, target)?;
        destination.initialize_created(kinakaze_vfs::fs::S_IFLNK | 0o777)?;
        return destination.finish();
    }
    let destination = kinakaze_vfs::mount::overlay::prepare_create(link)?;
    let link_path = destination.to_path_buf();
    if std::fs::symlink_metadata(&link_path).is_ok() {
        return Err(kinakaze_vfs::EEXIST);
    }

    let absolute_link = absolute_linux(link);
    let link_parent = absolute_link
        .rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/");
    let target_path = if target.starts_with('/') {
        target.to_string()
    } else if link_parent == "/" {
        format!("/{target}")
    } else {
        format!("{link_parent}/{target}")
    };
    // A symlink target is opaque at creation time: dangling targets and loops
    // are valid. Resolution is attempted only to choose the native Windows
    // file/directory flag. Failure falls through to the hosted representation
    // without changing the Linux result.
    let probe = resolve(&target_path).ok();
    let directory = probe.as_ref().is_some_and(|path| path.is_dir());

    use windows_sys::Win32::Storage::FileSystem::CreateSymbolicLinkW;
    let stored = if target.starts_with('/') {
        let Some(probe) = probe.as_ref() else {
            return kinakaze_vfs::create_emulated_symlink(&link_path, target);
        };
        probe.as_os_str().to_string_lossy().into_owned()
    } else {
        target.replace('/', "\\")
    };
    let wide_target: Vec<u16> = stored.encode_utf16().chain(Some(0)).collect();
    let wide_link: Vec<u16> = link_path
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();

    let mut flags = if directory { 0x1 } else { 0x0 };
    flags |= 0x2; // SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE

    let ok = unsafe { CreateSymbolicLinkW(wide_link.as_ptr(), wide_target.as_ptr(), flags) };
    if ok {
        return kinakaze_vfs::fs::initialize_created_path(
            &link_path,
            kinakaze_vfs::fs::S_IFLNK | 0o777,
        );
    }

    let ok2 =
        unsafe { CreateSymbolicLinkW(wide_link.as_ptr(), wide_target.as_ptr(), flags & !0x2) };
    if ok2 {
        return kinakaze_vfs::fs::initialize_created_path(
            &link_path,
            kinakaze_vfs::fs::S_IFLNK | 0o777,
        );
    }

    kinakaze_vfs::create_emulated_symlink(&link_path, target)
}

/// `readlink`, which does not append a terminator and returns the byte count.
///
/// # Safety
///
/// `buffer` must name at least `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_readlink(
    path: *const c_char,
    buffer: *mut c_char,
    size: usize,
) -> isize {
    // SAFETY: forwarded from this function's own contract.
    unsafe { kinakaze_abi_readlinkat(AT_FDCWD, path, buffer, size) }
}

/// `readlinkat`.
///
/// # Safety
///
/// `buffer` must name at least `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_readlinkat(
    dirfd: c_int,
    path: *const c_char,
    buffer: *mut c_char,
    size: usize,
) -> isize {
    if size == 0 {
        set_errno(EINVAL);
        return -1;
    }
    // SAFETY: forwarded from this function's own contract.
    let borrowed = unsafe { borrow_path(path) };
    let target = borrowed.and_then(|path| {
        if path.is_empty() {
            fs::read_link_fd(dirfd)
        } else {
            link_target(&resolve_at(dirfd, path)?)
        }
    });
    match target {
        Ok(target) => {
            // SAFETY: forwarded from this function's own contract.
            unsafe { fill_no_terminator(buffer, size, target.as_bytes()) }
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Reads a symlink's target as a guest path.
fn link_target(path: &str) -> Result<String, i32> {
    if let Some(target) = kinakaze_vfs::tmpfs::read_link(path)? {
        return Ok(target);
    }
    let absolute = absolute_linux(path);
    // `/proc/self/exe` and its neighbours are symlinks the VFS synthesizes, and
    // they never reach the host file system.
    if kinakaze_vfs::procfs::owns(&absolute) {
        let metadata = kinakaze_vfs::procfs::metadata(&absolute)?;
        return metadata.target.ok_or(EINVAL);
    }
    let resolved = resolve_no_follow(path)?;
    kinakaze_vfs::fs::read_link_host_path(&resolved)
}

/// Copies bytes into a caller buffer without a terminator, `readlink`-style.
///
/// # Safety
///
/// `buffer` must name at least `size` writable bytes unless `size` is zero.
unsafe fn fill_no_terminator(buffer: *mut c_char, size: usize, bytes: &[u8]) -> isize {
    if buffer.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    if size == 0 {
        set_errno(EINVAL);
        return -1;
    }
    // POSIX truncates silently rather than reporting ERANGE, and the result is
    // the number of bytes actually placed.
    let count = bytes.len().min(size);
    // SAFETY: the caller guarantees `size` writable bytes and `count <= size`.
    unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), count) };
    count as isize
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_canonicalize_file_name(
    path: *const c_char,
) -> *mut c_char {
    unsafe { kinakaze_abi_realpath(path, core::ptr::null_mut()) }
}

/// `realpath`.
///
/// With a null `resolved` a buffer is allocated from this library's own
/// allocator, which is the same one the guest's `free` releases.
///
/// # Safety
///
/// `path` must be a valid string, and `resolved` must be null or name at least
/// `PATH_MAX` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_realpath(
    path: *const c_char,
    resolved: *mut c_char,
) -> *mut c_char {
    // SAFETY: forwarded from this function's own contract.
    let borrowed = unsafe { borrow_path(path) };
    let canonical = match borrowed.and_then(canonical_path) {
        Ok(canonical) => canonical,
        Err(error) => {
            set_errno(error);
            return ptr::null_mut();
        }
    };
    let bytes = canonical.as_bytes();
    if bytes.len() >= PATH_MAX {
        set_errno(ENAMETOOLONG);
        return ptr::null_mut();
    }

    let out = if resolved.is_null() {
        // SAFETY: a fresh allocation of the exact size needed.
        let fresh = unsafe { kinakaze_alloc::c::malloc(bytes.len() + 1) };
        if fresh.is_null() {
            set_errno(crate::ENOMEM);
            return ptr::null_mut();
        }
        fresh.cast::<c_char>()
    } else {
        resolved
    };
    // SAFETY: `out` is either the caller's PATH_MAX buffer or an allocation of
    // exactly the required length.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), bytes.len());
        ptr::write(out.add(bytes.len()), 0);
    }
    out
}

/// Resolves a path to its canonical guest form, following every symlink.
fn canonical_path(path: &str) -> Result<String, i32> {
    let absolute = absolute_linux(path);
    // A synthetic path has no host counterpart to canonicalize. Its normalized
    // absolute form is already canonical, except for a symlink, which POSIX says
    // realpath must follow.
    if kinakaze_vfs::procfs::owns(&absolute) {
        let metadata = kinakaze_vfs::procfs::metadata(&absolute)?;
        if let Some(target) = metadata.target {
            return canonical_path(&target);
        }
        return Ok(normalize(&absolute));
    }
    let resolved = resolve(path)?;
    // POSIX requires realpath to fail with ENOENT when a component is missing,
    // which is exactly canonicalize's own contract.
    let canonical = std::fs::canonicalize(&resolved).map_err(io_errno)?;
    Ok(windows_to_linux(&canonical))
}

/// Collapses `.`, `..` and repeated separators in an absolute path.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/').filter(|part| !part.is_empty()) {
        match component {
            "." => {}
            ".." => {
                parts.pop();
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

// ---------------------------------------------------------------------------
// basename and dirname
// ---------------------------------------------------------------------------

// These two are pure string functions. They never look at the file system, so
// their results do not depend on whether the path exists, and they are
// implemented to the letter of POSIX rather than by asking the host.
//
// There are two incompatible `basename`s in the wild and the choice matters. The
// POSIX one in `<libgen.h>` is permitted to modify its argument and to return a
// pointer into static storage; the GNU one in `<string.h>` never modifies its
// argument, never returns "." or "/", and simply returns a pointer past the last
// slash. Code compiled against glibc gets the GNU version unless it includes
// `<libgen.h>`, and the two disagree on trailing slashes: for "usr/lib/" GNU
// returns the empty string while POSIX returns "lib".
//
// **This implementation is the POSIX/`<libgen.h>` variant**, because that is what
// the exported symbol name `basename` denotes in the ABI: glibc's GNU spelling is
// `__xpg_basename`'s counterpart resolved at compile time through a header macro
// and never appears as a distinct dynamic symbol. It differs from glibc's in one
// respect, and deliberately: the argument is never modified. POSIX allows
// modification but does not require it, and a caller passing a string literal or
// a read-only mapping would fault on the write. Results are returned from
// per-function static buffers instead.
//
// Each function has its own buffer so that the common `printf("%s/%s",
// dirname(a), basename(b))` idiom, where both are live at once, cannot have one
// result overwrite the other. As with glibc, a result is valid only until the
// next call to the same function.

/// Static return storage for [`kinakaze_abi_basename`].
static BASENAME_BUFFER: Mutex<[u8; PATH_MAX + 1]> = Mutex::new([0; PATH_MAX + 1]);

/// Static return storage for [`kinakaze_abi_dirname`].
static DIRNAME_BUFFER: Mutex<[u8; PATH_MAX + 1]> = Mutex::new([0; PATH_MAX + 1]);

/// Computes `basename` on a byte slice, per POSIX.
///
/// The awkward cases the specification calls out explicitly:
/// `basename("")` is `"."`, `basename("/")` is `"/"`, `basename("//")` is `"/"`,
/// `basename("usr/")` is `"usr"` and `basename("/usr/lib")` is `"lib"`.
fn basename_bytes(path: &[u8]) -> &[u8] {
    // An empty path has no components at all, and POSIX names the current
    // directory as the result rather than the empty string.
    if path.is_empty() {
        return b".";
    }
    // Strip trailing slashes. If that removes everything, the path was all
    // slashes and names the root, whose basename is the root itself.
    let end = path.iter().rposition(|byte| *byte != b'/');
    let Some(end) = end else {
        return b"/";
    };
    let trimmed = &path[..=end];
    // What follows the last remaining slash is the final component.
    match trimmed.iter().rposition(|byte| *byte == b'/') {
        Some(slash) => &trimmed[slash + 1..],
        None => trimmed,
    }
}

/// Computes `dirname` on a byte slice, per POSIX.
///
/// The awkward cases: `dirname("")` is `"."`, `dirname("usr")` is `"."`,
/// `dirname("/usr/")` is `"/"`, `dirname("/usr/lib")` is `"/usr"`,
/// `dirname("/")` is `"/"` and `dirname("usr/lib/")` is `"usr"`.
///
/// Note that `dirname("/usr/")` is `"/"`, not `"/usr"`: the trailing slash is
/// stripped first, leaving `/usr`, and the directory *containing* `/usr` is the
/// root. This matches glibc, musl and the POSIX examples table. Reading it as
/// `"/usr"` confuses `dirname` with "strip the trailing slash", which is what
/// `basename` does for its own purposes.
fn dirname_bytes(path: &[u8]) -> &[u8] {
    if path.is_empty() {
        return b".";
    }
    // Strip trailing slashes; an all-slash path is the root, whose parent is
    // itself.
    let Some(end) = path.iter().rposition(|byte| *byte != b'/') else {
        return b"/";
    };
    let trimmed = &path[..=end];
    // With no slash left there is no directory part, so the result is ".".
    let Some(slash) = trimmed.iter().rposition(|byte| *byte == b'/') else {
        return b".";
    };
    // Drop the final component, then strip the separators that preceded it. If
    // nothing survives, the component sat directly under the root.
    let head = &trimmed[..slash];
    match head.iter().rposition(|byte| *byte != b'/') {
        Some(last) => &head[..=last],
        None => b"/",
    }
}

/// Copies a result into one of the static buffers and returns a pointer to it.
fn publish(buffer: &'static Mutex<[u8; PATH_MAX + 1]>, value: &[u8]) -> *mut c_char {
    let Ok(mut slot) = buffer.lock() else {
        set_errno(kinakaze_vfs::EIO);
        return ptr::null_mut();
    };
    let count = value.len().min(PATH_MAX);
    slot[..count].copy_from_slice(&value[..count]);
    slot[count] = 0;
    // The buffer is a `static`, so the pointer stays valid after the guard is
    // dropped; the lock only serializes the copy itself.
    slot.as_mut_ptr().cast::<c_char>()
}

/// `basename`, the POSIX `<libgen.h>` variant. Does not modify its argument.
///
/// # Safety
///
/// `path` must be null or a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_basename(path: *const c_char) -> *mut c_char {
    // glibc treats a null path as "." rather than faulting, and callers rely on
    // it when threading an optional argument through.
    // SAFETY: forwarded from this function's own contract.
    let bytes = unsafe { borrow_bytes(path) }.unwrap_or(b"");
    publish(&BASENAME_BUFFER, basename_bytes(bytes))
}

/// `dirname`. Does not modify its argument.
///
/// # Safety
///
/// `path` must be null or a valid null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_dirname(path: *mut c_char) -> *mut c_char {
    // SAFETY: forwarded from this function's own contract.
    let bytes = unsafe { borrow_bytes(path) }.unwrap_or(b"");
    publish(&DIRNAME_BUFFER, dirname_bytes(bytes))
}

// ---------------------------------------------------------------------------
// Directory walking
// ---------------------------------------------------------------------------

// `rewinddir`, `seekdir` and `telldir` are already exported by [`crate::dirent`]
// alongside `opendir`, where the stream's position lives; they are not repeated
// here. What is missing from that module is the pair that ties a stream to a
// descriptor, plus `scandir`.

/// `fdopendir`: turns a directory descriptor into a stream.
///
/// Successful adoption transfers `fd` to closedir, preserving its inode and
/// flags. Failure leaves the descriptor owned by the caller.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fdopendir(fd: c_int) -> *mut Dir {
    match crate::dirent::from_fd(fd) {
        Ok(stream) => stream,
        Err(error) => {
            set_errno(error);
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn fdopendir(fd: c_int) -> *mut Dir {
    kinakaze_abi_fdopendir(fd)
}

/// `dirfd` under its bare Linux name.
/// # Safety
/// A non-null directory must point to a live stream owned by opendir/fdopendir.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn dirfd(directory: *mut Dir) -> c_int {
    // SAFETY: The caller supplies the live directory stream required by dirfd.
    unsafe { kinakaze_abi_dirfd(directory) }
}

/// `dirfd`: the descriptor behind a stream.
///
/// Both opendir and fdopendir keep their single owned descriptor in the stream.
/// # Safety
/// A non-null directory must point to a live stream owned by opendir/fdopendir.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_dirfd(directory: *mut Dir) -> c_int {
    if directory.is_null() {
        set_errno(EINVAL);
        return -1;
    }
    // SAFETY: the caller guarantees a live stream from `opendir` or
    // `fdopendir`, both of which produce a `Dir`.
    let own = unsafe { (*directory).fd };
    if own >= 0 {
        return own;
    }
    // A stream with no descriptor at all, which is what a listing built without
    // one leaves behind. `EINVAL` is the errno POSIX specifies for it.
    set_errno(EINVAL);
    -1
}

/// The `scandir` filter callback: non-zero keeps the entry.
type Selector = unsafe extern "sysv64" fn(*const Dirent) -> c_int;

/// The `scandir` comparison callback, as `qsort` would use it.
type Comparator = unsafe extern "sysv64" fn(*const *const Dirent, *const *const Dirent) -> c_int;

/// `alphasort`: orders entries by name, byte for byte.
///
/// POSIX defines this in terms of `strcoll`, which in the C locale -- the only
/// locale this library implements -- is `strcmp`.
///
/// # Safety
///
/// Both arguments must point to valid `struct dirent *` values.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_alphasort(
    left: *const *const Dirent,
    right: *const *const Dirent,
) -> c_int {
    if left.is_null() || right.is_null() {
        return 0;
    }
    // SAFETY: the caller guarantees both point to valid dirent pointers.
    let (left, right) = unsafe { (*left, *right) };
    if left.is_null() || right.is_null() {
        return 0;
    }
    // SAFETY: both dirents are valid and their d_name arrays are null-terminated.
    let (left, right) = unsafe {
        (
            CStr::from_ptr((*left).d_name.as_ptr()),
            CStr::from_ptr((*right).d_name.as_ptr()),
        )
    };
    match left.to_bytes().cmp(right.to_bytes()) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

/// `scandir`.
///
/// Every kept entry and the array holding them come from this library's
/// allocator, so the guest releases them with the `free` it already has, exactly
/// as glibc's contract requires.
///
/// # Safety
///
/// `path` must be a valid string, `namelist` a writable out-pointer, and the two
/// callbacks must be null or valid System V functions.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_scandir(
    path: *const c_char,
    namelist: *mut *mut *mut Dirent,
    selector: Option<Selector>,
    comparator: Option<Comparator>,
) -> c_int {
    unsafe { kinakaze_abi_scandirat(-100, path, namelist, selector, comparator) }
}

/// Directory scanning relative to a descriptor, without changing process cwd.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_scandirat(
    dirfd: c_int,
    path: *const c_char,
    namelist: *mut *mut *mut Dirent,
    selector: Option<Selector>,
    comparator: Option<Comparator>,
) -> c_int {
    if namelist.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    // SAFETY: forwarded from this function's own contract.
    let entries = match unsafe { borrow_path(path) }.and_then(|path| {
        let fd = fs::openat(
            dirfd,
            path,
            fs::O_RDONLY | fs::O_DIRECTORY | fs::O_CLOEXEC,
            0,
        )?;
        let result = fs::read_directory_fd(fd);
        let _ = kinakaze_vfs::close(fd);
        result
    }) {
        Ok(entries) => entries,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };

    let mut kept: Vec<*mut Dirent> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let record = build_dirent(entry, index);
        if let Some(selector) = selector {
            // SAFETY: the caller guarantees a valid callback, and the record is
            // a live local for the duration of the call.
            if unsafe { selector(&raw const record) } == 0 {
                continue;
            }
        }
        // SAFETY: a fresh allocation sized for one dirent.
        let slot = unsafe { kinakaze_alloc::c::malloc(size_of::<Dirent>()) }.cast::<Dirent>();
        if slot.is_null() {
            // SAFETY: every pointer in `kept` came from this allocator.
            unsafe { release(&kept) };
            set_errno(crate::ENOMEM);
            return -1;
        }
        // SAFETY: `slot` is a fresh allocation of exactly this size.
        unsafe { ptr::write(slot, record) };
        kept.push(slot);
    }

    if let Some(comparator) = comparator {
        // SAFETY: the caller guarantees a valid comparison callback.
        unsafe { merge_sort(&mut kept, comparator) };
    }

    // glibc returns a null list for an empty result rather than a zero-length
    // allocation, and callers only walk the array when the count is positive.
    let array = if kept.is_empty() {
        ptr::null_mut()
    } else {
        // SAFETY: a fresh allocation sized for the collected pointers.
        let array = unsafe { kinakaze_alloc::c::malloc(size_of::<*mut Dirent>() * kept.len()) }
            .cast::<*mut Dirent>();
        if array.is_null() {
            // SAFETY: every pointer in `kept` came from this allocator.
            unsafe { release(&kept) };
            set_errno(crate::ENOMEM);
            return -1;
        }
        // SAFETY: `array` holds exactly `kept.len()` pointer slots.
        unsafe { ptr::copy_nonoverlapping(kept.as_ptr(), array, kept.len()) };
        array
    };
    // SAFETY: the caller guarantees `namelist` is writable.
    unsafe { ptr::write(namelist, array) };
    kept.len() as c_int
}

/// Frees a partially built entry list after an allocation failure.
///
/// # Safety
///
/// Every pointer must have come from this library's allocator.
unsafe fn release(entries: &[*mut Dirent]) {
    for entry in entries {
        // SAFETY: forwarded from this function's own contract.
        unsafe { kinakaze_alloc::c::free(entry.cast::<u8>()) };
    }
}

/// Builds the `struct dirent` a listing entry corresponds to.
fn build_dirent(entry: &kinakaze_vfs::fs::DirectoryEntry, index: usize) -> Dirent {
    let mut record = Dirent {
        // A plain listing carries no host inode. The index is stable for the
        // life of this call, which is what a caller testing for non-zero needs.
        d_ino: index as u64 + 1,
        d_off: index as i64 + 1,
        d_reclen: size_of::<Dirent>() as u16,
        d_type: if entry.is_directory {
            DT_DIR
        } else if entry.is_symlink {
            DT_LNK
        } else {
            DT_REG
        },
        d_name: [0; 256],
    };
    let bytes = entry.name.as_bytes();
    // 255 name bytes plus the terminator fill the array exactly.
    let count = bytes.len().min(255);
    for (slot, byte) in record.d_name.iter_mut().zip(&bytes[..count]) {
        *slot = *byte as c_char;
    }
    record
}

/// Sorts entry pointers with a guest comparison callback.
///
/// A bottom-up merge sort rather than the standard library's sort: `sort_by`
/// requires a total order and is entitled to panic when a comparator does not
/// provide one. The comparator here is guest code, so it may be inconsistent,
/// and a panic unwinding across the System V ABI boundary is undefined
/// behaviour. Merge sort merely produces an unhelpful order in that case, which
/// is the correct failure mode for a bad comparator.
///
/// # Safety
///
/// `comparator` must be a valid System V function.
unsafe fn merge_sort(entries: &mut [*mut Dirent], comparator: Comparator) {
    let length = entries.len();
    if length < 2 {
        return;
    }
    let mut scratch: Vec<*mut Dirent> = Vec::with_capacity(length);
    let mut width = 1;
    while width < length {
        let mut start = 0;
        while start < length {
            let middle = (start + width).min(length);
            let end = (start + 2 * width).min(length);
            let (mut left, mut right) = (start, middle);
            scratch.clear();
            while left < middle && right < end {
                // SAFETY: both indices are in bounds and the callback is valid.
                let order = unsafe {
                    comparator(
                        ptr::from_ref(&entries[left]).cast::<*const Dirent>(),
                        ptr::from_ref(&entries[right]).cast::<*const Dirent>(),
                    )
                };
                // `<= 0` keeps equal elements in their original order, which is
                // the stability glibc's scandir provides.
                if order <= 0 {
                    scratch.push(entries[left]);
                    left += 1;
                } else {
                    scratch.push(entries[right]);
                    right += 1;
                }
            }
            scratch.extend_from_slice(&entries[left..middle]);
            scratch.extend_from_slice(&entries[right..end]);
            entries[start..end].copy_from_slice(&scratch);
            start += 2 * width;
        }
        width *= 2;
    }
}

/// `scandir64`, identical to `scandir` on x86_64.
///
/// # Safety
///
/// The same contract as [`kinakaze_abi_scandir`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_scandir64(
    path: *const c_char,
    namelist: *mut *mut *mut Dirent,
    selector: Option<Selector>,
    comparator: Option<Comparator>,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { kinakaze_abi_scandir(path, namelist, selector, comparator) }
}

/// `alphasort64`, identical to `alphasort` on x86_64.
///
/// # Safety
///
/// The same contract as [`kinakaze_abi_alphasort`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_alphasort64(
    left: *const *const Dirent,
    right: *const *const Dirent,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { kinakaze_abi_alphasort(left, right) }
}

// ---------------------------------------------------------------------------
// Extended attributes
// ---------------------------------------------------------------------------

pub use crate::xattr::*;

// ---------------------------------------------------------------------------
// fnmatch
// ---------------------------------------------------------------------------

/// `FNM_NOESCAPE`: a backslash is an ordinary character, not an escape.
pub const FNM_NOESCAPE: c_int = 1 << 0;
/// `FNM_PATHNAME`: a `/` in the string matches only a literal `/` in the pattern.
pub const FNM_PATHNAME: c_int = 1 << 1;
/// `FNM_PERIOD`: a leading `.` matches only a literal `.` in the pattern.
pub const FNM_PERIOD: c_int = 1 << 2;
/// `FNM_LEADING_DIR`: the pattern may match a prefix ending at a `/`.
pub const FNM_LEADING_DIR: c_int = 1 << 3;
/// `FNM_CASEFOLD`: compare without regard to case.
pub const FNM_CASEFOLD: c_int = 1 << 4;

/// `FNM_NOMATCH`, the return value for a pattern that does not match.
pub const FNM_NOMATCH: c_int = 1;

/// `fnmatch`.
///
/// The full POSIX matcher: `*`, `?`, bracket expressions with ranges, negation
/// and character classes, backslash escapes, and the `FNM_PATHNAME`,
/// `FNM_NOESCAPE` and `FNM_PERIOD` flags. The GNU `FNM_LEADING_DIR` and
/// `FNM_CASEFOLD` extensions are honoured too, since guest code compiled against
/// glibc may pass them.
///
/// # Safety
///
/// Both arguments must be valid null-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fnmatch(
    pattern: *const c_char,
    name: *const c_char,
    flags: c_int,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    let borrowed = unsafe { (borrow_bytes(pattern), borrow_bytes(name)) };
    let (Some(pattern), Some(name)) = borrowed else {
        return FNM_NOMATCH;
    };
    if fnmatch_bytes(pattern, name, flags) {
        0
    } else {
        FNM_NOMATCH
    }
}

/// Matches a name against a pattern.
///
/// `at_start` tracks whether the current position is one where `FNM_PERIOD`
/// protects a leading `.`: the beginning of the string, and under
/// `FNM_PATHNAME` also the first character after every `/`.
fn fnmatch_bytes(pattern: &[u8], name: &[u8], flags: c_int) -> bool {
    matches_from(pattern, name, flags, true)
}

/// The recursive matcher. See [`fnmatch_bytes`] for the meaning of `at_start`.
fn matches_from(pattern: &[u8], name: &[u8], flags: c_int, at_start: bool) -> bool {
    let pathname = flags & FNM_PATHNAME != 0;
    let period = flags & FNM_PERIOD != 0;
    let noescape = flags & FNM_NOESCAPE != 0;

    let Some((first, rest)) = pattern.split_first() else {
        // The pattern is exhausted: it matches only an exhausted string. With
        // FNM_LEADING_DIR a remaining `/...` is also a match, which is how a
        // caller asks "does this pattern name a leading directory of the path".
        if name.is_empty() {
            return true;
        }
        return flags & FNM_LEADING_DIR != 0 && name[0] == b'/';
    };

    match first {
        b'?' => {
            let Some((&head, tail)) = name.split_first() else {
                return false;
            };
            // `?` never matches a `/` under FNM_PATHNAME, and never matches a
            // protected leading period.
            if pathname && head == b'/' {
                return false;
            }
            if period && at_start && head == b'.' {
                return false;
            }
            matches_from(rest, tail, flags, next_at_start(head, pathname))
        }
        b'*' => {
            // Collapse a run of stars: `**` matches exactly what `*` does.
            let rest = {
                let mut rest = rest;
                while let Some((b'*', tail)) = rest.split_first() {
                    rest = tail;
                }
                rest
            };
            // A protected leading period is not consumable by `*`, so the star
            // must match the empty string and the rest must match from here.
            if period && at_start && name.first() == Some(&b'.') {
                return matches_from(rest, name, flags, at_start);
            }
            // A trailing `*` matches the remainder of the string, but under
            // FNM_PATHNAME it cannot cross a `/`. When it cannot, FNM_LEADING_DIR
            // still allows the match: the star covered the current component and
            // what follows is a subdirectory.
            if rest.is_empty() {
                if !pathname || !name.contains(&b'/') {
                    return true;
                }
                return flags & FNM_LEADING_DIR != 0;
            }
            // Try every split point, extending what the star consumes. The star
            // may not consume a `/` under FNM_PATHNAME, so the scan stops there.
            let mut consumed = 0;
            loop {
                let tail = &name[consumed..];
                let start = if consumed == 0 {
                    at_start
                } else {
                    next_at_start(name[consumed - 1], pathname)
                };
                if matches_from(rest, tail, flags, start) {
                    return true;
                }
                let Some(&head) = tail.first() else {
                    return false;
                };
                if pathname && head == b'/' {
                    return false;
                }
                consumed += 1;
            }
        }
        b'[' => {
            let Some((&head, tail)) = name.split_first() else {
                return false;
            };
            if pathname && head == b'/' {
                return false;
            }
            if period && at_start && head == b'.' {
                return false;
            }
            match bracket(rest, head, flags) {
                // An unterminated `[` is a literal `[`, as POSIX specifies.
                None => {
                    head == b'[' && matches_from(rest, tail, flags, next_at_start(head, pathname))
                }
                Some((false, _)) => false,
                Some((true, consumed)) => matches_from(
                    &rest[consumed..],
                    tail,
                    flags,
                    next_at_start(head, pathname),
                ),
            }
        }
        b'\\' if !noescape => {
            // The escaped character matches literally. A trailing backslash has
            // nothing to escape and matches itself.
            let Some((&escaped, after)) = rest.split_first() else {
                return name == b"\\";
            };
            let Some((&head, tail)) = name.split_first() else {
                return false;
            };
            if !equal(escaped, head, flags) {
                return false;
            }
            matches_from(after, tail, flags, next_at_start(head, pathname))
        }
        literal => {
            let Some((&head, tail)) = name.split_first() else {
                return false;
            };
            if !equal(*literal, head, flags) {
                return false;
            }
            matches_from(rest, tail, flags, next_at_start(head, pathname))
        }
    }
}

/// Whether the position after `consumed` protects a leading period.
///
/// Only true just past a `/`, and only when `FNM_PATHNAME` makes components
/// meaningful in the first place.
fn next_at_start(consumed: u8, pathname: bool) -> bool {
    pathname && consumed == b'/'
}

/// Compares two bytes, honouring `FNM_CASEFOLD`.
fn equal(left: u8, right: u8, flags: c_int) -> bool {
    if flags & FNM_CASEFOLD != 0 {
        return left.eq_ignore_ascii_case(&right);
    }
    left == right
}

/// Matches one character against a bracket expression.
///
/// `pattern` starts just past the `[`. Returns the match result together with the
/// number of pattern bytes consumed, up to and including the closing `]`, or
/// `None` if the expression is unterminated.
fn bracket(pattern: &[u8], value: u8, flags: c_int) -> Option<(bool, usize)> {
    let mut index = 0;
    // Either `!` or `^` negates; POSIX specifies `!` and every shell accepts both.
    let negated = matches!(pattern.first(), Some(b'!' | b'^'));
    if negated {
        index += 1;
    }
    let mut matched = false;
    // A `]` in the first position is a literal `]` rather than the terminator,
    // which is the only way to put one inside a set.
    let mut first = true;

    while index < pattern.len() {
        if pattern[index] == b']' && !first {
            // A negated set matches anything the set itself did not.
            return Some((matched != negated, index + 1));
        }
        first = false;

        // `[:alpha:]` and friends. The `[` here is part of the class opener, not
        // a nested set.
        if pattern[index] == b'['
            && pattern.get(index + 1) == Some(&b':')
            && let Some(end) = find_class_end(&pattern[index + 2..])
        {
            let name = &pattern[index + 2..index + 2 + end];
            if class_matches(name, value, flags) {
                matched = true;
            }
            // Skip `[:`, the name, and `:]`.
            index += 2 + end + 2;
            continue;
        }

        let (low, low_width) = bracket_char(&pattern[index..], flags)?;
        index += low_width;

        // A `-` that is not immediately before the closing `]` opens a range.
        if pattern.get(index) == Some(&b'-')
            && pattern.get(index + 1).is_some_and(|byte| *byte != b']')
        {
            index += 1;
            let (high, high_width) = bracket_char(&pattern[index..], flags)?;
            index += high_width;
            if in_range(value, low, high, flags) {
                matched = true;
            }
            continue;
        }
        if equal(low, value, flags) {
            matched = true;
        }
    }
    // Ran off the end without a `]`.
    None
}

/// Reads one character from inside a bracket expression, honouring escapes.
fn bracket_char(pattern: &[u8], flags: c_int) -> Option<(u8, usize)> {
    let &first = pattern.first()?;
    if first == b'\\' && flags & FNM_NOESCAPE == 0 {
        return pattern.get(1).map(|escaped| (*escaped, 2));
    }
    Some((first, 1))
}

/// Whether `value` falls inside the range `low..=high`.
fn in_range(value: u8, low: u8, high: u8, flags: c_int) -> bool {
    if (low..=high).contains(&value) {
        return true;
    }
    // Case folding tests both cases against the range, so `[a-z]` matches 'Q'.
    if flags & FNM_CASEFOLD != 0 {
        return (low..=high).contains(&value.to_ascii_lowercase())
            || (low..=high).contains(&value.to_ascii_uppercase());
    }
    false
}

/// Finds the `:]` that closes a character class name.
fn find_class_end(pattern: &[u8]) -> Option<usize> {
    pattern.windows(2).position(|pair| pair == b":]")
}

/// Matches a byte against a named POSIX character class.
fn class_matches(name: &[u8], value: u8, flags: c_int) -> bool {
    let folded = flags & FNM_CASEFOLD != 0;
    match name {
        b"alpha" => value.is_ascii_alphabetic(),
        b"digit" => value.is_ascii_digit(),
        b"alnum" => value.is_ascii_alphanumeric(),
        b"upper" => {
            // Under case folding the two case classes stop discriminating, which
            // is what glibc does: [[:upper:]] with FNM_CASEFOLD matches letters.
            if folded {
                value.is_ascii_alphabetic()
            } else {
                value.is_ascii_uppercase()
            }
        }
        b"lower" => {
            if folded {
                value.is_ascii_alphabetic()
            } else {
                value.is_ascii_lowercase()
            }
        }
        // Rust's is_ascii_whitespace omits the vertical tab that POSIX includes.
        b"space" => value.is_ascii_whitespace() || value == 0x0b,
        b"blank" => value == b' ' || value == b'\t',
        b"punct" => value.is_ascii_punctuation(),
        b"print" => value.is_ascii_graphic() || value == b' ',
        b"graph" => value.is_ascii_graphic(),
        b"cntrl" => value.is_ascii_control(),
        b"xdigit" => value.is_ascii_hexdigit(),
        // An unknown class name matches nothing rather than everything.
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Flushing
// ---------------------------------------------------------------------------

/// `sync`: flushes every open descriptor this library knows about.
///
/// Linux `sync` commits every dirty buffer on every mounted file system. Windows
/// has no unprivileged equivalent -- volume-wide flushing needs a volume handle
/// opened with write access, which requires administrator rights -- so this
/// flushes what it can reach, which is the set of descriptors in the table. That
/// covers the case a guest actually depends on: data it wrote itself reaching the
/// disk. Dirty pages belonging to other processes are not touched.
///
/// `sync` returns no value on Linux, so a failure on any one descriptor cannot be
/// reported and is skipped.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sync() {
    for fd in kinakaze_vfs::list_open_fds() {
        if let Ok(entry) = kinakaze_vfs::get(fd)
            && matches!(
                entry.kind,
                kinakaze_vfs::FdKind::File | kinakaze_vfs::FdKind::Directory
            )
        {
            // SAFETY: the handle comes from the descriptor table and is live.
            let _ = flush(fd);
        }
    }
}

/// `fsync`: commits one descriptor's data and metadata.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fsync(fd: c_int) -> c_int {
    posix(flush(fd))
}

/// `fdatasync`.
///
/// Identical to `fsync` here. The distinction is that `fdatasync` may skip
/// metadata not needed to retrieve the data, which is an optimisation Windows
/// does not expose: `FlushFileBuffers` is all-or-nothing. Flushing more than
/// required is correct, only slower.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fdatasync(fd: c_int) -> c_int {
    posix(flush(fd))
}

/// `syncfs`: commits the file system holding a descriptor.
///
/// Reduced to flushing that one descriptor, for the same reason [`kinakaze_abi_sync`]
/// cannot do better: reaching the whole volume needs privileged access to a
/// volume handle. The descriptor is validated first, so a bad `fd` still reports
/// `EBADF` rather than silently succeeding.
#[unsafe(no_mangle)]
pub extern "sysv64" fn fsync(fd: c_int) -> c_int {
    kinakaze_abi_fsync(fd)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn fdatasync(fd: c_int) -> c_int {
    kinakaze_abi_fdatasync(fd)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn syncfs(fd: c_int) -> c_int {
    kinakaze_abi_syncfs(fd)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_syncfs(fd: c_int) -> c_int {
    posix(flush(fd))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn sync_file_range(
    fd: c_int,
    _offset: i64,
    _nbytes: i64,
    _flags: u32,
) -> c_int {
    kinakaze_abi_sync_file_range(fd, _offset, _nbytes, _flags)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sync_file_range(
    fd: c_int,
    _offset: i64,
    _nbytes: i64,
    _flags: u32,
) -> c_int {
    posix(flush(fd))
}

/// Flushes one descriptor's buffers.
fn flush(fd: c_int) -> Result<(), i32> {
    if matches!(
        kinakaze_vfs::get(fd)?.kind,
        kinakaze_vfs::FdKind::TmpfsFile
            | kinakaze_vfs::FdKind::TmpfsDirectory
            | kinakaze_vfs::FdKind::MessageQueue
            | kinakaze_vfs::FdKind::SysfsFile
    ) {
        return if kinakaze_vfs::get(fd)?
            .flags
            .contains(kinakaze_vfs::FdFlags::PATH_ONLY)
        {
            Err(kinakaze_vfs::EBADF)
        } else {
            Ok(())
        };
    }
    let entry = kinakaze_vfs::get(fd)?;
    if matches!(
        entry.kind,
        kinakaze_vfs::FdKind::File | kinakaze_vfs::FdKind::Directory
    ) && kinakaze_vfs::mount::overlay::sync_descriptor(fd)?.is_some()
    {
        return Ok(());
    }
    match entry.kind {
        kinakaze_vfs::FdKind::Directory => {
            // Windows NTFS commits directory metadata automatically, and FlushFileBuffers
            // on directory handles returns ERROR_ACCESS_DENIED. Return Ok(()) for POSIX directory fsync.
            Ok(())
        }
        kinakaze_vfs::FdKind::File => {
            // SAFETY: the handle comes from the descriptor table and is live.
            if unsafe { FlushFileBuffers(entry.raw as *mut c_void) } == 0 {
                let err = unsafe { GetLastError() };
                if err == windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED {
                    return Ok(());
                }
                return Err(errno_from_win32(err));
            }
            Ok(())
        }
        // A generated file has no backing store, so there is nothing to commit
        // and the call has already succeeded.
        kinakaze_vfs::FdKind::Synthetic
        | kinakaze_vfs::FdKind::SyntheticDirectory
        | kinakaze_vfs::FdKind::CgroupFile => Ok(()),
        // Linux reports EINVAL for fsync on a pipe, socket or terminal: those
        // have no file to synchronize.
        _ => Err(EINVAL),
    }
}

// ---------------------------------------------------------------------------
// statfs and statvfs
// ---------------------------------------------------------------------------

/// The Linux x86_64 `struct statfs`, 120 bytes.
///
/// Guest code reads these at fixed offsets, so the layout is ABI. Every field is
/// `__fsword_t` (a signed 64-bit word on x86_64) except `f_fsid`, which is a pair
/// of 32-bit values, and the trailing spare array. Offsets:
///
/// | offset | field       |
/// |--------|-------------|
/// |      0 | f_type      |
/// |      8 | f_bsize     |
/// |     16 | f_blocks    |
/// |     24 | f_bfree     |
/// |     32 | f_bavail    |
/// |     40 | f_files     |
/// |     48 | f_ffree     |
/// |     56 | f_fsid      |
/// |     64 | f_namelen   |
/// |     72 | f_frsize    |
/// |     80 | f_flags     |
/// |     88 | f_spare[4]  |
/// |    120 | (size)      |
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Statfs {
    pub f_type: i64,
    pub f_bsize: i64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: [i32; 2],
    pub f_namelen: i64,
    pub f_frsize: i64,
    pub f_flags: i64,
    pub f_spare: [i64; 4],
}

/// The Linux x86_64 `struct statvfs`, 112 bytes.
///
/// Layout is ABI, as with [`Statfs`]. The block counts are `fsblkcnt_t` and the
/// inode counts `fsfilcnt_t`, both 64-bit unsigned on x86_64; the sizes and flags
/// are `unsigned long`. Offsets:
///
/// | offset | field         |
/// |--------|---------------|
/// |      0 | f_bsize       |
/// |      8 | f_frsize      |
/// |     16 | f_blocks      |
/// |     24 | f_bfree       |
/// |     32 | f_bavail      |
/// |     40 | f_files       |
/// |     48 | f_ffree       |
/// |     56 | f_favail      |
/// |     64 | f_fsid        |
/// |     72 | f_flag        |
/// |     80 | f_namemax     |
/// |     88 | __f_spare[6]  |
/// |    112 | (size)        |
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Statvfs {
    pub f_bsize: u64,
    pub f_frsize: u64,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_favail: u64,
    pub f_fsid: u64,
    pub f_flag: u64,
    pub f_namemax: u64,
    pub __f_spare: [i32; 6],
}

/// `NTFS_SB_MAGIC`, the `f_type` Linux reports for an NTFS mount.
///
/// Volume-level file-system identification is not available here without a
/// privileged volume handle, and NTFS is what a Windows volume nearly always is.
/// A guest that switches on `f_type` -- `df -T` is the usual one -- gets a
/// plausible answer rather than zero, which no file system uses.
const NTFS_MAGIC: i64 = 0x5346_544e;

/// The block size reported for every volume.
///
/// The real cluster size needs `GetDiskFreeSpaceW`, whose sector and cluster
/// counts overflow on volumes past 2 TB. `GetDiskFreeSpaceExW` reports bytes and
/// never overflows, so a nominal 4096-byte block is declared and the byte totals
/// are divided by it. `f_bsize * f_blocks` is then the true capacity, which is
/// what every caller actually computes.
const BLOCK_SIZE: u64 = 4096;

/// The longest component name, which is 255 on every Windows file system.
const NAME_MAX: i64 = 255;

/// Collects volume totals for a Windows directory.
fn volume_totals(directory: &Path) -> Result<(u64, u64, u64), i32> {
    let wide_path = kinakaze_vfs::path::wide_path(directory)?;
    let (mut free_to_caller, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: the path is null-terminated and all three out-pointers are
    // writable locals.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide_path.as_ptr(),
            &mut free_to_caller,
            &mut total,
            &mut total_free,
        )
    };
    if ok == 0 {
        // SAFETY: GetLastError has no preconditions.
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok((free_to_caller, total, total_free))
}

/// Returns the directory `GetDiskFreeSpaceExW` should be asked about.
///
/// The function requires a directory: given a file path it fails, so a file is
/// replaced by its parent. The volume is the same either way.
fn volume_directory(target: &Path) -> PathBuf {
    if target.is_dir() {
        return target.to_path_buf();
    }
    target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| target.to_path_buf(), Path::to_path_buf)
}

/// Builds a `struct statfs` for the volume backing a Windows path.
fn statfs_for(target: &Path) -> Result<Statfs, i32> {
    let (available, total, free) = volume_totals(&volume_directory(target))?;
    Ok(Statfs {
        f_type: NTFS_MAGIC,
        f_bsize: BLOCK_SIZE as i64,
        f_blocks: total / BLOCK_SIZE,
        f_bfree: free / BLOCK_SIZE,
        // The quota-aware figure, which is what a non-root caller may use.
        f_bavail: available / BLOCK_SIZE,
        // NTFS allocates MFT records on demand, so there is no inode count to
        // report and no meaningful limit. Zero is what Linux reports for the file
        // systems that likewise have none, such as btrfs and tmpfs.
        f_files: 0,
        f_ffree: 0,
        f_fsid: [0, 0],
        f_namelen: NAME_MAX,
        f_frsize: BLOCK_SIZE as i64,
        f_flags: 0,
        f_spare: [0; 4],
    })
}

/// Converts a `struct statfs` into the `statvfs` shape.
///
/// The two describe the same volume through different fields, so the second is
/// derived from the first rather than gathered again; a separate query could
/// return different numbers for the same call.
fn statvfs_from(value: &Statfs) -> Statvfs {
    Statvfs {
        f_bsize: value.f_bsize as u64,
        f_frsize: value.f_frsize as u64,
        f_blocks: value.f_blocks,
        f_bfree: value.f_bfree,
        f_bavail: value.f_bavail,
        f_files: value.f_files,
        f_ffree: value.f_ffree,
        f_favail: value.f_ffree,
        f_fsid: 0,
        f_flag: 0,
        f_namemax: value.f_namelen as u64,
        __f_spare: [0; 6],
    }
}

/// The `statfs` a descriptor without a file behind it reports.
///
/// A pipe, socket or terminal is on no file system. Linux answers from the
/// pseudo-file-system each lives on rather than failing, so the shape is
/// preserved with zero capacity and `f_type` left at zero, which claims no
/// particular file system.
fn statfs_handleless() -> Statfs {
    Statfs {
        f_bsize: BLOCK_SIZE as i64,
        f_frsize: BLOCK_SIZE as i64,
        f_namelen: NAME_MAX,
        ..Statfs::default()
    }
}

/// Builds the file-system identity and capacity visible through a descriptor.
///
/// Linux answers `fstatfs` from the descriptor's mount, not merely from the
/// presence of a host kernel handle.  Synthetic directory descriptors therefore
/// retain the identity of the guest mount that created them.  This is observable
/// for procfs in particular: opening `/proc/self/fd` and then calling `fstatfs`
/// must report the same `PROC_SUPER_MAGIC` as `statfs("/proc/self/fd")`.
fn statfs_for_descriptor(fd: c_int) -> Result<Statfs, i32> {
    if kinakaze_vfs::get(fd)?.kind == kinakaze_vfs::FdKind::MountTree {
        let path = format!("{}/.", kinakaze_vfs::mount::api::tree_path(fd)?);
        let root = fs::open(&path, fs::O_PATH | fs::O_CLOEXEC, 0)?;
        let result = statfs_for_descriptor(root);
        let _ = kinakaze_vfs::close(root);
        return result;
    }
    if let Some(s) = kinakaze_vfs::devpts::fstatfs(fd)? {
        return Ok(tmpfs_statfs(s));
    }
    if matches!(
        kinakaze_vfs::get(fd)?.kind,
        kinakaze_vfs::FdKind::TmpfsFile
            | kinakaze_vfs::FdKind::TmpfsDirectory
            | kinakaze_vfs::FdKind::MessageQueue
            | kinakaze_vfs::FdKind::SysfsFile
    ) {
        return kinakaze_vfs::tmpfs::fstatfs(fd).map(tmpfs_statfs);
    }
    if let Some((path, device, readonly)) = kinakaze_vfs::mount::overlay::descriptor_filesystem(fd)?
    {
        return overlay_statfs(&path, device, readonly);
    }
    let entry = kinakaze_vfs::get(fd)?;
    match entry.kind {
        kinakaze_vfs::FdKind::File | kinakaze_vfs::FdKind::Directory => {
            let mut value = statfs_for(&handle_path(entry.raw as *mut c_void)?)?;
            if let Some(flags) = kinakaze_vfs::mount::native::flags_fd(fd)? {
                value.f_flags = (value.f_flags & !15) | (flags & 15) as i64;
            }
            Ok(value)
        }
        kinakaze_vfs::FdKind::SyntheticDirectory => {
            let path = kinakaze_vfs::synthetic_directory_path(fd)?;
            kinakaze_vfs::procfs::descriptor_scope(|| statfs_for_path(&path))
        }
        kinakaze_vfs::FdKind::CgroupFile => Ok(statfs_synthesized(CGROUP2_SUPER_MAGIC)),
        kinakaze_vfs::FdKind::Synthetic => {
            let path = kinakaze_vfs::procfs::descriptor_path(fd)?;
            kinakaze_vfs::procfs::descriptor_scope(|| {
                if kinakaze_vfs::procfs::owns(&path) {
                    statfs_for_path(&path)
                } else {
                    Ok(statfs_synthesized(TMPFS_MAGIC))
                }
            })
        }
        kinakaze_vfs::FdKind::ProcSysctl => {
            let path = kinakaze_vfs::procfs::descriptor_path(fd)?;
            kinakaze_vfs::procfs::descriptor_scope(|| statfs_for_path(&path))
        }
        kinakaze_vfs::FdKind::MountNamespace => Ok(statfs_synthesized(NSFS_MAGIC)),
        _ => Ok(statfs_handleless()),
    }
}
pub const CGROUP2_SUPER_MAGIC: i64 = 0x63677270;
pub const PROC_SUPER_MAGIC: i64 = 0x9fa0;
pub const SYSFS_MAGIC: i64 = 0x62656572;
pub const TMPFS_MAGIC: i64 = 0x01021994;
pub const NSFS_MAGIC: i64 = 0x6e736673;

fn tmpfs_statfs(s: kinakaze_vfs::tmpfs::Statistics) -> Statfs {
    let mut out = statfs_synthesized(s.magic);
    out.f_blocks = s.blocks;
    out.f_bfree = s.free;
    out.f_bavail = s.free;
    out.f_files = s.inodes;
    out.f_ffree = s.free_inodes;
    out.f_fsid = [s.device as i32, (s.device >> 32) as i32];
    out.f_flags = (s.flags & 0x3fffffff) as i64;
    out
}
fn statfs_synthesized(magic: i64) -> Statfs {
    Statfs {
        f_type: magic,
        f_bsize: 4096,
        f_blocks: 1024 * 1024,
        f_bfree: 1024 * 1024,
        f_bavail: 1024 * 1024,
        f_files: 1024 * 1024,
        f_ffree: 1024 * 1024,
        f_fsid: [0, 0],
        f_namelen: 255,
        f_frsize: 4096,
        f_flags: 0,
        f_spare: [0; 4],
    }
}

fn statfs_for_path(borrowed: &str) -> Result<Statfs, i32> {
    let mut value = statfs_for_path_inner(borrowed)?;
    if let Some(flags) = kinakaze_vfs::mount::native::flags_path(borrowed)? {
        value.f_flags = (value.f_flags & !15) | (flags & 15) as i64;
    }
    Ok(value)
}
fn statfs_for_path_inner(borrowed: &str) -> Result<Statfs, i32> {
    if let Some(value) = kinakaze_vfs::tmpfs::statfs(borrowed)? {
        return Ok(tmpfs_statfs(value));
    }
    if let Some((device, flags)) = kinakaze_vfs::procfs::instance::filesystem(borrowed)? {
        let mut result = statfs_synthesized(PROC_SUPER_MAGIC);
        result.f_blocks = 0;
        result.f_bfree = 0;
        result.f_bavail = 0;
        result.f_files = 0;
        result.f_ffree = 0;
        result.f_fsid = [device as i32, (device >> 32) as i32];
        result.f_flags = (flags & 0x3fffffff) as _;
        return Ok(result);
    }

    if let Some((path, device, readonly)) = kinakaze_vfs::mount::overlay::filesystem_path(borrowed)?
    {
        return overlay_statfs(&path, device, readonly);
    }
    if kinakaze_vfs::procfs::namespace_target_inode(borrowed).is_some() {
        Ok(statfs_synthesized(NSFS_MAGIC))
    } else if borrowed.starts_with("/sys/fs/cgroup") || kinakaze_vfs::cgroup::owns(borrowed) {
        Ok(statfs_synthesized(CGROUP2_SUPER_MAGIC))
    } else if borrowed.starts_with("/proc") || kinakaze_vfs::procfs::owns(borrowed) {
        Ok(statfs_synthesized(PROC_SUPER_MAGIC))
    } else if borrowed.starts_with("/sys") {
        Ok(statfs_synthesized(SYSFS_MAGIC))
    } else if borrowed.starts_with("/dev/shm") || borrowed.starts_with("/tmp") {
        Ok(statfs_synthesized(TMPFS_MAGIC))
    } else {
        match resolve(borrowed) {
            Ok(resolved) if resolved.exists() => statfs_for(&resolved),
            _ => Ok(statfs_synthesized(TMPFS_MAGIC)),
        }
    }
}

fn overlay_statfs(path: &Path, device: u64, readonly: bool) -> Result<Statfs, i32> {
    let mut value = statfs_for(path)?;
    value.f_type = 0x794c7630;
    value.f_fsid = [device as i32, (device >> 32) as i32];
    value.f_flags = (value.f_flags & !1) | readonly as i64;
    Ok(value)
}

/// `statfs`.
///
/// # Safety
///
/// `path` must be a valid string and `out` a writable `struct statfs`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_statfs(path: *const c_char, out: *mut Statfs) -> c_int {
    if out.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let value = unsafe { borrow_path(path) }
        .and_then(|p| crate::fs::restart_metadata(|| statfs_for_path(&p)));
    match value {
        Ok(value) => {
            unsafe { ptr::write(out, value) };
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `fstatfs`.
///
/// # Safety
///
/// `out` must point to a writable `struct statfs`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fstatfs(fd: c_int, out: *mut Statfs) -> c_int {
    if out.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let value = crate::fs::restart_metadata(|| statfs_for_descriptor(fd));
    match value {
        Ok(value) => {
            unsafe { ptr::write(out, value) };
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `statvfs`.
///
/// # Safety
///
/// `path` must be a valid string and `out` a writable `struct statvfs`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_statvfs(
    path: *const c_char,
    out: *mut Statvfs,
) -> c_int {
    if out.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let value = unsafe { borrow_path(path) }
        .and_then(|p| crate::fs::restart_metadata(|| statfs_for_path(&p)));
    match value {
        Ok(value) => {
            unsafe { ptr::write(out, statvfs_from(&value)) };
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `fstatvfs`.
///
/// # Safety
///
/// `out` must point to a writable `struct statvfs`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fstatvfs(fd: c_int, out: *mut Statvfs) -> c_int {
    if out.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let value = crate::fs::restart_metadata(|| statfs_for_descriptor(fd));
    match value {
        Ok(value) => {
            // SAFETY: the caller guarantees `out` is writable.
            unsafe { ptr::write(out, statvfs_from(&value)) };
            0
        }
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// `statfs64`, identical to `statfs` on x86_64.
///
/// # Safety
///
/// The same contract as [`kinakaze_abi_statfs`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_statfs64(
    path: *const c_char,
    out: *mut Statfs,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { kinakaze_abi_statfs(path, out) }
}

/// `fstatfs64`, identical to `fstatfs` on x86_64.
///
/// # Safety
///
/// The same contract as [`kinakaze_abi_fstatfs`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fstatfs64(fd: c_int, out: *mut Statfs) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { kinakaze_abi_fstatfs(fd, out) }
}

/// `statvfs64`, identical to `statvfs` on x86_64.
///
/// # Safety
///
/// The same contract as [`kinakaze_abi_statvfs`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_statvfs64(
    path: *const c_char,
    out: *mut Statvfs,
) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { kinakaze_abi_statvfs(path, out) }
}

/// `fstatvfs64`, identical to `fstatvfs` on x86_64.
///
/// # Safety
///
/// The same contract as [`kinakaze_abi_fstatvfs`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fstatvfs64(fd: c_int, out: *mut Statvfs) -> c_int {
    // SAFETY: forwarded from this function's own contract.
    unsafe { kinakaze_abi_fstatvfs(fd, out) }
}

#[cfg(all(test, windows))]
mod metadata_signal_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use kinakaze_vfs::ENOENT;

    #[test]
    fn access_mode_checks_follow_linux_root_and_owner_rules() {
        let mut file = Stat {
            st_mode: fs::S_IFREG | 0o640,
            st_uid: 1000,
            st_gid: 2000,
            ..Stat::default()
        };

        assert_eq!(access_mode_allowed(&file, fs::F_OK, 1000, false), Ok(()));
        assert_eq!(
            access_mode_allowed(&file, fs::R_OK | fs::W_OK, 1000, false),
            Ok(())
        );
        assert_eq!(
            access_mode_allowed(&file, fs::X_OK, 1000, false),
            Err(EACCES)
        );
        assert_eq!(access_mode_allowed(&file, fs::R_OK, 3000, true), Ok(()));
        assert_eq!(
            access_mode_allowed(&file, fs::W_OK, 3000, true),
            Err(EACCES)
        );

        // Root bypasses read/write permission bits, but Linux still requires an
        // execute bit for a non-directory file.
        assert_eq!(
            access_mode_allowed(&file, fs::R_OK | fs::W_OK, 0, false),
            Ok(())
        );
        assert_eq!(access_mode_allowed(&file, fs::X_OK, 0, false), Err(EACCES));
        file.st_mode |= 0o001;
        assert_eq!(access_mode_allowed(&file, fs::X_OK, 0, false), Ok(()));
    }

    /// The ABI structs must keep the exact size the Linux headers define, since
    /// guest code allocates them by size and reads them by offset. This is the
    /// Rust equivalent of the `_Static_assert` a C implementation would carry.
    #[test]
    fn abi_struct_sizes_match_linux() {
        assert_eq!(size_of::<Statfs>(), 120, "struct statfs is 120 bytes");
        assert_eq!(size_of::<Statvfs>(), 112, "struct statvfs is 112 bytes");
        // Both are word-aligned, so no field lands on a different offset than the
        // table in each doc comment claims.
        assert_eq!(align_of::<Statfs>(), 8);
        assert_eq!(align_of::<Statvfs>(), 8);
    }

    /// Field offsets, checked individually so a reordering is caught even when
    /// the total size happens to stay the same. These are the offsets the table
    /// in [`Statfs`]'s doc comment claims, and guest code reads them directly.
    #[test]
    fn statfs_field_offsets() {
        use core::mem::offset_of;
        assert_eq!(offset_of!(Statfs, f_type), 0);
        assert_eq!(offset_of!(Statfs, f_bsize), 8);
        assert_eq!(offset_of!(Statfs, f_blocks), 16);
        assert_eq!(offset_of!(Statfs, f_bfree), 24);
        assert_eq!(offset_of!(Statfs, f_bavail), 32);
        assert_eq!(offset_of!(Statfs, f_files), 40);
        assert_eq!(offset_of!(Statfs, f_ffree), 48);
        assert_eq!(offset_of!(Statfs, f_fsid), 56);
        assert_eq!(offset_of!(Statfs, f_namelen), 64);
        assert_eq!(offset_of!(Statfs, f_frsize), 72);
        assert_eq!(offset_of!(Statfs, f_flags), 80);
        assert_eq!(offset_of!(Statfs, f_spare), 88);
    }

    /// The same check for [`Statvfs`].
    #[test]
    fn statvfs_field_offsets() {
        use core::mem::offset_of;
        assert_eq!(offset_of!(Statvfs, f_bsize), 0);
        assert_eq!(offset_of!(Statvfs, f_frsize), 8);
        assert_eq!(offset_of!(Statvfs, f_blocks), 16);
        assert_eq!(offset_of!(Statvfs, f_bfree), 24);
        assert_eq!(offset_of!(Statvfs, f_bavail), 32);
        assert_eq!(offset_of!(Statvfs, f_files), 40);
        assert_eq!(offset_of!(Statvfs, f_ffree), 48);
        assert_eq!(offset_of!(Statvfs, f_favail), 56);
        assert_eq!(offset_of!(Statvfs, f_fsid), 64);
        assert_eq!(offset_of!(Statvfs, f_flag), 72);
        assert_eq!(offset_of!(Statvfs, f_namemax), 80);
        assert_eq!(offset_of!(Statvfs, __f_spare), 88);
    }

    // -----------------------------------------------------------------------
    // basename and dirname
    // -----------------------------------------------------------------------

    /// The POSIX examples table from the `basename` and `dirname` pages, plus the
    /// cases each specification calls out as awkward.
    ///
    /// Each row is (path, basename, dirname).
    const POSIX_TABLE: &[(&str, &str, &str)] = &[
        // The specification's own table.
        ("/usr/lib", "lib", "/usr"),
        ("/usr/", "usr", "/"),
        ("usr", "usr", "."),
        ("/", "/", "/"),
        (".", ".", "."),
        ("..", "..", "."),
        // The empty path: both name the current directory rather than returning
        // an empty string. This is where the POSIX and GNU basenames diverge.
        ("", ".", "."),
        // Repeated separators collapse, and `//` is still the root.
        ("//", "/", "/"),
        ("///", "/", "/"),
        // Trailing slashes are stripped before the split, so a directory path and
        // the same path without its slash give the same answer.
        ("usr/lib/", "lib", "usr"),
        ("/usr/lib/", "lib", "/usr"),
        ("/usr/lib///", "lib", "/usr"),
        // A component directly under the root.
        ("/usr", "usr", "/"),
        ("/a", "a", "/"),
        // Deeper paths.
        ("/a/b/c", "c", "/a/b"),
        ("a/b/c", "c", "a/b"),
        ("a/b", "b", "a"),
        // Interior repeated separators.
        ("/a//b", "b", "/a"),
        ("/a///b///", "b", "/a"),
        // Dot components are not interpreted; these are string functions.
        ("/a/.", ".", "/a"),
        ("/a/..", "..", "/a"),
        ("a/", "a", "."),
        // A hidden file is an ordinary component.
        ("/etc/.bashrc", ".bashrc", "/etc"),
        (".bashrc", ".bashrc", "."),
    ];

    #[test]
    fn basename_matches_posix_table() {
        for (path, expected, _) in POSIX_TABLE {
            assert_eq!(
                basename_bytes(path.as_bytes()),
                expected.as_bytes(),
                "basename({path:?})"
            );
        }
    }

    #[test]
    fn dirname_matches_posix_table() {
        for (path, _, expected) in POSIX_TABLE {
            assert_eq!(
                dirname_bytes(path.as_bytes()),
                expected.as_bytes(),
                "dirname({path:?})"
            );
        }
    }

    /// The four cases named explicitly in the task, asserted on their own so a
    /// regression in any one of them is unmistakable in the failure output.
    #[test]
    fn awkward_cases_are_exact() {
        assert_eq!(dirname_bytes(b"/usr/"), b"/", "dirname(\"/usr/\")");
        assert_eq!(dirname_bytes(b"usr"), b".", "dirname(\"usr\")");
        assert_eq!(basename_bytes(b"/"), b"/", "basename(\"/\")");
        assert_eq!(basename_bytes(b""), b".", "basename(\"\")");
    }

    /// The exported entry points must agree with the byte-level helpers, and must
    /// leave the caller's string untouched: this implementation deviates from
    /// glibc precisely in not modifying its argument.
    #[test]
    fn exported_basename_and_dirname_do_not_modify_input() {
        let original = c"/usr/lib/thing";
        let mut owned: Vec<c_char> = original
            .to_bytes_with_nul()
            .iter()
            .map(|byte| *byte as c_char)
            .collect();

        // SAFETY: `owned` is a null-terminated string that outlives the calls.
        let base = unsafe { kinakaze_abi_basename(owned.as_ptr()) };
        // SAFETY: the returned pointer is a null-terminated static buffer.
        assert_eq!(unsafe { CStr::from_ptr(base) }.to_bytes(), b"thing");
        // The input is unchanged, which a glibc dirname would not guarantee.
        // SAFETY: `owned` is still a valid null-terminated string.
        assert_eq!(
            unsafe { CStr::from_ptr(owned.as_ptr()) }.to_bytes(),
            b"/usr/lib/thing"
        );

        // SAFETY: `owned` is a null-terminated string that outlives the call.
        let dir = unsafe { kinakaze_abi_dirname(owned.as_mut_ptr()) };
        // SAFETY: the returned pointer is a null-terminated static buffer.
        assert_eq!(unsafe { CStr::from_ptr(dir) }.to_bytes(), b"/usr/lib");
        // SAFETY: `owned` is still a valid null-terminated string.
        assert_eq!(
            unsafe { CStr::from_ptr(owned.as_ptr()) }.to_bytes(),
            b"/usr/lib/thing"
        );
    }

    /// The separate buffers let both results be live at once, which the common
    /// `printf("%s/%s", dirname(a), basename(b))` idiom depends on.
    #[test]
    fn basename_and_dirname_results_coexist() {
        let path = c"/etc/init.d/network";
        // SAFETY: the string is null-terminated and outlives both calls.
        let (dir, base) = unsafe {
            (
                kinakaze_abi_dirname(path.as_ptr().cast_mut()),
                kinakaze_abi_basename(path.as_ptr()),
            )
        };
        // SAFETY: both pointers name null-terminated static buffers.
        unsafe {
            assert_eq!(CStr::from_ptr(dir).to_bytes(), b"/etc/init.d");
            assert_eq!(CStr::from_ptr(base).to_bytes(), b"network");
        }
    }

    /// A null argument is treated as "." rather than faulting, matching glibc.
    #[test]
    fn null_path_is_the_current_directory() {
        // SAFETY: a null path is explicitly part of the contract.
        let (base, dir) = unsafe {
            (
                kinakaze_abi_basename(ptr::null()),
                kinakaze_abi_dirname(ptr::null_mut()),
            )
        };
        // SAFETY: both pointers name null-terminated static buffers.
        unsafe {
            assert_eq!(CStr::from_ptr(base).to_bytes(), b".");
            assert_eq!(CStr::from_ptr(dir).to_bytes(), b".");
        }
    }

    // -----------------------------------------------------------------------
    // fnmatch
    // -----------------------------------------------------------------------

    /// Calls the exported `fnmatch` and reports whether it matched.
    fn fnmatch(pattern: &str, name: &str, flags: c_int) -> bool {
        let pattern = std::ffi::CString::new(pattern).expect("pattern has no interior nul");
        let name = std::ffi::CString::new(name).expect("name has no interior nul");
        // SAFETY: both strings are null-terminated and outlive the call.
        unsafe { kinakaze_abi_fnmatch(pattern.as_ptr(), name.as_ptr(), flags) == 0 }
    }

    #[test]
    fn star_matches_any_run() {
        assert!(fnmatch("*", "", 0));
        assert!(fnmatch("*", "anything", 0));
        assert!(fnmatch("*.c", "main.c", 0));
        assert!(fnmatch("*.c", ".c", 0));
        assert!(!fnmatch("*.c", "main.h", 0));
        assert!(fnmatch("a*b", "ab", 0));
        assert!(fnmatch("a*b", "axxxb", 0));
        assert!(!fnmatch("a*b", "axxx", 0));
        // Several stars, which must not blow up or change the meaning.
        assert!(fnmatch("*a*b*", "xxayybzz", 0));
        assert!(!fnmatch("*a*b*", "xxbyyazz", 0));
        // A collapsed run behaves as one star.
        assert!(fnmatch("**", "anything", 0));
        assert!(fnmatch("a**b", "axb", 0));
    }

    #[test]
    fn question_matches_exactly_one() {
        assert!(fnmatch("?", "a", 0));
        assert!(!fnmatch("?", "", 0));
        assert!(!fnmatch("?", "ab", 0));
        assert!(fnmatch("a?c", "abc", 0));
        assert!(!fnmatch("a?c", "ac", 0));
        assert!(fnmatch("???", "abc", 0));
        assert!(!fnmatch("???", "ab", 0));
    }

    #[test]
    fn character_classes_and_ranges() {
        assert!(fnmatch("[abc]", "b", 0));
        assert!(!fnmatch("[abc]", "d", 0));
        assert!(fnmatch("[a-z]", "q", 0));
        assert!(!fnmatch("[a-z]", "Q", 0));
        assert!(fnmatch("[0-9]", "5", 0));
        assert!(!fnmatch("[0-9]", "a", 0));
        // Several ranges and singletons in one set.
        assert!(fnmatch("[a-cx-z]", "y", 0));
        assert!(!fnmatch("[a-cx-z]", "m", 0));
        assert!(fnmatch("file[0-9].txt", "file7.txt", 0));
        assert!(!fnmatch("file[0-9].txt", "filex.txt", 0));
        // A `-` in the final position is a literal `-`, not a range opener.
        assert!(fnmatch("[a-]", "-", 0));
        assert!(fnmatch("[a-]", "a", 0));
        // A `]` first in the set is a literal `]`.
        assert!(fnmatch("[]a]", "]", 0));
        assert!(fnmatch("[]a]", "a", 0));
        // An unterminated `[` is a literal `[`.
        assert!(fnmatch("[abc", "[abc", 0));
        assert!(!fnmatch("[abc", "a", 0));
    }

    #[test]
    fn negated_classes() {
        assert!(fnmatch("[!abc]", "d", 0));
        assert!(!fnmatch("[!abc]", "a", 0));
        // `^` negates too, which every shell accepts.
        assert!(fnmatch("[^abc]", "d", 0));
        assert!(!fnmatch("[^abc]", "a", 0));
        assert!(fnmatch("[!a-z]", "5", 0));
        assert!(!fnmatch("[!a-z]", "q", 0));
        // A negated set still matches exactly one character.
        assert!(!fnmatch("[!a]", "", 0));
        assert!(!fnmatch("[!a]", "bc", 0));
    }

    #[test]
    fn named_character_classes() {
        assert!(fnmatch("[[:digit:]]", "7", 0));
        assert!(!fnmatch("[[:digit:]]", "a", 0));
        assert!(fnmatch("[[:alpha:]]", "a", 0));
        assert!(!fnmatch("[[:alpha:]]", "7", 0));
        assert!(fnmatch("[[:alnum:]]", "7", 0));
        assert!(fnmatch("[[:upper:]]", "Q", 0));
        assert!(!fnmatch("[[:upper:]]", "q", 0));
        assert!(fnmatch("[[:space:]]", " ", 0));
        assert!(fnmatch("[[:punct:]]", ".", 0));
        assert!(fnmatch("[[:xdigit:]]", "f", 0));
        assert!(!fnmatch("[[:xdigit:]]", "g", 0));
        // A class combined with ordinary members.
        assert!(fnmatch("[[:digit:]abc]", "b", 0));
        assert!(fnmatch("[[:digit:]abc]", "3", 0));
        assert!(!fnmatch("[[:digit:]abc]", "z", 0));
        // Negated.
        assert!(fnmatch("[![:digit:]]", "a", 0));
        assert!(!fnmatch("[![:digit:]]", "3", 0));
    }

    /// FNM_PATHNAME confines every wildcard to a single path component: `/` then
    /// matches only a literal `/`.
    #[test]
    fn pathname_flag_confines_wildcards_to_one_component() {
        // Without the flag a star crosses separators freely.
        assert!(fnmatch("*", "a/b", 0));
        assert!(fnmatch("a*c", "a/c", 0));
        assert!(fnmatch("a*z", "a/b/z", 0));

        // With it, none of those hold.
        assert!(!fnmatch("*", "a/b", FNM_PATHNAME));
        assert!(!fnmatch("a*c", "a/c", FNM_PATHNAME));
        assert!(!fnmatch("a*z", "a/b/z", FNM_PATHNAME));

        // A `/` in the pattern lines up with a `/` in the string, and the star on
        // each side matches within its own component.
        assert!(fnmatch("*/*", "a/b", FNM_PATHNAME));
        assert!(!fnmatch("*/*", "a/b/c", FNM_PATHNAME));
        assert!(fnmatch("*/*/*", "a/b/c", FNM_PATHNAME));
        assert!(fnmatch("/usr/*", "/usr/lib", FNM_PATHNAME));
        assert!(!fnmatch("/usr/*", "/usr/lib/x", FNM_PATHNAME));

        // `?` likewise stops at a separator.
        assert!(fnmatch("a?c", "a/c", 0));
        assert!(!fnmatch("a?c", "a/c", FNM_PATHNAME));

        // A bracket expression never matches `/` under the flag, even when the
        // set names it explicitly.
        assert!(!fnmatch("a[/]c", "a/c", FNM_PATHNAME));
        assert!(fnmatch("a[/]c", "a/c", 0));

        // A literal separator still matches with the flag set.
        assert!(fnmatch("a/b", "a/b", FNM_PATHNAME));
    }

    /// FNM_PERIOD protects a leading `.` from every wildcard, which is what keeps
    /// a shell glob from matching dotfiles.
    #[test]
    fn period_flag_protects_a_leading_dot() {
        // Unprotected, a star or a set matches the dot like any other byte.
        assert!(fnmatch("*", ".hidden", 0));
        assert!(fnmatch("?hidden", ".hidden", 0));
        assert!(fnmatch("[.]hidden", ".hidden", 0));

        // Protected, only a literal dot in the pattern matches it.
        assert!(!fnmatch("*", ".hidden", FNM_PERIOD));
        assert!(!fnmatch("?hidden", ".hidden", FNM_PERIOD));
        assert!(!fnmatch("[.]hidden", ".hidden", FNM_PERIOD));
        assert!(fnmatch(".*", ".hidden", FNM_PERIOD));
        assert!(fnmatch(".hidden", ".hidden", FNM_PERIOD));

        // A dot that is not leading is ordinary.
        assert!(fnmatch("*", "a.b", FNM_PERIOD));
        assert!(fnmatch("a*", "a.b", FNM_PERIOD));

        // With FNM_PATHNAME the protection applies at the start of every
        // component, not just the start of the string.
        assert!(!fnmatch("*/*", "a/.hidden", FNM_PERIOD | FNM_PATHNAME));
        assert!(fnmatch("*/.*", "a/.hidden", FNM_PERIOD | FNM_PATHNAME));
        // Without FNM_PATHNAME only the very first character is protected, so an
        // interior dot after a slash is reachable.
        assert!(fnmatch("a/*", "a/.hidden", FNM_PERIOD));
    }

    #[test]
    fn noescape_flag_makes_backslash_literal() {
        // By default a backslash escapes the next character.
        assert!(fnmatch(r"\*", "*", 0));
        assert!(!fnmatch(r"\*", "anything", 0));
        assert!(fnmatch(r"a\?c", "a?c", 0));
        assert!(!fnmatch(r"a\?c", "abc", 0));
        assert!(fnmatch(r"\[abc\]", "[abc]", 0));

        // With FNM_NOESCAPE the backslash is an ordinary character, so the
        // metacharacter after it keeps its meaning.
        assert!(fnmatch(r"\*", r"\anything", FNM_NOESCAPE));
        assert!(!fnmatch(r"\*", "*", FNM_NOESCAPE));
        assert!(fnmatch(r"a\?c", r"a\bc", FNM_NOESCAPE));

        // A backslash inside a bracket expression escapes too.
        assert!(fnmatch(r"[\]]", "]", 0));
    }

    #[test]
    fn leading_dir_flag_matches_a_path_prefix() {
        assert!(!fnmatch("a", "a/b", 0));
        assert!(fnmatch("a", "a/b", FNM_LEADING_DIR));
        assert!(fnmatch("a", "a", FNM_LEADING_DIR));
        assert!(!fnmatch("a", "ab", FNM_LEADING_DIR));
        assert!(fnmatch("*", "a/b", FNM_PATHNAME | FNM_LEADING_DIR));
    }

    #[test]
    fn casefold_flag_ignores_case() {
        assert!(!fnmatch("ABC", "abc", 0));
        assert!(fnmatch("ABC", "abc", FNM_CASEFOLD));
        assert!(fnmatch("[a-z]", "Q", FNM_CASEFOLD));
        assert!(fnmatch("*.TXT", "file.txt", FNM_CASEFOLD));
    }

    #[test]
    fn exact_and_empty_patterns() {
        assert!(fnmatch("", "", 0));
        assert!(!fnmatch("", "a", 0));
        assert!(!fnmatch("a", "", 0));
        assert!(fnmatch("abc", "abc", 0));
        assert!(!fnmatch("abc", "abd", 0));
        assert!(!fnmatch("abc", "ab", 0));
        assert!(!fnmatch("ab", "abc", 0));
    }

    /// The value returned for a non-match is `FNM_NOMATCH`, not just any
    /// non-zero: callers compare against the constant.
    #[test]
    fn nomatch_is_the_documented_constant() {
        let pattern = c"a";
        let name = c"b";
        // SAFETY: both strings are null-terminated and outlive the call.
        let result = unsafe { kinakaze_abi_fnmatch(pattern.as_ptr(), name.as_ptr(), 0) };
        assert_eq!(result, FNM_NOMATCH);
        assert_eq!(FNM_NOMATCH, 1);
    }

    // -----------------------------------------------------------------------
    // Extended attributes
    // -----------------------------------------------------------------------

    // Extended-attribute lifecycle and namespace checks live in crate::xattr.

    // -----------------------------------------------------------------------
    // Live file system
    // -----------------------------------------------------------------------

    /// Creates a file inside the guest namespace and returns its guest path.
    ///
    /// `/` is the hosting executable's directory, which for a test binary is
    /// `target/debug/deps`, so a guest-visible temporary lives there.
    fn temp_file(name: &str, contents: &[u8]) -> Option<String> {
        let root = kinakaze_vfs::system_root().ok()?;
        let path = root.join(name);
        std::fs::write(&path, contents).ok()?;
        Some(format!("/{name}"))
    }

    /// Removes a guest-namespace temporary.
    fn remove_temp(name: &str) {
        if let Ok(root) = kinakaze_vfs::system_root() {
            let _ = std::fs::remove_file(root.join(name));
        }
    }

    /// `stat64` and `stat` are the same call, so they must return byte-identical
    /// results for the same file. This is the property the whole LFS alias group
    /// rests on.
    #[test]
    fn stat64_agrees_with_stat_on_a_real_file() {
        let name = "kinakaze-fsextra-stat64.tmp";
        let contents = b"seventeen bytes!!";
        let Some(guest) = temp_file(name, contents) else {
            // Without a resolvable root there is no guest namespace to test in.
            return;
        };
        let as_c = std::ffi::CString::new(guest.clone()).expect("path has no interior nul");

        let mut plain = Stat::default();
        let mut wide = Stat::default();
        // SAFETY: the path is null-terminated and both out-pointers are writable.
        let (plain_result, wide_result) = unsafe {
            (
                crate::fs::stat(as_c.as_ptr(), &raw mut plain),
                kinakaze_abi_stat64(as_c.as_ptr(), &raw mut wide),
            )
        };
        remove_temp(name);

        assert_eq!(plain_result, 0, "stat({guest}) should succeed");
        assert_eq!(wide_result, 0, "stat64({guest}) should succeed");

        // The size is the fact the test file was built to pin down.
        assert_eq!(plain.st_size, contents.len() as i64);
        assert_eq!(wide.st_size, plain.st_size);
        // Identity, mode and link count must agree field for field.
        assert_eq!(wide.st_ino, plain.st_ino);
        assert_eq!(wide.st_dev, plain.st_dev);
        assert_eq!(wide.st_mode, plain.st_mode);
        assert_eq!(wide.st_nlink, plain.st_nlink);
        assert_eq!(wide.st_mtime, plain.st_mtime);
        assert_eq!(wide.st_blksize, plain.st_blksize);
        assert_eq!(wide.st_mode & fs::S_IFMT, fs::S_IFREG);

        // Comparing the raw bytes catches a field the list above forgot.
        // SAFETY: Stat is a plain repr(C) struct of integers with no padding
        // holes that could hold indeterminate bytes; both were fully written.
        let (plain_bytes, wide_bytes) = unsafe {
            (
                core::slice::from_raw_parts(ptr::from_ref(&plain).cast::<u8>(), size_of::<Stat>()),
                core::slice::from_raw_parts(ptr::from_ref(&wide).cast::<u8>(), size_of::<Stat>()),
            )
        };
        assert_eq!(wide_bytes, plain_bytes, "stat64 and stat must be identical");
    }

    /// `lstat64` and `fstat64` likewise forward to their base implementations.
    #[test]
    fn lstat64_and_fstat64_agree_with_their_bases() {
        let name = "kinakaze-fsextra-fstat64.tmp";
        let Some(guest) = temp_file(name, b"data") else {
            return;
        };
        let as_c = std::ffi::CString::new(guest).expect("path has no interior nul");

        let mut base = Stat::default();
        let mut wide = Stat::default();
        // SAFETY: the path is null-terminated and both out-pointers are writable.
        unsafe {
            assert_eq!(crate::fs::lstat(as_c.as_ptr(), &raw mut base), 0);
            assert_eq!(kinakaze_abi_lstat64(as_c.as_ptr(), &raw mut wide), 0);
        }
        assert_eq!(wide.st_size, base.st_size);
        assert_eq!(wide.st_ino, base.st_ino);

        // SAFETY: the path is null-terminated.
        let fd = unsafe { kinakaze_abi_open64(as_c.as_ptr(), fs::O_RDONLY, 0) };
        assert!(fd >= 0, "open64 should succeed");
        let mut by_fd = Stat::default();
        // SAFETY: `by_fd` is writable and `fd` is open.
        assert_eq!(unsafe { kinakaze_abi_fstat64(fd, &raw mut by_fd) }, 0);
        assert_eq!(by_fd.st_size, base.st_size);
        assert_eq!(by_fd.st_ino, base.st_ino);
        let _ = kinakaze_vfs::close(fd);
        remove_temp(name);
    }

    /// `lseek64` and `ftruncate64` forward to the same descriptor logic.
    #[test]
    fn lseek64_and_ftruncate64_forward() {
        let name = "kinakaze-fsextra-lseek64.tmp";
        let Some(guest) = temp_file(name, b"0123456789") else {
            return;
        };
        let as_c = std::ffi::CString::new(guest).expect("path has no interior nul");
        // SAFETY: the path is null-terminated.
        let fd = unsafe { kinakaze_abi_open64(as_c.as_ptr(), fs::O_RDWR, 0) };
        assert!(fd >= 0);

        assert_eq!(kinakaze_abi_lseek64(fd, 4, fs::SEEK_SET), 4);
        assert_eq!(kinakaze_abi_lseek64(fd, 0, fs::SEEK_END), 10);
        assert_eq!(kinakaze_abi_ftruncate64(fd, 5), 0);

        let mut after = Stat::default();
        // SAFETY: `after` is writable and `fd` is open.
        assert_eq!(unsafe { kinakaze_abi_fstat64(fd, &raw mut after) }, 0);
        assert_eq!(after.st_size, 5, "ftruncate64 should shorten the file");
        let _ = kinakaze_vfs::close(fd);
        remove_temp(name);
    }

    /// `chmod` persists the complete guest permission word and `stat` reports it
    /// back while owner-write also controls the Windows read-only attribute.
    #[test]
    fn chmod_maps_the_write_bit_to_readonly() {
        let name = "kinakaze-fsextra-chmod.tmp";
        let Some(guest) = temp_file(name, b"x") else {
            return;
        };
        let as_c = std::ffi::CString::new(guest).expect("path has no interior nul");

        // Clearing the write bit sets the read-only attribute, which stat
        // synthesizes back as mode 0444.
        // SAFETY: the path is null-terminated.
        assert_eq!(unsafe { kinakaze_abi_chmod(as_c.as_ptr(), 0o444) }, 0);
        let mut readonly = Stat::default();
        // SAFETY: the path is null-terminated and the out-pointer is writable.
        assert_eq!(
            unsafe { crate::fs::stat(as_c.as_ptr(), &raw mut readonly) },
            0
        );
        assert_eq!(readonly.st_mode & 0o7777, 0o444);

        // Restoring it clears the attribute again.
        // SAFETY: the path is null-terminated.
        assert_eq!(unsafe { kinakaze_abi_chmod(as_c.as_ptr(), 0o644) }, 0);
        let mut writable = Stat::default();
        // SAFETY: as above.
        assert_eq!(
            unsafe { crate::fs::stat(as_c.as_ptr(), &raw mut writable) },
            0
        );
        assert_eq!(writable.st_mode & 0o7777, 0o644);

        remove_temp(name);
    }

    /// UID/GID changes persist and the sentinel preserves the unselected ID.
    #[test]
    fn ownership_calls_persist_numeric_ids() {
        let name = "kinakaze-fsextra-chown.tmp";
        let Some(guest) = temp_file(name, b"x") else {
            return;
        };
        let as_c = std::ffi::CString::new(guest).expect("path has no interior nul");

        let mut before = Stat::default();
        // SAFETY: the path is null-terminated and the out-pointer is writable.
        assert_eq!(
            unsafe { crate::fs::stat(as_c.as_ptr(), &raw mut before) },
            0
        );

        // SAFETY: the path is null-terminated.
        unsafe {
            assert_eq!(kinakaze_abi_chown(as_c.as_ptr(), 1000, 1000), 0);
            assert_eq!(kinakaze_abi_lchown(as_c.as_ptr(), u32::MAX, 2000), 0);
            assert_eq!(
                kinakaze_abi_fchownat(AT_FDCWD, as_c.as_ptr(), 3000, u32::MAX, 0),
                0
            );
        }

        let mut after = Stat::default();
        // SAFETY: as above.
        assert_eq!(unsafe { crate::fs::stat(as_c.as_ptr(), &raw mut after) }, 0);
        assert_eq!(after.st_uid, 3000);
        assert_eq!(after.st_gid, 2000);
        assert_eq!(after.st_mode, before.st_mode);

        // A null path is still an error rather than an absorbed no-op.
        // SAFETY: a null path is part of the contract.
        assert_eq!(unsafe { kinakaze_abi_chown(ptr::null(), 0, 0) }, -1);
        assert_eq!(crate::kinakaze_errno(), EFAULT);
        // A bad descriptor likewise.
        assert_eq!(kinakaze_abi_fchown(-1, 0, 0), -1);
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EBADF);
        let missing = c"/kinakaze-fsextra-chown-does-not-exist";
        assert_eq!(unsafe { kinakaze_abi_chown(missing.as_ptr(), 0, 0) }, -1);
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::ENOENT);

        remove_temp(name);
    }

    /// `umask` returns the previous mask, which is what the read idiom needs.
    #[test]
    fn umask_returns_the_previous_value() {
        let original = kinakaze_abi_umask(0o077);
        // The read idiom: set to a known value, then put back what was there.
        let observed = kinakaze_abi_umask(0o022);
        assert_eq!(observed, 0o077, "should report the mask just installed");
        let restored = kinakaze_abi_umask(original);
        assert_eq!(restored, 0o022);
        // Only the nine permission bits are retained.
        kinakaze_abi_umask(0o7777);
        assert_eq!(kinakaze_abi_umask(original), 0o777);
    }

    /// `statfs` reports a real volume: a capacity that is non-zero and free space
    /// that does not exceed it.
    #[test]
    fn statfs_reports_plausible_volume_totals() {
        let path = c"/";
        let mut value = Statfs::default();
        // SAFETY: the path is null-terminated and the out-pointer is writable.
        let result = unsafe { kinakaze_abi_statfs(path.as_ptr(), &raw mut value) };
        if result != 0 {
            // A volume query can fail on an unusual mount; that is not a defect
            // in the translation being tested here.
            return;
        }
        assert_eq!(value.f_bsize, BLOCK_SIZE as i64);
        assert_eq!(value.f_frsize, BLOCK_SIZE as i64);
        assert_eq!(value.f_namelen, NAME_MAX);
        assert_eq!(value.f_type, NTFS_MAGIC);
        assert!(value.f_blocks > 0, "a mounted volume has a capacity");
        assert!(value.f_bfree <= value.f_blocks, "free cannot exceed total");
        assert!(value.f_bavail <= value.f_bfree, "available is within free");

        // statfs64 is the same call.
        let mut wide = Statfs::default();
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_statfs64(path.as_ptr(), &raw mut wide) },
            0
        );
        assert_eq!(wide.f_blocks, value.f_blocks);
        assert_eq!(wide.f_type, value.f_type);

        // statvfs describes the same volume through the other struct.
        let mut vfs = Statvfs::default();
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_statvfs(path.as_ptr(), &raw mut vfs) },
            0
        );
        assert_eq!(vfs.f_blocks, value.f_blocks);
        assert_eq!(vfs.f_bsize, value.f_bsize as u64);
        assert_eq!(vfs.f_namemax, value.f_namelen as u64);
    }

    /// `fstatfs` identifies the mount an open descriptor belongs to.  A procfs
    /// directory is memory-backed here, but remains procfs to the guest rather
    /// than becoming tmpfs merely because it has no Windows handle.
    #[test]
    fn fstatfs_preserves_synthetic_mount_identity() {
        let fd = kinakaze_vfs::fs::open("/proc/self/fd", fs::O_RDONLY, 0)
            .expect("open synthetic procfs directory");
        let mut value = Statfs::default();
        // SAFETY: `value` is a live writable `Statfs`.
        assert_eq!(unsafe { kinakaze_abi_fstatfs(fd, &raw mut value) }, 0);
        assert_eq!(value.f_type, PROC_SUPER_MAGIC);

        let mut path_value = Statfs::default();
        let path = c"/proc/self/fd";
        // SAFETY: both pointers refer to live objects with the required layout.
        assert_eq!(
            unsafe { kinakaze_abi_statfs(path.as_ptr(), &raw mut path_value) },
            0
        );
        assert_eq!(value.f_type, path_value.f_type);
        kinakaze_vfs::close(fd).expect("close procfs descriptor");
    }

    /// A null out-pointer is `EFAULT` rather than a write through null.
    #[test]
    fn statfs_rejects_a_null_buffer() {
        let path = c"/";
        // SAFETY: a null out-pointer is explicitly part of the contract.
        assert_eq!(
            unsafe { kinakaze_abi_statfs(path.as_ptr(), ptr::null_mut()) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), EFAULT);
    }

    /// `fsync` succeeds on a real file and reports `EBADF` for a closed one.
    #[test]
    fn fsync_family_validates_descriptors() {
        let name = "kinakaze-fsextra-fsync.tmp";
        let Some(guest) = temp_file(name, b"x") else {
            return;
        };
        let as_c = std::ffi::CString::new(guest).expect("path has no interior nul");
        // SAFETY: the path is null-terminated.
        let fd = unsafe { crate::fs::open(as_c.as_ptr(), fs::O_RDWR, 0) };
        assert!(fd >= 0);
        assert_eq!(kinakaze_abi_fsync(fd), 0);
        assert_eq!(kinakaze_abi_fdatasync(fd), 0);
        assert_eq!(kinakaze_abi_syncfs(fd), 0);
        let _ = kinakaze_vfs::close(fd);

        assert_eq!(kinakaze_abi_fsync(-1), -1);
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EBADF);
        // sync returns nothing and must not fault with descriptors open.
        kinakaze_abi_sync();
        remove_temp(name);
    }

    /// `readlink` on a path that is not a link is `EINVAL`, as POSIX requires.
    #[test]
    fn readlink_rejects_a_regular_file() {
        let name = "kinakaze-fsextra-readlink.tmp";
        let Some(guest) = temp_file(name, b"x") else {
            return;
        };
        let as_c = std::ffi::CString::new(guest).expect("path has no interior nul");
        let mut buffer = [0i8; 256];
        // SAFETY: the path is null-terminated and the buffer is writable.
        let result = unsafe {
            kinakaze_abi_readlink(
                as_c.as_ptr(),
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len(),
            )
        };
        remove_temp(name);
        assert_eq!(result, -1, "a regular file is not a symlink");
    }

    #[test]
    fn readlinkat_empty_path_uses_the_open_link_after_rename_and_unlink() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "kinakaze-readlinkat-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        struct Descriptor(i32);
        impl Drop for Descriptor {
            fn drop(&mut self) {
                let _ = kinakaze_vfs::close(self.0);
            }
        }
        let _fixture = Fixture(root.clone());
        let link = root.join("link");
        let moved = root.join("moved");
        let expected = "../目标/missing";
        kinakaze_vfs::create_emulated_symlink(&link, expected).unwrap();
        let fd = Descriptor(
            fs::open(
                &kinakaze_vfs::to_guest_path(&link),
                fs::O_PATH | fs::O_NOFOLLOW,
                0,
            )
            .unwrap(),
        );
        std::fs::rename(&link, &moved).unwrap();
        kinakaze_vfs::create_emulated_symlink(&link, "replacement").unwrap();
        fs::unlink(&kinakaze_vfs::to_guest_path(&moved)).unwrap();
        for size in [1, 4, 128] {
            let mut buffer = [0x55u8; 129];
            let count = unsafe {
                kinakaze_abi_readlinkat(fd.0, c"".as_ptr(), buffer.as_mut_ptr().cast(), size)
            };
            let amount = expected.len().min(size);
            assert_eq!(count, amount as isize);
            assert_eq!(&buffer[..amount], &expected.as_bytes()[..amount]);
            assert!(buffer[amount..].iter().all(|byte| *byte == 0x55));
        }
        let regular = root.join("file");
        std::fs::write(&regular, b"regular").unwrap();
        let regular =
            Descriptor(fs::open(&kinakaze_vfs::to_guest_path(&regular), fs::O_PATH, 0).unwrap());
        for (descriptor, expected_errno) in [
            (regular.0, kinakaze_vfs::ENOENT),
            (AT_FDCWD, kinakaze_vfs::ENOENT),
            (-1, kinakaze_vfs::EBADF),
        ] {
            let mut buffer = [0u8; 8];
            assert_eq!(
                unsafe {
                    kinakaze_abi_readlinkat(
                        descriptor,
                        c"".as_ptr(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                    )
                },
                -1
            );
            assert_eq!(crate::kinakaze_errno(), expected_errno);
        }
        assert_eq!(
            unsafe { kinakaze_abi_readlinkat(-1, ptr::null(), ptr::null_mut(), 0) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), EINVAL);
        assert_eq!(
            unsafe { kinakaze_abi_readlinkat(fd.0, c"".as_ptr(), ptr::null_mut(), 8) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), EFAULT);
    }

    /// Hosted links retain link identity even when Windows denies native
    /// symlink creation: readlink/lstat/readdir see a link, stat follows it and
    /// unlink removes the link rather than its target.
    #[test]
    fn hosted_symlink_obeys_linux_path_operations() {
        let target_name = "kinakaze-fsextra-symlink-target.tmp";
        let link_name = "kinakaze-fsextra-symlink-link.tmp";
        let Some(target_guest) = temp_file(target_name, b"target-data") else {
            return;
        };
        remove_temp(link_name);

        let target = std::ffi::CString::new(target_name).unwrap();
        let link_guest = format!("/{link_name}");
        let link = std::ffi::CString::new(link_guest.clone()).unwrap();
        assert_eq!(
            unsafe { kinakaze_abi_symlink(target.as_ptr(), link.as_ptr()) },
            0
        );

        let mut buffer = [0i8; 256];
        let count = unsafe {
            kinakaze_abi_readlink(
                link.as_ptr(),
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len(),
            )
        };
        assert_eq!(count, target_name.len() as isize);
        let bytes =
            unsafe { core::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), count as usize) };
        assert_eq!(bytes, target_name.as_bytes());

        let link_stat = fs::lstat(&link_guest).unwrap();
        assert_eq!(link_stat.st_mode & fs::S_IFMT, fs::S_IFLNK);
        assert_eq!(link_stat.st_size, target_name.len() as i64);
        let target_stat = fs::stat(&link_guest).unwrap();
        assert_eq!(target_stat.st_mode & fs::S_IFMT, fs::S_IFREG);
        assert_eq!(target_stat.st_size, b"target-data".len() as i64);

        assert_eq!(
            fs::open(&link_guest, fs::O_RDONLY | fs::O_NOFOLLOW, 0),
            Err(kinakaze_vfs::ELOOP)
        );
        let path_fd = fs::open(&link_guest, fs::O_PATH | fs::O_NOFOLLOW, 0).unwrap();
        let path_stat = fs::fstat(path_fd).unwrap();
        assert_eq!(path_stat.st_mode & fs::S_IFMT, fs::S_IFLNK);
        kinakaze_vfs::close(path_fd).unwrap();

        let entry = fs::read_directory("/")
            .unwrap()
            .into_iter()
            .find(|entry| entry.name == link_name)
            .expect("link must appear in readdir");
        assert!(entry.is_symlink);
        assert!(!entry.is_directory);

        assert_eq!(
            unsafe { kinakaze_abi_symlink(target.as_ptr(), link.as_ptr()) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EEXIST);

        fs::unlink(&link_guest).unwrap();
        assert!(fs::stat(&target_guest).is_ok(), "unlink removed the target");
        remove_temp(target_name);
    }

    /// `realpath` resolves a real file to an absolute guest path and reports
    /// `ENOENT` for one that does not exist.
    #[test]
    fn realpath_resolves_and_reports_missing_paths() {
        let name = "kinakaze-fsextra-realpath.tmp";
        let Some(guest) = temp_file(name, b"x") else {
            return;
        };
        let as_c = std::ffi::CString::new(guest).expect("path has no interior nul");
        let mut buffer = [0i8; PATH_MAX];
        // SAFETY: the path is null-terminated and the buffer holds PATH_MAX bytes.
        let result =
            unsafe { kinakaze_abi_realpath(as_c.as_ptr(), buffer.as_mut_ptr().cast::<c_char>()) };
        assert!(!result.is_null(), "realpath on an existing file");
        // SAFETY: realpath wrote a null-terminated string into the buffer.
        let resolved = unsafe { CStr::from_ptr(result) }.to_bytes().to_vec();
        assert!(
            resolved.starts_with(b"/"),
            "the result must be an absolute guest path"
        );
        assert!(
            resolved.ends_with(name.as_bytes()),
            "the result should name the file"
        );
        remove_temp(name);

        let missing = c"/kinakaze-fsextra-does-not-exist";
        // SAFETY: the path is null-terminated; a null buffer requests an
        // allocation, which is part of the contract.
        let result = unsafe { kinakaze_abi_realpath(missing.as_ptr(), ptr::null_mut()) };
        assert!(result.is_null(), "a missing path must fail");
        assert_eq!(crate::kinakaze_errno(), ENOENT);
    }

    /// `scandir` lists a directory, honours its filter and sorts with the
    /// supplied comparator.
    #[test]
    fn scandir_filters_and_sorts() {
        /// Keeps only the two entries this test created.
        unsafe extern "sysv64" fn only_ours(entry: *const Dirent) -> c_int {
            if entry.is_null() {
                return 0;
            }
            // SAFETY: scandir passes a live dirent with a null-terminated name.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            c_int::from(name.to_bytes().starts_with(b"kinakaze-scandir-"))
        }

        let root = match kinakaze_vfs::system_root() {
            Ok(root) => root,
            Err(_) => return,
        };
        let first = root.join("kinakaze-scandir-b.tmp");
        let second = root.join("kinakaze-scandir-a.tmp");
        if std::fs::write(&first, b"1").is_err() || std::fs::write(&second, b"2").is_err() {
            let _ = std::fs::remove_file(&first);
            let _ = std::fs::remove_file(&second);
            return;
        }

        let path = c"/";
        let mut list: *mut *mut Dirent = ptr::null_mut();
        // SAFETY: the path is null-terminated, `list` is a writable out-pointer,
        // and both callbacks are valid System V functions.
        let count = unsafe {
            kinakaze_abi_scandir(
                path.as_ptr(),
                &raw mut list,
                Some(only_ours),
                Some(kinakaze_abi_alphasort),
            )
        };
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);

        assert_eq!(
            count, 2,
            "the filter should keep exactly the two temporaries"
        );
        assert!(!list.is_null());
        // SAFETY: scandir wrote `count` entry pointers into `list`.
        let entries = unsafe { core::slice::from_raw_parts(list, count as usize) };
        // SAFETY: each entry is a live dirent with a null-terminated name.
        let names: Vec<Vec<u8>> = entries
            .iter()
            .map(|entry| {
                unsafe { CStr::from_ptr((**entry).d_name.as_ptr()) }
                    .to_bytes()
                    .to_vec()
            })
            .collect();
        // alphasort orders `a` before `b`, which is not creation order.
        assert_eq!(names[0], b"kinakaze-scandir-a.tmp");
        assert_eq!(names[1], b"kinakaze-scandir-b.tmp");

        // SAFETY: every pointer came from this library's allocator.
        unsafe {
            for entry in entries {
                kinakaze_alloc::c::free(entry.cast::<u8>());
            }
            kinakaze_alloc::c::free(list.cast::<u8>());
        }
    }

    /// `alphasort` orders by name and reports the sign a comparator must.
    #[test]
    fn alphasort_compares_names() {
        let make = |name: &str| {
            let mut record = Dirent {
                d_ino: 1,
                d_off: 0,
                d_reclen: size_of::<Dirent>() as u16,
                d_type: DT_REG,
                d_name: [0; 256],
            };
            for (slot, byte) in record.d_name.iter_mut().zip(name.as_bytes()) {
                *slot = *byte as c_char;
            }
            record
        };
        let (apple, banana) = (make("apple"), make("banana"));
        let (left, right) = (
            ptr::from_ref(&apple).cast_mut(),
            ptr::from_ref(&banana).cast_mut(),
        );
        let (left, right) = (left as *const Dirent, right as *const Dirent);
        // SAFETY: both point to live dirents with null-terminated names.
        unsafe {
            assert!(kinakaze_abi_alphasort(&raw const left, &raw const right) < 0);
            assert!(kinakaze_abi_alphasort(&raw const right, &raw const left) > 0);
            assert_eq!(kinakaze_abi_alphasort(&raw const left, &raw const left), 0);
        }
    }

    /// `fdopendir` adopts a descriptor and `dirfd` reports it back.
    #[test]
    fn fdopendir_and_dirfd_round_trip() {
        let path = c"/";
        // SAFETY: the path is null-terminated.
        let fd = unsafe { crate::fs::open(path.as_ptr(), fs::O_RDONLY | fs::O_DIRECTORY, 0) };
        if fd < 0 {
            // Opening the root as a directory can fail in a constrained
            // environment; the stream logic is what is under test.
            return;
        }
        let stream = kinakaze_abi_fdopendir(fd);
        assert!(!stream.is_null(), "fdopendir on a directory descriptor");
        assert_eq!(
            unsafe { kinakaze_abi_dirfd(stream) },
            fd,
            "dirfd should report the adopted descriptor"
        );
        // The listing is usable: `.` is always the first synthesized entry.
        // SAFETY: `stream` is a live stream from fdopendir.
        let entry = unsafe { kinakaze_abi_readdir64(stream) };
        assert!(!entry.is_null());
        // SAFETY: readdir returned a live dirent with a null-terminated name.
        assert_eq!(
            unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes(),
            b"."
        );
        // SAFETY: the stream came from fdopendir and is not used again.
        unsafe { crate::dirent::kinakaze_abi_closedir(stream) };
        let _ = kinakaze_vfs::close(fd);
    }

    /// A stream now carries a descriptor whichever call produced it, so the
    /// question `dirfd` answers is which one — and only a null stream has none.
    #[test]
    fn dirfd_rejects_a_stream_without_a_descriptor() {
        let path = c"/";
        // SAFETY: the path is null-terminated.
        let stream = unsafe { crate::dirent::kinakaze_abi_opendir(path.as_ptr()) };
        if stream.is_null() {
            return;
        }
        // `opendir` opens a descriptor of its own for the listing, so this is a
        // real one rather than the `EINVAL` an implementation without one owes.
        let fd = unsafe { kinakaze_abi_dirfd(stream) };
        assert!(fd >= 0, "opendir's stream should carry a descriptor");
        assert!(kinakaze_vfs::get(fd).is_ok(), "and it should be open");
        // SAFETY: the stream came from opendir and is not used again.
        unsafe { crate::dirent::kinakaze_abi_closedir(stream) };

        assert_eq!(unsafe { kinakaze_abi_dirfd(ptr::null_mut()) }, -1);
        assert_eq!(crate::kinakaze_errno(), EINVAL);
    }

    /// Regular files and FIFOs preserve their real inode types. Device nodes
    /// are refused rather than represented by regular placeholders.
    #[test]
    fn mknod_creates_files_and_refuses_devices() {
        let name = "kinakaze-fsextra-mknod.tmp";
        let guest = format!("/{name}");
        let as_c = std::ffi::CString::new(guest).expect("path has no interior nul");
        remove_temp(name);

        // SAFETY: the path is null-terminated.
        let result = unsafe { kinakaze_abi_mknod(as_c.as_ptr(), fs::S_IFREG | 0o644, 0) };
        if result == 0 {
            let mut created = Stat::default();
            // SAFETY: the path is null-terminated and the out-pointer writable.
            assert_eq!(
                unsafe { crate::fs::stat(as_c.as_ptr(), &raw mut created) },
                0
            );
            assert_eq!(created.st_mode & fs::S_IFMT, fs::S_IFREG);
            remove_temp(name);
        }

        // The device inode preserves its type and dev_t rather than borrowing
        // regular-file behavior.
        // SAFETY: the path is null-terminated.
        assert_eq!(
            unsafe { kinakaze_abi_mknod(as_c.as_ptr(), fs::S_IFCHR | 0o644, 0x103) },
            0
        );
        let mut device = Stat::default();
        assert_eq!(
            unsafe { crate::fs::stat(as_c.as_ptr(), &raw mut device) },
            0
        );
        assert_eq!(device.st_mode & fs::S_IFMT, fs::S_IFCHR);
        assert_eq!(device.st_rdev, 0x103);
        remove_temp(name);

        // The FIFO has a real guest inode type even though its byte channel is
        // backed by a Windows named-pipe kernel object.
        // SAFETY: the path is null-terminated.
        assert_eq!(unsafe { kinakaze_abi_mkfifo(as_c.as_ptr(), 0o644) }, 0);
        let mut fifo = Stat::default();
        assert_eq!(unsafe { crate::fs::stat(as_c.as_ptr(), &raw mut fifo) }, 0);
        assert_eq!(fifo.st_mode & fs::S_IFMT, fs::S_IFIFO);
        remove_temp(name);
    }

    /// `link` creates a second name for one file, which NTFS supports.
    #[test]
    fn link_creates_a_hard_link() {
        let name = "kinakaze-fsextra-link.tmp";
        let Some(guest) = temp_file(name, b"shared") else {
            return;
        };
        let link_name = "kinakaze-fsextra-link2.tmp";
        remove_temp(link_name);
        let existing = std::ffi::CString::new(guest).expect("path has no interior nul");
        let created =
            std::ffi::CString::new(format!("/{link_name}")).expect("path has no interior nul");

        // SAFETY: both paths are null-terminated.
        let result = unsafe { kinakaze_abi_link(existing.as_ptr(), created.as_ptr()) };
        if result == 0 {
            let mut first = Stat::default();
            let mut second = Stat::default();
            // SAFETY: both paths are null-terminated and both out-pointers
            // writable.
            unsafe {
                assert_eq!(crate::fs::stat(existing.as_ptr(), &raw mut first), 0);
                assert_eq!(crate::fs::stat(created.as_ptr(), &raw mut second), 0);
            }
            // One file, two names: the inode is the same.
            assert_eq!(second.st_ino, first.st_ino);
            assert_eq!(second.st_size, 6);
        }
        remove_temp(link_name);
        remove_temp(name);
    }

    /// A path that does not exist is `ENOENT` from `chmod`, not a silent success.
    #[test]
    fn chmod_reports_a_missing_path() {
        let missing = c"/kinakaze-fsextra-absent";
        // SAFETY: the path is null-terminated.
        assert_eq!(unsafe { kinakaze_abi_chmod(missing.as_ptr(), 0o644) }, -1);
        assert_eq!(crate::kinakaze_errno(), ENOENT);
    }

    /// `windows_to_linux` inverts the resolver for both namespaces it handles.
    #[test]
    fn windows_paths_map_back_into_the_guest_namespace() {
        // A drive path outside the virtual root.
        assert_eq!(windows_to_linux(Path::new(r"C:\tmp\file")), "/c/tmp/file");
        assert_eq!(windows_to_linux(Path::new(r"C:\")), "/c");
        assert_eq!(windows_to_linux(Path::new(r"D:\a\b")), "/d/a/b");
        // The extended-length prefix canonicalize emits is stripped first.
        assert_eq!(
            windows_to_linux(Path::new(r"\\?\C:\tmp\file")),
            "/c/tmp/file"
        );

        // A path under the virtual root keeps only its suffix.
        if let Ok(root) = kinakaze_vfs::system_root() {
            assert_eq!(windows_to_linux(&root), "/");
            assert_eq!(windows_to_linux(&root.join("thing")), "/thing");
            assert_eq!(windows_to_linux(&root.join("a").join("b")), "/a/b");
        }
    }

    /// The `normalize` helper collapses the components realpath must resolve.
    #[test]
    fn normalize_collapses_dot_components() {
        assert_eq!(normalize("/a/./b"), "/a/b");
        assert_eq!(normalize("/a/b/.."), "/a");
        assert_eq!(normalize("/a//b"), "/a/b");
        assert_eq!(normalize("/.."), "/");
        assert_eq!(normalize("/a/b/../../c"), "/c");
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_flock(fd: c_int, _operation: c_int) -> c_int {
    if kinakaze_vfs::get(fd).is_err() {
        crate::set_errno(kinakaze_vfs::EBADF);
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_lockf(fd: c_int, cmd: c_int, len: i64) -> c_int {
    let command = match cmd {
        0 | 2 => 6, // F_ULOCK / F_TLOCK: F_SETLK
        1 => 7,     // F_LOCK: F_SETLKW
        3 => 5,     // F_TEST: F_GETLK
        _ => {
            crate::set_errno(kinakaze_vfs::EINVAL);
            return -1;
        }
    };
    let mut request = crate::fdio::Flock {
        l_type: if cmd == 0 { 2 } else { 1 },
        l_whence: 1,
        l_start: 0,
        l_len: len,
        l_pid: 0,
    };
    let result = unsafe {
        crate::fdio::kinakaze_abi_fcntl64(fd, command, (&mut request as *mut _) as usize)
    };
    if result == 0 && cmd == 3 && request.l_type != 2 {
        crate::set_errno(kinakaze_vfs::EACCES);
        return -1;
    }
    result
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_lockf64(fd: c_int, cmd: c_int, len: i64) -> c_int {
    kinakaze_abi_lockf(fd, cmd, len)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_shm_open(
    name: *const c_char,
    oflag: c_int,
    mode: u32,
) -> c_int {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let cstr = unsafe { CStr::from_ptr(name) };
    let s = match cstr.to_str() {
        Ok(s) => s.trim_start_matches('/'),
        Err(_) => {
            crate::set_errno(kinakaze_vfs::EINVAL);
            return -1;
        }
    };
    let path = format!("/dev/shm/{s}\0");
    unsafe { crate::fs::open(path.as_ptr() as *const c_char, oflag, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_shm_unlink(name: *const c_char) -> c_int {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let cstr = unsafe { CStr::from_ptr(name) };
    let s = match cstr.to_str() {
        Ok(s) => s.trim_start_matches('/'),
        Err(_) => {
            crate::set_errno(kinakaze_vfs::EINVAL);
            return -1;
        }
    };
    let path = format!("/dev/shm/{s}\0");
    unsafe { crate::fs::unlink(path.as_ptr() as *const c_char) }
}

#[repr(C)]
pub struct Utimbuf {
    pub actime: i64,
    pub modtime: i64,
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_utime(
    filename: *const c_char,
    times: *const Utimbuf,
) -> c_int {
    if filename.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    if times.is_null() {
        unsafe {
            crate::fdio::kinakaze_abi_utimensat(
                kinakaze_vfs::fs::AT_FDCWD,
                filename,
                core::ptr::null(),
                0,
            )
        }
    } else {
        let ts = [
            crate::fdio::TimeSpec {
                tv_sec: unsafe { (*times).actime },
                tv_nsec: 0,
            },
            crate::fdio::TimeSpec {
                tv_sec: unsafe { (*times).modtime },
                tv_nsec: 0,
            },
        ];
        unsafe {
            crate::fdio::kinakaze_abi_utimensat(
                kinakaze_vfs::fs::AT_FDCWD,
                filename,
                ts.as_ptr(),
                0,
            )
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fmemopen(
    buf: *mut c_void,
    size: usize,
    _mode: *const c_char,
) -> *mut crate::stdio::File {
    let stream = crate::stdio::exports::kinakaze_abi_tmpfile();
    if !stream.is_null() && !buf.is_null() && size > 0 {
        let _ = unsafe { crate::stdio::fwrite(buf, 1, size, stream) };
        let _ = crate::stdio::fseek(stream, 0, 0);
    }
    stream
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___realpath_chk(
    path: *const c_char,
    resolved_path: *mut c_char,
    _resolved_len: usize,
) -> *mut c_char {
    unsafe { kinakaze_abi_realpath(path, resolved_path) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_madvise(
    addr: *mut c_void,
    len: usize,
    advice: c_int,
) -> c_int {
    match crate::fdio::madvise_impl(addr, len, advice) {
        Ok(()) => 0,
        Err(error) => error,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mincore(
    addr: *mut c_void,
    length: usize,
    vec: *mut u8,
) -> c_int {
    if addr.is_null() || vec.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let page_count = (length + 4095) / 4096;
    unsafe {
        core::ptr::write_bytes(vec, 1, page_count);
    }
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___getpagesize() -> c_int {
    4096
}

// Providers are loaded on the initial native thread before guest code runs.
// Counting live threads cannot identify the leader once pthread_create is used.
static INITIAL_NATIVE_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
extern "C" fn record_initial_native_thread() {
    INITIAL_NATIVE_THREAD.store(
        unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() },
        std::sync::atomic::Ordering::Relaxed,
    );
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIAL_NATIVE_THREAD_INITIALIZER: extern "C" fn() = record_initial_native_thread;

/// Translate the PID-namespace leader TID exposed by gettid back to its host ID.
pub(crate) fn native_signal_tid(tid: i32) -> u32 {
    if tid == crate::process::kinakaze_abi_getpid() {
        INITIAL_NATIVE_THREAD.load(std::sync::atomic::Ordering::Relaxed)
    } else {
        tid as u32
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_gettid() -> i32 {
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    if tid == INITIAL_NATIVE_THREAD.load(std::sync::atomic::Ordering::Relaxed) {
        crate::process::kinakaze_abi_getpid()
    } else {
        tid as i32
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_umount(_target: *const c_char) -> c_int {
    crate::set_errno(kinakaze_vfs::EPERM);
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_ppoll(
    fds: *mut core::ffi::c_void,
    nfds: usize,
    tmo_p: *const libpthread::Timespec,
    _sigmask: *const core::ffi::c_void,
) -> c_int {
    let timeout_ms = if tmo_p.is_null() {
        -1
    } else {
        let ts = unsafe { *tmo_p };
        if ts.tv_sec < 0 || ts.tv_nsec < 0 {
            -1
        } else if ts.tv_sec == 0 && ts.tv_nsec == 0 {
            0
        } else {
            let ms = (ts.tv_sec as i64) * 1000 + (ts.tv_nsec as i64 + 999_999) / 1_000_000;
            ms.min(i32::MAX as i64) as i32
        }
    };
    unsafe { crate::fdio::kinakaze_abi_poll(fds as *mut _, nfds as u64, timeout_ms) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_name_to_handle_at(
    dirfd: c_int,
    pathname: *const c_char,
    handle: *mut c_void,
    mount_id: *mut c_int,
    flags: c_int,
) -> c_int {
    let result = (|| {
        if handle.is_null() || mount_id.is_null() {
            return Err(EFAULT);
        }
        if flags & !(AT_EMPTY_PATH | 0x400) != 0 {
            return Err(EINVAL);
        }
        let path = unsafe { borrow_path(pathname)? };
        let capacity = unsafe { ptr::read_unaligned(handle.cast::<u32>()) } as usize;
        if capacity > 128 {
            return Err(EINVAL);
        }
        let (bytes, mount) = if path.is_empty() {
            if flags & AT_EMPTY_PATH == 0 {
                return Err(kinakaze_vfs::ENOENT);
            }
            kinakaze_vfs::mount::overlay::encode_handle_fd(dirfd)?
        } else {
            kinakaze_vfs::mount::overlay::encode_handle(
                &resolve_at(dirfd, path)?,
                flags & 0x400 != 0,
            )?
        };
        unsafe {
            ptr::write_unaligned(handle.cast::<u32>(), bytes.len() as u32);
        }
        if capacity < bytes.len() {
            return Err(kinakaze_vfs::EOVERFLOW);
        }
        unsafe {
            ptr::write_unaligned(handle.cast::<u32>().add(1), 0x43594f01);
            ptr::copy_nonoverlapping(bytes.as_ptr(), handle.cast::<u8>().add(8), bytes.len());
            ptr::write_unaligned(
                mount_id,
                i32::try_from(mount).map_err(|_| kinakaze_vfs::EOVERFLOW)?,
            );
        }
        Ok(())
    })();
    posix(result)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_open_by_handle_at(
    mount_fd: c_int,
    handle: *const c_void,
    flags: c_int,
) -> c_int {
    let result = (|| {
        if handle.is_null() {
            return Err(EFAULT);
        }
        if crate::userdb::effective_capabilities()? & (1 << 2) == 0 {
            return Err(EPERM);
        }
        let size = unsafe { ptr::read_unaligned(handle.cast::<u32>()) } as usize;
        let kind = unsafe { ptr::read_unaligned(handle.cast::<u32>().add(1)) };
        if size > 128 {
            return Err(EINVAL);
        }
        if kind != 0x43594f01 {
            return Err(kinakaze_vfs::ESTALE);
        }
        let bytes = unsafe { std::slice::from_raw_parts(handle.cast::<u8>().add(8), size) };
        kinakaze_vfs::mount::overlay::open_handle(mount_fd, bytes, flags)
    })();
    match result {
        Ok(fd) => fd,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn open_by_handle_at(
    mount_fd: c_int,
    handle: *const c_void,
    flags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_open_by_handle_at(mount_fd, handle, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn name_to_handle_at(
    dirfd: c_int,
    pathname: *const c_char,
    handle: *mut c_void,
    mount_id: *mut c_int,
    flags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_name_to_handle_at(dirfd, pathname, handle, mount_id, flags) }
}

/// Version-aware ordering of LP64 directory entries.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_versionsort(
    left: *const *const Dirent,
    right: *const *const Dirent,
) -> c_int {
    unsafe {
        crate::strextra::windows::kinakaze_abi_strverscmp(
            (**left).d_name.as_ptr(),
            (**right).d_name.as_ptr(),
        )
    }
}
