//! Directory streams: `opendir`, `readdir`, `closedir`.
//!
//! A `DIR` buffers getdents64 records. Kernel cursors live in the VFS open
//! description; the stream keeps only its unread buffer and telldir cookie.

use core::ffi::{CStr, c_char, c_int};
use core::ptr;

use kinakaze_vfs::fs;

/// `d_type` values.
pub const DT_UNKNOWN: u8 = 0;
pub const DT_DIR: u8 = 4;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;

/// Longest name reported, matching the Linux `struct dirent`.
const NAME_MAX: usize = 255;

/// The Linux x86_64 `struct dirent`.
///
/// Field order and the 256-byte name array are ABI: guest code indexes
/// `d_name` directly.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Dirent {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [c_char; 256],
}

/// An open directory stream.
pub struct Dir {
    pub fd: c_int,
    buffer: Vec<u8>,
    next: usize,
    filled: usize,
    position: i64,
    /// Storage for the entry returned by the most recent `readdir`.
    ///
    /// `readdir` returns a pointer that stays valid until the next call on the
    /// same stream, so the struct lives here rather than on the stack.
    current: Dirent,
}

/// `opendir`.
///
/// # Safety
///
/// `path` must be a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn opendir(path: *const c_char) -> *mut Dir {
    unsafe { kinakaze_abi_opendir(path) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_opendir(path: *const c_char) -> *mut Dir {
    if path.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a null-terminated string.
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    };
    let fd = match fs::open(path, fs::O_RDONLY | fs::O_DIRECTORY | fs::O_CLOEXEC, 0) {
        Ok(fd) => fd,
        Err(error) => {
            crate::set_errno(error);
            return ptr::null_mut();
        }
    };
    match from_fd(fd) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = kinakaze_vfs::close(fd);
            crate::set_errno(error);
            ptr::null_mut()
        }
    }
}

/// Adopt the caller's descriptor only on success; closedir owns exactly this fd.
pub(crate) fn from_fd(fd: c_int) -> Result<*mut Dir, i32> {
    let entry = kinakaze_vfs::get(fd)?;
    if entry.flags.contains(kinakaze_vfs::FdFlags::PATH_ONLY) {
        return Err(kinakaze_vfs::EBADF);
    }
    if !matches!(
        entry.kind,
        kinakaze_vfs::FdKind::Directory
            | kinakaze_vfs::FdKind::SyntheticDirectory
            | kinakaze_vfs::FdKind::TmpfsDirectory
    ) {
        return Err(kinakaze_vfs::ENOTDIR);
    }
    let position = fs::lseek(fd, 0, fs::SEEK_CUR)? as i64;
    Ok(Box::into_raw(Box::new(Dir {
        fd,
        buffer: vec![0; 32768],
        next: 0,
        filled: 0,
        position,
        current: Dirent {
            d_ino: 0,
            d_off: 0,
            d_reclen: size_of::<Dirent>() as u16,
            d_type: DT_UNKNOWN,
            d_name: [0; 256],
        },
    })))
}

/// `readdir`, returning null at the end of the stream.
///
/// # Safety
///
/// `directory` must be a stream from [`kinakaze_abi_opendir`] that has not been
/// closed.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn readdir(directory: *mut Dir) -> *mut Dirent {
    unsafe { kinakaze_abi_readdir(directory) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_readdir(directory: *mut Dir) -> *mut Dirent {
    if directory.is_null() {
        crate::set_errno(kinakaze_vfs::EBADF);
        return ptr::null_mut();
    }
    // SAFETY: the caller guarantees a live stream from `opendir`.
    let directory = unsafe { &mut *directory };
    if directory.next == directory.filled {
        match crate::fs::restart_metadata(|| {
            fs::read_directory_bytes(directory.fd, &mut directory.buffer, true)
        }) {
            Ok(0) => return ptr::null_mut(),
            Ok(length) => {
                directory.filled = length;
                directory.next = 0;
            }
            Err(error) => {
                crate::set_errno(error);
                return ptr::null_mut();
            }
        }
    }
    let record = &directory.buffer[directory.next..directory.filled];
    let length = u16::from_ne_bytes(record[16..18].try_into().unwrap()) as usize;
    let name = &record[19..length];
    let name_length = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len())
        .min(NAME_MAX);
    directory.current.d_name = [0; 256];
    for (slot, byte) in directory
        .current
        .d_name
        .iter_mut()
        .zip(&name[..name_length])
    {
        *slot = *byte as c_char;
    }
    directory.current.d_type = record[18];
    directory.current.d_reclen = length as u16;
    directory.current.d_off = i64::from_ne_bytes(record[8..16].try_into().unwrap());
    directory.current.d_ino = u64::from_ne_bytes(record[..8].try_into().unwrap());
    directory.position = directory.current.d_off;
    directory.next += length;
    &raw mut directory.current
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_readdir_r(
    directory: *mut Dir,
    entry: *mut Dirent,
    result: *mut *mut Dirent,
) -> c_int {
    if entry.is_null() || result.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if directory.is_null() {
        return kinakaze_vfs::EBADF;
    }
    let previous = kinakaze_tls::errno();
    crate::set_errno(0);
    let next = unsafe { kinakaze_abi_readdir(directory) };
    let error = kinakaze_tls::errno();
    crate::set_errno(previous);
    if !next.is_null() {
        unsafe {
            entry.write(next.read());
            *result = entry;
        }
        0
    } else {
        error
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_readdir64_r(
    directory: *mut Dir,
    entry: *mut Dirent,
    result: *mut *mut Dirent,
) -> c_int {
    unsafe { kinakaze_abi_readdir_r(directory, entry, result) }
}

/// `closedir`.
///
/// # Safety
///
/// `directory` must be a stream from [`kinakaze_abi_opendir`] that is not used
/// again.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn closedir(directory: *mut Dir) -> c_int {
    unsafe { kinakaze_abi_closedir(directory) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_closedir(directory: *mut Dir) -> c_int {
    if directory.is_null() {
        crate::set_errno(kinakaze_vfs::EBADF);
        return -1;
    }
    // SAFETY: the caller guarantees the stream came from `opendir` and is not
    // used again, so reclaiming the box is sound.
    let dir = unsafe { Box::from_raw(directory) };
    if dir.fd >= 0 {
        let _ = kinakaze_vfs::close(dir.fd);
    }
    0
}

/// `rewinddir`.
///
/// # Safety
///
/// `directory` must be a live stream from [`kinakaze_abi_opendir`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn rewinddir(directory: *mut Dir) {
    unsafe { kinakaze_abi_rewinddir(directory) };
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_rewinddir(directory: *mut Dir) {
    if directory.is_null() {
        return;
    }
    // SAFETY: the caller guarantees a live stream.
    let directory = unsafe { &mut *directory };
    seek_stream(directory, 0);
}

/// `telldir`.
///
/// # Safety
///
/// `directory` must be a live stream from [`kinakaze_abi_opendir`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn telldir(directory: *mut Dir) -> i64 {
    unsafe { kinakaze_abi_telldir(directory) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_telldir(directory: *mut Dir) -> i64 {
    if directory.is_null() {
        crate::set_errno(kinakaze_vfs::EBADF);
        return -1;
    }
    // SAFETY: the caller guarantees a live stream.
    unsafe { (*directory).position as i64 }
}

/// `seekdir`.
///
/// # Safety
///
/// `directory` must be a live stream from [`kinakaze_abi_opendir`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn seekdir(directory: *mut Dir, position: i64) {
    unsafe { kinakaze_abi_seekdir(directory, position) };
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_seekdir(directory: *mut Dir, position: i64) {
    if directory.is_null() || position < 0 {
        return;
    }
    if position == 0 {
        unsafe {
            kinakaze_abi_rewinddir(directory);
        }
        return;
    }
    // SAFETY: the caller guarantees a live stream.
    let directory = unsafe { &mut *directory };
    seek_stream(directory, position);
}

fn seek_stream(directory: &mut Dir, position: i64) {
    match fs::lseek(directory.fd, position, fs::SEEK_SET) {
        Ok(position) => {
            directory.position = position as i64;
            directory.next = 0;
            directory.filled = 0;
        }
        Err(error) => crate::set_errno(error),
    }
}

/// Common implementation of the x86_64 directory syscalls. Signal handlers
/// run only after the VFS has unwound its description locks and temporary pins.
unsafe fn getdents_impl(fd: c_int, buffer: *mut u8, count: usize, wide: bool) -> isize {
    if buffer.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let output = unsafe { core::slice::from_raw_parts_mut(buffer, count) };
    match crate::fs::restart_metadata(|| fs::read_directory_bytes(fd, output, wide)) {
        Ok(length) => length as isize,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// Raw `getdents(2)` using the x86_64 variable-length `linux_dirent` layout.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getdents(
    fd: c_int,
    buffer: *mut u8,
    count: usize,
) -> isize {
    unsafe { getdents_impl(fd, buffer, count, false) }
}

/// Raw `getdents64(2)` using `linux_dirent64` records.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getdents64(
    fd: c_int,
    buffer: *mut u8,
    count: usize,
) -> isize {
    unsafe { getdents_impl(fd, buffer, count, true) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdopendir_preserves_kernel_position_and_seek_discards_buffered_entries() {
        let fd = fs::open("/proc/self/ns", fs::O_RDONLY | fs::O_DIRECTORY, 0).unwrap();
        let mut first = [0u8; 24];
        assert_eq!(
            unsafe { kinakaze_abi_getdents64(fd, first.as_mut_ptr(), first.len()) },
            24
        );
        assert_eq!(&first[19..21], b".\0");
        let stream = from_fd(fd).unwrap();
        unsafe {
            let entry = kinakaze_abi_readdir(stream);
            assert!(!entry.is_null());
            assert_eq!(CStr::from_ptr((*entry).d_name.as_ptr()).to_bytes(), b"..");
            let cookie = kinakaze_abi_telldir(stream);
            let next = kinakaze_abi_readdir(stream);
            assert!(!next.is_null());
            let name = CStr::from_ptr((*next).d_name.as_ptr()).to_owned();
            kinakaze_abi_seekdir(stream, cookie);
            let next = kinakaze_abi_readdir(stream);
            assert!(!next.is_null());
            assert_eq!(CStr::from_ptr((*next).d_name.as_ptr()), name.as_c_str());
            kinakaze_abi_rewinddir(stream);
            let entry = kinakaze_abi_readdir(stream);
            assert!(!entry.is_null());
            assert_eq!(CStr::from_ptr((*entry).d_name.as_ptr()).to_bytes(), b".");
            assert_eq!(kinakaze_abi_closedir(stream), 0);
        }
        assert!(matches!(kinakaze_vfs::get(fd), Err(kinakaze_vfs::EBADF)));
    }

    #[test]
    fn raw_getdents64_packs_linux_records_and_tracks_the_cursor() {
        let fd = fs::open(
            "/proc/self/ns",
            fs::O_RDONLY | fs::O_DIRECTORY | fs::O_CLOEXEC,
            0,
        )
        .unwrap();
        let mut buffer = [0u8; 4096];
        let read = unsafe { kinakaze_abi_getdents64(fd, buffer.as_mut_ptr(), buffer.len()) };
        assert!(read > 0);

        let mut names = Vec::new();
        let mut offset = 0usize;
        while offset < read as usize {
            let record_len =
                u16::from_ne_bytes(buffer[offset + 16..offset + 18].try_into().unwrap()) as usize;
            assert!(record_len >= 24);
            let name_start = offset + 19;
            let name_end = buffer[name_start..offset + record_len]
                .iter()
                .position(|byte| *byte == 0)
                .map(|end| name_start + end)
                .unwrap();
            names.push(String::from_utf8(buffer[name_start..name_end].to_vec()).unwrap());
            offset += record_len;
        }
        assert!(names.iter().any(|name| name == "."));
        assert!(names.iter().any(|name| name == ".."));
        assert!(names.iter().any(|name| name == "cgroup"));
        assert_eq!(
            unsafe { kinakaze_abi_getdents64(fd, buffer.as_mut_ptr(), buffer.len()) },
            0
        );
        kinakaze_vfs::close(fd).unwrap();
    }
}
