//! File system C ABI exports.
//!
//! Every entry point converts the guest's C strings and out-pointers into safe
//! Rust values, forwards to [`kinakaze_vfs::fs`], then applies the POSIX
//! convention of returning `-1` with `errno` set.

use core::ffi::{CStr, c_char};
use core::ptr;

use kinakaze_vfs::fs::{self, Stat};
use kinakaze_vfs::{EFAULT, EINVAL, ENAMETOOLONG, ERANGE};

use crate::set_errno;

/// Longest guest path accepted, matching Linux `PATH_MAX`.
const PATH_MAX: usize = 4096;

/// Borrows a guest path string.
///
/// # Safety
///
/// `path` must be null or point to a null-terminated string that stays valid
/// for the duration of the call.
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
    raw.to_str().map_err(|_| kinakaze_vfs::EINVAL)
}

/// Applies the POSIX `-1`/`errno` convention to a result.
fn posix<T>(result: Result<T, i32>, success: impl FnOnce(T) -> i64) -> i64 {
    match result {
        Ok(value) => success(value),
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

pub(crate) fn open_with_restart(
    flags: i32,
    mut operation: impl FnMut() -> Result<i32, i32>,
) -> Result<i32, i32> {
    if flags & fs::O_PATH != 0 {
        return restart_metadata(operation);
    }
    loop {
        let interrupted_before = kinakaze_vfs::signal::nonrestart_epoch();
        let result = operation();
        if result != Err(kinakaze_vfs::EINTR) {
            return result;
        }
        // Overlay copy-up and native metadata cancellation leave the signal
        // pending until all inode locks unwind. FIFO rendezvous dispatches
        // internally to retain its endpoint across SA_RESTART. Its already
        // handled non-restarting signal must still propagate EINTR here.
        let delivery = kinakaze_vfs::signal::deliver_pending();
        if delivery == kinakaze_vfs::signal::Delivery::Interrupted
            || kinakaze_vfs::signal::nonrestart_epoch() != interrupted_before
        {
            return result;
        }
    }
}

/// Only for operations whose error paths have released all locks and rolled
/// back their visible state. FIFO/data I/O owns its own restart bookkeeping.
pub(crate) fn restart_metadata<T>(mut operation: impl FnMut() -> Result<T, i32>) -> Result<T, i32> {
    loop {
        match operation() {
            Err(kinakaze_vfs::EINTR) => {
                // Metadata locks unwind before returning EINTR. Dispatch here,
                // after all VFS guards have dropped, so a handler can safely
                // enter the filesystem. Ignored signals and SA_RESTART must
                // not leak a transient inode-lock interruption to openat.
                if kinakaze_vfs::signal::deliver_pending()
                    == kinakaze_vfs::signal::Delivery::Interrupted
                {
                    return Err(kinakaze_vfs::EINTR);
                }
            }
            result => return result,
        }
    }
}

/// Runs a path operation that returns 0 on success.
///
/// # Safety
///
/// `path` must satisfy the [`borrow_path`] contract.
unsafe fn path_op(path: *const c_char, operation: impl FnOnce(&str) -> Result<(), i32>) -> i32 {
    // SAFETY: forwarded from this function's own contract.
    let borrowed = unsafe { borrow_path(path) };
    posix(borrowed.and_then(operation), |()| 0) as i32
}

/// `open`.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
pub unsafe extern "sysv64" fn open(path: *const c_char, flags: i32, mode: u32) -> i32 {
    // SAFETY: forwarded from this function's contract.
    let borrowed = unsafe { borrow_path(path) };
    let res = posix(
        borrowed.and_then(|path| {
            open_with_restart(flags, || {
                fs::open(path, flags, crate::fsextra::creation_mode(mode))
            })
        }),
        i64::from,
    ) as i32;
    if crate::trace_enabled() {
        eprintln!(
            "kinakaze: [TRACE open] path={:?} flags={:#x} -> fd={}",
            borrowed, flags, res
        );
    }
    res
}

/// `openat`.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
pub unsafe extern "sysv64" fn openat(
    dirfd: i32,
    path: *const c_char,
    flags: i32,
    mode: u32,
) -> i32 {
    // SAFETY: forwarded from this function's contract.
    let borrowed = unsafe { borrow_path(path) };
    let res = posix(
        borrowed.and_then(|path| {
            open_with_restart(flags, || {
                fs::openat(dirfd, path, flags, crate::fsextra::creation_mode(mode))
            })
        }),
        i64::from,
    ) as i32;
    if crate::trace_enabled() {
        eprintln!(
            "kinakaze: [TRACE openat] dirfd={} path={:?} flags={:#x} -> fd={}",
            dirfd, borrowed, flags, res
        );
    }
    res
}

/// `creat`, defined by POSIX as `open` with write-only create and truncate.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
pub unsafe extern "sysv64" fn creat(path: *const c_char, mode: u32) -> i32 {
    // SAFETY: forwarded from this function's contract.
    unsafe { open(path, fs::O_WRONLY | fs::O_CREAT | fs::O_TRUNC, mode) }
}

pub extern "sysv64" fn lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    posix(fs::lseek(fd, offset, whence), |position| position as i64)
}

/// Copies a `Stat` into the guest's buffer.
///
/// # Safety
///
/// `out` must be null or point to a writable `struct stat`.
unsafe fn write_stat(out: *mut Stat, value: Result<Stat, i32>) -> i32 {
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

/// `stat`.
///
/// # Safety
///
/// `path` must be a valid string and `out` a writable `struct stat`.
pub unsafe extern "sysv64" fn stat(path: *const c_char, out: *mut Stat) -> i32 {
    // SAFETY: forwarded from this function's contract.
    let borrowed = unsafe { borrow_path(path) };
    let value = borrowed.and_then(|path| restart_metadata(|| fs::stat(path)));
    if crate::trace_enabled() {
        eprintln!(
            "kinakaze: [TRACE stat] path={borrowed:?} result={:?}",
            value.as_ref().map(|value| value.st_size)
        );
    }
    // SAFETY: forwarded from this function's contract.
    unsafe { write_stat(out, value) }
}

/// `lstat`.
///
/// # Safety
///
/// `path` must be a valid string and `out` a writable `struct stat`.
pub unsafe extern "sysv64" fn lstat(path: *const c_char, out: *mut Stat) -> i32 {
    // SAFETY: forwarded from this function's contract.
    let value = unsafe { borrow_path(path) }.and_then(|path| restart_metadata(|| fs::lstat(path)));
    // SAFETY: forwarded from this function's contract.
    unsafe { write_stat(out, value) }
}

/// `fstat`.
///
/// # Safety
///
/// `out` must point to a writable `struct stat`.
pub unsafe extern "sysv64" fn fstat(fd: i32, out: *mut Stat) -> i32 {
    // SAFETY: forwarded from this function's contract.
    unsafe { write_stat(out, restart_metadata(|| fs::fstat(fd))) }
}

/// `unlink`.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
pub unsafe extern "sysv64" fn unlink(path: *const c_char) -> i32 {
    // SAFETY: forwarded from this function's contract.
    let borrowed = unsafe { borrow_path(path) };
    posix(
        borrowed.and_then(|path| restart_metadata(|| fs::unlink(path))),
        |()| 0,
    ) as i32
}

/// `rmdir`.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
pub unsafe extern "sysv64" fn rmdir(path: *const c_char) -> i32 {
    // SAFETY: forwarded from this function's contract.
    unsafe { path_op(path, fs::rmdir) }
}

/// `mkdir`.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
pub unsafe extern "sysv64" fn mkdir(path: *const c_char, mode: u32) -> i32 {
    // Keep the borrowed bytes stable while SA_RESTART repeats an interrupted
    // directory lookup or create after all VFS locks have unwound.
    let borrowed = unsafe { borrow_path(path) };
    posix(
        borrowed.and_then(|path| {
            restart_metadata(|| fs::mkdir(path, crate::fsextra::creation_mode(mode)))
        }),
        |()| 0,
    ) as i32
}

/// `chdir`.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
pub unsafe extern "sysv64" fn chdir(path: *const c_char) -> i32 {
    // Keep the borrowed bytes stable across an SA_RESTART retry. The VFS only
    // publishes the new fs_struct after the directory lookup has completed.
    let borrowed = unsafe { borrow_path(path) };
    posix(
        borrowed.and_then(|path| restart_metadata(|| fs::chdir(path))),
        |()| 0,
    ) as i32
}

/// `access`.
///
/// # Safety
///
/// `path` must be a valid null-terminated string.
pub unsafe extern "sysv64" fn access(path: *const c_char, mode: i32) -> i32 {
    // SAFETY: forwarded from this function's contract.
    unsafe { path_op(path, |path| restart_metadata(|| fs::access(path, mode))) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_eaccess(path: *const c_char, mode: i32) -> i32 {
    unsafe { access(path, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn eaccess(path: *const c_char, mode: i32) -> i32 {
    unsafe { access(path, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_euidaccess(path: *const c_char, mode: i32) -> i32 {
    unsafe { access(path, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn euidaccess(path: *const c_char, mode: i32) -> i32 {
    unsafe { access(path, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___openat64_2(
    dirfd: i32,
    path: *const c_char,
    flags: i32,
) -> i32 {
    unsafe { openat(dirfd, path, flags, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __openat64_2(dirfd: i32, path: *const c_char, flags: i32) -> i32 {
    unsafe { openat(dirfd, path, flags, 0) }
}

/// `rename`.
///
/// # Safety
///
/// Both arguments must be valid null-terminated strings.
pub unsafe extern "sysv64" fn rename(from: *const c_char, to: *const c_char) -> i32 {
    // SAFETY: forwarded from this function's contract.
    let from = unsafe { borrow_path(from) };
    // SAFETY: forwarded from this function's contract.
    let to = unsafe { borrow_path(to) };
    let result = from.and_then(|from| to.and_then(|to| fs::rename(from, to)));
    posix(result, |()| 0) as i32
}

pub extern "sysv64" fn ftruncate(fd: i32, length: i64) -> i32 {
    let result = fs::ftruncate(fd, length).and_then(|()| {
        crate::fdio::refresh_file_mappings(fd, u64::try_from(length).map_err(|_| EINVAL)?)
    });
    if result == Err(27) {
        kinakaze_vfs::signal::deliver_pending();
    }
    posix(result, |()| 0) as i32
}

/// `getcwd`, which writes into the caller's buffer and returns it.
///
/// # Safety
///
/// `buffer` must name at least `size` writable bytes unless it is null.
pub unsafe extern "sysv64" fn getcwd(buffer: *mut c_char, size: usize) -> *mut c_char {
    let cwd = fs::getcwd();
    let bytes = cwd.as_bytes();
    let needed = bytes.len() + 1;
    let target = if buffer.is_null() {
        let alloc_size = if size == 0 { needed } else { size.max(needed) };
        let allocated = unsafe { crate::c_malloc(alloc_size) } as *mut c_char;
        if allocated.is_null() {
            set_errno(kinakaze_vfs::ENOMEM);
            return ptr::null_mut();
        }
        allocated
    } else {
        if size < needed {
            set_errno(ERANGE);
            return ptr::null_mut();
        }
        buffer
    };
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast::<u8>(), bytes.len());
        ptr::write(target.add(bytes.len()), 0);
    }
    target
}

/// The Linux kernel `getcwd(2)` ABI used by a bare `syscall` instruction.
///
/// Unlike the libc wrapper above, the kernel never allocates and returns the
/// number of bytes written, including the trailing NUL.  Keeping this separate
/// prevents the libc pointer-return convention from leaking into raw syscall
/// callers such as Go's runtime.
///
/// # Safety
///
/// A non-null `buffer` must name at least `size` writable bytes.
pub(crate) unsafe fn raw_getcwd(buffer: *mut c_char, size: usize) -> i64 {
    if buffer.is_null() {
        return -i64::from(EFAULT);
    }
    let cwd = fs::getcwd();
    let bytes = cwd.as_bytes();
    let needed = bytes.len() + 1;
    if size < needed {
        return -i64::from(ERANGE);
    }
    // SAFETY: the caller provides `size` writable bytes and the check above
    // proves the path plus its terminator fits.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), bytes.len());
        ptr::write(buffer.add(bytes.len()), 0);
    }
    needed as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_get_current_dir_name() -> *mut c_char {
    unsafe { getcwd(ptr::null_mut(), 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn get_current_dir_name() -> *mut c_char {
    unsafe { getcwd(ptr::null_mut(), 0) }
}

/// `isatty`.
///
/// The probe is per descriptor and truthful: a pty end resolves to its terminal
/// and the console is confirmed with `GetConsoleMode`. Windows' `FILE_TYPE_CHAR`
/// is not enough on its own — it also covers `NUL` — and Linux's contract is
/// stronger: success here implies the terminal ioctls work, so this uses the
/// same resolution `tcgetattr` does and the two can never disagree.
pub extern "sysv64" fn isatty(fd: i32) -> i32 {
    match kinakaze_vfs::get(fd) {
        Ok(_) if kinakaze_vfs::tty::isatty(fd) => 1,
        Ok(_) => {
            // POSIX specifies ENOTTY for a valid descriptor that is not a terminal.
            set_errno(kinakaze_vfs::ENOTTY);
            0
        }
        Err(error) => {
            set_errno(error);
            0
        }
    }
}

/// Registers the unprefixed System V exports the ELF guest resolves against.
///
/// The functions above are already `sysv64`; this module re-exports them under
/// the bare Linux names through `#[no_mangle]` shims.
pub mod exports {
    use super::*;

    /// Exports an entry point that borrows guest pointers.
    macro_rules! sysv_export {
        ($alias:ident, $name:ident ( $($argument:ident : $type:ty),* ) -> $result:ty) => {
            #[unsafe(no_mangle)]
            /// System V ABI export. See the wrapped function for the contract.
            ///
            /// # Safety
            ///
            /// Pointer arguments must satisfy the wrapped function's contract.
            pub unsafe extern "sysv64" fn $alias($($argument: $type),*) -> $result {
                // SAFETY: forwarded from this shim's own contract.
                unsafe { super::$name($($argument),*) }
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "sysv64" fn $name($($argument: $type),*) -> $result {
                unsafe { super::$name($($argument),*) }
            }
        };
    }

    /// Exports an entry point whose arguments are all plain scalars.
    macro_rules! sysv_export_safe {
        ($alias:ident, $name:ident ( $($argument:ident : $type:ty),* ) -> $result:ty) => {
            #[unsafe(no_mangle)]
            /// System V ABI export. See the wrapped function for behaviour.
            pub extern "sysv64" fn $alias($($argument: $type),*) -> $result {
                super::$name($($argument),*)
            }

            #[unsafe(no_mangle)]
            pub extern "sysv64" fn $name($($argument: $type),*) -> $result {
                super::$name($($argument),*)
            }
        };
    }

    sysv_export!(kinakaze_abi_open, open(path: *const c_char, flags: i32, mode: u32) -> i32);
    sysv_export!(kinakaze_abi_openat, openat(dirfd: i32, path: *const c_char, flags: i32, mode: u32) -> i32);
    sysv_export!(kinakaze_abi_creat, creat(path: *const c_char, mode: u32) -> i32);
    sysv_export_safe!(kinakaze_abi_lseek, lseek(fd: i32, offset: i64, whence: i32) -> i64);
    sysv_export!(kinakaze_abi_stat, stat(path: *const c_char, out: *mut Stat) -> i32);
    sysv_export!(kinakaze_abi_lstat, lstat(path: *const c_char, out: *mut Stat) -> i32);
    sysv_export!(kinakaze_abi_fstat, fstat(fd: i32, out: *mut Stat) -> i32);
    sysv_export!(kinakaze_abi_unlink, unlink(path: *const c_char) -> i32);
    sysv_export!(kinakaze_abi_rmdir, rmdir(path: *const c_char) -> i32);
    sysv_export!(kinakaze_abi_mkdir, mkdir(path: *const c_char, mode: u32) -> i32);
    sysv_export!(kinakaze_abi_chdir, chdir(path: *const c_char) -> i32);
    sysv_export!(kinakaze_abi_access, access(path: *const c_char, mode: i32) -> i32);
    sysv_export!(kinakaze_abi_rename, rename(from: *const c_char, to: *const c_char) -> i32);
    sysv_export_safe!(kinakaze_abi_ftruncate, ftruncate(fd: i32, length: i64) -> i32);
    sysv_export!(kinakaze_abi_getcwd, getcwd(buffer: *mut c_char, size: usize) -> *mut c_char);
    sysv_export_safe!(kinakaze_abi_isatty, isatty(fd: i32) -> i32);

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __open_2(path: *const c_char, flags: i32) -> i32 {
        unsafe { super::open(path, flags, 0) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___open_2(path: *const c_char, flags: i32) -> i32 {
        unsafe { super::open(path, flags, 0) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn __openat_2(dirfd: i32, path: *const c_char, flags: i32) -> i32 {
        unsafe { super::openat(dirfd, path, flags, 0) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "sysv64" fn kinakaze_abi___openat_2(
        dirfd: i32,
        path: *const c_char,
        flags: i32,
    ) -> i32 {
        unsafe { super::openat(dirfd, path, flags, 0) }
    }
}
