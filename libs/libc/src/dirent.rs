//! Directory streams: `opendir`, `readdir`, `closedir`.
//!
//! A `DIR` holds a snapshot taken at `opendir` time. POSIX leaves the visibility
//! of concurrent changes unspecified, and a snapshot is both the simplest honest
//! choice and what Windows directory enumeration naturally provides.

use core::ffi::{CStr, c_char, c_int};
use core::ptr;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use kinakaze_vfs::fs::{self, DirectoryEntry};

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
    entries: Vec<DirectoryEntry>,
    untyped_native: bool,
    overlay_records: Option<Vec<kinakaze_vfs::mount::overlay::DirectoryRecord>>,
    position: usize,
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

fn directory_snapshot(fd: c_int) -> Result<(Vec<DirectoryEntry>, bool), i32> {
    if let Some(names) = fs::read_directory_names_fd(fd)? {
        Ok((
            names
                .into_iter()
                .map(|name| DirectoryEntry {
                    is_directory: name == "." || name == "..",
                    is_symlink: false,
                    name,
                })
                .collect(),
            true,
        ))
    } else {
        fs::read_directory_fd(fd).map(|entries| (entries, false))
    }
}

/// Adopt the caller's descriptor only on success; closedir owns exactly this fd.
pub(crate) fn from_fd(fd: c_int) -> Result<*mut Dir, i32> {
    let overlay_records = kinakaze_vfs::mount::overlay::directory_records(fd)?;
    let (entries, untyped_native) = if let Some(records) = &overlay_records {
        (
            records
                .iter()
                .map(|r| DirectoryEntry {
                    name: r.name.clone(),
                    is_directory: r.kind == DT_DIR,
                    is_symlink: r.kind == DT_LNK,
                })
                .collect(),
            false,
        )
    } else {
        directory_snapshot(fd)?
    };
    Ok(Box::into_raw(Box::new(Dir {
        fd,
        entries,
        untyped_native,
        overlay_records,
        position: 0,
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
    let Some(entry) = directory.entries.get(directory.position) else {
        // End of stream is not an error, so errno is left untouched.
        return ptr::null_mut();
    };
    directory.position += 1;

    let bytes = entry.name.as_bytes();
    let length = bytes.len().min(NAME_MAX);
    directory.current.d_name = [0; 256];
    for (slot, byte) in directory.current.d_name.iter_mut().zip(&bytes[..length]) {
        *slot = *byte as c_char;
    }
    directory.current.d_type = dirent_type(entry, directory.untyped_native);
    // d_off is the offset of the *next* entry, which is what a caller uses to
    // resume with seekdir.
    directory.current.d_off = directory.position as i64;
    // The host inode is not available from a plain listing; a stable synthetic
    // value keeps callers that only test for non-zero working.
    directory.current.d_ino = directory.position as u64;
    if let Some(records) = &directory.overlay_records {
        let record = &records[directory.position - 1];
        directory.current.d_ino = record.inode;
        directory.current.d_type = record.kind;
    }
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
    if directory.overlay_records.is_some() {
        match kinakaze_vfs::mount::overlay::directory_records(directory.fd) {
            Ok(Some(records)) => {
                directory.entries = records
                    .iter()
                    .map(|r| DirectoryEntry {
                        name: r.name.clone(),
                        is_directory: r.kind == DT_DIR,
                        is_symlink: r.kind == DT_LNK,
                    })
                    .collect();
                directory.overlay_records = Some(records);
            }
            Err(error) => crate::set_errno(error),
            _ => {}
        }
    }
    directory.position = 0;
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
    // Clamp rather than allow a position past the end, so the next readdir
    // reports end of stream instead of indexing out of range.
    directory.position = (position as usize).min(directory.entries.len());
}

struct RawDirectory {
    generation: u32,
    entries: Vec<DirectoryEntry>,
    untyped_native: bool,
    position: usize,
}

fn raw_directories() -> &'static Mutex<HashMap<c_int, RawDirectory>> {
    static DIRECTORIES: OnceLock<Mutex<HashMap<c_int, RawDirectory>>> = OnceLock::new();
    DIRECTORIES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dirent_type(entry: &DirectoryEntry, untyped_native: bool) -> u8 {
    if entry.is_directory {
        DT_DIR
    } else if untyped_native {
        // Linux permits DT_UNKNOWN; name-only users such as GDir avoid a stat
        // per icon, and scandir users resolve types lazily with fstatat/lstat.
        DT_UNKNOWN
    } else if entry.is_symlink {
        DT_LNK
    } else {
        DT_REG
    }
}

/// Common implementation of the x86_64 `getdents` and `getdents64` kernel ABIs.
///
/// Directory snapshots are keyed by both descriptor and descriptor generation,
/// so closing and reusing an fd cannot inherit the old directory's cursor.
unsafe fn getdents_impl(fd: c_int, buffer: *mut u8, count: usize, wide: bool) -> isize {
    // Native enumeration can retire pending I/O on a signal. Unwind the
    // directory snapshot lock before delivering handlers, and honor SA_RESTART
    // just like the other metadata syscalls. No cursor advances on failure.
    match crate::fs::restart_metadata(|| {
        let result = unsafe { getdents_once(fd, buffer, count, wide) };
        if result < 0 {
            Err(crate::kinakaze_errno())
        } else {
            Ok(result)
        }
    }) {
        Ok(result) => result,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

unsafe fn getdents_once(fd: c_int, buffer: *mut u8, count: usize, wide: bool) -> isize {
    if buffer.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    if kinakaze_vfs::get(fd).is_ok_and(|e| e.kind == kinakaze_vfs::FdKind::TmpfsDirectory) {
        return match kinakaze_vfs::tmpfs::read_directory_bytes(
            fd,
            unsafe { core::slice::from_raw_parts_mut(buffer, count) },
            wide,
        ) {
            Ok(n) => n as isize,
            Err(e) => {
                crate::set_errno(e);
                -1
            }
        };
    }
    match kinakaze_vfs::mount::overlay::read_directory_bytes(
        fd,
        unsafe { core::slice::from_raw_parts_mut(buffer, count) },
        wide,
    ) {
        Ok(Some(length)) => return length as isize,
        Ok(None) => {}
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    }
    let descriptor = match kinakaze_vfs::get(fd) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    };
    if !matches!(
        descriptor.kind,
        kinakaze_vfs::FdKind::Directory
            | kinakaze_vfs::FdKind::SyntheticDirectory
            | kinakaze_vfs::FdKind::TmpfsDirectory
    ) {
        crate::set_errno(kinakaze_vfs::ENOTDIR);
        return -1;
    }

    let mut directories = match raw_directories().lock() {
        Ok(directories) => directories,
        Err(_) => {
            crate::set_errno(kinakaze_vfs::EIO);
            return -1;
        }
    };
    let needs_snapshot = directories
        .get(&fd)
        .is_none_or(|directory| directory.generation != descriptor.generation);
    if needs_snapshot {
        let (entries, untyped_native) = match directory_snapshot(fd) {
            Ok(entries) => entries,
            Err(error) => {
                crate::set_errno(error);
                return -1;
            }
        };
        directories.insert(
            fd,
            RawDirectory {
                generation: descriptor.generation,
                entries,
                untyped_native,
                position: 0,
            },
        );
    }
    let directory = directories.get_mut(&fd).expect("snapshot was inserted");
    let output = unsafe { core::slice::from_raw_parts_mut(buffer, count) };
    let mut written = 0usize;

    while let Some(entry) = directory.entries.get(directory.position) {
        let name = entry.name.as_bytes();
        let unaligned = if wide {
            19usize.saturating_add(name.len()).saturating_add(1)
        } else {
            18usize.saturating_add(name.len()).saturating_add(2)
        };
        let record_len = unaligned.saturating_add(7) & !7;
        if record_len > u16::MAX as usize || written.saturating_add(record_len) > count {
            if written == 0 {
                crate::set_errno(kinakaze_vfs::EINVAL);
                return -1;
            }
            break;
        }

        let record = &mut output[written..written + record_len];
        record.fill(0);
        let inode = (directory.position + 1) as u64;
        let next = (directory.position + 1) as i64;
        record[0..8].copy_from_slice(&inode.to_ne_bytes());
        record[8..16].copy_from_slice(&next.to_ne_bytes());
        record[16..18].copy_from_slice(&(record_len as u16).to_ne_bytes());
        if wide {
            record[18] = dirent_type(entry, directory.untyped_native);
            record[19..19 + name.len()].copy_from_slice(name);
        } else {
            record[18..18 + name.len()].copy_from_slice(name);
            record[record_len - 1] = dirent_type(entry, directory.untyped_native);
        }
        directory.position += 1;
        written += record_len;
    }
    written as isize
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
