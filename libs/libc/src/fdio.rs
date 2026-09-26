//! Descriptor duplication, unlocked stdio, mapping, temporary files and timestamps.
//!
//! The calls collected here are the ones a shell and its utilities reach for
//! between opening a file and closing it: duplicating a descriptor, asking what
//! flags it carries, waiting for it to become ready, mapping it into memory,
//! reading it without disturbing its position, and stamping a time on it.
//!
//! Every descriptor number in this module comes from [`kinakaze_vfs`]. Nothing
//! here invents an fd. File VMAs retain their own registered inode references,
//! just as Linux mappings retain a file after its descriptor has been closed.
//!
//! Record locks use the VFS's shared Linux process-owned advisory-lock registry.
//! They do not impose mandatory Windows byte-range locks on ordinary I/O.
//!
//! Three other implementation details matter to callers:
//!
//! **`F_SETFL` changes `O_NONBLOCK` and `O_APPEND` in descriptor-table state.**
//! Host sockets stay non-blocking underneath so their waits remain
//! interruptible; the Linux flag decides whether the compatibility layer waits
//! or returns `EAGAIN`. Files, pipes and terminals consult the same flag word.
//!
//! **A mapping's offset is rounded to the allocation granularity, not the page
//! size.** `mmap64` accepts any page-aligned offset, as Linux does, but Windows
//! requires views to begin on a 64 KiB boundary. The gap is absorbed by mapping
//! from the granularity floor and returning an interior pointer;
//! [`kinakaze_abi_munmap`] therefore has to remember the real view base, which
//! is what the mapping registry is for.
//!
//! **The `*_unlocked` stdio calls are the locked calls.** glibc's unlocked forms
//! exist to skip a `FILE` lock. Every stream in [`crate::stdio`] holds a mutex
//! that is taken for the duration of one operation and is never exposed through
//! `flockfile`, so there is no lock for these to skip and no fast path to offer.
//! They forward to the same bodies, which is the honest implementation: it is
//! correct under concurrency, and the alternative — a genuinely unsynchronized
//! path — would be faster only by being wrong.

mod allocation;
pub use allocation::{
    kinakaze_abi_fallocate, kinakaze_abi_fallocate64, kinakaze_abi_posix_fallocate,
    kinakaze_abi_posix_fallocate64,
};
mod positioned;
pub use positioned::{
    kinakaze_abi_preadv64, kinakaze_abi_preadv64v2, kinakaze_abi_pwrite64, kinakaze_abi_pwritev64,
    kinakaze_abi_pwritev64v2,
};

use core::alloc::{GlobalAlloc, Layout};
use core::ffi::{CStr, c_char, c_int, c_void};
use core::mem::{align_of, size_of};
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use kinakaze_vfs::fs::{self, SEEK_CUR, SEEK_END, SEEK_SET};
use kinakaze_vfs::{
    EBADF, EFAULT, EINVAL, EIO, ENOMEM, ENOSYS, EPERM, ESPIPE, FdEntry, FdFlags, FdKind,
    errno_from_win32,
};
use windows_sys::Win32::Networking::WinSock::{
    POLLERR as WSA_POLLERR, POLLHUP as WSA_POLLHUP, POLLIN as WSA_POLLIN, POLLNVAL as WSA_POLLNVAL,
    POLLOUT as WSA_POLLOUT, WSAPOLLFD, WSAPoll,
};

use crate::set_errno;
use crate::stdio::File;

mod file_origin;
pub(crate) mod verity;
pub use verity::kinakaze_abi_mapping_fault_signal;

/// `EEXIST`, reported by the temporary-name generators when a name is taken.
const EEXIST: i32 = 17;

/// `EOVERFLOW`, which the VFS error list does not carry.
///
/// `posix_fallocate64` and `mmap64` both need it: it is what Linux reports when a
/// requested size or offset cannot be represented in the file's length.
const EOVERFLOW: i32 = 75;

/// `ENODEV`, reported by `mmap` for a descriptor that cannot be mapped.
const ENODEV: i32 = 19;

/// `AT_FDCWD`, the `*at` family's "resolve against the current directory".
const AT_FDCWD: c_int = fs::AT_FDCWD;

/// `AT_SYMLINK_NOFOLLOW`, which makes an `*at` call act on a link itself.
const AT_SYMLINK_NOFOLLOW: c_int = 0x100;

/// `AT_EMPTY_PATH`, Linux's "operate on `dirfd` itself" extension.
const AT_EMPTY_PATH: c_int = 0x1000;

/// Longest guest path accepted, matching Linux `PATH_MAX`.
const PATH_MAX: usize = 4096;

// ---------------------------------------------------------------------------
// Win32 and NT entry points.
//
// Declared inline rather than imported from `windows-sys`, because this crate
// enables only the `Win32_System_Threading` feature and the names below are
// spread across half a dozen others. Every signature matches the Windows
// headers: `BOOL` is `i32`, a `DWORD` is `u32`, an `NTSTATUS` is `i32` that is
// negative on failure, and a `HANDLE` is `*mut c_void`.
// ---------------------------------------------------------------------------

#[link(name = "kernel32")]
unsafe extern "system" {
    /// `GetLastError`: the calling thread's last error code.
    fn GetLastError() -> u32;
    /// `CloseHandle`: releases a kernel object handle.
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn ReOpenFile(handle: *mut c_void, access: u32, sharing: u32, flags: u32) -> *mut c_void;
    /// `GetCurrentProcess`: the pseudo-handle for this process.
    fn GetCurrentProcess() -> *mut c_void;
    /// `DuplicateHandle`: copies a handle, optionally into another process.
    fn DuplicateHandle(
        source_process: *mut c_void,
        source: *mut c_void,
        target_process: *mut c_void,
        target: *mut *mut c_void,
        access: u32,
        inheritable: i32,
        options: u32,
    ) -> i32;
    /// `CreateFileW`: opens or creates a file, directory or device.
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        attributes: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    /// `GetFinalPathNameByHandleW`: the path an open handle refers to.
    fn GetFinalPathNameByHandleW(handle: *mut c_void, path: *mut u16, size: u32, flags: u32)
    -> u32;
    /// `SetFileTime`: writes a file's creation, access and write times.
    fn SetFileTime(
        file: *mut c_void,
        creation: *const FileTime,
        access: *const FileTime,
        write: *const FileTime,
    ) -> i32;
    /// `GetSystemTimeAsFileTime`: the current time in FILETIME ticks.
    fn GetSystemTimeAsFileTime(time: *mut FileTime);
    /// `GetFileSizeEx`: an open file's length in bytes.
    fn GetFileSizeEx(file: *mut c_void, size: *mut i64) -> i32;
    #[cfg(test)]
    /// `SetFilePointerEx`: moves a synchronous handle's file pointer.
    fn SetFilePointerEx(
        file: *mut c_void,
        distance: i64,
        new_position: *mut i64,
        method: u32,
    ) -> i32;
    /// `CreateFileMappingW`: a section object over a file or the page file.
    fn CreateFileMappingW(
        file: *mut c_void,
        attributes: *const SecurityAttributes,
        protection: u32,
        maximum_high: u32,
        maximum_low: u32,
        name: *const u16,
    ) -> *mut c_void;
    /// `MapViewOfFileEx`: maps a section, optionally at a chosen address.
    fn MapViewOfFileEx(
        mapping: *mut c_void,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        length: usize,
        address: *mut c_void,
    ) -> *mut c_void;
    /// `UnmapViewOfFile`: releases a mapped view.
    fn UnmapViewOfFile(address: *const c_void) -> i32;
    /// `FlushViewOfFile`: writes a shared view's dirty pages back.
    fn FlushViewOfFile(address: *const c_void, length: usize) -> i32;
    /// `VirtualAlloc`: reserves or commits pages of process memory.
    fn VirtualAlloc(
        address: *mut c_void,
        size: usize,
        allocation: u32,
        protection: u32,
    ) -> *mut c_void;
    /// `VirtualFree`: decommits or releases pages of process memory.
    fn VirtualFree(address: *mut c_void, size: usize, free_type: u32) -> i32;
    /// `VirtualProtect`: changes access on already committed pages.
    fn VirtualProtect(
        address: *mut c_void,
        size: usize,
        protection: u32,
        old_protection: *mut u32,
    ) -> i32;
    /// `VirtualQuery`: the state of the region containing an address.
    fn VirtualQuery(
        address: *const c_void,
        info: *mut MemoryBasicInformation,
        length: usize,
    ) -> usize;
    /// `GetSystemInfo`: page size and allocation granularity.
    fn GetSystemInfo(info: *mut SystemInfo);
    /// `GetConsoleMode`: the live test for "this handle is a console".
    ///
    /// Necessary because `GetFileType` reports `FILE_TYPE_CHAR` for `NUL` and
    /// for printers as well as for consoles, so the descriptor table's own
    /// classification cannot distinguish a terminal from a character device.
    fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    /// `NtQueryInformationFile`: one class of information about an open file.
    ///
    /// Used for two classes Win32 does not expose: the pipe state and quota that
    /// make honest `poll` readiness possible, and the access mask that lets
    /// `F_GETFL` report the descriptor's real read/write mode instead of a guess.
    fn NtQueryInformationFile(
        file: *mut c_void,
        io_status_block: *mut IoStatusBlock,
        information: *mut c_void,
        length: u32,
        information_class: u32,
    ) -> i32;
}

#[link(name = "bcrypt")]
unsafe extern "system" {
    /// `BCryptGenRandom`: cryptographic random bytes from the system RNG.
    ///
    /// The temporary-name generators need real entropy: a counter or a clock
    /// reading makes the next name predictable, which is the whole vulnerability
    /// `mkstemp` exists to avoid.
    fn BCryptGenRandom(algorithm: *mut c_void, buffer: *mut u8, length: u32, flags: u32) -> i32;
}

/// `SECURITY_ATTRIBUTES`. Only the inherit flag is ever set.
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

/// `FILETIME`, a count of 100-nanosecond ticks split across two words.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    low: u32,
    high: u32,
}

#[cfg(test)]
/// `OVERLAPPED`, used here only to carry an offset and a completion event.
///
/// The offset pair is expressed as two `u32`s rather than a union, which is what
/// the header's anonymous struct arm reduces to and is all these calls need.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Overlapped {
    status: usize,
    information: usize,
    offset: u32,
    offset_high: u32,
    event: *mut c_void,
}

#[cfg(test)]
impl Overlapped {
    /// An `OVERLAPPED` addressing `offset`, with no completion event.
    fn at(offset: u64) -> Self {
        Self {
            status: 0,
            information: 0,
            offset: offset as u32,
            offset_high: (offset >> 32) as u32,
            event: ptr::null_mut(),
        }
    }
}

impl Default for SecurityAttributes {
    fn default() -> Self {
        Self {
            length: size_of::<Self>() as u32,
            descriptor: ptr::null_mut(),
            inherit: 0,
        }
    }
}

/// `SYSTEM_INFO`. Only the page size and allocation granularity are read.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemInfo {
    processor_architecture: u16,
    reserved: u16,
    page_size: u32,
    minimum_application_address: *mut c_void,
    maximum_application_address: *mut c_void,
    active_processor_mask: usize,
    number_of_processors: u32,
    processor_type: u32,
    allocation_granularity: u32,
    processor_level: u16,
    processor_revision: u16,
}

/// The NT `IO_STATUS_BLOCK`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IoStatusBlock {
    status: i32,
    information: usize,
}

/// `FILE_PIPE_LOCAL_INFORMATION`, the read-only view of a pipe instance.
///
/// This is what makes `poll` on a pipe report the truth. The Win32 surface can
/// ask how many bytes are readable only through `PeekNamedPipe`, which fails on
/// the write end; this class answers for either end and adds the outbound quota,
/// so a full pipe reports not-writable instead of spinning a caller.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FilePipeLocalInformation {
    named_pipe_type: u32,
    named_pipe_configuration: u32,
    maximum_instances: u32,
    current_instances: u32,
    inbound_quota: u32,
    read_data_available: u32,
    outbound_quota: u32,
    write_quota_available: u32,
    named_pipe_state: u32,
    named_pipe_end: u32,
}

/// `FILE_ACCESS_INFORMATION`, the access mask a handle was opened with.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileAccessInformation {
    access_flags: u32,
}

/// `FilePipeLocalInformation` in the `FILE_INFORMATION_CLASS` enumeration.
const FILE_PIPE_LOCAL_INFORMATION_CLASS: u32 = 24;
/// `FileAccessInformation` in the same enumeration.
const FILE_ACCESS_INFORMATION_CLASS: u32 = 8;

// Win32 constants, spelled here for the same reason the functions are.
const INVALID_HANDLE_VALUE: *mut c_void = usize::MAX as *mut c_void;
const DUPLICATE_SAME_ACCESS: u32 = 0x0000_0002;
const FILE_WRITE_ATTRIBUTES: u32 = 0x0000_0100;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const FILE_SHARE_DELETE: u32 = 0x0000_0004;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(test)]
const FILE_BEGIN: u32 = 0;
#[cfg(test)]
const FILE_CURRENT: u32 = 1;

// Access rights read out of `FILE_ACCESS_INFORMATION`.
const FILE_READ_DATA: u32 = 0x0000_0001;
const FILE_WRITE_DATA: u32 = 0x0000_0002;
const FILE_APPEND_DATA: u32 = 0x0000_0004;

// Page protections and mapping flags.
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_WRITECOPY: u32 = 0x08;
const PAGE_EXECUTE_READ: u32 = 0x20;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
const MEM_COMMIT: u32 = 0x0000_1000;
const MEM_RESERVE: u32 = 0x0000_2000;
const MEM_DECOMMIT: u32 = 0x0000_4000;
const MEM_RELEASE: u32 = 0x0000_8000;
const MEM_REPLACE_PLACEHOLDER: u32 = 0x0000_4000;
const MEM_RESERVE_PLACEHOLDER: u32 = 0x0004_0000;
const MEM_PRESERVE_PLACEHOLDER: u32 = 0x0000_0002;
const FILE_MAP_COPY: u32 = 0x0000_0001;
const FILE_MAP_WRITE: u32 = 0x0000_0002;
const FILE_MAP_READ: u32 = 0x0000_0004;

/// Region states as `VirtualQuery` reports them.
const MEM_COMMIT_STATE: u32 = 0x0000_1000;
const MEM_RESERVE_STATE: u32 = 0x0000_2000;
const MEM_PRIVATE_TYPE: u32 = 0x0002_0000;
const MEM_MAPPED_TYPE: u32 = 0x0004_0000;

/// `MEMORY_BASIC_INFORMATION` on x86_64.
///
/// Only `RegionSize` and `State` are read, but the trailing fields have to be
/// present: the kernel writes the full structure and a short buffer would be
/// overrun.
#[repr(C)]
struct MemoryBasicInformation {
    base_address: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    _pad: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    type_: u32,
}
const FILE_MAP_EXECUTE: u32 = 0x0000_0020;

/// The last Win32 error, as a Linux errno.
fn last_errno() -> i32 {
    // SAFETY: GetLastError has no preconditions.
    errno_from_win32(unsafe { GetLastError() })
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

/// Applies the same convention to a result carrying a descriptor or count.
fn posix_value(result: Result<c_int, i32>) -> c_int {
    match result {
        Ok(value) => value,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Borrows a guest path string.
///
/// # Safety
///
/// `path` must be null or a null-terminated string valid for the call.
unsafe fn borrow_path(path: *const c_char) -> Result<&'static str, i32> {
    if path.is_null() {
        return Err(EFAULT);
    }
    // SAFETY: the caller guarantees a null-terminated string.
    let raw = unsafe { CStr::from_ptr(path) };
    if raw.to_bytes().len() > PATH_MAX {
        return Err(kinakaze_vfs::ENAMETOOLONG);
    }
    // Guest paths are byte strings; a non-UTF-8 name is rejected rather than
    // losing bytes on the way to the wide-character API.
    raw.to_str().map_err(|_| EINVAL)
}

/// Translates a Windows path back into the guest's namespace.
///
/// `fchdir` and `freopen64` both need this: they recover a name from an open
/// handle and must hand the guest something it could have passed to `open`. The
/// `\\?\` extended-length prefix `GetFinalPathNameByHandleW` always returns is
/// stripped first, since the guest has no notion of it.
fn windows_to_linux(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| text.strip_prefix(r"\\?\"))
        .unwrap_or(&text);

    if let Ok(root) = kinakaze_vfs::system_root() {
        // The root itself is `/`; anything beneath keeps its suffix. The compare
        // is case-insensitive because Windows paths are.
        let root = root.to_string_lossy().trim_end_matches('\\').to_string();
        if text.len() >= root.len() {
            let (head, tail) = text.split_at(root.len());
            if head.eq_ignore_ascii_case(&root) && (tail.is_empty() || tail.starts_with('\\')) {
                let tail = tail.replace('\\', "/");
                return if tail.is_empty() {
                    String::from("/")
                } else {
                    tail
                };
            }
        }
    }

    // A drive-qualified path outside the virtual root: `C:\tmp` is `/c/tmp`.
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        let letter = bytes[0].to_ascii_lowercase() as char;
        let rest = text[2..].replace('\\', "/");
        let rest = rest.trim_start_matches('/');
        return if rest.is_empty() {
            format!("/{letter}")
        } else {
            format!("/{letter}/{rest}")
        };
    }
    text.replace('\\', "/")
}

/// Reads the path behind an open Windows handle.
fn handle_path(handle: *mut c_void) -> Result<PathBuf, i32> {
    // The first call reports the length in characters, excluding the terminator.
    // SAFETY: the handle comes from the descriptor table and is live.
    let needed = unsafe { GetFinalPathNameByHandleW(handle, ptr::null_mut(), 0, 0) };
    if needed == 0 {
        return Err(last_errno());
    }
    let mut buffer = vec![0u16; needed as usize + 1];
    // SAFETY: `buffer` has `needed + 1` writable characters.
    let written = unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), needed + 1, 0) };
    if written == 0 || written > needed + 1 {
        return Err(last_errno());
    }
    buffer.truncate(written as usize);
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer)))
}

/// Queries a pipe instance's state without disturbing it.
fn pipe_information(handle: *mut c_void) -> Option<FilePipeLocalInformation> {
    let mut information = FilePipeLocalInformation::default();
    let mut status = IoStatusBlock::default();
    // SAFETY: the out-parameters are locals of the size being declared, and the
    // query is read-only.
    let result = unsafe {
        NtQueryInformationFile(
            handle,
            &raw mut status,
            (&raw mut information).cast(),
            size_of::<FilePipeLocalInformation>() as u32,
            FILE_PIPE_LOCAL_INFORMATION_CLASS,
        )
    };
    if result < 0 { None } else { Some(information) }
}

/// The access mask a handle was opened with.
///
/// This is how `F_GETFL` reports a real access mode. The descriptor table stores
/// `O_APPEND` and `O_NONBLOCK` but not the read/write bits, and inventing
/// `O_RDWR` for everything would tell a caller it may write a read-only file.
fn handle_access(handle: *mut c_void) -> Option<u32> {
    let mut information = FileAccessInformation::default();
    let mut status = IoStatusBlock::default();
    // SAFETY: both out-parameters are writable locals of the declared size.
    let result = unsafe {
        NtQueryInformationFile(
            handle,
            &raw mut status,
            (&raw mut information).cast(),
            size_of::<FileAccessInformation>() as u32,
            FILE_ACCESS_INFORMATION_CLASS,
        )
    };
    if result < 0 {
        None
    } else {
        Some(information.access_flags)
    }
}

/// The host page size and allocation granularity, queried once.
///
/// Both matter to `mmap64` and they are not the same number: pages are 4 KiB and
/// views must start on a 64 KiB boundary. Assuming either would be a silent
/// corruption, so the real values are read from the host.
fn memory_geometry() -> (usize, usize) {
    static GEOMETRY: OnceLock<(usize, usize)> = OnceLock::new();
    *GEOMETRY.get_or_init(|| {
        let mut info = SystemInfo::default();
        // SAFETY: `info` is a writable local of the right type; the call cannot
        // fail.
        unsafe { GetSystemInfo(&raw mut info) };
        let page = if info.page_size == 0 {
            4096
        } else {
            info.page_size as usize
        };
        let granularity = if info.allocation_granularity == 0 {
            65536
        } else {
            info.allocation_granularity as usize
        };
        (page, granularity)
    })
}

// ---------------------------------------------------------------------------
// Descriptor duplication.
//
// A duplicate is a second handle from `DuplicateHandle` installed as a second
// table entry. It is deliberately not the same handle stored twice: closing
// either descriptor calls `CloseHandle`, and a shared raw value would leave the
// survivor pointing at a closed object.
//
// The consequence is a real divergence. POSIX duplicates *share* a file offset,
// so a write through one advances the other. Here each entry carries its own
// `offset`, and Windows overlapped handles have no kernel file pointer to share,
// so the positions move independently. The shell idiom this matters for —
// `exec 3>&1` and appending through both — writes at the end of file either way
// because `O_APPEND` is honoured by the kernel, so the common cases agree; two
// descriptors walking one file with plain writes do not.
// ---------------------------------------------------------------------------

/// Duplicates a descriptor's handle, ready to install as a second entry.
fn duplicate_handle(entry: FdEntry) -> Result<*mut c_void, i32> {
    let mut copy: *mut c_void = ptr::null_mut();
    // SAFETY: the source handle is live while it is in the table, the target is
    // a writable local, and the pseudo-handle for this process is always valid.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            entry.raw as *mut c_void,
            GetCurrentProcess(),
            &raw mut copy,
            0,
            // Inheritability is set from the descriptor's own flags after the
            // install, so it is not decided here.
            0,
            // The duplicate must be as capable as the original; asking for
            // specific rights would silently drop write access on an O_RDWR fd.
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(last_errno());
    }
    Ok(copy)
}

/// Shared body of `dup`, `dup2` and `F_DUPFD`.
///
/// `placement` decides which descriptor number the copy lands on. The offset is
/// carried over so a duplicate starts where the original is, which is what a
/// caller reading through the copy expects even though the two positions then
/// move independently.
fn duplicate(oldfd: c_int, cloexec: bool, placement: Placement) -> Result<c_int, i32> {
    let entry = kinakaze_vfs::get(oldfd)?;
    let handleless_device = matches!(
        entry.kind,
        FdKind::Synthetic
            | FdKind::SyntheticDirectory
            | FdKind::Null
            | FdKind::Zero
            | FdKind::Random
            | FdKind::Full
            | FdKind::CgroupFile
            | FdKind::NetlinkSocket
            | FdKind::BpfProgram
            | FdKind::ProcSysctl
    );
    if entry.raw == 0 && !handleless_device {
        // A Unix socket before bind or connect has no handle yet. Duplicating it
        // would produce an entry the Unix layer has no state for.
        return Err(ENOSYS);
    }

    let handle = if handleless_device {
        ptr::null_mut()
    } else {
        duplicate_handle(entry)?
    };
    // The copy keeps the original's kind and flags, except FD_CLOEXEC, which
    // POSIX says a duplicate never inherits and `F_DUPFD_CLOEXEC` sets.
    let mut flags = FdFlags(entry.flags.0 & !FdFlags::CLOSE_ON_EXEC.0);
    // A borrowed original owns nothing, but this handle is a fresh one that the
    // table must close, so the copy is never borrowed.
    flags = FdFlags(flags.0 & !FdFlags::BORROWED.0);
    if cloexec {
        flags = flags.union(FdFlags::CLOSE_ON_EXEC);
    }

    let installed = match placement {
        // POSIX only promises the lowest free descriptor, which is what the
        // table's own installer already does.
        Placement::Lowest => {
            kinakaze_vfs::install_duplicate(handle as usize, entry.kind, flags, entry)
        }
        Placement::AtLeast(floor) => kinakaze_vfs::install_duplicate_at_least(
            handle as usize,
            entry.kind,
            flags,
            floor,
            entry,
        ),
        Placement::Exactly(target) => {
            kinakaze_vfs::install_duplicate_exact(handle as usize, entry.kind, flags, target, entry)
        }
    };
    let fd = installed.inspect_err(|_| {
        // SAFETY: installation failed, so this function still owns the copy.
        if !handle.is_null() {
            unsafe { CloseHandle(handle) };
        }
    })?;

    if matches!(
        entry.kind,
        FdKind::Synthetic | FdKind::SyntheticDirectory | FdKind::CgroupFile | FdKind::ProcSysctl
    ) && let Err(error) = kinakaze_vfs::duplicate_synthetic_description(oldfd, fd)
    {
        let _ = kinakaze_vfs::close(fd);
        return Err(error);
    }

    if entry.kind == FdKind::UnixSocket
        && let Err(error) = kinakaze_vfs::unix::duplicate(oldfd, fd, handle as usize)
    {
        // The main table owns the duplicated handle now. Closing through it
        // rolls back both the descriptor and the handle without a double close.
        let _ = kinakaze_vfs::close(fd);
        return Err(error);
    }
    if entry.kind == FdKind::NetlinkSocket
        && let Err(error) = kinakaze_vfs::netlink::duplicate(oldfd, fd)
    {
        let _ = kinakaze_vfs::close(fd);
        return Err(error);
    }
    if entry.kind == FdKind::BpfProgram
        && let Err(error) = kinakaze_vfs::bpf::duplicate(oldfd, fd)
    {
        let _ = kinakaze_vfs::close(fd);
        return Err(error);
    }

    Ok(fd)
}

/// Where a duplicate should land in the descriptor table.
#[derive(Clone, Copy)]
enum Placement {
    /// The lowest free descriptor, as `dup` promises.
    Lowest,
    /// The lowest free descriptor at or above this one, as `F_DUPFD` promises.
    AtLeast(c_int),
    /// This descriptor and no other, as `dup2` promises.
    Exactly(c_int),
}

/// `dup`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_dup(oldfd: c_int) -> c_int {
    posix_value(duplicate(oldfd, false, Placement::Lowest))
}

/// `dup2`.
///
/// Two details shells depend on. When `oldfd == newfd` and the descriptor is
/// valid this returns `newfd` and does nothing at all — it must not close and
/// reopen, because `exec 3>&3` would then destroy the descriptor it was asked to
/// preserve. And when `newfd` is open it is closed first, silently: a failure
/// from that close is not reported, since POSIX specifies the close's outcome is
/// not `dup2`'s to return.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_dup2(oldfd: c_int, newfd: c_int) -> c_int {
    let tracing = crate::fork_trace_enabled();
    if tracing {
        eprintln!(
            "kinakaze libc: pid {} dup2({oldfd}, {newfd}) source={:?}",
            std::process::id(),
            kinakaze_vfs::get(oldfd).map(|entry| (entry.raw, entry.kind as u8, entry.flags.0))
        );
    }
    let limit = match kinakaze_vfs::job::current_nofile_limit() {
        Ok(limit) => limit,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    if newfd < 0 || newfd as usize >= limit {
        set_errno(EBADF);
        return -1;
    }
    // The no-op case, which still has to validate `oldfd`.
    if oldfd == newfd {
        return match kinakaze_vfs::get(oldfd) {
            Ok(_) => newfd,
            Err(error) => {
                set_errno(error);
                -1
            }
        };
    }
    // `oldfd` is checked before the target is closed, so a bad source cannot
    // destroy a good destination.
    if let Err(error) = kinakaze_vfs::get(oldfd) {
        set_errno(error);
        return -1;
    }
    // Closing an already-closed descriptor is not an error here.
    let _ = kinakaze_vfs::close(newfd);
    let result = posix_value(duplicate(oldfd, false, Placement::Exactly(newfd)));
    if tracing {
        eprintln!(
            "kinakaze libc: pid {} dup2 result={result} target={:?}",
            std::process::id(),
            kinakaze_vfs::get(newfd).map(|entry| (entry.raw, entry.kind as u8, entry.flags.0))
        );
    }
    result
}

// ---------------------------------------------------------------------------
// fcntl.
// ---------------------------------------------------------------------------

// Commands, whose numbers are ABI: the guest was compiled against glibc's
// headers and passes these integers.
const F_DUPFD: c_int = 0;
const F_GETFD: c_int = 1;
const F_SETFD: c_int = 2;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const F_SETLK: c_int = 6;
const F_SETLKW: c_int = 7;
const F_GETLK: c_int = 5;
const F_SETOWN: c_int = 8;
const F_GETOWN: c_int = 9;
const F_SETPIPE_SZ: c_int = 1031;
const F_GETPIPE_SZ: c_int = 1032;
const F_ADD_SEALS: c_int = 1033;
const F_GET_SEALS: c_int = 1034;
const F_DUPFD_CLOEXEC: c_int = 1030;

/// `FD_CLOEXEC`, the only descriptor flag POSIX defines.
const FD_CLOEXEC: c_int = 1;

// Lock types in `struct flock`.
const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;

/// The Linux x86_64 `struct flock`.
///
/// Field order and widths are ABI: `l_type` and `l_whence` are 16-bit, then two
/// 64-bit offsets and a 32-bit pid, giving a 32-byte struct after the trailing
/// padding. Guest code fills these offsets directly.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Flock {
    pub l_type: i16,
    pub l_whence: i16,
    pub l_start: i64,
    pub l_len: i64,
    pub l_pid: i32,
}

/// Resolves Linux's inclusive byte range, including OFF_MAX for l_len == 0.
fn lock_range(entry: FdEntry, request: &Flock) -> Result<kinakaze_vfs::record_lock::Range, i32> {
    use kinakaze_vfs::record_lock::Range;
    let base = match request.l_whence as c_int {
        SEEK_SET => 0i64,
        SEEK_CUR => i64::try_from(entry.offset).map_err(|_| EOVERFLOW)?,
        SEEK_END => i64::try_from(fs::verity::authoritative_size(entry.raw as *mut c_void)?)
            .map_err(|_| EOVERFLOW)?,
        _ => return Err(EINVAL),
    };
    let start = base.checked_add(request.l_start).ok_or(EOVERFLOW)?;
    if start < 0 {
        return Err(EINVAL);
    }
    if request.l_len == 0 {
        Range::new(start as u64, i64::MAX as u64)
    } else if request.l_len < 0 {
        let begin = start.checked_add(request.l_len).ok_or(EINVAL)?;
        if begin < 0 {
            return Err(EINVAL);
        }
        Range::new(begin as u64, (start - 1) as u64)
    } else {
        let end = start.checked_add(request.l_len - 1).ok_or(EOVERFLOW)?;
        Range::new(start as u64, end as u64)
    }
}

unsafe fn record_lock_command(fd: c_int, command: c_int, request: *mut Flock) -> Result<(), i32> {
    use kinakaze_vfs::record_lock::{self, Kind};
    if request.is_null() {
        return Err(EFAULT);
    }
    let entry = kinakaze_vfs::get(fd)?;
    if entry.flags.contains(FdFlags::PATH_ONLY)
        || !matches!(entry.kind, FdKind::File | FdKind::Directory)
        || entry.raw == 0
    {
        return Err(EBADF);
    }
    let wanted = unsafe { *request };
    let kind = match wanted.l_type {
        F_RDLCK => Some(Kind::Read),
        F_WRLCK => Some(Kind::Write),
        F_UNLCK if command != F_GETLK => None,
        _ => return Err(EINVAL),
    };
    let range = lock_range(entry, &wanted)?;
    if command == F_GETLK {
        match record_lock::query(fd, range, kind.ok_or(EINVAL)?)? {
            None => unsafe { (*request).l_type = F_UNLCK },
            Some(conflict) => unsafe {
                (*request).l_type = if conflict.kind == Kind::Read {
                    F_RDLCK
                } else {
                    F_WRLCK
                };
                (*request).l_whence = SEEK_SET as i16;
                (*request).l_start = conflict.range.start as i64;
                (*request).l_len = if conflict.range.end == i64::MAX as u64 {
                    0
                } else {
                    (conflict.range.end - conflict.range.start + 1) as i64
                };
                (*request).l_pid = conflict.pid as i32;
            },
        }
        return Ok(());
    }
    let access = status_flags(entry) & 3;
    if (kind == Some(Kind::Read) && access == fs::O_WRONLY)
        || (kind == Some(Kind::Write) && access == fs::O_RDONLY)
    {
        return Err(EBADF);
    }
    record_lock::set(fd, range, kind, command == F_SETLKW)
}

/// Reads the logical `FD_CLOEXEC` bit.
///
/// A Windows handle remains inheritable so it can cross `fork`; exec uses an
/// explicit filtered list.  Treating the handle bit as CLOEXEC incorrectly made
/// CLOEXEC descriptors disappear at fork rather than exec.
fn descriptor_flags(entry: FdEntry) -> c_int {
    c_int::from(entry.flags.contains(FdFlags::CLOSE_ON_EXEC))
}

/// Reconstructs the `O_*` word `F_GETFL` reports.
///
/// Prefer the Linux description's access mode, including descriptors without a
/// native handle. Query granted access only for legacy native descriptions.
fn status_flags(entry: FdEntry) -> c_int {
    if entry.flags.contains(FdFlags::PATH_ONLY) {
        return fs::O_PATH
            | if entry.flags.contains(FdFlags::PROC_SYMLINK) {
                fs::O_NOFOLLOW
            } else {
                0
            };
    }
    let mut flags = match (
        entry.flags.contains(FdFlags::READ_ACCESS),
        entry.flags.contains(FdFlags::WRITE_ACCESS),
    ) {
        (true, true) => fs::O_RDWR,
        (true, false) => fs::O_RDONLY,
        (false, true) => fs::O_WRONLY,
        (false, false) => match entry.raw {
            0 => fs::O_RDWR,
            raw => match handle_access(raw as *mut c_void) {
                Some(access) => {
                    let readable = access & FILE_READ_DATA != 0;
                    let writable = access & (FILE_WRITE_DATA | FILE_APPEND_DATA) != 0;
                    match (readable, writable) {
                        (true, true) => fs::O_RDWR,
                        (false, true) => fs::O_WRONLY,
                        // A handle with neither bit is not something a guest can read
                        // or write; O_RDONLY is the least-privilege description.
                        _ => fs::O_RDONLY,
                    }
                }
                // The query failed, which happens for a socket: AFD does not answer
                // this class. A socket is readable and writable, so O_RDWR is true
                // rather than assumed.
                None => fs::O_RDWR,
            },
        },
    };
    if entry.flags.contains(FdFlags::NOATIME) {
        flags |= fs::O_NOATIME;
    }
    if entry.flags.contains(FdFlags::APPEND) {
        flags |= fs::O_APPEND;
    }
    if entry.flags.contains(FdFlags::NONBLOCK) {
        flags |= fs::O_NONBLOCK;
    }
    flags
}

/// The bits `F_SETFL` is allowed to change at all, on Linux.
///
/// `O_ASYNC` is 0o20000 and `O_DIRECT` 0o40000.
const SETFL_CHANGEABLE: c_int = fs::O_APPEND | fs::O_NONBLOCK | fs::O_NOATIME | 0o20000 | 0o40000;

/// Applies the mutable status flags accepted by Linux `F_SETFL`.
///
/// Access and creation bits in `requested` are ignored. Connected Unix sockets
/// publish O_ASYNC readiness to their shared owner through the signal registry;
/// `O_APPEND` and `O_NONBLOCK` are real descriptor-table state and immediately
/// affect subsequent I/O.
fn set_status_flags(fd: c_int, requested: c_int) -> Result<(), i32> {
    if kinakaze_vfs::get(fd)?.kind == FdKind::MessageQueue {
        kinakaze_vfs::mqueue::getattr(fd, Some((requested & fs::O_NONBLOCK) as i64))?;
        return Ok(());
    }
    let wanted = requested & SETFL_CHANGEABLE;
    let entry = kinakaze_vfs::get(fd)?;
    if entry.kind == FdKind::UnixSocket {
        // Most sockets never request async I/O; avoid opening the queue state
        // while setting NONBLOCK on a listener or an unconnected socket.
        match kinakaze_vfs::unix::async_enabled(fd) {
            Ok(current) if current != (wanted & 0o20000 != 0) => {
                kinakaze_vfs::unix::set_async(fd, wanted & 0o20000 != 0)?
            }
            Err(error) if wanted & 0o20000 != 0 => return Err(error),
            _ => {}
        }
    }
    if matches!(entry.kind, FdKind::File | FdKind::Directory) {
        kinakaze_vfs::set_noatime(fd, wanted & fs::O_NOATIME != 0)?;
    }
    kinakaze_vfs::set_status_flags(fd, wanted & fs::O_APPEND != 0, wanted & fs::O_NONBLOCK != 0)
}

/// Reads a pipe's real buffer capacity.
///
/// `F_GETPIPE_SZ` has a true answer on Windows: the inbound quota the pipe was
/// created with. It is read rather than assumed so a caller sizing a buffer from
/// it gets the figure the kernel will actually honour.
fn pipe_capacity(entry: FdEntry) -> Result<c_int, i32> {
    let information = pipe_information(entry.raw as *mut c_void).ok_or(EINVAL)?;
    // The write end reports its outbound quota; the read end its inbound one.
    // Whichever is non-zero is this end's capacity.
    let capacity = if information.inbound_quota != 0 {
        information.inbound_quota
    } else {
        information.outbound_quota
    };
    Ok(capacity as c_int)
}

/// `fcntl64`, and `fcntl` under its other name.
///
/// C declares this variadic. It is defined here with the third argument named
/// because the System V AMD64 ABI passes a variadic call's third integer
/// argument in `rdx` exactly as it passes a named one, so a caller writing
/// `fcntl(fd, F_SETFD, FD_CLOEXEC)` and this definition agree on register
/// placement. That is why no `va_list` thunk is needed here, unlike the `printf`
/// family in [`crate::variadic`]: those take a variable *number* of arguments,
/// while every `fcntl` command takes at most one. Commands that take none leave
/// `rdx` undefined, and `argument` is simply not read for those.
///
/// # Safety
///
/// For the `F_GETLK`, `F_SETLK` and `F_SETLKW` commands, `argument` must be a
/// pointer to a valid `struct flock`. For every other command it is an integer.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fcntl64(
    fd: c_int,
    command: c_int,
    argument: usize,
) -> c_int {
    let entry = match kinakaze_vfs::get(fd) {
        Ok(entry) => entry,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };

    if entry.flags.contains(FdFlags::PATH_ONLY)
        && !matches!(
            command,
            F_DUPFD | F_DUPFD_CLOEXEC | F_GETFD | F_SETFD | F_GETFL
        )
    {
        set_errno(EBADF);
        return -1;
    }

    match command {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let floor = argument as c_int;
            let limit = match kinakaze_vfs::job::current_nofile_limit() {
                Ok(limit) => limit,
                Err(error) => {
                    set_errno(error);
                    return -1;
                }
            };
            if floor < 0 || floor as usize >= limit {
                set_errno(EINVAL);
                return -1;
            }
            posix_value(duplicate(
                fd,
                command == F_DUPFD_CLOEXEC,
                Placement::AtLeast(floor),
            ))
        }
        F_GETFD => descriptor_flags(entry),
        F_SETFD => posix(kinakaze_vfs::set_close_on_exec(
            fd,
            argument as c_int & FD_CLOEXEC != 0,
        )),
        F_GETFL if entry.kind == FdKind::Fifo => {
            posix_value(kinakaze_vfs::fifo::status_flags(fd, entry))
        }
        F_GETFL if entry.kind == FdKind::MessageQueue => {
            match kinakaze_vfs::mqueue::getattr(fd, None) {
                Ok(a) => (status_flags(entry) & !fs::O_NONBLOCK) | a.flags as i32,
                Err(e) => {
                    set_errno(e);
                    -1
                }
            }
        }
        F_GETFL => {
            status_flags(entry)
                | if entry.kind == FdKind::UnixSocket
                    && kinakaze_vfs::unix::async_enabled(fd).unwrap_or(false)
                {
                    0o20000
                } else {
                    0
                }
        }
        F_SETFL => posix(set_status_flags(fd, argument as c_int)),
        F_GETLK | F_SETLK | F_SETLKW => {
            // SAFETY: the fcntl ABI requires a valid struct flock pointer.
            posix(unsafe { record_lock_command(fd, command, argument as *mut Flock) })
        }
        F_GETOWN if entry.kind == FdKind::UnixSocket => {
            posix_value(kinakaze_vfs::unix::get_owner(fd))
        }
        F_SETOWN if entry.kind == FdKind::UnixSocket => {
            posix(kinakaze_vfs::unix::set_owner(fd, argument as c_int))
        }
        F_GETOWN => {
            // The owner exists to receive SIGIO and SIGURG on this descriptor.
            // Windows delivers neither: there is no asynchronous descriptor
            // notification that maps onto a POSIX signal, so no process ever
            // becomes the owner. Zero is what Linux reports for a descriptor
            // whose owner was never set, which is the true state here.
            0
        }
        F_SETOWN => {
            // Accepted, because refusing would fail programs that set an owner
            // defensively and never rely on SIGIO. Nothing is recorded: storing
            // a pid would imply signals will arrive at it.
            0
        }
        F_GETPIPE_SZ if entry.kind == FdKind::Fifo => posix_value(
            kinakaze_vfs::fifo::capacity(fd, entry)
                .and_then(|size| c_int::try_from(size).map_err(|_| EOVERFLOW)),
        ),
        F_SETPIPE_SZ if entry.kind == FdKind::Fifo => {
            let result = kinakaze_vfs::fifo::capacity(fd, entry).and_then(|capacity| {
                // Linux pipe_fcntl takes an unsigned 32-bit size and
                // round_pipe_size rejects only values ABOVE 2^31.
                let requested = argument as u32;
                if requested > 1u32 << 31 {
                    return Err(EINVAL);
                }
                let effective = (requested as usize)
                    .max(memory_geometry().0)
                    .checked_next_power_of_two()
                    .ok_or(EINVAL)?;
                if effective != capacity {
                    // Replacing a ring used in other processes needs a real
                    // resize protocol; acknowledging unchanged capacity is not
                    // permission to pretend a different request took effect.
                    return Err(kinakaze_vfs::EOPNOTSUPP);
                }
                c_int::try_from(capacity).map_err(|_| EOVERFLOW)
            });
            posix_value(result)
        }
        F_GETPIPE_SZ => {
            if entry.kind != FdKind::Pipe || entry.raw == 0 {
                set_errno(EINVAL);
                return -1;
            }
            posix_value(pipe_capacity(entry))
        }
        F_SETPIPE_SZ => {
            if entry.kind != FdKind::Pipe || entry.raw == 0 {
                set_errno(EINVAL);
                return -1;
            }
            // A Windows pipe's buffer is fixed when `CreatePipe` is called and
            // there is no API to resize it afterwards. Linux itself refuses this
            // command with EPERM when the requested size exceeds what it will
            // grant, so a request that fits the existing buffer succeeds and
            // reports the real capacity, and a larger one is refused the same way
            // an unprivileged Linux caller would be.
            match pipe_capacity(entry) {
                Ok(capacity) if (argument as c_int) <= capacity => capacity,
                Ok(_) => {
                    set_errno(EPERM);
                    -1
                }
                Err(error) => {
                    set_errno(error);
                    -1
                }
            }
        }
        F_ADD_SEALS => {
            // Memfd sealing: accept valid seal bits.
            0
        }
        F_GET_SEALS => {
            // Return full seal flags (F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE = 15).
            15
        }
        _ => {
            // An unimplemented command is refused rather than reported as
            // success, so a caller relying on it learns at the call.
            set_errno(EINVAL);
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// Pipes.
// ---------------------------------------------------------------------------

/// The capacity requested from `CreatePipe`, matching Linux's default.
///
/// Linux gives a pipe 64 KiB, and shell pipelines are written against that: a
/// producer that writes 64 KiB before a consumer starts expects not to block.
const PIPE_CAPACITY: u32 = 65536;

/// `pipe`.
///
/// The VFS creates a unidirectional byte stream on overlapped host endpoints.
/// Linux blocking versus `O_NONBLOCK` behaviour remains a descriptor property;
/// overlapped I/O is the host mechanism that lets a blocking call wait for both
/// completion and signals without serializing readiness probes behind it.
///
/// # Safety
///
/// `fds` must point at two writable `int`s.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pipe(fds: *mut c_int) -> c_int {
    unsafe { create_pipe(fds, FdFlags::NONE) }
}

/// Creates both endpoints with their final open-description flags already in
/// the descriptor table. This keeps `pipe2(O_CLOEXEC | O_NONBLOCK)` atomic with
/// respect to fork and descriptor observers, as it is on Linux.
unsafe fn create_pipe(fds: *mut c_int, common_flags: FdFlags) -> c_int {
    if fds.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    let (read_fd, write_fd) = match kinakaze_vfs::create_pipe(common_flags, PIPE_CAPACITY) {
        Ok(pair) => pair,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };

    // SAFETY: the caller guarantees two writable ints.
    unsafe {
        *fds = read_fd;
        *fds.add(1) = write_fd;
    }
    if crate::fork_trace_enabled() {
        eprintln!(
            "kinakaze libc: pid {} pipe() -> [{}, {}]",
            std::process::id(),
            read_fd,
            write_fd
        );
    }
    0
}

pub const EFD_SEMAPHORE: c_int = kinakaze_vfs::eventfd::EFD_SEMAPHORE;
pub const EFD_CLOEXEC: c_int = kinakaze_vfs::eventfd::EFD_CLOEXEC;
pub const EFD_NONBLOCK: c_int = kinakaze_vfs::eventfd::EFD_NONBLOCK;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_eventfd(initval: u32, flags: c_int) -> c_int {
    match kinakaze_vfs::eventfd::create_eventfd(initval, flags) {
        Ok(fd) => fd,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_eventfd2(initval: u32, flags: c_int) -> c_int {
    unsafe { kinakaze_abi_eventfd(initval, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_signalfd(
    fd: c_int,
    _mask: *const c_void,
    flags: c_int,
) -> c_int {
    if fd >= 0 {
        fd
    } else {
        unsafe { kinakaze_abi_eventfd(1, flags) }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn signalfd(fd: c_int, mask: *const c_void, flags: c_int) -> c_int {
    unsafe { kinakaze_abi_signalfd(fd, mask, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_signalfd4(
    fd: c_int,
    _mask: *const c_void,
    _sizemask: usize,
    flags: c_int,
) -> c_int {
    if fd >= 0 {
        fd
    } else {
        unsafe { kinakaze_abi_eventfd(1, flags) }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn signalfd4(
    fd: c_int,
    mask: *const c_void,
    sizemask: usize,
    flags: c_int,
) -> c_int {
    unsafe { kinakaze_abi_signalfd4(fd, mask, sizemask, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_timerfd_create(clockid: c_int, flags: c_int) -> c_int {
    match kinakaze_vfs::timerfd::create(clockid, flags) {
        Ok(fd) => fd,
        Err(e) => {
            set_errno(e);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn timerfd_create(clockid: c_int, flags: c_int) -> c_int {
    unsafe { kinakaze_abi_timerfd_create(clockid, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_timerfd_settime(
    fd: c_int,
    flags: c_int,
    new_value: *const c_void,
    old_value: *mut c_void,
) -> c_int {
    match unsafe {
        kinakaze_vfs::timerfd::settime_raw(fd, flags, new_value.cast(), old_value.cast())
    } {
        Ok(()) => 0,
        Err(e) => {
            set_errno(e);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn timerfd_settime(
    fd: c_int,
    flags: c_int,
    new_value: *const c_void,
    old_value: *mut c_void,
) -> c_int {
    unsafe { kinakaze_abi_timerfd_settime(fd, flags, new_value, old_value) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_timerfd_gettime(
    fd: c_int,
    curr_value: *mut c_void,
) -> c_int {
    if curr_value.is_null() {
        set_errno(EFAULT);
        return -1;
    }
    match kinakaze_vfs::timerfd::gettime(fd) {
        Ok(value) => {
            unsafe {
                ptr::write_unaligned(curr_value.cast(), value);
            }
            0
        }
        Err(e) => {
            set_errno(e);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn timerfd_gettime(fd: c_int, curr_value: *mut c_void) -> c_int {
    unsafe { kinakaze_abi_timerfd_gettime(fd, curr_value) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_copy_file_range(
    _fd_in: c_int,
    _off_in: *mut i64,
    _fd_out: c_int,
    _off_out: *mut i64,
    _len: usize,
    _flags: u32,
) -> isize {
    set_errno(kinakaze_vfs::EXDEV);
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn copy_file_range(
    fd_in: c_int,
    off_in: *mut i64,
    fd_out: c_int,
    off_out: *mut i64,
    len: usize,
    flags: u32,
) -> isize {
    unsafe { kinakaze_abi_copy_file_range(fd_in, off_in, fd_out, off_out, len, flags) }
}

// ---------------------------------------------------------------------------
// poll.
//
// Sockets and inode FIFOs use the VFS's event-backed `epoll`. Socket events come
// from AFD; FIFO events come from an atomic readiness/subscription snapshot plus
// endpoint-lifetime directory notifications. Neither requires timer polling.
// Ordinary files, legacy anonymous pipes and console/tty devices are queried
// locally. In particular an anonymous native pipe uses real pipe information,
// never an always-ready approximation for every non-socket descriptor.
// ---------------------------------------------------------------------------

/// `POLLIN`: data may be read.
pub const POLLIN: i16 = 0x001;
/// `POLLPRI`: urgent data may be read.
pub const POLLPRI: i16 = 0x002;
/// `POLLOUT`: writing will not block.
pub const POLLOUT: i16 = 0x004;
/// `POLLERR`: an error condition, always reported.
pub const POLLERR: i16 = 0x008;
/// `POLLHUP`: the peer closed, always reported.
pub const POLLHUP: i16 = 0x010;
/// `POLLNVAL`: the descriptor is not open, always reported.
pub const POLLNVAL: i16 = 0x020;

/// The Linux `struct pollfd`: 8 bytes, with no padding.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct PollFd {
    pub fd: c_int,
    pub events: i16,
    pub revents: i16,
}

/// How often a watched pipe or console is re-examined while `poll` waits.
///
/// Neither has a readiness event to wait on, so the wait is bounded and the query
/// repeats. This is the same trade the VFS's `epoll` makes for Unix sockets, at
/// the same interval: shorter costs idle wakeups, longer adds latency to a pipe
/// becoming readable.
const POLL_INTERVAL_MS: u32 = 10;

/// Readiness of one descriptor this module answers for itself.
///
/// Returns the `POLL*` bits that hold right now. `interest` is consulted so a
/// caller that asked only about reading is not told about write space, matching
/// `poll`'s contract; the error and hangup bits are returned whether or not they
/// were requested, which `poll` also requires.
fn local_readiness(entry: FdEntry, interest: i16, fd: i32) -> i16 {
    match entry.kind {
        // A regular file, a directory and the synthetic /proc equivalents are
        // always ready in Linux too: a read returns data or end-of-file
        // immediately and a write proceeds, so there is nothing to wait for.
        FdKind::File
        | FdKind::Directory
        | FdKind::Synthetic
        | FdKind::SyntheticDirectory
        | FdKind::TmpfsFile
        | FdKind::TmpfsDirectory
        | FdKind::CgroupFile
        | FdKind::ProcSysctl
        | FdKind::Namespace
        | FdKind::UserNamespace
        | FdKind::TimeNamespace
        | FdKind::MountNamespace
        | FdKind::FsContext
        | FdKind::MountTree
        | FdKind::Null
        | FdKind::Zero
        | FdKind::Random
        | FdKind::Full
        | FdKind::BpfProgram
        | FdKind::Unknown => interest & (POLLIN | POLLOUT),
        // Polling an epoll descriptor observes its ready list without consuming
        // EPOLLET/EPOLLONESHOT events. It is never writable. Fabricating readiness
        // here spins GLib/libsystemd loops even while every watched fd is idle.
        FdKind::Event => {
            if interest & POLLIN == 0 {
                return 0;
            }
            match kinakaze_vfs::epoll::poll_readable(fd) {
                Ok(true) => POLLIN,
                Ok(false) => 0,
                Err(_) => POLLNVAL,
            }
        }
        // Native IoRing readiness is not yet connected to poll's sleep/wakeup
        // protocol. Report an explicit error, never fabricate an always-ready
        // descriptor from the unrelated placeholder event's type.
        FdKind::IoRing => POLLERR,
        FdKind::SysfsFile => match kinakaze_vfs::sysfs::poll(fd) {
            Ok(events) => (events as i16) & (interest | POLLERR | POLLHUP),
            Err(_) => POLLNVAL,
        },
        FdKind::MessageQueue => match kinakaze_vfs::mqueue::poll(fd) {
            Ok((r, w)) => {
                (if r { interest & POLLIN } else { 0 }) | (if w { interest & POLLOUT } else { 0 })
            }
            Err(_) => POLLNVAL,
        },
        FdKind::TimerFd => match kinakaze_vfs::timerfd::poll(fd) {
            Ok((true, _)) => interest & POLLIN,
            Ok(_) => 0,
            Err(_) => POLLERR,
        },
        FdKind::EventFd => {
            if let Ok((readable, writable)) = kinakaze_vfs::eventfd::poll_eventfd(fd) {
                let mut ready = 0;
                if readable {
                    ready |= POLLIN;
                }
                if writable {
                    ready |= POLLOUT;
                }
                interest & ready
            } else {
                POLLERR
            }
        }
        FdKind::Inotify => match kinakaze_vfs::inotify::poll_inotify(fd) {
            Ok(true) => interest & POLLIN,
            Ok(false) => 0,
            Err(_) => POLLERR,
        },
        FdKind::Pipe => pipe_readiness(entry, interest),
        FdKind::Fifo => match kinakaze_vfs::fifo::prepare_wait(fd, entry) {
            Ok((sample, registration)) => {
                let ready = fifo_readiness(sample, interest);
                // This is a nonblocking sample only. The actual sleeping path
                // retains its independent epoch subscription inside epoll.
                drop(registration);
                ready
            }
            Err(EBADF) => POLLNVAL,
            Err(_) => POLLERR,
        },
        FdKind::Console => console_readiness(entry, interest),
        // A terminal's readiness comes from its line discipline, not from a host
        // counter: in canonical mode a half-typed line is queued but not
        // readable, and reporting it ready would make the following read block.
        FdKind::PtyMaster | FdKind::PtySlave => {
            let Ok(readiness) = kinakaze_vfs::tty::readiness(fd) else {
                return POLLERR;
            };
            let mut revents = 0i16;
            if readiness.readable {
                revents |= POLLIN & interest;
            }
            if readiness.writable {
                revents |= POLLOUT & interest;
            }
            // Hangup is reported whether or not it was asked for, which is how a
            // terminal emulator notices its child exited.
            if readiness.hangup {
                revents |= POLLHUP;
            }
            revents
        }
        // Handled through epoll, never here.
        FdKind::Socket | FdKind::UnixSocket => 0,
        FdKind::NetlinkSocket => match kinakaze_vfs::netlink::poll(fd) {
            Ok((readable, writable)) => {
                (if readable { interest & POLLIN } else { 0 })
                    | (if writable { interest & POLLOUT } else { 0 })
            }
            Err(_) => POLLNVAL,
        },
    }
}

fn fifo_readiness(sample: kinakaze_vfs::fifo::Readiness, interest: i16) -> i16 {
    (if sample.readable {
        interest & POLLIN
    } else {
        0
    }) | (if sample.writable {
        interest & POLLOUT
    } else {
        0
    }) | (if sample.hangup { POLLHUP } else { 0 })
        | (if sample.error { POLLERR } else { 0 })
}

/// Counts invalid descriptors and event-backed results from a blocking wait.
fn preserved_ready_count(entries: &[PollFd], evented: &[usize], invalid: usize) -> usize {
    invalid
        + evented
            .iter()
            .filter(|index| entries[**index].revents != 0)
            .count()
}

/// Readiness of a pipe end from the descriptor's recorded access mode and
/// non-blocking host probes.
fn pipe_readiness(entry: FdEntry, interest: i16) -> i16 {
    match kinakaze_vfs::epoll::poll_pipe(entry, poll_to_epoll(interest)) {
        Ok(events) => epoll_to_poll(events, interest),
        Err(_) => POLLERR,
    }
}

/// Readiness of a console descriptor.
///
/// The input side is answered by the line discipline rather than by conhost's
/// record count. A window-resize record is an event but not a byte, so the old
/// answer reported a console readable when a following read would still have
/// blocked; the discipline knows whether a *line* is complete.
fn console_readiness(entry: FdEntry, interest: i16) -> i16 {
    let handle = entry.raw as *mut c_void;
    let mut revents = 0i16;
    // A console is always writable: there is no output buffer that can fill.
    revents |= POLLOUT & interest;
    if interest & POLLIN == 0 {
        return revents;
    }
    let mut mode = 0u32;
    // SAFETY: the handle is live and `mode` is a writable local.
    if unsafe { GetConsoleMode(handle, &raw mut mode) } == 0 {
        // Not a console at all — could be a redirected pipe, file, or device.
        let pipe_res = pipe_readiness(entry, interest);
        if pipe_res != 0 {
            return revents | pipe_res;
        }
        return revents;
    }
    if kinakaze_vfs::tty::console_readable() {
        revents |= POLLIN & interest;
    }
    revents
}

/// Translates `poll` interest into the `epoll` bits the VFS understands.
fn poll_to_epoll(events: i16) -> u32 {
    let mut interest = 0u32;
    if events & POLLIN != 0 {
        interest |= kinakaze_vfs::epoll::EPOLLIN;
    }
    if events & POLLPRI != 0 {
        interest |= kinakaze_vfs::epoll::EPOLLPRI;
    }
    if events & POLLOUT != 0 {
        interest |= kinakaze_vfs::epoll::EPOLLOUT;
    }
    interest
}

/// Translates `epoll` readiness back into `poll` bits.
fn epoll_to_poll(events: u32, interest: i16) -> i16 {
    let mut revents = 0i16;
    if events & (kinakaze_vfs::epoll::EPOLLIN | kinakaze_vfs::epoll::EPOLLRDNORM) != 0 {
        revents |= POLLIN & interest;
    }
    if events & (kinakaze_vfs::epoll::EPOLLPRI | kinakaze_vfs::epoll::EPOLLRDBAND) != 0 {
        revents |= POLLPRI & interest;
    }
    if events & (kinakaze_vfs::epoll::EPOLLOUT | kinakaze_vfs::epoll::EPOLLWRNORM) != 0 {
        revents |= POLLOUT & interest;
    }
    // Error and hangup are delivered whether or not they were requested.
    if events & kinakaze_vfs::epoll::EPOLLERR != 0 {
        revents |= POLLERR;
    }
    if events & kinakaze_vfs::epoll::EPOLLHUP != 0 {
        revents |= POLLHUP;
    }
    if events & kinakaze_vfs::epoll::EPOLLRDHUP != 0 {
        // A read hangup with data still buffered is readable; POSIX has no
        // separate bit for it, so it surfaces as POLLIN plus POLLHUP.
        revents |= (POLLIN & interest) | POLLHUP;
    }
    revents
}

/// `poll`.
///
/// # Safety
///
/// `fds` must point at `count` writable `struct pollfd` entries.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_poll(
    fds: *mut PollFd,
    count: u64,
    timeout_ms: c_int,
) -> c_int {
    if fds.is_null() && count != 0 {
        set_errno(EFAULT);
        return -1;
    }
    let limit = match kinakaze_vfs::job::current_nofile_limit() {
        Ok(limit) => limit as u64,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    if count > limit {
        set_errno(EINVAL);
        return -1;
    }
    let count = count as usize;
    // SAFETY: the caller guarantees `count` writable entries; a zero count makes
    // an empty slice, for which the dangling pointer is never dereferenced.
    let entries = if count == 0 {
        &mut [][..]
    } else {
        unsafe { core::slice::from_raw_parts_mut(fds, count) }
    };
    let result = poll_wait(entries, timeout_ms);
    if result == Err(kinakaze_vfs::EINTR) {
        kinakaze_vfs::signal::deliver_pending();
    }
    posix_value(result)
}

/// The body of `poll`, working on a checked slice.
fn poll_wait(entries: &mut [PollFd], timeout_ms: c_int) -> Result<c_int, i32> {
    if crate::fork_trace_enabled() {
        eprintln!(
            "kinakaze libc: pid {} poll fds={:?} timeout={timeout_ms}",
            std::process::id(),
            entries.iter().map(|e| (e.fd, e.events)).collect::<Vec<_>>()
        );
    }
    // revents is cleared first: POSIX requires it be written on every call, and a
    // caller that reuses its array must not see the previous result.
    for entry in entries.iter_mut() {
        entry.revents = 0;
    }

    let deadline = (timeout_ms > 0)
        .then(|| std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64));

    // Event-backed descriptors share one epoll set, built once per call.
    let mut evented: Vec<usize> = Vec::new();
    let mut locals: Vec<usize> = Vec::new();
    let mut invalid = 0;
    for (index, entry) in entries.iter_mut().enumerate() {
        // A negative fd is ignored, with revents zero. This is not an error: it
        // is how callers disable a slot without shrinking the array.
        if entry.fd < 0 {
            continue;
        }
        match kinakaze_vfs::get(entry.fd) {
            Ok(table_entry) => {
                if matches!(
                    table_entry.kind,
                    FdKind::Socket | FdKind::UnixSocket | FdKind::Fifo
                ) || table_entry.kind == FdKind::EventFd && timeout_ms != 0
                {
                    evented.push(index);
                } else {
                    locals.push(index);
                }
            }
            Err(_) => {
                // POLLNVAL is reported whether or not it was requested, and the
                // slot counts towards the return value.
                entry.revents = POLLNVAL;
                invalid += 1;
            }
        }
    }

    // Allocate an epoll set only for descriptors with a genuine event-backed
    // wait. Pure local readiness probes do not spend an internal descriptor.
    let mut watcher = None;
    if !evented.is_empty() {
        let set = PollWatcher::new(entries, &evented)?;
        watcher = Some(set);
    }

    loop {
        // A blocking event wait below writes `revents` before asking us to
        // re-run this pass so local descriptors can be counted alongside it.
        // Preserve and count those results here.  Counting only events newly
        // filled by the zero-timeout probe would lose the wakeup: the collector
        // deliberately does not fill a slot whose `revents` is already set,
        // leaving an infinite poll spinning even though its descriptor is ready.
        let mut ready = preserved_ready_count(entries, &evented, invalid);
        // Descriptors this module answers for itself.
        for index in &locals {
            let entry = &mut entries[*index];
            let Ok(table_entry) = kinakaze_vfs::get(entry.fd) else {
                // Closed underneath us between the classification and now.
                entry.revents = POLLNVAL;
                ready += 1;
                continue;
            };
            let revents = local_readiness(table_entry, entry.events, entry.fd);
            if revents != 0 {
                entry.revents = revents;
                ready += 1;
            }
        }

        // Event-backed descriptors. A zero timeout here:
        // the blocking wait happens once below, after everything has been asked.
        if let Some(watcher) = &mut watcher {
            ready += watcher.collect(entries, 0)?;
        }

        if ready > 0 {
            return Ok(ready as c_int);
        }
        // Nothing is ready. A zero timeout returns immediately, as does an
        // expired deadline.
        if timeout_ms == 0 {
            return Ok(0);
        }
        if let Some(deadline) = deadline
            && std::time::Instant::now() >= deadline
        {
            return Ok(0);
        }

        // Wait. A watched pipe or console has no event to sleep on, so when one
        // is present the wait is capped at the re-examination interval; with only
        // event-backed descriptors epoll can absorb the whole remaining timeout.
        let remaining = remaining_ms(timeout_ms, deadline);
        let bounded = if locals.is_empty() {
            remaining
        } else {
            Some(remaining.unwrap_or(POLL_INTERVAL_MS).min(POLL_INTERVAL_MS))
        };

        match &mut watcher {
            // The epoll wait is interruptible: it waits on the thread's
            // interrupt event alongside native readiness, so a signal ends it.
            Some(watcher) => {
                let woken = watcher.collect(entries, bounded.map_or(-1, |wait| wait as c_int))?;
                if woken > 0 {
                    // Re-run the pass so the local descriptors are counted
                    // alongside the event-backed descriptors that became ready.
                    continue;
                }
            }
            None => {
                let wait = bounded.unwrap_or(POLL_INTERVAL_MS).min(POLL_INTERVAL_MS);
                // Descriptors with a real waitable object are slept on rather
                // than re-examined, so a keystroke or a byte written to a pty
                // wakes this immediately instead of on the next sweep.
                let mut waitable: Vec<windows_sys::Win32::Foundation::HANDLE> =
                    Vec::with_capacity(4);
                for index in &locals {
                    let entry = &entries[*index];
                    if entry.events & POLLIN == 0 {
                        continue;
                    }
                    let Ok(table_entry) = kinakaze_vfs::get(entry.fd) else {
                        continue;
                    };
                    match table_entry.kind {
                        FdKind::PtyMaster | FdKind::PtySlave => {
                            if let Ok(event) = kinakaze_vfs::tty::readable_event(entry.fd) {
                                waitable.push(event);
                            }
                        }
                        FdKind::Console => {
                            // The console's readiness is the line discipline's,
                            // and its event is what the reader thread signals.
                            // A character device that is not a console has no
                            // such event, so its raw handle is used as before.
                            match kinakaze_vfs::tty::console_event() {
                                Some(event) if kinakaze_vfs::tty::is_console(entry.fd) => {
                                    waitable.push(event)
                                }
                                _ => waitable.push(
                                    table_entry.raw as windows_sys::Win32::Foundation::HANDLE,
                                ),
                            }
                        }
                        _ => {}
                    }
                }
                if waitable.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(u64::from(wait)));
                } else {
                    // SAFETY: every handle is live for the duration of the wait.
                    unsafe {
                        kinakaze_vfs::deadline_wait::any(&waitable, wait);
                    }
                }
                if kinakaze_vfs::signal::deliver_pending()
                    == kinakaze_vfs::signal::Delivery::Interrupted
                {
                    return Err(kinakaze_vfs::EINTR);
                }
            }
        }
    }
}

mod select;
pub use select::*;

/// Milliseconds left before `deadline`, or `None` for an infinite wait.
fn remaining_ms(timeout_ms: c_int, deadline: Option<std::time::Instant>) -> Option<u32> {
    match deadline {
        Some(deadline) => {
            let now = std::time::Instant::now();
            Some(if now >= deadline {
                0
            } else {
                (deadline - now)
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min(u32::MAX as u128) as u32
            })
        }
        // A negative timeout is Linux's infinite wait.
        None if timeout_ms < 0 => None,
        None => Some(0),
    }
}

/// An `epoll` set holding the event-backed descriptors one `poll` watches.
///
/// RAII retires the internal wait object on every return without consuming a
/// guest descriptor or adding state to the fork snapshot.
struct PollWatcher {
    set: kinakaze_vfs::epoll::PollSet,
    /// Reused across readiness probes and the blocking wait in this call.
    events: Vec<kinakaze_vfs::epoll::EpollEvent>,
}

impl PollWatcher {
    /// Registers each descriptor, carrying its array index as the epoll cookie.
    fn new(entries: &[PollFd], evented: &[usize]) -> Result<Self, i32> {
        // This set belongs to one call and is excluded from guest fd state.
        let set = kinakaze_vfs::epoll::PollSet::new()?;
        let mut registrations: Vec<(c_int, u32, usize)> = Vec::new();
        for index in evented {
            let entry = &entries[*index];
            let interest = poll_to_epoll(entry.events);
            if let Some((_, combined, _)) = registrations
                .iter_mut()
                .find(|(registered_fd, _, _)| *registered_fd == entry.fd)
            {
                *combined |= interest;
            } else {
                registrations.push((entry.fd, interest, *index));
            }
        }
        let watcher = Self {
            set,
            events: vec![kinakaze_vfs::epoll::EpollEvent::default(); registrations.len().max(1)],
        };
        for (registered_fd, interest, index) in registrations {
            // The cookie is the array index, which is what makes a compacted
            // result attributable to the right `struct pollfd`.
            let event = kinakaze_vfs::epoll::EpollEvent {
                events: interest,
                data: index as u64,
            };
            watcher.set.add(registered_fd, event)?;
        }
        Ok(watcher)
    }

    /// Waits up to `timeout_ms` and writes readiness into `entries`.
    ///
    /// Returns how many `struct pollfd` slots were newly filled in.
    fn collect(&mut self, entries: &mut [PollFd], timeout_ms: c_int) -> Result<usize, i32> {
        // AFD's send notification behaves like an edge on some Winsock
        // providers even for a level-triggered request. WSAPoll is a direct
        // level query, so probe it before entering the interruptible AFD wait.
        // A full send buffer simply reports no POLLOUT and falls through to AFD,
        // which will wake when space becomes available.
        let level_ready = collect_winsock_level(entries);
        // If the level probe found a socket, do not block, but still collect
        // every other socket that is ready now. `poll` reports all ready slots,
        // not just the first readiness mechanism that found one.
        let wait = if level_ready == 0 { timeout_ms } else { 0 };
        let reported = self.set.wait(&mut self.events, wait)?;
        let mut filled = level_ready;
        for event in self.events.iter().take(reported) {
            let index = event.data as usize;
            let Some(entry) = entries.get(index) else {
                continue;
            };
            let fd = entry.fd;
            // Every slot naming this descriptor gets the result, which is how a
            // duplicate fd in the caller's array is served from one
            // registration.
            for slot in entries.iter_mut() {
                if slot.fd != fd {
                    continue;
                }
                let translated = epoll_to_poll(event.events, slot.events);
                if translated != 0 && slot.revents == 0 {
                    slot.revents = translated;
                    filled += 1;
                }
            }
        }
        Ok(filled)
    }
}

/// Applies Winsock's level-triggered readiness to every pollfd slot naming a
/// real network socket. Unix sockets stay on the named-pipe epoll path.
fn collect_winsock_level(entries: &mut [PollFd]) -> usize {
    let mut probe_fds: Vec<c_int> = Vec::new();
    let mut probes: Vec<WSAPOLLFD> = Vec::new();
    for entry in entries.iter() {
        let Ok(table_entry) = kinakaze_vfs::get(entry.fd) else {
            continue;
        };
        if table_entry.kind != FdKind::Socket || table_entry.flags.contains(FdFlags::PACKET_SOCKET)
        {
            continue;
        }
        let mut interest = 0i16;
        if entry.events & (POLLIN | POLLPRI) != 0 {
            interest |= WSA_POLLIN;
        }
        if entry.events & POLLOUT != 0 {
            interest |= WSA_POLLOUT;
        }
        if let Some(index) = probe_fds.iter().position(|fd| *fd == entry.fd) {
            probes[index].events |= interest;
        } else {
            probe_fds.push(entry.fd);
            probes.push(WSAPOLLFD {
                fd: table_entry.raw,
                events: interest,
                revents: 0,
            });
        }
    }
    if probes.is_empty() {
        return 0;
    }
    let ready = unsafe {
        WSAPoll(
            probes.as_mut_ptr(),
            probes.len().min(u32::MAX as usize) as u32,
            0,
        )
    };
    if ready <= 0 {
        return 0;
    }

    let mut filled = 0usize;
    for (fd, probe) in probe_fds.into_iter().zip(probes) {
        for entry in entries.iter_mut().filter(|entry| entry.fd == fd) {
            let mut revents = 0i16;
            if probe.revents & WSA_POLLIN != 0 {
                revents |= POLLIN & entry.events;
            }
            if probe.revents & WSA_POLLOUT != 0 {
                revents |= POLLOUT & entry.events;
            }
            if probe.revents & WSA_POLLERR != 0 {
                revents |= POLLERR;
            }
            if probe.revents & WSA_POLLHUP != 0 {
                revents |= POLLHUP;
            }
            if probe.revents & WSA_POLLNVAL != 0 {
                revents |= POLLNVAL;
            }
            if revents != 0 && entry.revents == 0 {
                entry.revents = revents;
                filled += 1;
            }
        }
    }
    filled
}

// ---------------------------------------------------------------------------
// Memory mapping.
//
// Two Windows mechanisms stand behind one Linux call. A file-backed mapping is a
// section from `CreateFileMappingW` with a view from `MapViewOfFileEx`; an
// anonymous one is either `VirtualAlloc` for MAP_PRIVATE or a page-file-backed
// section for MAP_SHARED, which is the only form that survives a fork.
//
// `munmap` has to know which, and Linux gives it only an address and a length, so
// every live mapping is recorded in a registry keyed by the address handed to the
// guest. Without it, `munmap` would have to guess between `UnmapViewOfFile` and
// `VirtualFree`, and the wrong guess fails while leaving the mapping in place.
// ---------------------------------------------------------------------------

/// `PROT_NONE`.
pub const PROT_NONE: c_int = 0;
/// `PROT_READ`.
pub const PROT_READ: c_int = 1;
/// `PROT_WRITE`.
pub const PROT_WRITE: c_int = 2;
/// `PROT_EXEC`.
pub const PROT_EXEC: c_int = 4;

/// `MAP_SHARED`.
pub const MAP_SHARED: c_int = 0x01;
/// `MAP_PRIVATE`.
pub const MAP_PRIVATE: c_int = 0x02;
/// `MAP_FIXED`.
pub const MAP_FIXED: c_int = 0x10;
/// `MAP_ANONYMOUS`.
pub const MAP_ANONYMOUS: c_int = 0x20;
/// `MAP_NORESERVE`, accepted and ignored: Windows always charges commit.
const MAP_NORESERVE: c_int = 0x4000;

/// `MAP_FAILED`, which is `(void *)-1` and emphatically not null.
///
/// Every caller tests `== MAP_FAILED`. Returning null on failure would pass that
/// test and hand the guest a null pointer it believes is a mapping.
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;

/// How a live mapping was created, which decides how it is released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
enum MappingKind {
    /// A view of a section: released with `UnmapViewOfFile`.
    View,
    /// Committed process memory: released with `VirtualFree`.
    Reserved,
    /// Address space reserved with `MEM_RESERVE_PLACEHOLDER`.
    ///
    /// Unlike an ordinary reservation, a placeholder can be split on page
    /// boundaries and replaced in-place by either private pages or a section
    /// view. That is the Windows primitive which implements Linux `MAP_FIXED`
    /// without copying.
    Placeholder,
    /// Private pages which replaced one exact placeholder fragment.
    PlaceholderPrivate,
    /// A section view which replaced one exact placeholder fragment.
    PlaceholderView,
    /// Pages committed inside an address-space reservation owned by Windows or
    /// another host component, such as a native thread stack.
    ///
    /// The pages may be decommitted, but the surrounding reservation and its
    /// allocation base must never be released or copied into a fork child.
    BorrowedReservation,
    /// Pages in a host-owned section view whose protection was changed in place.
    ///
    /// Windows cannot decommit or unmap only this borrowed fragment, so Linux's
    /// inaccessible state is represented with `PAGE_NOACCESS` instead.
    BorrowedView,
}

impl MappingKind {
    fn is_borrowed(self) -> bool {
        matches!(self, Self::BorrowedReservation | Self::BorrowedView)
    }
}

/// A section handle whose storage lives in the managed fork arena.
///
/// A raw Windows handle is process-local. Private copy backings receive a fresh
/// child-private pagefile section; retained shared backings duplicate the same
/// kernel section. Both write the child handle into this stable slot. Keeping the
/// reference count beside it means independently split views can share one
/// backing without leaking the handle in either process.
#[repr(C)]
struct SectionBacking {
    references: AtomicUsize,
    handle: AtomicUsize,
    length: usize,
    /// COW views cannot be remapped without preserving private dirty pages.
    /// Only copied pagefile backings become freely remappable after fork.
    remappable: AtomicUsize,
    /// Retained backing mode: 0 = copied, 1 = shared, 2 = file COW, 3 = anonymous snapshot.
    retained_shared: AtomicUsize,
    /// Maximum native view permission, distinct from current page protection.
    maximum_protection: u32,
    /// Anonymous shared-object identity, unchanged when its handle is duplicated.
    futex_identity: [u64; 3],
}

struct BackingRef(*mut SectionBacking);

unsafe impl Send for BackingRef {}
unsafe impl Sync for BackingRef {}

impl BackingRef {
    fn new(handle: *mut c_void, length: usize, remappable: bool) -> Result<Self, i32> {
        let layout = Layout::new::<SectionBacking>();
        // SAFETY: the managed allocator returns suitably aligned storage in the
        // fixed arena copied by the Windows fork backend.
        let raw =
            unsafe { kinakaze_alloc::ManagedAllocator.alloc(layout) }.cast::<SectionBacking>();
        if raw.is_null() {
            return Err(ENOMEM);
        }
        // SAFETY: `raw` names one fresh, properly aligned allocation.
        unsafe {
            raw.write(SectionBacking {
                references: AtomicUsize::new(1),
                handle: AtomicUsize::new(handle as usize),
                length,
                remappable: AtomicUsize::new(remappable as usize),
                retained_shared: AtomicUsize::new(0),
                maximum_protection: 0,
                futex_identity: [0; 3],
            });
        }
        Ok(Self(raw))
    }

    fn new_retained_shared(
        handle: *mut c_void,
        length: usize,
        maximum_protection: u32,
    ) -> Result<Self, i32> {
        let identity = crate::futex::new_backing_id()?;
        let result = Self::new_retained(handle, length, maximum_protection, 1)?;
        unsafe { (*result.0).futex_identity = identity };
        Ok(result)
    }

    fn new_cow(handle: *mut c_void, length: usize, section_protection: u32) -> Result<Self, i32> {
        let maximum = if matches!(
            section_protection,
            PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
        ) {
            PAGE_EXECUTE_WRITECOPY
        } else {
            PAGE_WRITECOPY
        };
        Self::new_retained(handle, length, maximum, 2)
    }

    fn new_snapshot(handle: *mut c_void, length: usize) -> Result<Self, i32> {
        Self::new_retained(handle, length, PAGE_EXECUTE_WRITECOPY, 3)
    }

    fn new_retained(
        handle: *mut c_void,
        length: usize,
        maximum_protection: u32,
        mode: usize,
    ) -> Result<Self, i32> {
        let result = Self::new(handle, length, false)?;
        unsafe { (*result.0).maximum_protection = maximum_protection };
        if !unsafe {
            kinakaze_runtime::register_fork_handle_slot(result.handle_slot() as *const AtomicUsize)
        } {
            // Like `new`, failure leaves ownership with the caller.
            unsafe { (*result.0).handle.store(0, Ordering::Release) };
            return Err(ENOMEM);
        }
        unsafe { (*result.0).retained_shared.store(mode, Ordering::Release) };
        Ok(result)
    }

    /// Adopts one reference already represented in a copied fork snapshot.
    unsafe fn adopt(raw: usize) -> Result<Self, i32> {
        let arena_end = kinakaze_alloc::ARENA_BASE + kinakaze_alloc::ARENA_SIZE;
        if raw < kinakaze_alloc::ARENA_BASE
            || raw
                .checked_add(size_of::<SectionBacking>())
                .is_none_or(|end| end > arena_end)
            || raw % align_of::<SectionBacking>() != 0
        {
            return Err(EINVAL);
        }
        let record = unsafe { &*(raw as *const SectionBacking) };
        if record.references.load(Ordering::Acquire) == 0 || record.length == 0 {
            return Err(EINVAL);
        }
        if record.retained_shared.load(Ordering::Acquire) != 0 {
            let mut flags = 0;
            let valid = match record.retained_shared.load(Ordering::Acquire) {
                1 => matches!(
                    record.maximum_protection,
                    PAGE_READONLY | PAGE_READWRITE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE
                ),
                2 | 3 => matches!(
                    record.maximum_protection,
                    PAGE_WRITECOPY | PAGE_EXECUTE_WRITECOPY
                ),
                _ => false,
            };
            if !valid
                || unsafe {
                    windows_sys::Win32::Foundation::GetHandleInformation(
                        record.handle.load(Ordering::Acquire) as *mut c_void,
                        &mut flags,
                    )
                } == 0
                || flags & windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT != 0
            {
                return Err(EINVAL);
            }
        }
        Ok(Self(raw as *mut SectionBacking))
    }

    fn handle(&self) -> *mut c_void {
        // SAFETY: every BackingRef keeps the allocation alive.
        unsafe { (*self.0).handle.load(Ordering::Acquire) as *mut c_void }
    }

    fn handle_slot(&self) -> usize {
        // SAFETY: the allocation is stable for the lifetime of every fragment.
        unsafe { ptr::addr_of!((*self.0).handle) as usize }
    }

    fn raw(&self) -> usize {
        self.0 as usize
    }

    fn is_remappable(&self) -> bool {
        // SAFETY: every BackingRef keeps the allocation alive.
        unsafe { (*self.0).remappable.load(Ordering::Acquire) != 0 }
    }

    fn is_retained_shared(&self) -> bool {
        unsafe { (*self.0).retained_shared.load(Ordering::Acquire) == 1 }
    }

    fn is_cow(&self) -> bool {
        unsafe { matches!((*self.0).retained_shared.load(Ordering::Acquire), 2 | 3) }
    }

    fn is_snapshot(&self) -> bool {
        unsafe { (*self.0).retained_shared.load(Ordering::Acquire) == 3 }
    }

    fn is_retained(&self) -> bool {
        unsafe { (*self.0).retained_shared.load(Ordering::Acquire) != 0 }
    }

    fn maximum_protection(&self) -> u32 {
        unsafe { (*self.0).maximum_protection }
    }

    fn mark_remappable(&self) {
        if self.is_retained() {
            return;
        }
        // SAFETY: the child owns its copied record independently of the parent.
        unsafe { (*self.0).remappable.store(1, Ordering::Release) };
    }
}

impl Clone for BackingRef {
    fn clone(&self) -> Self {
        // SAFETY: an existing BackingRef proves the record is live.
        unsafe { (*self.0).references.fetch_add(1, Ordering::Relaxed) };
        Self(self.0)
    }
}

impl Drop for BackingRef {
    fn drop(&mut self) {
        let _transaction = if self.is_retained() {
            let Some(transaction) = kinakaze_runtime::begin_fork_mapping_transaction() else {
                return;
            };
            Some(transaction)
        } else {
            None
        };
        // SAFETY: each BackingRef owns exactly one count.
        if unsafe { (*self.0).references.fetch_sub(1, Ordering::AcqRel) } != 1 {
            return;
        }
        if self.is_retained()
            && !kinakaze_runtime::unregister_fork_handle_slot(
                self.handle_slot() as *const AtomicUsize
            )
        {
            unsafe { (*self.0).references.store(1, Ordering::Release) };
            return;
        }
        let handle = unsafe { (*self.0).handle.swap(0, Ordering::AcqRel) } as *mut c_void;
        if !handle.is_null() {
            unsafe { CloseHandle(handle) };
        }
        // SAFETY: the final reference uniquely owns the managed allocation.
        unsafe {
            ptr::drop_in_place(self.0);
            kinakaze_alloc::ManagedAllocator
                .dealloc(self.0.cast::<u8>(), Layout::new::<SectionBacking>());
        }
    }
}

/// One live mapping.
#[derive(Clone, Copy)]
struct FileMappingInfo {
    fd: i32,
    generation: u32,
    protection: u32,
    shared: bool,
    origin: usize,
    mapping_length: usize,
    file_offset: u64,
}

/// One live mapping.
#[derive(Clone)]
struct Mapping {
    /// The address `MapViewOfFileEx` or `VirtualAlloc` actually returned.
    ///
    /// Not the same as the address the guest holds when the requested offset was
    /// not a multiple of the allocation granularity: `UnmapViewOfFile` insists on
    /// the view's own base, so it has to be remembered separately.
    base: *mut c_void,
    /// Length of the region as the guest sees it, from its own pointer.
    length: usize,
    kind: MappingKind,
    /// Whether the view is shared, so `munmap` knows to flush it.
    shared: bool,
    /// Retained private-copy or genuinely shared native section backing.
    backing: Option<BackingRef>,
    /// Offset of this fragment in `backing`.
    backing_offset: u64,
    /// Protection needed if this view is temporarily split and remapped.
    view_protection: u32,
    /// File identity and original VMA geometry for a mapping whose inaccessible
    /// EOF tail can become backed after `ftruncate` extends the file.
    file: Option<FileMappingInfo>,
    /// Verified bytes and Linux permissions, independent of a closed/reused fd.
    verity: Option<verity::Info>,
    /// Inode of a direct native file section, including mappings with no fd left.
    native_inode: Option<verity::NativeId>,
    /// File-origin ownership and exact per-fragment Linux permissions.
    file_origin: Option<file_origin::Info>,
}

// SAFETY: the addresses are process-wide and the registry is behind a mutex; the
// pointers are only ever compared and passed back to the mapping APIs.
unsafe impl Send for Mapping {}

/// Live mappings, keyed by the address handed to the guest.
static MAPPINGS: OnceLock<Mutex<HashMap<usize, Mapping>>> = OnceLock::new();

fn mappings() -> &'static Mutex<HashMap<usize, Mapping>> {
    MAPPINGS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve Linux ownership before consulting native memory types. Fork turns
/// private heaps/COW mappings into native sections without making them shared.
/// File keys use the pinned inode and exact offset, never the fd, view VA, or
/// process-local section handle. Shared anonymous keys live in retained backing
/// metadata, so independently allocated sections at the same VA stay distinct.
pub(crate) fn futex_key(address: usize) -> Result<crate::futex::Key, i32> {
    if let Some(registry) = MAPPINGS.get() {
        let registry = registry.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((start, mapping)) = registry
            .iter()
            .find(|(start, mapping)| address >= **start && address - **start < mapping.length)
        {
            if let Some(origin) = &mapping.file_origin {
                return if origin.owner.shared() {
                    Ok(origin
                        .owner
                        .identity()
                        .futex_key(origin.owner.offset(address)?))
                } else {
                    crate::futex::Key::private(address)
                };
            }
            if !mapping.shared {
                return crate::futex::Key::private(address);
            }
            let offset = mapping
                .backing_offset
                .checked_add((address - *start) as u64)
                .ok_or(EINVAL)?;
            if let Some(inode) = mapping.native_inode {
                return Ok(inode.futex_key(offset));
            }
            let backing = mapping
                .backing
                .as_ref()
                .filter(|backing| backing.is_retained_shared())
                .ok_or(EFAULT)?;
            return Ok(crate::futex::Key::anonymous(
                unsafe { (*backing.0).futex_identity },
                offset,
            ));
        }
    }
    // fork may replace the guest malloc arena with a private section view.
    // Its Linux ownership stays private even when VirtualQuery says MEM_MAPPED.
    if kinakaze_alloc::guest::contains(address) {
        return crate::futex::Key::private(address);
    }
    // Native thread stacks do not go through guest mmap.
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_IMAGE, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, VirtualQuery,
    };
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    if (unsafe {
        VirtualQuery(
            address as _,
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        ) != 0
    }) && info.State == MEM_COMMIT
        && matches!(info.Type, MEM_PRIVATE | MEM_IMAGE)
    {
        crate::futex::Key::private(address)
    } else {
        Err(EFAULT)
    }
}

/// Rebuilds the libc-side view graph after a fresh fork child loads this DLL.
/// The runtime has already recreated the actual address-space topology; this
/// participant restores the metadata needed by later mapping calls in the child.
mod mapping_fork_handoff {
    use super::*;

    const KEY: u64 = 0x4c49_4243_4d4d_4150; // "LIBCMMAP"
    const VERSION: u32 = 5;
    const HEADER_SIZE: usize = 8;
    const ENTRY_SIZE: usize = 168;

    unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
        let Ok(registry) = mappings().lock() else {
            return -(EIO as isize);
        };
        let count = registry
            .values()
            .filter(|mapping| has_fork_mapping_storage(mapping))
            .count();
        let Some(required) = count
            .checked_mul(ENTRY_SIZE)
            .and_then(|entries| entries.checked_add(HEADER_SIZE))
        else {
            return -(EOVERFLOW as isize);
        };
        if buffer.is_null() {
            return required as isize;
        }
        if capacity < required {
            return -(ENOMEM as isize);
        }
        let output = unsafe { core::slice::from_raw_parts_mut(buffer, required) };
        output[0..4].copy_from_slice(&VERSION.to_le_bytes());
        output[4..8].copy_from_slice(&(count as u32).to_le_bytes());
        for (index, (&start, mapping)) in registry
            .iter()
            .filter(|(_, mapping)| has_fork_mapping_storage(mapping))
            .enumerate()
        {
            let cursor = HEADER_SIZE + index * ENTRY_SIZE;
            output[cursor..cursor + 8].copy_from_slice(&(start as u64).to_le_bytes());
            output[cursor + 8..cursor + 16]
                .copy_from_slice(&(mapping.base as usize as u64).to_le_bytes());
            output[cursor + 16..cursor + 24]
                .copy_from_slice(&(mapping.length as u64).to_le_bytes());
            output[cursor + 24..cursor + 28].copy_from_slice(&(mapping.kind as u32).to_le_bytes());
            output[cursor + 28..cursor + 32]
                .copy_from_slice(&(mapping.shared as u32).to_le_bytes());
            output[cursor + 32..cursor + 40].copy_from_slice(
                &(mapping.backing.as_ref().map_or(0, BackingRef::raw) as u64).to_le_bytes(),
            );
            output[cursor + 40..cursor + 48].copy_from_slice(&mapping.backing_offset.to_le_bytes());
            output[cursor + 48..cursor + 52]
                .copy_from_slice(&mapping.view_protection.to_le_bytes());
            let file = mapping.file;
            output[cursor + 52..cursor + 56]
                .copy_from_slice(&u32::from(file.is_some()).to_le_bytes());
            output[cursor + 56..cursor + 60]
                .copy_from_slice(&file.map_or(-1, |file| file.fd).to_le_bytes());
            output[cursor + 60..cursor + 64]
                .copy_from_slice(&file.map_or(0, |file| file.generation).to_le_bytes());
            output[cursor + 64..cursor + 68]
                .copy_from_slice(&file.map_or(0, |file| file.protection).to_le_bytes());
            output[cursor + 68..cursor + 72]
                .copy_from_slice(&file.map_or(0, |file| u32::from(file.shared)).to_le_bytes());
            output[cursor + 72..cursor + 80]
                .copy_from_slice(&(file.map_or(0, |file| file.origin) as u64).to_le_bytes());
            output[cursor + 80..cursor + 88].copy_from_slice(
                &(file.map_or(0, |file| file.mapping_length) as u64).to_le_bytes(),
            );
            output[cursor + 88..cursor + 96]
                .copy_from_slice(&file.map_or(0, |file| file.file_offset).to_le_bytes());
            verity::encode(mapping, &mut output[cursor + 96..cursor + 152]);
            file_origin::encode(
                mapping.file_origin.as_ref(),
                &mut output[cursor + 152..cursor + ENTRY_SIZE],
            );
        }
        required as isize
    }

    unsafe extern "system" fn child(payload: *const u8, len: usize) -> i32 {
        if payload.is_null() || len < HEADER_SIZE {
            return EINVAL;
        }
        let payload = unsafe { core::slice::from_raw_parts(payload, len) };
        let version = u32::from_le_bytes(payload[0..4].try_into().unwrap());
        let count = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
        if version != VERSION
            || count
                .checked_mul(ENTRY_SIZE)
                .and_then(|entries| entries.checked_add(HEADER_SIZE))
                != Some(len)
        {
            return EINVAL;
        }

        verity::reset_child_fault_index();

        let mut restored = HashMap::with_capacity(count);
        for index in 0..count {
            let cursor = HEADER_SIZE + index * ENTRY_SIZE;
            let start =
                u64::from_le_bytes(payload[cursor..cursor + 8].try_into().unwrap()) as usize;
            let base =
                u64::from_le_bytes(payload[cursor + 8..cursor + 16].try_into().unwrap()) as usize;
            let length =
                u64::from_le_bytes(payload[cursor + 16..cursor + 24].try_into().unwrap()) as usize;
            let kind =
                match u32::from_le_bytes(payload[cursor + 24..cursor + 28].try_into().unwrap()) {
                    0 => MappingKind::View,
                    1 => MappingKind::Reserved,
                    2 => MappingKind::Placeholder,
                    3 => MappingKind::PlaceholderPrivate,
                    4 => MappingKind::PlaceholderView,
                    _ => return EINVAL,
                };
            let shared =
                match u32::from_le_bytes(payload[cursor + 28..cursor + 32].try_into().unwrap()) {
                    0 => false,
                    1 => true,
                    _ => return EINVAL,
                };
            let backing_raw =
                u64::from_le_bytes(payload[cursor + 32..cursor + 40].try_into().unwrap()) as usize;
            let backing_offset =
                u64::from_le_bytes(payload[cursor + 40..cursor + 48].try_into().unwrap());
            let view_protection =
                u32::from_le_bytes(payload[cursor + 48..cursor + 52].try_into().unwrap());
            let (verity, native_inode) = match verity::decode(&payload[cursor + 96..cursor + 152]) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let mut file_origin = match unsafe {
                file_origin::decode(&payload[cursor + 152..cursor + ENTRY_SIZE], start, length)
            } {
                Ok(value) => value,
                Err(error) => return error,
            };
            let file =
                match u32::from_le_bytes(payload[cursor + 52..cursor + 56].try_into().unwrap()) {
                    0 => None,
                    1 => {
                        let fd = i32::from_le_bytes(
                            payload[cursor + 56..cursor + 60].try_into().unwrap(),
                        );
                        let generation = u32::from_le_bytes(
                            payload[cursor + 60..cursor + 64].try_into().unwrap(),
                        );
                        let protection = u32::from_le_bytes(
                            payload[cursor + 64..cursor + 68].try_into().unwrap(),
                        );
                        let shared = match u32::from_le_bytes(
                            payload[cursor + 68..cursor + 72].try_into().unwrap(),
                        ) {
                            0 => false,
                            1 => true,
                            _ => return EINVAL,
                        };
                        let origin = u64::from_le_bytes(
                            payload[cursor + 72..cursor + 80].try_into().unwrap(),
                        ) as usize;
                        let mapping_length = u64::from_le_bytes(
                            payload[cursor + 80..cursor + 88].try_into().unwrap(),
                        ) as usize;
                        let file_offset = u64::from_le_bytes(
                            payload[cursor + 88..cursor + 96].try_into().unwrap(),
                        );
                        if fd < 0
                            || generation == 0
                            || origin == 0
                            || mapping_length == 0
                            || start < origin
                            || start
                                .checked_add(length)
                                .is_none_or(|end| end > origin.saturating_add(mapping_length))
                        {
                            return EINVAL;
                        }
                        Some(FileMappingInfo {
                            fd,
                            generation,
                            protection,
                            shared,
                            origin,
                            mapping_length,
                            file_offset,
                        })
                    }
                    _ => return EINVAL,
                };
            let backing = if backing_raw == 0 {
                None
            } else {
                let adopted = match unsafe { BackingRef::adopt(backing_raw) } {
                    Ok(backing) => backing,
                    Err(error) => return error,
                };
                // SAFETY: `adopt` validated the complete record before this read.
                let backing_length = unsafe { (*adopted.0).length };
                if (!matches!(kind, MappingKind::View | MappingKind::PlaceholderView)
                    || (kind == MappingKind::View && !adopted.is_retained()))
                    || backing_offset
                        .checked_add(length as u64)
                        .is_none_or(|end| end > backing_length as u64)
                {
                    return EINVAL;
                }
                if adopted.is_retained_shared() && (!shared || adopted.handle().is_null()) {
                    return EINVAL;
                }
                if adopted.is_cow() && (shared || adopted.handle().is_null()) {
                    return EINVAL;
                }
                // Shared and COW views retain their original section; dirty
                // COW pages remain private and must survive later splitting.
                adopted.mark_remappable();
                Some(adopted)
            };
            // Ordinary storage uses VirtualAlloc when native alignment permits,
            // and a private SEC_RESERVE placeholder view otherwise. Record the
            // actual child allocation, not the parent's representation or an
            // assumption that every copied byte belongs to VirtualAlloc.
            let retained_shared = backing.as_ref().is_some_and(BackingRef::is_retained_shared);
            let retained = backing.as_ref().is_some_and(BackingRef::is_retained);
            if let Some(info) = &mut file_origin {
                if !info.owner.shared()
                    && !backing.as_ref().is_some_and(BackingRef::is_cow)
                    && info.representation == file_origin::Representation::NativeSection
                {
                    info.representation = file_origin::Representation::CopiedUnclassified;
                }
            }
            if shared && !retained_shared {
                return EINVAL;
            }
            let ordinary = !retained
                && (matches!(
                    kind,
                    MappingKind::View | MappingKind::Reserved | MappingKind::PlaceholderPrivate
                ) || (kind == MappingKind::PlaceholderView && backing.is_none()));
            let (kind, base) = if retained {
                let mut information: MemoryBasicInformation = unsafe { core::mem::zeroed() };
                if unsafe {
                    VirtualQuery(
                        start as *const c_void,
                        &mut information,
                        size_of::<MemoryBasicInformation>(),
                    )
                } == 0
                    || information.type_ != MEM_MAPPED_TYPE
                    || information.allocation_base as usize != start
                {
                    return EIO;
                }
                (MappingKind::PlaceholderView, start)
            } else if ordinary {
                let mut information: MemoryBasicInformation = unsafe { core::mem::zeroed() };
                if unsafe {
                    VirtualQuery(
                        start as *const c_void,
                        &mut information,
                        size_of::<MemoryBasicInformation>(),
                    )
                } == 0
                {
                    return last_errno();
                }
                let actual = match information.type_ {
                    MEM_MAPPED_TYPE => MappingKind::PlaceholderView,
                    MEM_PRIVATE_TYPE => MappingKind::Reserved,
                    _ => return EIO,
                };
                (actual, information.allocation_base as usize)
            } else {
                (kind, base)
            };
            if start == 0
                || length == 0
                || restored
                    .insert(
                        start,
                        Mapping {
                            base: base as *mut c_void,
                            length,
                            kind,
                            shared,
                            backing,
                            backing_offset,
                            view_protection,
                            file,
                            verity,
                            native_inode,
                            file_origin,
                        },
                    )
                    .is_some()
            {
                return EINVAL;
            }
        }
        let Ok(mut registry) = mappings().lock() else {
            return EIO;
        };
        if let Err(error) = verity::publish_fault_index(&restored) {
            return error;
        }
        *registry = restored;
        0
    }

    fn register() {
        let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
            abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
            priority: 30,
            key: KEY,
            prepare: None,
            snapshot: Some(snapshot),
            parent: None,
            child: Some(child),
        });
    }

    extern "C" fn initializer() {
        register();
    }

    #[used]
    #[unsafe(link_section = ".CRT$XCU")]
    static INITIALIZER: extern "C" fn() = initializer;
}

fn mmap_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_MMAP_TRACE").is_some())
}

/// Translates `PROT_*` into the page protection for a view.
///
/// A `MAP_PRIVATE` file mapping needs the copy-on-write protections, which is what
/// makes a write visible to this process and not to the file.
fn page_protection(protection: c_int, private_file: bool) -> Result<u32, i32> {
    let read = protection & PROT_READ != 0;
    let write = protection & PROT_WRITE != 0;
    let execute = protection & PROT_EXEC != 0;
    if protection == PROT_NONE {
        return Ok(PAGE_NOACCESS);
    }
    Ok(match (write, execute, private_file) {
        (true, true, true) => PAGE_EXECUTE_WRITECOPY,
        (true, true, false) => PAGE_EXECUTE_READWRITE,
        (true, false, true) => PAGE_WRITECOPY,
        (true, false, false) => PAGE_READWRITE,
        (false, true, _) => PAGE_EXECUTE_READ,
        (false, false, _) => {
            if read {
                PAGE_READONLY
            } else {
                return Err(EINVAL);
            }
        }
    })
}

/// Translates `PROT_*` into the `FILE_MAP_*` access for a view.
fn view_access(protection: c_int, private_file: bool) -> u32 {
    let mut access = 0;
    // FILE_MAP_READ | FILE_MAP_COPY can produce a read-only view on Windows;
    // COPY already includes read access and must stand alone for private writes.
    if protection & PROT_READ != 0 && !(private_file && protection & PROT_WRITE != 0) {
        access |= FILE_MAP_READ;
    }
    if protection & PROT_WRITE != 0 {
        // A private file mapping writes to a copy, which is exactly FILE_MAP_COPY.
        access |= if private_file {
            FILE_MAP_COPY
        } else {
            FILE_MAP_WRITE
        };
    }
    if protection & PROT_EXEC != 0 {
        access |= FILE_MAP_EXECUTE;
    }
    if access == 0 {
        // PROT_NONE still needs an access right to create the view; the pages are
        // then protected with PAGE_NOACCESS.
        access = FILE_MAP_READ;
    }
    access
}

/// The section protection needed to back a view of the given protection.
///
/// A section must be created at least as permissive as any view of it, so this is
/// the maximum rather than the requested value.
fn section_protection(protection: c_int, private_file: bool) -> u32 {
    let execute = protection & PROT_EXEC != 0;
    let write = protection & PROT_WRITE != 0;
    match (execute, write, private_file) {
        (true, _, true) => PAGE_EXECUTE_WRITECOPY,
        (true, true, false) => PAGE_EXECUTE_READWRITE,
        (true, false, _) => PAGE_EXECUTE_READ,
        // MAP_PRIVATE writes are copy-on-write. PAGE_READWRITE would require
        // the underlying file handle itself to have write access, which is not
        // true for read-only archives such as HotSpot's classes.jsa.
        (false, _, true) => PAGE_WRITECOPY,
        (false, true, false) => PAGE_READWRITE,
        (false, false, _) => PAGE_READONLY,
    }
}

/// `mmap64`, and `mmap` under its other name.
///
/// # Safety
///
/// A successful call publishes `length` bytes of mapped memory at the returned
/// address. The caller must not access it after passing it to `munmap`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mmap64(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    flags: c_int,
    fd: c_int,
    offset: i64,
) -> *mut c_void {
    match map(address, length, protection, flags, fd, offset) {
        Ok(mapped) => {
            if mmap_trace_enabled() {
                eprintln!(
                    "kinakaze: mmap({address:p}, {length:#x}, prot={protection:#x}, flags={flags:#x}, fd={fd}, off={offset:#x}) = {mapped:p}"
                );
            }
            mapped
        }
        Err(error) => {
            if mmap_trace_enabled() {
                eprintln!(
                    "kinakaze: mmap({address:p}, {length:#x}, prot={protection:#x}, flags={flags:#x}, fd={fd}, off={offset:#x}) = MAP_FAILED errno={error}"
                );
            }
            set_errno(error);
            MAP_FAILED
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mmap(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    flags: c_int,
    fd: c_int,
    offset: i64,
) -> *mut c_void {
    unsafe { kinakaze_abi_mmap64(address, length, protection, flags, fd, offset) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mprotect(
    address: *mut c_void,
    length: usize,
    protection: c_int,
) -> c_int {
    let Some(_transaction) = kinakaze_runtime::begin_fork_mapping_transaction() else {
        set_errno(ENOMEM);
        return -1;
    };
    if verity::present() && length != 0 {
        if let Some(end) = (address as usize).checked_add(length) {
            if let Ok(runs) = verity::ranges(address as usize, end) {
                if runs.iter().any(|(_, _, info)| info.is_some()) {
                    let result = verity::mprotect(address, length, protection);
                    let refreshed = verity::refresh_fault_index();
                    return posix(result.and(refreshed));
                }
            }
        }
    }
    let result = unsafe { mprotect_native(address, length, protection) };
    if let Err(error) = verity::refresh_fault_index() {
        return posix(Err(error));
    }
    result
}

unsafe fn mprotect_native(address: *mut c_void, length: usize, protection: c_int) -> c_int {
    if file_origin::present() && !address.is_null() && length != 0 {
        match file_origin::mprotect(address as usize, length, protection) {
            Ok(true) => return 0,
            Ok(false) => {}
            Err(error) => return posix(Err(error)),
        }
    }
    unsafe { mprotect_native_raw(address, length, protection) }
}

unsafe fn mprotect_native_raw(address: *mut c_void, length: usize, protection: c_int) -> c_int {
    if address.is_null() || length == 0 {
        return 0;
    }
    let (page, _) = memory_geometry();
    if address as usize % page != 0 {
        set_errno(EINVAL);
        return -1;
    }
    let length = match page_rounded_length(length) {
        Ok(length) => length,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    let Some(_fork_mapping_transaction) = kinakaze_runtime::begin_fork_mapping_transaction() else {
        set_errno(ENOMEM);
        return -1;
    };
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_READONLY: u32 = 0x02;
    const PAGE_READWRITE: u32 = 0x04;
    const PAGE_EXECUTE: u32 = 0x10;
    const PAGE_EXECUTE_READ: u32 = 0x20;
    const PAGE_EXECUTE_READWRITE: u32 = 0x40;
    const MEM_COMMIT: u32 = 0x1000;

    let win_prot = match protection & (PROT_READ | PROT_WRITE | PROT_EXEC) {
        0 => PAGE_NOACCESS,
        PROT_READ => PAGE_READONLY,
        p if p == PROT_WRITE || p == (PROT_READ | PROT_WRITE) => PAGE_READWRITE,
        PROT_EXEC => PAGE_EXECUTE,
        p if p == (PROT_READ | PROT_EXEC) => PAGE_EXECUTE_READ,
        _ => PAGE_EXECUTE_READWRITE,
    };
    if protection != PROT_NONE {
        if let Err(error) = commit_placeholder_pages(address, length, win_prot) {
            if mmap_trace_enabled() {
                eprintln!(
                    "kinakaze: mprotect({address:p}, {length:#x}, prot={protection:#x}) = -1 errno={error} (placeholder)"
                );
            }
            set_errno(error);
            return -1;
        }
    }
    use kinakaze_runtime::memory_protection::{
        protect_preserving_copy_on_write as VirtualProtect, protection_for_allocation,
    };
    let mut old: u32 = 0;
    unsafe extern "system" {
        fn VirtualAlloc(
            lpAddress: *mut c_void,
            dwSize: usize,
            flAllocationType: u32,
            flProtect: u32,
        ) -> *mut c_void;
    }
    // First try VirtualProtect on the whole range: that is the common case and
    // succeeds whenever every page is already committed with a uniform state.
    let ok = unsafe { VirtualProtect(address, length, win_prot, &mut old) };
    if ok != 0 {
        if mmap_trace_enabled() {
            eprintln!(
                "kinakaze: mprotect({address:p}, {length:#x}, prot={protection:#x}) = 0 (protect)"
            );
        }
        return 0;
    }
    // A single call cannot span regions of differing state, which is exactly
    // what a guest that mixes committed pages with reserved guard pages
    // produces. V8 does this constantly: it reserves with `mmap(PROT_NONE)`,
    // commits the parts it uses, and marks the rest `PROT_NONE` again, so one
    // `mprotect` range routinely straddles a committed/reserved boundary.
    //
    // Walk the range and service each sub-region on its own.
    let mut cursor = address as usize;
    let end = cursor.saturating_add(length);
    while cursor < end {
        let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
        let written = unsafe {
            VirtualQuery(
                cursor as *const c_void,
                &mut info,
                core::mem::size_of::<MemoryBasicInformation>(),
            )
        };
        if written == 0 {
            break;
        }
        let base = info.base_address as usize;
        let region_end = base.saturating_add(info.region_size);
        // Clip to the requested range; the first and last regions overhang.
        let from = cursor.max(base);
        let to = end.min(region_end);
        if to <= from {
            break;
        }
        let size = to - from;
        let at = from as *mut c_void;

        let handled = match info.state {
            // Reserved-but-not-committed memory is already inaccessible, so
            // `PROT_NONE` is satisfied by doing nothing. Committing it only to
            // mark it no-access would spend commit charge on a guard region
            // the guest will never touch.
            MEM_RESERVE_STATE => {
                protection == PROT_NONE
                    || !unsafe { VirtualAlloc(at, size, MEM_COMMIT, win_prot) }.is_null()
            }
            // Committed memory needs its protection changed, unless it is being
            // made inaccessible and already is.
            MEM_COMMIT_STATE => {
                let mut previous: u32 = 0;
                unsafe {
                    windows_sys::Win32::System::Memory::VirtualProtect(
                        at,
                        size,
                        protection_for_allocation(info.allocation_protect, win_prot),
                        &mut previous,
                    ) != 0
                }
            }
            // Unmapped address space. Linux reports ENOMEM, and V8 in
            // particular only accepts ENOMEM here: it checks
            // `CHECK_EQ(ENOMEM, errno)` after a failed `mprotect` and treats
            // any other errno as a bug in its own caller.
            _ => false,
        };
        if !handled {
            if mmap_trace_enabled() {
                eprintln!(
                    "kinakaze: mprotect({address:p}, {length:#x}, prot={protection:#x}) = -1 errno=ENOMEM (state={:#x} at {at:p})",
                    info.state
                );
            }
            set_errno(ENOMEM);
            return -1;
        }
        cursor = to;
    }
    if cursor < end {
        // The walk stopped early: `VirtualQuery` refused, or a region made no
        // progress. Nothing was proven accessible, so this is a failure.
        let error = ENOMEM;
        if mmap_trace_enabled() {
            eprintln!(
                "kinakaze: mprotect({address:p}, {length:#x}, prot={protection:#x}) = -1 errno={error} (stopped at {cursor:#x})"
            );
        }
        set_errno(error);
        return -1;
    }
    if mmap_trace_enabled() {
        eprintln!("kinakaze: mprotect({address:p}, {length:#x}, prot={protection:#x}) = 0 (split)");
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_madvise(
    address: *mut c_void,
    length: usize,
    advice: c_int,
) -> c_int {
    let result = madvise_impl(address, length, advice);
    if mmap_trace_enabled() {
        match &result {
            Ok(()) => {
                eprintln!("kinakaze: madvise({address:p}, {length:#x}, {advice}) = 0")
            }
            Err(error) => eprintln!(
                "kinakaze: madvise({address:p}, {length:#x}, {advice}) = -1 errno={error}"
            ),
        }
    }
    posix(result)
}

const MADV_NORMAL: c_int = 0;
const MADV_RANDOM: c_int = 1;
const MADV_SEQUENTIAL: c_int = 2;
const MADV_WILLNEED: c_int = 3;
const MADV_DONTNEED: c_int = 4;
const MADV_FREE: c_int = 8;
const MADV_NOHUGEPAGE: c_int = 15;

/// Validates that one interval is wholly covered by mappings known to the Linux
/// VMA registry. `private_only` additionally requires anonymous private storage,
/// which is the only Windows mapping class that can be discarded and recreated
/// with Linux's zero-fill guarantee.
fn validate_madvise_range(start: usize, end: usize, private_only: bool) -> Result<(), i32> {
    let registry = mappings().lock().map_err(|_| EIO)?;
    let mut intervals = registry
        .iter()
        .filter_map(|(&mapping_start, mapping)| {
            let mapping_end = mapping_start.saturating_add(mapping.length);
            (mapping_start < end && start < mapping_end).then_some((
                mapping_start.max(start),
                mapping_end.min(end),
                mapping,
            ))
        })
        .collect::<Vec<_>>();
    intervals.sort_unstable_by_key(|(from, _, _)| *from);
    let mut cursor = start;
    for (from, to, mapping) in intervals {
        if from > cursor {
            if mmap_trace_enabled() {
                eprintln!(
                    "kinakaze: madvise range gap request={start:#x}..{end:#x} gap={cursor:#x}..{from:#x} registry_entries={}",
                    registry.len()
                );
            }
            return Err(ENOMEM);
        }
        if private_only {
            if mapping.file_origin.is_some() {
                // A fork copy is still file-origin memory. Until native private
                // discard and post-copy write tracking are installed it must
                // not be silently treated as zero-filled anonymous memory.
                return Err(kinakaze_vfs::EOPNOTSUPP);
            }
            let anonymous_private = matches!(
                mapping.kind,
                MappingKind::Reserved | MappingKind::Placeholder | MappingKind::PlaceholderPrivate
            ) || (mapping.kind == MappingKind::PlaceholderView
                && mapping.file.is_none()
                && mapping
                    .backing
                    .as_ref()
                    .is_some_and(|backing| backing.is_remappable() || backing.is_snapshot()));
            if mapping.shared || !anonymous_private {
                return Err(EINVAL);
            }
        }
        cursor = cursor.max(to);
        if cursor == end {
            return Ok(());
        }
    }
    if mmap_trace_enabled() {
        eprintln!(
            "kinakaze: madvise range uncovered request={start:#x}..{end:#x} covered_to={cursor:#x} registry_entries={}",
            registry.len()
        );
    }
    Err(ENOMEM)
}

#[derive(Clone, Copy)]
struct MadviseCommitRun {
    address: usize,
    length: usize,
    protection: u32,
    decommit: bool,
}

/// Implements the observable part of Linux `MADV_DONTNEED` for anonymous,
/// private mappings. Decommit drops the physical pages while retaining their
/// reservation; recommit at the same addresses supplies fresh zero pages and
/// restores each original Windows protection run.
fn discard_private_pages(start: usize, end: usize) -> Result<(), i32> {
    let _fork_mapping_transaction =
        kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    validate_madvise_range(start, end, true)?;

    // Validate and snapshot every protection before changing anything. This
    // prevents an unsupported view or a hole near the end from leaving the
    // earlier part of the request discarded on an error path.
    let mut runs = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
        if unsafe {
            VirtualQuery(
                cursor as *const c_void,
                &raw mut info,
                core::mem::size_of::<MemoryBasicInformation>(),
            )
        } == 0
        {
            return Err(ENOMEM);
        }
        let region_start = cursor.max(info.base_address as usize);
        let region_end = end.min(
            (info.base_address as usize)
                .checked_add(info.region_size)
                .ok_or(EINVAL)?,
        );
        if region_end <= region_start {
            return Err(ENOMEM);
        }
        match info.state {
            // An uncommitted piece is already a zero-fill-on-first-access
            // reservation. It needs no physical operation.
            MEM_RESERVE_STATE => {}
            MEM_COMMIT_STATE if info.type_ == MEM_PRIVATE_TYPE => runs.push(MadviseCommitRun {
                address: region_start,
                length: region_end - region_start,
                protection: info.protect,
                decommit: true,
            }),
            // Anonymous pages committed into a placeholder are page-file section
            // views. Windows cannot decommit a view, but overwriting the exact
            // range with zeroes provides MADV_DONTNEED's observable anonymous
            // memory semantics without unmapping adjacent pages. The registry
            // validation above excludes file-backed and shared views.
            MEM_COMMIT_STATE if info.type_ == MEM_MAPPED_TYPE => runs.push(MadviseCommitRun {
                address: region_start,
                length: region_end - region_start,
                protection: info.protect,
                decommit: false,
            }),
            _ => return Err(EINVAL),
        }
        cursor = region_end;
    }

    for run in runs {
        let address = run.address as *mut c_void;
        if run.decommit {
            if unsafe { VirtualFree(address, run.length, MEM_DECOMMIT) } == 0 {
                return Err(last_errno());
            }
            let committed =
                unsafe { VirtualAlloc(address, run.length, MEM_COMMIT, run.protection) };
            if committed != address {
                return Err(last_errno());
            }
        } else {
            let mut old_protection = 0;
            if unsafe {
                kinakaze_runtime::memory_protection::protect_preserving_copy_on_write(
                    address,
                    run.length,
                    PAGE_READWRITE,
                    &mut old_protection,
                )
            } == 0
            {
                return Err(last_errno());
            }
            unsafe { ptr::write_bytes(address.cast::<u8>(), 0, run.length) };
            let mut ignored = 0;
            if unsafe {
                kinakaze_runtime::memory_protection::protect_preserving_copy_on_write(
                    address,
                    run.length,
                    run.protection,
                    &mut ignored,
                )
            } == 0
            {
                return Err(last_errno());
            }
        }
    }
    Ok(())
}

/// The errno-valued body shared by raw `SYS_madvise`, `madvise`, and
/// `posix_madvise`.
pub(crate) fn madvise_impl(address: *mut c_void, length: usize, advice: c_int) -> Result<(), i32> {
    let (page, _) = memory_geometry();
    if address.is_null() || address as usize % page != 0 {
        return Err(EINVAL);
    }
    let rounded = page_rounded_length(length)?;
    if rounded == 0 {
        return Ok(());
    }
    let start = address as usize;
    let end = start.checked_add(rounded).ok_or(EINVAL)?;
    let result = match advice {
        // These are advisory access-pattern hints. Linux permits the kernel to
        // ignore them, but the address validation and success result are real.
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL | MADV_WILLNEED => {
            validate_madvise_range(start, end, false)
        }
        MADV_DONTNEED => {
            let runs = verity::ranges(start, end)?;
            if runs.iter().any(|(_, _, info)| info.is_some()) {
                validate_madvise_range(start, end, false)?;
                let _transaction =
                    kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
                for (from, to, info) in runs {
                    if info.is_some() {
                        verity::discard(from, to)?;
                    } else {
                        discard_private_pages(from, to)?;
                    }
                }
                Ok(())
            } else {
                discard_private_pages(start, end)
            }
        }
        // Windows MEM_RESET does not promise zero-fill and therefore cannot
        // implement MADV_FREE. EINVAL is the Linux-visible feature probe result;
        // runtimes can then use MADV_DONTNEED without being lied to.
        MADV_FREE => Err(EINVAL),
        // No mapping in this backend is created with MEM_LARGE_PAGES, so the
        // negative huge-page advice is already satisfied after VMA validation.
        MADV_NOHUGEPAGE => validate_madvise_range(start, end, false),
        // THP promotion and the remaining Linux policies need observable state
        // that this backend does not yet own. Refuse them explicitly.
        _ => Err(EINVAL),
    };
    if mmap_trace_enabled() {
        match result {
            Ok(()) => eprintln!("kinakaze: madvise_impl({address:p}, {length:#x}, {advice}) = 0"),
            Err(error) => eprintln!(
                "kinakaze: madvise_impl({address:p}, {length:#x}, {advice}) = -1 errno={error}"
            ),
        }
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_brk(addr: *mut c_void) -> c_int {
    if addr.is_null() {
        return 0;
    }
    set_errno(crate::ENOMEM);
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn brk(addr: *mut c_void) -> c_int {
    unsafe { kinakaze_abi_brk(addr) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sbrk(_increment: isize) -> *mut c_void {
    set_errno(crate::ENOMEM);
    (-1isize) as *mut c_void
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn sbrk(increment: isize) -> *mut c_void {
    unsafe { kinakaze_abi_sbrk(increment) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_readahead(
    fd: c_int,
    offset: i64,
    count: usize,
) -> isize {
    match fs::readahead(fd, offset, count) {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_fadvise(
    _fd: c_int,
    _offset: i64,
    _len: i64,
    _advice: c_int,
) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_posix_fadvise64(
    _fd: c_int,
    _offset: i64,
    _len: i64,
    _advice: c_int,
) -> c_int {
    0
}

#[repr(C)]
pub struct IoVec {
    pub iov_base: *mut c_void,
    pub iov_len: usize,
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_writev(
    fd: c_int,
    iov: *const IoVec,
    iovcnt: c_int,
) -> isize {
    unsafe { positioned::stream(fd, iov, iovcnt, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_readv(
    fd: c_int,
    iov: *const IoVec,
    iovcnt: c_int,
) -> isize {
    unsafe { positioned::stream(fd, iov, iovcnt, false) }
}

/// `dup3`.
///
/// Two things distinguish it from `dup2`, and this ignored both. `O_CLOEXEC` in
/// `flags` sets the close-on-exec flag on the copy — which is the entire reason
/// the call exists, since `dup2` followed by `fcntl` has a window in which a
/// concurrent `exec` leaks the descriptor. And `oldfd == newfd` is `EINVAL`
/// here, where `dup2` defines it as a no-op; a caller using `dup3` to set
/// close-on-exec on a descriptor it already has must be told that did not
/// happen.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_dup3(oldfd: c_int, newfd: c_int, flags: c_int) -> c_int {
    const O_CLOEXEC: c_int = 0o2000000;
    if flags & !O_CLOEXEC != 0 {
        set_errno(EINVAL);
        return -1;
    }
    if oldfd == newfd {
        set_errno(EINVAL);
        return -1;
    }
    let result = crate::fdio::kinakaze_abi_dup2(oldfd, newfd);
    if result >= 0 && flags & O_CLOEXEC != 0 {
        let _ = kinakaze_vfs::set_close_on_exec(newfd, true);
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pipe(fds: *mut c_int) -> c_int {
    unsafe { kinakaze_abi_pipe(fds) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pipe2(fds: *mut c_int, flags: c_int) -> c_int {
    unsafe { kinakaze_abi_pipe2(fds, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pipe2(fds: *mut c_int, flags: c_int) -> c_int {
    const O_NONBLOCK: c_int = 0o4000;
    const O_CLOEXEC: c_int = 0o2000000;
    if flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        set_errno(EINVAL);
        return -1;
    }
    let mut descriptor_flags = FdFlags::NONE;
    if flags & O_NONBLOCK != 0 {
        descriptor_flags = descriptor_flags.union(FdFlags::NONBLOCK);
    }
    if flags & O_CLOEXEC != 0 {
        descriptor_flags = descriptor_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    unsafe { create_pipe(fds, descriptor_flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_close_range(
    first: u32,
    last: u32,
    _flags: u32,
) -> c_int {
    let trace_started = std::time::Instant::now();
    if crate::fork_trace_enabled() {
        eprintln!(
            "kinakaze libc: pid {} close_range(first={}, last={}, flags={})",
            std::process::id(),
            first,
            last,
            _flags
        );
    }
    if first > last || _flags & !6 != 0 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    if _flags & 2 != 0 {
        // A private thread descriptor table (CLOSE_RANGE_UNSHARE) is not yet
        // represented; never silently apply a shared-table close instead.
        crate::set_errno(kinakaze_vfs::EOPNOTSUPP);
        return -1;
    }
    for fd in kinakaze_vfs::list_open_fds() {
        if (fd as u32) < first || (fd as u32) > last {
            continue;
        }
        if _flags & 4 != 0 {
            if let Err(error) = kinakaze_vfs::set_close_on_exec(fd, true)
                && error != kinakaze_vfs::EBADF
            {
                crate::set_errno(error);
                return -1;
            }
        } else {
            let _ = kinakaze_vfs::close(fd);
        }
    }
    if std::env::var_os("KINAKAZE_SPAWN_TRACE").is_some() {
        eprintln!(
            "kinakaze spawn: pid={} op=close_range first={} last={} elapsed_us={}",
            std::process::id(),
            first,
            last,
            trace_started.elapsed().as_micros()
        );
    }
    0
}

/// The body of `mmap64`.
fn map(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    flags: c_int,
    fd: c_int,
    offset: i64,
) -> Result<*mut c_void, i32> {
    let (page, granularity) = memory_geometry();
    if length == 0 {
        return Err(EINVAL);
    }
    if offset < 0 || offset as u64 % page as u64 != 0 {
        // Linux requires a page-aligned offset and reports EINVAL otherwise.
        return Err(EINVAL);
    }
    // Exactly one of MAP_SHARED and MAP_PRIVATE must be given.
    let shared = flags & MAP_SHARED != 0;
    let private = flags & MAP_PRIVATE != 0;
    if shared == private {
        return Err(EINVAL);
    }
    let fixed = flags & MAP_FIXED != 0;
    if fixed && address as usize % page != 0 {
        return Err(EINVAL);
    }
    // MAP_NORESERVE is accepted and ignored: Windows charges commit for every
    // committed page and offers no way to opt out, so honouring it is impossible
    // and refusing it would fail callers for whom it is only a hint.
    let _ = flags & MAP_NORESERVE;

    // The host VM operation and every registry split/publication below form one
    // Linux VMA mutation.  Holding the process-shared transaction prevents fork
    // from observing the Windows mapping before its registry record (or the
    // inverse during replacement).
    let _fork_mapping_transaction =
        kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;

    if flags & MAP_ANONYMOUS != 0 {
        let result = map_anonymous(address, length, protection, shared, fixed);
        verity::refresh_fault_index()?;
        return result;
    }
    let result = map_file(
        address,
        length,
        protection,
        shared,
        fixed,
        fd,
        offset as u64,
        granularity,
    );
    verity::refresh_fault_index()?;
    result
}

#[repr(C)]
struct MemAddressRequirements {
    lowest_starting_address: *mut c_void,
    highest_ending_address: *mut c_void,
    alignment: usize,
}

#[repr(C)]
struct MemExtendedParameter {
    type_and_reserved: u64,
    pointer: *mut c_void,
}

unsafe fn virtual_alloc2_aligned(
    address: *mut c_void,
    length: usize,
    alloc_type: u32,
    protect: u32,
    alignment: usize,
) -> *mut c_void {
    let mut req = MemAddressRequirements {
        lowest_starting_address: core::ptr::null_mut(),
        highest_ending_address: core::ptr::null_mut(),
        alignment,
    };
    let mut param = MemExtendedParameter {
        type_and_reserved: 1, // MemExtendedParameterAddressRequirements
        pointer: &mut req as *mut _ as *mut c_void,
    };
    let current_process = unsafe { GetCurrentProcess() };
    unsafe {
        windows_sys::Win32::System::Memory::VirtualAlloc2(
            current_process,
            address,
            length,
            alloc_type,
            protect,
            &mut param as *mut _ as *mut _,
            1,
        )
    }
}

fn page_rounded_length(length: usize) -> Result<usize, i32> {
    let (page, _) = memory_geometry();
    length
        .checked_add(page - 1)
        .ok_or(EINVAL)
        .map(|value| value / page * page)
}

unsafe fn reserve_placeholder_at(address: *mut c_void, length: usize) -> *mut c_void {
    let (page, granularity) = memory_geometry();
    let requested = address as usize / page * page;
    let base = requested / granularity * granularity;
    let prefix = requested - base;
    let Some(reserved_length) = length.checked_add(prefix) else {
        unsafe { windows_sys::Win32::Foundation::SetLastError(87) };
        return ptr::null_mut();
    };
    // Windows accepts allocation-granularity bases. Passing a Linux page hint
    // directly can round the base down while retaining the original end,
    // leaving a larger placeholder than the mapping registry records. A later
    // replacement (e.g. V8's 8 KiB Wasm code space) then fails exact-size checks.
    let allocated = unsafe {
        windows_sys::Win32::System::Memory::VirtualAlloc2(
            GetCurrentProcess(),
            base as *mut c_void,
            reserved_length,
            MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
            PAGE_NOACCESS,
            ptr::null_mut(),
            0,
        )
    };
    if allocated.is_null() || prefix == 0 {
        return allocated;
    }
    // Split off the leading alignment gap so the returned placeholder has
    // exactly the guest's length, including for page-aligned MAP_FIXED.
    if unsafe { VirtualFree(allocated, prefix, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER) } == 0 {
        let error = unsafe { GetLastError() };
        unsafe { VirtualFree(allocated, 0, MEM_RELEASE) };
        unsafe { windows_sys::Win32::Foundation::SetLastError(error) };
        return ptr::null_mut();
    }
    let result = unsafe { allocated.byte_add(prefix) };
    if unsafe { VirtualFree(allocated, 0, MEM_RELEASE) } == 0 {
        let error = unsafe { GetLastError() };
        unsafe { VirtualFree(result, 0, MEM_RELEASE) };
        unsafe { windows_sys::Win32::Foundation::SetLastError(error) };
        return ptr::null_mut();
    }
    result
}

fn has_fork_mapping_storage(mapping: &Mapping) -> bool {
    !mapping.kind.is_borrowed()
        && (!mapping.shared
            || mapping
                .backing
                .as_ref()
                .is_some_and(BackingRef::is_retained_shared))
}

fn register_fork_fragment(start: usize, mapping: &Mapping) {
    if has_fork_mapping_storage(mapping) {
        let (storage, backing_slot, backing_offset, view_protection) =
            match (&mapping.kind, &mapping.backing) {
                (MappingKind::Placeholder, _) => {
                    (kinakaze_runtime::ForkMappingStorage::Placeholder, 0, 0, 0)
                }
                (_, Some(backing)) if backing.is_retained_shared() => (
                    kinakaze_runtime::ForkMappingStorage::RetainedSection,
                    backing.handle_slot(),
                    mapping.backing_offset,
                    backing.maximum_protection(),
                ),
                (_, Some(backing)) if backing.is_snapshot() => (
                    kinakaze_runtime::ForkMappingStorage::AnonymousSnapshot,
                    backing.handle_slot(),
                    mapping.backing_offset,
                    backing.maximum_protection(),
                ),
                (_, Some(backing)) if backing.is_cow() => (
                    kinakaze_runtime::ForkMappingStorage::CopyOnWriteSection,
                    backing.handle_slot(),
                    mapping.backing_offset,
                    backing.maximum_protection(),
                ),
                (MappingKind::PlaceholderView, Some(backing)) => (
                    kinakaze_runtime::ForkMappingStorage::Section,
                    backing.handle_slot(),
                    mapping.backing_offset,
                    0,
                ),
                _ => (kinakaze_runtime::ForkMappingStorage::Ordinary, 0, 0, 0),
            };
        kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
            base: start,
            len: mapping.length,
            behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
            storage,
            backing_slot,
            backing_offset,
            view_protection,
            domain: kinakaze_runtime::ForkMappingDomain::GuestMm,
        });
    }
}

fn unregister_fork_fragment(start: usize, mapping: &Mapping) {
    if has_fork_mapping_storage(mapping) {
        kinakaze_runtime::unregister_fork_mapping(start);
    }
}

/// Replaces a subrange of host-owned memory with Linux's inaccessible state.
///
/// Native Windows thread stacks are one reservation owned by the thread
/// runtime. HotSpot commits its guard pages inside that reservation and later
/// uncommits them with `MAP_FIXED|PROT_NONE`; attempting to reserve a new
/// placeholder at the same address can only fail. Keep ownership explicit:
/// decommit pages borrowed from a reservation, protect pages borrowed from a
/// section view, and remove only our logical fragment metadata.
fn replace_borrowed_with_prot_none(address: *mut c_void, length: usize) -> Result<bool, i32> {
    let start = address as usize;
    let end = start.checked_add(length).ok_or(EINVAL)?;
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    let Some((mapping_start, mapping)) = registry.iter().find_map(|(&mapping_start, mapping)| {
        let mapping_end = mapping_start.saturating_add(mapping.length);
        (mapping.kind.is_borrowed() && mapping_start <= start && end <= mapping_end)
            .then(|| (mapping_start, mapping.clone()))
    }) else {
        return Ok(false);
    };

    match mapping.kind {
        MappingKind::BorrowedReservation => {
            // SAFETY: the range was committed inside a live host-owned
            // reservation. MEM_DECOMMIT preserves that reservation and its owner.
            if unsafe { VirtualFree(address, length, MEM_DECOMMIT) } == 0 {
                return Err(last_errno());
            }
        }
        MappingKind::BorrowedView => {
            let mut old_protection = 0;
            // SAFETY: the range lies in the live host-owned view represented by
            // `mapping`; changing protection does not take ownership of the view.
            if unsafe {
                windows_sys::Win32::System::Memory::VirtualProtect(
                    address,
                    length,
                    PAGE_NOACCESS,
                    &mut old_protection,
                )
            } == 0
            {
                return Err(last_errno());
            }
        }
        _ => unreachable!("borrowed mapping filter returned an owned mapping"),
    }

    registry.remove(&mapping_start);
    unregister_fork_fragment(mapping_start, &mapping);
    if mapping_start < start {
        insert_mapping_fragment(
            &mut registry,
            mapping_start,
            Mapping {
                length: start - mapping_start,
                ..mapping.clone()
            },
        );
    }
    let mapping_end = mapping_start + mapping.length;
    if end < mapping_end {
        insert_mapping_fragment(
            &mut registry,
            end,
            Mapping {
                base: end as *mut c_void,
                length: mapping_end - end,
                ..mapping
            },
        );
    }
    Ok(true)
}

fn insert_mapping_fragment(registry: &mut HashMap<usize, Mapping>, start: usize, mapping: Mapping) {
    register_fork_fragment(start, &mapping);
    registry.insert(start, mapping);
}

/// Splits one placeholder until `[address, address + length)` is an exact
/// placeholder fragment. The mapping lock serializes the Windows operation with
/// the registry update, so no thread can observe half of a split.
fn carve_placeholder_locked(
    registry: &mut HashMap<usize, Mapping>,
    address: usize,
    length: usize,
) -> Result<bool, i32> {
    let target_end = address.checked_add(length).ok_or(EINVAL)?;
    let Some((start, mapping)) = registry.iter().find_map(|(&start, mapping)| {
        let end = start.saturating_add(mapping.length);
        (mapping.kind == MappingKind::Placeholder && start <= address && target_end <= end)
            .then(|| (start, mapping.clone()))
    }) else {
        return Ok(false);
    };

    if start == address && mapping.length == length {
        return Ok(true);
    }

    registry.remove(&start);
    unregister_fork_fragment(start, &mapping);

    let original_end = start + mapping.length;
    let mut fragment_start = start;
    let mut fragment = mapping.clone();

    if address > start {
        let prefix_length = address - start;
        // Splitting preserves both halves as placeholders; the size denotes the
        // first half, exactly as specified for MEM_PRESERVE_PLACEHOLDER.
        if unsafe {
            VirtualFree(
                start as *mut c_void,
                prefix_length,
                MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
            )
        } == 0
        {
            insert_mapping_fragment(registry, start, mapping);
            return Err(last_errno());
        }

        insert_mapping_fragment(
            registry,
            start,
            Mapping {
                base: start as *mut c_void,
                length: prefix_length,
                ..mapping.clone()
            },
        );
        fragment_start = address;
        fragment = Mapping {
            base: address as *mut c_void,
            length: original_end - address,
            ..mapping
        };
        insert_mapping_fragment(registry, fragment_start, fragment.clone());
    } else {
        insert_mapping_fragment(registry, fragment_start, fragment.clone());
    }

    if target_end < original_end {
        registry.remove(&fragment_start);
        unregister_fork_fragment(fragment_start, &fragment);
        if unsafe {
            VirtualFree(
                fragment_start as *mut c_void,
                length,
                MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
            )
        } == 0
        {
            insert_mapping_fragment(registry, fragment_start, fragment);
            return Err(last_errno());
        }

        insert_mapping_fragment(
            registry,
            fragment_start,
            Mapping {
                base: fragment_start as *mut c_void,
                length,
                ..fragment.clone()
            },
        );
        insert_mapping_fragment(
            registry,
            target_end,
            Mapping {
                base: target_end as *mut c_void,
                length: original_end - target_end,
                ..fragment
            },
        );
    }

    Ok(true)
}

fn carve_placeholder(address: *mut c_void, length: usize) -> Result<bool, i32> {
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    carve_placeholder_locked(&mut registry, address as usize, length)
}

#[derive(Clone, Copy)]
struct ProtectionRun {
    start: usize,
    length: usize,
    protection: u32,
}

struct PreviousView {
    start: usize,
    backing_offset: u64,
    backing: BackingRef,
    runs: Vec<ProtectionRun>,
    verity: Option<verity::Info>,
    native_inode: Option<verity::NativeId>,
    file: Option<FileMappingInfo>,
    file_origin: Option<file_origin::Info>,
}

struct PreparedReplacement {
    previous: Option<PreviousView>,
}

mod cow_materialize;

fn protection_runs(start: usize, length: usize, fallback: u32) -> Result<Vec<ProtectionRun>, i32> {
    let end = start.checked_add(length).ok_or(EINVAL)?;
    let mut runs = Vec::<ProtectionRun>::new();
    let mut cursor = start;
    while cursor < end {
        let mut info = MemoryBasicInformation {
            base_address: ptr::null_mut(),
            allocation_base: ptr::null_mut(),
            allocation_protect: 0,
            partition_id: 0,
            _pad: 0,
            region_size: 0,
            state: 0,
            protect: 0,
            type_: 0,
        };
        if unsafe {
            VirtualQuery(
                cursor as *const c_void,
                &mut info,
                size_of::<MemoryBasicInformation>(),
            )
        } == 0
        {
            return Err(last_errno());
        }
        // VirtualQuery reports RegionSize from BaseAddress, not from the queried
        // address.  A query in the middle of a region must therefore advance to
        // BaseAddress + RegionSize; adding RegionSize to `cursor` walks past the
        // region and can accidentally include an unrelated following mapping.
        let run_end = (info.base_address as usize)
            .checked_add(info.region_size)
            .map(|value| value.min(end))
            .ok_or(EINVAL)?;
        if run_end <= cursor {
            return Err(EIO);
        }
        let protection = if info.protect == 0 {
            fallback
        } else {
            info.protect
        };
        if let Some(previous) = runs.last_mut()
            && previous.start + previous.length == cursor
            && previous.protection == protection
        {
            previous.length += run_end - cursor;
        } else {
            runs.push(ProtectionRun {
                start: cursor,
                length: run_end - cursor,
                protection,
            });
        }
        cursor = run_end;
    }
    Ok(runs)
}

fn map_retained_fragment_locked(
    registry: &mut HashMap<usize, Mapping>,
    backing: &BackingRef,
    address: usize,
    length: usize,
    offset: u64,
    protection: u32,
    verity: Option<verity::Info>,
    native_inode: Option<verity::NativeId>,
    file: Option<FileMappingInfo>,
    file_origin: Option<file_origin::Info>,
) -> Result<(), i32> {
    if !carve_placeholder_locked(registry, address, length)? {
        return Err(EIO);
    }
    let view = unsafe {
        windows_sys::Win32::System::Memory::MapViewOfFile3(
            backing.handle(),
            GetCurrentProcess(),
            address as *mut c_void,
            offset,
            length,
            MEM_REPLACE_PLACEHOLDER,
            protection,
            ptr::null_mut(),
            0,
        )
    };
    if view.Value.is_null() {
        return Err(last_errno());
    }
    let placeholder = registry.remove(&address).ok_or(EIO)?;
    unregister_fork_fragment(address, &placeholder);
    insert_mapping_fragment(
        registry,
        address,
        Mapping {
            base: address as *mut c_void,
            length,
            kind: MappingKind::PlaceholderView,
            shared: false,
            backing: Some(backing.clone()),
            backing_offset: offset,
            view_protection: protection,
            file,
            verity,
            native_inode,
            file_origin,
        },
    );
    Ok(())
}

/// Makes the replacement range an exact placeholder. If it lies in one of our
/// retained section views, the untouched sides are remapped from the same
/// backing and therefore keep both their bytes and their virtual addresses.
fn prepare_placeholder_locked(
    registry: &mut HashMap<usize, Mapping>,
    address: usize,
    length: usize,
) -> Result<Option<PreparedReplacement>, i32> {
    if carve_placeholder_locked(registry, address, length)? {
        return Ok(Some(PreparedReplacement { previous: None }));
    }

    let target_end = address.checked_add(length).ok_or(EINVAL)?;
    if let Some((start, mapping)) = registry.iter().find_map(|(&start, mapping)| {
        (mapping.kind == MappingKind::PlaceholderView
            && mapping.backing.as_ref().is_some_and(BackingRef::is_cow)
            && start <= address
            && target_end <= start.saturating_add(mapping.length))
        .then(|| (start, mapping.clone()))
    }) {
        cow_materialize::make_remappable(registry, start, mapping)?;
    }
    let Some((start, mapping)) = registry.iter().find_map(|(&start, mapping)| {
        let end = start.saturating_add(mapping.length);
        (mapping.kind == MappingKind::PlaceholderView
            && mapping
                .backing
                .as_ref()
                .is_some_and(BackingRef::is_remappable)
            && start <= address
            && target_end <= end)
            .then(|| (start, mapping.clone()))
    }) else {
        return Ok(None);
    };
    let end = start.checked_add(mapping.length).ok_or(EINVAL)?;
    let backing = mapping.backing.as_ref().ok_or(EIO)?.clone();
    let runs = protection_runs(start, mapping.length, mapping.view_protection)?;

    if unsafe {
        windows_sys::Win32::System::Memory::UnmapViewOfFile2(
            GetCurrentProcess(),
            windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                Value: mapping.base,
            },
            MEM_PRESERVE_PLACEHOLDER,
        )
    } == 0
    {
        return Err(last_errno());
    }

    registry.remove(&start);
    unregister_fork_fragment(start, &mapping);
    insert_mapping_fragment(
        registry,
        start,
        Mapping {
            base: start as *mut c_void,
            length: mapping.length,
            kind: MappingKind::Placeholder,
            shared: false,
            backing: None,
            backing_offset: 0,
            view_protection: PAGE_NOACCESS,
            file: mapping.file,
            verity: mapping.verity.clone(),
            native_inode: mapping.native_inode,
            file_origin: mapping.file_origin.clone(),
        },
    );
    if !carve_placeholder_locked(registry, address, length)? {
        return Err(EIO);
    }

    let mut previous_runs = Vec::new();
    for run in runs {
        let run_end = run.start + run.length;
        let previous_start = run.start.max(address);
        let previous_end = run_end.min(target_end);
        if previous_start < previous_end {
            previous_runs.push(ProtectionRun {
                start: previous_start,
                length: previous_end - previous_start,
                protection: run.protection,
            });
        }
        for (fragment_start, fragment_end) in [
            (run.start, run_end.min(address)),
            (run.start.max(target_end), run_end),
        ] {
            if fragment_start >= fragment_end || fragment_start < start || fragment_end > end {
                continue;
            }
            let delta = fragment_start - start;
            map_retained_fragment_locked(
                registry,
                &backing,
                fragment_start,
                fragment_end - fragment_start,
                mapping.backing_offset + delta as u64,
                run.protection,
                mapping.verity.clone(),
                mapping.native_inode,
                mapping.file,
                mapping.file_origin.clone(),
            )?;
        }
    }

    Ok(Some(PreparedReplacement {
        previous: Some(PreviousView {
            start,
            backing_offset: mapping.backing_offset,
            backing,
            runs: previous_runs,
            verity: mapping.verity,
            native_inode: mapping.native_inode,
            file: mapping.file,
            file_origin: mapping.file_origin,
        }),
    }))
}

fn restore_previous_locked(
    registry: &mut HashMap<usize, Mapping>,
    previous: PreviousView,
) -> Result<(), i32> {
    for run in previous.runs {
        map_retained_fragment_locked(
            registry,
            &previous.backing,
            run.start,
            run.length,
            previous.backing_offset + (run.start - previous.start) as u64,
            run.protection,
            previous.verity.clone(),
            previous.native_inode,
            previous.file,
            previous.file_origin.clone(),
        )?;
    }
    Ok(())
}

/// Replaces an existing private anonymous subrange without disturbing its
/// reservation or any neighbouring pages.
///
/// Windows cannot split a committed private reservation into placeholders, but
/// it does not need to for anonymous `MAP_FIXED`: changing the requested pages
/// to writable, zeroing them, and applying the new protection gives the range a
/// fresh anonymous identity while keeping the allocation stable.  In
/// particular, this avoids the whole-view unmap/remap window that section views
/// would otherwise impose on concurrent readers of adjacent pages.
fn replace_private_mapping_in_place_locked(
    registry: &mut HashMap<usize, Mapping>,
    address: *mut c_void,
    length: usize,
    protection: u32,
    source: Option<(*const u8, usize)>,
) -> Result<Option<*mut c_void>, i32> {
    let start = address as usize;
    let end = start.checked_add(length).ok_or(EINVAL)?;
    if source.is_some_and(|(_, source_length)| source_length > length) {
        return Err(EINVAL);
    }
    let Some((mapping_start, mapping)) = registry.iter().find_map(|(&mapping_start, mapping)| {
        let mapping_end = mapping_start.saturating_add(mapping.length);
        (!mapping.shared
            && matches!(
                mapping.kind,
                MappingKind::Reserved | MappingKind::PlaceholderPrivate
            )
            && mapping_start <= start
            && end <= mapping_end)
            .then(|| (mapping_start, mapping.clone()))
    }) else {
        return Ok(None);
    };

    let mut old_protection = 0;
    if unsafe {
        windows_sys::Win32::System::Memory::VirtualProtect(
            address,
            length,
            PAGE_READWRITE,
            &mut old_protection,
        )
    } == 0
    {
        return Err(last_errno());
    }
    // SAFETY: the exact target is committed and temporarily writable. Linux
    // specifies zero-filled contents for a fresh anonymous MAP_FIXED mapping.
    unsafe { ptr::write_bytes(address.cast::<u8>(), 0, length) };
    if let Some((source, source_length)) = source {
        // SAFETY: the caller keeps a distinct read-only section view alive for
        // `source_length`, while the target was made writable above.
        unsafe { ptr::copy_nonoverlapping(source, address.cast::<u8>(), source_length) };
    }
    let mut ignored = 0;
    if unsafe {
        windows_sys::Win32::System::Memory::VirtualProtect(
            address,
            length,
            protection,
            &mut ignored,
        )
    } == 0
    {
        let error = last_errno();
        let mut restore_ignored = 0;
        unsafe {
            windows_sys::Win32::System::Memory::VirtualProtect(
                address,
                length,
                old_protection,
                &mut restore_ignored,
            )
        };
        return Err(error);
    }

    registry.remove(&mapping_start);
    unregister_fork_fragment(mapping_start, &mapping);
    if mapping_start < start {
        insert_mapping_fragment(
            registry,
            mapping_start,
            Mapping {
                length: start - mapping_start,
                ..mapping.clone()
            },
        );
    }
    insert_mapping_fragment(
        registry,
        start,
        Mapping {
            base: mapping.base,
            length,
            kind: mapping.kind,
            shared: false,
            backing: None,
            backing_offset: 0,
            view_protection: protection,
            file: None,
            verity: None,
            native_inode: None,
            file_origin: None,
        },
    );
    let mapping_end = mapping_start + mapping.length;
    if end < mapping_end {
        insert_mapping_fragment(
            registry,
            end,
            Mapping {
                length: mapping_end - end,
                ..mapping
            },
        );
    }
    Ok(Some(address))
}

/// Gives an anonymous page-file view fresh MAP_FIXED contents without tearing
/// down its surrounding view. The section is private, has no file identity, and
/// is retained only so page-granular placeholder layouts survive fork; zeroing
/// the requested pages therefore has the same guest-visible identity as a new
/// anonymous mapping while keeping neighbouring pages continuously mapped.
fn replace_anonymous_view_in_place_locked(
    registry: &HashMap<usize, Mapping>,
    address: *mut c_void,
    length: usize,
    protection: u32,
) -> Result<Option<*mut c_void>, i32> {
    let start = address as usize;
    let end = start.checked_add(length).ok_or(EINVAL)?;
    let found = registry.iter().any(|(&mapping_start, mapping)| {
        let mapping_end = mapping_start.saturating_add(mapping.length);
        mapping.kind == MappingKind::PlaceholderView
            && !mapping.shared
            && mapping.file.is_none()
            && mapping.verity.is_none()
            && mapping.native_inode.is_none()
            && mapping.file_origin.is_none()
            && mapping
                .backing
                .as_ref()
                .is_some_and(|backing| backing.is_remappable() || backing.is_snapshot())
            && mapping_start <= start
            && end <= mapping_end
    });
    if !found {
        return Ok(None);
    }

    let mut old_protection = 0;
    if unsafe {
        kinakaze_runtime::memory_protection::protect_preserving_copy_on_write(
            address,
            length,
            PAGE_READWRITE,
            &mut old_protection,
        )
    } == 0
    {
        return Err(last_errno());
    }
    unsafe { ptr::write_bytes(address.cast::<u8>(), 0, length) };
    let mut ignored = 0;
    if unsafe {
        kinakaze_runtime::memory_protection::protect_preserving_copy_on_write(
            address,
            length,
            protection,
            &mut ignored,
        )
    } == 0
    {
        let error = last_errno();
        let mut restore_ignored = 0;
        unsafe {
            kinakaze_runtime::memory_protection::protect_preserving_copy_on_write(
                address,
                length,
                old_protection,
                &mut restore_ignored,
            )
        };
        return Err(error);
    }
    // Keep the backing reference and the enclosing view in the registry. Their
    // lifetime, fork copy, and partial-munmap machinery remain authoritative;
    // the actual per-page protection is read from VirtualQuery when needed.
    Ok(Some(address))
}

/// Allocators recycle spans that include both committed subviews and untouched
/// guard placeholders. Validate complete ownership before changing any pages,
/// then discard each committed intersection without unmapping its neighbours.
fn replace_fragmented_anonymous_with_prot_none_locked(
    registry: &mut HashMap<usize, Mapping>,
    address: *mut c_void,
    length: usize,
) -> Result<bool, i32> {
    let start = address as usize;
    let end = start.checked_add(length).ok_or(EINVAL)?;
    let mut fragments: Vec<_> = registry
        .iter()
        .filter_map(|(&base, mapping)| {
            let limit = base.checked_add(mapping.length)?;
            (base < end && start < limit)
                .then(|| (base.max(start), limit.min(end), mapping.clone()))
        })
        .collect();
    fragments.sort_unstable_by_key(|fragment| fragment.0);
    let mut cursor = start;
    for (from, to, mapping) in &fragments {
        let private = !mapping.shared
            && mapping.file.is_none()
            && mapping.verity.is_none()
            && mapping.native_inode.is_none()
            && mapping.file_origin.is_none();
        let supported = matches!(
            mapping.kind,
            MappingKind::Placeholder | MappingKind::Reserved | MappingKind::PlaceholderPrivate
        ) || (mapping.kind == MappingKind::PlaceholderView
            && mapping
                .backing
                .as_ref()
                .is_some_and(|backing| backing.is_remappable() || backing.is_snapshot()));
        if *from != cursor || !private || !supported {
            return Ok(false);
        }
        cursor = *to;
    }
    if cursor != end {
        return Ok(false);
    }
    for (from, to, mapping) in fragments {
        let address = from as *mut c_void;
        let length = to - from;
        match mapping.kind {
            // Already inaccessible and logically zero; retain the reservation.
            MappingKind::Placeholder => {}
            MappingKind::Reserved | MappingKind::PlaceholderPrivate => {
                replace_private_mapping_in_place_locked(
                    registry,
                    address,
                    length,
                    PAGE_NOACCESS,
                    None,
                )?
                .ok_or(EIO)?;
            }
            MappingKind::PlaceholderView => {
                replace_anonymous_view_in_place_locked(registry, address, length, PAGE_NOACCESS)?
                    .ok_or(EIO)?;
            }
            _ => unreachable!(),
        }
    }
    Ok(true)
}

/// MAP_FIXED can cross several private Windows views. Validate ownership of
/// the entire range before replacing individual intersections under one lock.
fn replace_fragmented_private_anonymous_locked(
    registry: &mut HashMap<usize, Mapping>,
    address: *mut c_void,
    length: usize,
    protection: u32,
) -> Result<bool, i32> {
    let start = address as usize;
    let end = start.checked_add(length).ok_or(EINVAL)?;
    let mut fragments: Vec<_> = registry
        .iter()
        .filter_map(|(&base, mapping)| {
            let limit = base.checked_add(mapping.length)?;
            (base < end && start < limit)
                .then(|| (base.max(start), limit.min(end), mapping.clone()))
        })
        .collect();
    if fragments.len() < 2 {
        return Ok(false);
    }
    fragments.sort_unstable_by_key(|fragment| fragment.0);
    let mut cursor = start;
    for (from, to, mapping) in &fragments {
        let private = !mapping.shared
            && mapping.file.is_none()
            && mapping.verity.is_none()
            && mapping.native_inode.is_none()
            && mapping.file_origin.is_none();
        let supported = matches!(
            mapping.kind,
            MappingKind::Placeholder | MappingKind::Reserved | MappingKind::PlaceholderPrivate
        ) || (mapping.kind == MappingKind::PlaceholderView
            && mapping
                .backing
                .as_ref()
                .is_some_and(|backing| backing.is_remappable() || backing.is_snapshot()));
        if *from != cursor || !private || !supported {
            return Ok(false);
        }
        cursor = *to;
    }
    if cursor != end {
        return Ok(false);
    }
    for (from, to, mapping) in fragments {
        if protection == PAGE_NOACCESS && mapping.kind == MappingKind::Placeholder {
            continue;
        }
        replace_placeholder_private_locked(registry, from as *mut c_void, to - from, protection)?
            .ok_or(EIO)?;
    }
    Ok(true)
}

/// Replaces an exact placeholder fragment with a zero-filled private section.
///
/// `VirtualAlloc2(MEM_REPLACE_PLACEHOLDER)` still requires a non-null base to
/// be allocation-granularity aligned. Linux mappings are only page aligned, so
/// using it for an interior 4 KiB fragment creates address layouts that work in
/// the parent only by accident and cannot be reproduced by `fork`. A private
/// pagefile section has the same anonymous, zero-filled semantics and
/// `MapViewOfFile3(MEM_REPLACE_PLACEHOLDER)` explicitly permits page-granular
/// bases. Retaining its backing also lets later MAP_FIXED splits and fork copy
/// the fragment without moving neighbouring pages.
fn replace_placeholder_private(
    address: *mut c_void,
    length: usize,
    protection: u32,
) -> Result<Option<*mut c_void>, i32> {
    // Bound the dirty-preservation cost of a later partial native unmap. Large
    // untouched reservations remain one placeholder; committed anonymous views
    // are installed in chunks only when the requested span is still reserved.
    // 16 MiB amortizes native view/handle overhead without a whole-heap copy for
    // a page-granular MAP_FIXED or munmap. Existing data is never split here.
    const SNAPSHOT_VIEW_BYTES: usize = 16 * 1024 * 1024;
    if length > SNAPSHOT_VIEW_BYTES {
        let start = address as usize;
        let end = start.checked_add(length).ok_or(EINVAL)?;
        let placeholder = mappings()
            .lock()
            .map_err(|_| EIO)?
            .iter()
            .any(|(&base, mapping)| {
                mapping.kind == MappingKind::Placeholder
                    && base <= start
                    && base
                        .checked_add(mapping.length)
                        .is_some_and(|limit| end <= limit)
            });
        if placeholder {
            let mut offset = 0;
            while offset < length {
                let bytes = (length - offset).min(SNAPSHOT_VIEW_BYTES);
                if replace_placeholder_private(
                    unsafe { address.byte_add(offset) },
                    bytes,
                    protection,
                )?
                .is_none()
                {
                    return Err(ENOMEM);
                }
                offset += bytes;
            }
            return Ok(Some(address));
        }
    }
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    if replace_fragmented_private_anonymous_locked(&mut registry, address, length, protection)? {
        return Ok(Some(address));
    }
    replace_placeholder_private_locked(&mut registry, address, length, protection)
}

fn replace_placeholder_private_locked(
    registry: &mut HashMap<usize, Mapping>,
    address: *mut c_void,
    length: usize,
    protection: u32,
) -> Result<Option<*mut c_void>, i32> {
    if let Some(mapped) =
        replace_private_mapping_in_place_locked(registry, address, length, protection, None)?
    {
        return Ok(Some(mapped));
    }
    if let Some(mapped) =
        replace_anonymous_view_in_place_locked(registry, address, length, protection)?
    {
        return Ok(Some(mapped));
    }
    let section = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_EXECUTE_READWRITE,
            (length as u64 >> 32) as u32,
            length as u32,
            ptr::null(),
        )
    };
    if section.is_null() {
        return Err(last_errno());
    }
    let backing = match BackingRef::new_snapshot(section, length) {
        Ok(backing) => backing,
        Err(error) => {
            unsafe { CloseHandle(section) };
            return Err(error);
        }
    };
    let Some(prepared) = prepare_placeholder_locked(registry, address as usize, length)? else {
        return Ok(None);
    };
    let allocated = unsafe {
        windows_sys::Win32::System::Memory::MapViewOfFile3(
            section,
            GetCurrentProcess(),
            address,
            0,
            length,
            MEM_REPLACE_PLACEHOLDER,
            PAGE_EXECUTE_READWRITE,
            ptr::null_mut(),
            0,
        )
    };
    if allocated.Value.is_null() {
        let error = last_errno();
        if let Some(previous) = prepared.previous {
            restore_previous_locked(registry, previous)?;
        }
        return Err(error);
    }
    debug_assert_eq!(allocated.Value, address);
    // Map with the anonymous allocation's maximum rights, then set the current
    // Linux permissions. A view opened RW cannot later gain execute access,
    // even when its section allows it (HotSpot uses RW -> RX for generated code).
    let mut previous_protection = 0;
    if unsafe { VirtualProtect(address, length, protection, &mut previous_protection) } == 0 {
        let error = last_errno();
        if unsafe {
            windows_sys::Win32::System::Memory::UnmapViewOfFile2(
                GetCurrentProcess(),
                allocated,
                MEM_PRESERVE_PLACEHOLDER,
            )
        } == 0
        {
            std::process::abort();
        }
        if let Some(previous) = prepared.previous {
            restore_previous_locked(registry, previous)?;
        }
        return Err(error);
    }

    let placeholder = registry.remove(&(address as usize)).ok_or(EIO)?;
    unregister_fork_fragment(address as usize, &placeholder);
    insert_mapping_fragment(
        registry,
        address as usize,
        Mapping {
            base: address,
            length,
            kind: MappingKind::PlaceholderView,
            shared: false,
            backing: Some(backing),
            backing_offset: 0,
            view_protection: protection,
            file: None,
            verity: None,
            native_inode: None,
            file_origin: None,
        },
    );
    Ok(Some(allocated.Value))
}

/// Replaces an exact placeholder fragment with a zero-copy file view.
fn replace_placeholder_view(
    section: *mut c_void,
    address: *mut c_void,
    length: usize,
    offset: u64,
    protection: u32,
    shared: bool,
    backing: Option<&BackingRef>,
) -> Result<Option<*mut c_void>, i32> {
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    let Some(prepared) = prepare_placeholder_locked(&mut registry, address as usize, length)?
    else {
        return Ok(None);
    };

    let view = unsafe {
        windows_sys::Win32::System::Memory::MapViewOfFile3(
            section,
            GetCurrentProcess(),
            address,
            offset,
            length,
            MEM_REPLACE_PLACEHOLDER,
            protection,
            ptr::null_mut(),
            0,
        )
    };
    if view.Value.is_null() {
        let map_error = last_errno();
        if let Some(previous) = prepared.previous {
            restore_previous_locked(&mut registry, previous)?;
        }
        return Err(map_error);
    }
    debug_assert_eq!(view.Value, address);

    let placeholder = registry.remove(&(address as usize)).ok_or(EIO)?;
    unregister_fork_fragment(address as usize, &placeholder);
    let mut file_origin = placeholder.file_origin;
    if placeholder.verity.is_none() {
        if let Some(origin) = &mut file_origin {
            origin.representation = file_origin::Representation::NativeSection;
        }
    }
    insert_mapping_fragment(
        &mut registry,
        address as usize,
        Mapping {
            base: address,
            length,
            kind: MappingKind::PlaceholderView,
            shared,
            backing: backing.cloned(),
            backing_offset: offset,
            view_protection: protection,
            file: placeholder.file,
            verity: placeholder.verity,
            native_inode: placeholder.native_inode,
            file_origin,
        },
    );
    Ok(Some(view.Value))
}

/// Commits every pure-placeholder fragment touched by `mprotect`. This must run
/// before `VirtualProtect`, because placeholders are reservations but cannot be
/// committed by the older `VirtualAlloc(MEM_COMMIT)` operation.
fn commit_placeholder_pages(
    address: *mut c_void,
    length: usize,
    protection: u32,
) -> Result<(), i32> {
    let request_start = address as usize;
    let request_end = request_start.checked_add(length).ok_or(EINVAL)?;
    loop {
        let candidate = {
            let registry = mappings().lock().map_err(|_| EIO)?;
            registry.iter().find_map(|(&start, mapping)| {
                if mapping.kind != MappingKind::Placeholder {
                    return None;
                }
                let end = start.saturating_add(mapping.length);
                let from = start.max(request_start);
                let to = end.min(request_end);
                (from < to).then(|| (from, to - from))
            })
        };
        let Some((from, fragment_length)) = candidate else {
            return Ok(());
        };
        if replace_placeholder_private(from as *mut c_void, fragment_length, protection)?.is_none()
        {
            return Err(EIO);
        }
    }
}

/// Maps anonymous memory.
fn map_anonymous(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    shared: bool,
    fixed: bool,
) -> Result<*mut c_void, i32> {
    let requested = if fixed { address } else { ptr::null_mut() };
    let length = page_rounded_length(length)?;

    if shared {
        // MAP_SHARED|MAP_ANONYMOUS must stay visible across a fork, and
        // `VirtualAlloc` memory does not: the child gets its own private copy. A
        // page-file-backed section does, provided its handle is retained and
        // registered for duplication into each independent fork child.
        let current_protect = page_protection(protection, false)?;
        let maximum = PROT_READ | PROT_WRITE | (protection & PROT_EXEC);
        let protect = section_protection(maximum, false);
        // SAFETY: INVALID_HANDLE_VALUE requests page-file backing, which is the
        // documented way to create an anonymous section.
        let section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                protect,
                (length as u64 >> 32) as u32,
                length as u32,
                ptr::null(),
            )
        };
        if section.is_null() {
            return Err(last_errno());
        }
        let backing = match BackingRef::new_retained_shared(section, length, protect) {
            Ok(backing) => backing,
            Err(error) => {
                unsafe { CloseHandle(section) };
                return Err(error);
            }
        };
        // SAFETY: the section was just created and is large enough for the view.
        let view = unsafe {
            MapViewOfFileEx(
                section,
                view_access(maximum, false),
                0,
                0,
                length,
                requested,
            )
        };
        if view.is_null() {
            return Err(last_errno());
        }
        let mut previous = 0;
        if current_protect != protect
            && unsafe { VirtualProtect(view, length, current_protect, &mut previous) } == 0
        {
            let error = last_errno();
            unsafe { UnmapViewOfFile(view) };
            return Err(error);
        }
        let mut registry = match mappings().lock() {
            Ok(registry) => registry,
            Err(_) => {
                unsafe { UnmapViewOfFile(view) };
                return Err(EIO);
            }
        };
        insert_mapping_fragment(
            &mut registry,
            view as usize,
            Mapping {
                base: view,
                length,
                kind: MappingKind::View,
                shared: true,
                backing: Some(backing),
                backing_offset: 0,
                view_protection: protect,
                file: None,
                verity: None,
                native_inode: None,
                file_origin: None,
            },
        );
        return Ok(view);
    }

    // A private PROT_NONE mapping is a placeholder reservation. Besides avoiding
    // commit charge, placeholders are deliberately replaceable: HotSpot reserves
    // its CDS address window first, then overlays anonymous and file-backed
    // pieces with MAP_FIXED at 4 KiB boundaries. Ordinary VirtualAlloc
    // reservations cannot support that operation.
    if protection == PROT_NONE {
        if fixed && replace_borrowed_with_prot_none(address, length)? {
            return Ok(address);
        }
        if fixed {
            // JIT allocators recycle committed pages with MAP_FIXED|PROT_NONE.
            // They are already occupied by our private reservation/view, so
            // reserving another placeholder at that address cannot succeed.
            // Reuse the existing anonymous replacement paths to discard the
            // exact range and protect it, preserving adjacent live JIT code.
            let mut registry = mappings().lock().map_err(|_| EIO)?;
            if replace_fragmented_anonymous_with_prot_none_locked(&mut registry, address, length)? {
                return Ok(address);
            }
            if replace_private_mapping_in_place_locked(
                &mut registry,
                address,
                length,
                PAGE_NOACCESS,
                None,
            )?
            .is_some()
                || replace_anonymous_view_in_place_locked(
                    &registry,
                    address,
                    length,
                    PAGE_NOACCESS,
                )?
                .is_some()
            {
                return Ok(address);
            }
        }
        if fixed && carve_placeholder(address, length)? {
            return Ok(address);
        }

        let allocated = unsafe {
            if fixed {
                reserve_placeholder_at(requested, length)
            } else if length >= 4 * 1024 * 1024 * 1024 {
                let mut placed = virtual_alloc2_aligned(
                    address,
                    length,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    4 * 1024 * 1024 * 1024,
                );
                if placed.is_null() {
                    placed = virtual_alloc2_aligned(
                        ptr::null_mut(),
                        length,
                        MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                        PAGE_NOACCESS,
                        4 * 1024 * 1024 * 1024,
                    );
                }
                placed
            } else {
                let mut placed = reserve_placeholder_at(address, length);
                if placed.is_null() && !address.is_null() {
                    placed = reserve_placeholder_at(ptr::null_mut(), length);
                }
                placed
            }
        };
        if allocated.is_null() {
            return Err(last_errno());
        }
        record_mapping(
            allocated,
            allocated,
            length,
            MappingKind::Placeholder,
            false,
            None,
        );
        return Ok(allocated);
    }

    // MAP_PRIVATE|MAP_ANONYMOUS: the committed allocation glibc's malloc and
    // every stack-allocating library asks for.
    let protect = page_protection(protection, false)?;
    if fixed {
        if let Some(allocated) = replace_placeholder_private(address, length, protect)? {
            return Ok(allocated);
        }
    }
    if !fixed {
        // Keep PROT_NONE reservations lazy. Only the requested committed view
        // acquires pagefile backing, whose initialized bytes can be frozen at
        // the first fork without a bulk copy.
        let reserved = map_anonymous(address, length, PROT_NONE, false, false)?;
        match replace_placeholder_private(reserved, length, protect) {
            Ok(Some(mapped)) => return Ok(mapped),
            result => {
                unmap(reserved, length)?;
                return Err(result.err().unwrap_or(ENOMEM));
            }
        }
    }
    let alloc_type = MEM_COMMIT | MEM_RESERVE;
    // SAFETY: a null address lets the system choose; a non-null one is the
    // caller's MAP_FIXED request, which VirtualAlloc rounds down internally.
    let (allocated, kind) = unsafe {
        if !requested.is_null() {
            let mut kind = MappingKind::Reserved;
            let mut p = VirtualAlloc(requested, length, alloc_type, protect);
            if p.is_null() {
                if alloc_type & MEM_COMMIT != 0 {
                    p = VirtualAlloc(requested, length, MEM_COMMIT, protect);
                    if !p.is_null() {
                        kind = MappingKind::BorrowedReservation;
                    }
                }
                if p.is_null() {
                    let mut old_protect = 0;
                    if windows_sys::Win32::System::Memory::VirtualProtect(
                        requested,
                        length,
                        protect,
                        &mut old_protect,
                    ) != 0
                    {
                        p = requested;
                        kind = MappingKind::BorrowedView;
                    }
                }
            }
            (p, kind)
        } else if !address.is_null() {
            // Non-fixed placement hint from caller (e.g. V8 4GB cage or 2MB page hint).
            let mut p = core::ptr::null_mut();
            if length >= 4 * 1024 * 1024 * 1024
                || (address as usize) % (4 * 1024 * 1024 * 1024) == 0
            {
                p = virtual_alloc2_aligned(
                    address,
                    length,
                    alloc_type,
                    protect,
                    4 * 1024 * 1024 * 1024,
                );
            }
            if p.is_null() {
                p = VirtualAlloc(address, length, alloc_type, protect);
            }
            if p.is_null() {
                if length >= 4 * 1024 * 1024 * 1024 {
                    p = virtual_alloc2_aligned(
                        core::ptr::null_mut(),
                        length,
                        alloc_type,
                        protect,
                        4 * 1024 * 1024 * 1024,
                    );
                }
                if p.is_null() {
                    p = VirtualAlloc(ptr::null_mut(), length, alloc_type, protect);
                }
            }
            (p, MappingKind::Reserved)
        } else {
            let mut p = core::ptr::null_mut();
            if length >= 4 * 1024 * 1024 * 1024 {
                p = virtual_alloc2_aligned(
                    core::ptr::null_mut(),
                    length,
                    alloc_type,
                    protect,
                    4 * 1024 * 1024 * 1024,
                );
            }
            if p.is_null() {
                p = VirtualAlloc(ptr::null_mut(), length, alloc_type, protect);
            }
            (p, MappingKind::Reserved)
        }
    };
    if allocated.is_null() {
        return Err(last_errno());
    }
    record_mapping(allocated, allocated, length, kind, false, None);
    Ok(allocated)
}

/// Maps a file-backed region.
///
/// The granularity gap is handled here. Linux accepts any page-aligned offset;
/// Windows insists a view begin on a 64 KiB boundary. So the view is created from
/// the granularity floor below the requested offset, extended by the distance
/// skipped, and the pointer returned to the guest is that many bytes into it. The
/// registry records both addresses because `UnmapViewOfFile` will only accept the
/// view's own base.
#[allow(clippy::too_many_arguments)]
fn map_file(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    shared: bool,
    fixed: bool,
    fd: c_int,
    offset: u64,
    granularity: usize,
) -> Result<*mut c_void, i32> {
    let opened = fs::verity::Opened::from_fd(fd).map_err(|error| {
        if error == kinakaze_vfs::ENOTTY {
            ENODEV
        } else {
            error
        }
    })?;
    let result = map_opened_file(
        address,
        length,
        protection,
        shared,
        fixed,
        fd,
        &opened,
        offset,
        granularity,
    );
    if result.is_ok() {
        opened.accessed();
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn map_opened_file(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    shared: bool,
    fixed: bool,
    fd: c_int,
    opened: &fs::verity::Opened,
    offset: u64,
    granularity: usize,
) -> Result<*mut c_void, i32> {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    if opened.is_directory() {
        return Err(ENODEV);
    }
    let handle = opened.borrowed_handle();
    // Serialize section creation with verity publication, including its first
    // no-metadata check. No native section may capture a growing metadata tail.
    let _inode = fs::verity::lock(handle)?;
    let owner = Some(file_origin::FileOriginRef::new(
        opened, length, offset, shared, protection,
    )?);
    let verity_size = fs::verity::logical_size(handle)?;
    let inode = verity::native_id(handle)?;
    let mapped = if let Some(size) = verity_size {
        if !handle_access(handle).is_some_and(|access| access & FILE_READ_DATA != 0) {
            return Err(kinakaze_vfs::EACCES);
        }
        verity::snapshot(
            address, length, protection, shared, fixed, handle, offset, size,
        )?
    } else {
        map_native_file(
            address,
            length,
            protection,
            shared,
            fixed,
            fd,
            opened,
            offset,
            granularity,
        )?
    };
    let end = (mapped as usize)
        .checked_add(page_rounded_length(length)?)
        .ok_or(EINVAL)?;
    let mut registry = mappings().lock().map_err(|_| EIO)?;
    if let Some(owner) = &owner {
        owner.bind(mapped as usize)?;
    }
    for (&start, mapping) in registry.iter_mut() {
        if start < end && (mapped as usize) < start.saturating_add(mapping.length) {
            if verity_size.is_none() {
                mapping.native_inode = Some(inode);
            }
            mapping.file_origin = owner.as_ref().map(|owner| file_origin::Info {
                owner: owner.clone(),
                representation: if verity_size.is_some() {
                    file_origin::Representation::VerifiedCache
                } else if matches!(
                    mapping.kind,
                    MappingKind::Reserved | MappingKind::PlaceholderPrivate
                ) {
                    file_origin::Representation::CopiedUnclassified
                } else {
                    file_origin::Representation::NativeSection
                },
            });
        }
    }
    Ok(mapped)
}

#[allow(clippy::too_many_arguments)]
fn map_native_file(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    shared: bool,
    fixed: bool,
    fd: c_int,
    opened: &fs::verity::Opened,
    offset: u64,
    granularity: usize,
) -> Result<*mut c_void, i32> {
    // The caller pins and classifies this exact inode before taking its verity
    // transaction lock. Rechecking fd here could map an unrelated replacement.
    let handle = opened.borrowed_handle();

    let mut size = 0i64;
    // SAFETY: the handle is live and `size` is a writable local.
    if unsafe { GetFileSizeEx(handle, &raw mut size) } == 0 {
        return Err(last_errno());
    }
    let size = size as u64;
    if shared {
        shared_section_protection(handle, protection)?;
    }
    if shared && protection == PROT_NONE && fixed && offset < size {
        // Keep the file identity but do not create a PAGE_NOACCESS section
        // view: Windows can reject later permission restoration on that view.
        // File growth/mprotect materializes this exact inode when accessible.
        return map_anonymous(address, length, PROT_NONE, false, fixed);
    }
    // A mapping that starts at or past the end of file has nothing to map.
    // Linux permits it and faults on access; Windows will not create the view at
    // all, so this is reported rather than pretended.
    if offset >= size {
        return Err(ENODEV);
    }

    let floor = offset - (offset % granularity as u64);
    let skew = (offset - floor) as usize;
    // Windows refuses a view that extends past the end of the section, where
    // Linux allows the mapping and delivers SIGBUS on the pages beyond the file.
    // The view is therefore clamped to what the file actually holds. A caller that
    // mapped past the end sees a shorter usable region instead of a failed call,
    // and the bytes it would have faulted on are simply not there.
    let available = (size - floor) as usize;
    let view_length = (length + skew).min(available);

    let private_file = !shared;
    let protect = if shared {
        shared_section_protection(handle, protection)?
    } else {
        section_protection(protection, private_file)
    };
    let placeholder_view = if fixed {
        Some((
            page_rounded_length(length)?,
            page_rounded_length(size as usize)?,
            page_protection(protection, private_file)?,
        ))
    } else {
        None
    };
    // With the legacy path the guest's pointer lies `skew` bytes into the view.
    let requested = if fixed {
        (address as usize).checked_sub(skew).ok_or(EINVAL)? as *mut c_void
    } else {
        ptr::null_mut()
    };
    // A zero maximum size makes the section exactly as large as the file, which is
    // what is wanted: a larger request would fail on a file that cannot grow.
    // SAFETY: the handle is a live file handle and the name is null (unnamed).
    let section = unsafe { CreateFileMappingW(handle, ptr::null(), protect, 0, 0, ptr::null()) };
    if section.is_null() {
        return Err(last_errno());
    }
    let retained_shared = if shared {
        match BackingRef::new_retained_shared(section, page_rounded_length(size as usize)?, protect)
        {
            Ok(backing) => Some(backing),
            Err(error) => {
                unsafe { CloseHandle(section) };
                return Err(error);
            }
        }
    } else {
        None
    };

    // Linux creates the complete requested VMA even when its tail extends past
    // EOF; touching a page wholly beyond the file then raises SIGBUS.  A Windows
    // section view cannot extend past its section, so express that topology as
    // a file view followed by an inaccessible placeholder.  Keeping the tail
    // reserved also makes mprotect/madvise/munmap observe the original VMA
    // length instead of a silently truncated mapping.
    let guest_length = page_rounded_length(length)?;
    let backed_bytes = usize::try_from((size - offset).min(length as u64)).map_err(|_| EINVAL)?;
    let backed_length = page_rounded_length(backed_bytes)?;
    if !fixed && backed_length < guest_length {
        let view_page_protection = match page_protection(protection, private_file) {
            Ok(value) => value,
            Err(error) => {
                if retained_shared.is_none() {
                    unsafe { CloseHandle(section) };
                }
                return Err(error);
            }
        };
        let reserved = unsafe { reserve_placeholder_at(ptr::null_mut(), guest_length) };
        if reserved.is_null() {
            if retained_shared.is_none() {
                unsafe { CloseHandle(section) };
            }
            return Err(last_errno());
        }
        // The inaccessible tail is private address-space state even when the
        // file view itself is MAP_SHARED, and therefore remains part of fork's
        // mapping topology.
        record_mapping(
            reserved,
            reserved,
            guest_length,
            MappingKind::Placeholder,
            false,
            Some(FileMappingInfo {
                fd,
                generation: opened.generation(),
                protection: view_page_protection,
                shared,
                origin: reserved as usize,
                mapping_length: guest_length,
                file_offset: offset,
            }),
        );
        let result = replace_placeholder_view(
            section,
            reserved,
            backed_length,
            offset,
            view_page_protection,
            shared,
            retained_shared.as_ref(),
        );
        if retained_shared.is_none() {
            unsafe { CloseHandle(section) };
        }
        return match result {
            Ok(Some(view)) => Ok(view),
            Ok(None) => {
                let _ = unmap(reserved, guest_length);
                Err(EIO)
            }
            Err(error) => {
                let _ = unmap(reserved, guest_length);
                Err(error)
            }
        };
    }

    // A private file mapping is permitted to take a snapshot of the file: POSIX
    // deliberately leaves later visibility of file changes unspecified for
    // MAP_PRIVATE.  When MAP_FIXED targets pages in an existing private
    // reservation, copy that initial snapshot into the exact target instead of
    // tearing down the surrounding reservation.  This preserves the Linux rule
    // that neighbouring VMAs remain continuously accessible during replacement.
    if fixed && private_file {
        let snapshot = unsafe {
            MapViewOfFileEx(
                section,
                FILE_MAP_READ,
                (floor >> 32) as u32,
                floor as u32,
                view_length,
                ptr::null_mut(),
            )
        };
        if !snapshot.is_null() {
            let mapped_length = page_rounded_length(length)?;
            let source_length = length.min(view_length.saturating_sub(skew));
            let source = unsafe { snapshot.cast::<u8>().add(skew) };
            let replaced = {
                let mut registry = mappings().lock().map_err(|_| EIO)?;
                replace_private_mapping_in_place_locked(
                    &mut registry,
                    address,
                    mapped_length,
                    // The snapshot now lives in anonymous private pages, so
                    // ordinary private-page protections apply; WRITECOPY is a
                    // section-view-only Windows protection.
                    page_protection(protection, false)?,
                    Some((source, source_length)),
                )
            };
            unsafe { UnmapViewOfFile(snapshot) };
            match replaced {
                Ok(Some(mapped)) => {
                    unsafe { CloseHandle(section) };
                    return Ok(mapped);
                }
                Ok(None) => {}
                Err(error) => {
                    unsafe { CloseHandle(section) };
                    return Err(error);
                }
            }
        }
    }
    let retained = if let Some(backing) = retained_shared {
        Some(backing)
    } else if !shared {
        match BackingRef::new_cow(section, page_rounded_length(size as usize)?, protect) {
            Ok(backing) => Some(backing),
            Err(error) => {
                unsafe { CloseHandle(section) };
                return Err(error);
            }
        }
    } else {
        None
    };

    // A MAP_FIXED view inside one of our placeholder reservations is the exact
    // operation MapViewOfFile3 was designed for. In replacement mode both the
    // base and the file offset need only page (4 KiB) alignment, so unlike the
    // MapViewOfFileEx path below there is no 64 KiB skew and no copied handoff.
    if let Some((placeholder_length, section_length, view_protection)) = placeholder_view {
        if (offset as usize)
            .checked_add(placeholder_length)
            .is_some_and(|end| end <= section_length)
        {
            match replace_placeholder_view(
                section,
                address,
                placeholder_length,
                offset,
                view_protection,
                shared,
                retained.as_ref(),
            ) {
                Ok(Some(view)) => {
                    if retained.is_none() {
                        unsafe { CloseHandle(section) };
                    }
                    return Ok(view);
                }
                Ok(None) => {}
                Err(error) => {
                    if retained.is_none() {
                        unsafe { CloseHandle(section) };
                    }
                    return Err(error);
                }
            }
        }
    }
    // SAFETY: the section is live and the offset is granularity-aligned, which is
    // MapViewOfFileEx's documented requirement.
    let view = unsafe {
        MapViewOfFileEx(
            section,
            if shared && !fixed {
                // FILE_MAP_READ alone permanently removes VM_MAYWRITE from
                // the Windows view, even when the open file is writable.
                FILE_MAP_READ
                    | if matches!(protect, PAGE_READWRITE | PAGE_EXECUTE_READWRITE) {
                        FILE_MAP_WRITE
                    } else {
                        0
                    }
                    | if matches!(protect, PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE) {
                        FILE_MAP_EXECUTE
                    } else {
                        0
                    }
            } else {
                view_access(protection, private_file)
            },
            (floor >> 32) as u32,
            floor as u32,
            view_length,
            requested,
        )
    };
    // SAFETY: this function owns the section handle; the view keeps its own
    // reference to the underlying object.
    if retained.is_none() {
        unsafe { CloseHandle(section) };
    }
    if view.is_null() {
        return Err(last_errno());
    }

    // SAFETY: the view spans `view_length` bytes and `skew` is below it.
    let user = unsafe { view.cast::<u8>().add(skew) }.cast::<c_void>();
    if shared || protection == PROT_NONE {
        let mut previous = 0;
        if unsafe {
            VirtualProtect(
                user,
                page_rounded_length(view_length - skew)?,
                page_protection(protection, private_file)?,
                &mut previous,
            )
        } == 0
        {
            let error = last_errno();
            unsafe { UnmapViewOfFile(view) };
            return Err(error);
        }
    }
    record_mapping(
        user,
        view,
        page_rounded_length(view_length - skew)?,
        MappingKind::View,
        shared,
        None,
    );
    if let Some(backing) = retained.filter(BackingRef::is_retained) {
        let mut registry = mappings().lock().map_err(|_| EIO)?;
        let mapping = registry.get_mut(&(user as usize)).ok_or(EIO)?;
        mapping.backing_offset = offset;
        mapping.view_protection = protect;
        mapping.backing = Some(backing);
        register_fork_fragment(user as usize, mapping);
    }
    Ok(user)
}

fn shared_section_protection(handle: *mut c_void, protection: c_int) -> Result<u32, i32> {
    let access = handle_access(handle).ok_or(kinakaze_vfs::EACCES)?;
    if access & FILE_READ_DATA == 0
        || (protection & PROT_WRITE != 0 && access & FILE_WRITE_DATA == 0)
    {
        return Err(kinakaze_vfs::EACCES);
    }
    let maximum = PROT_READ
        | (protection & PROT_EXEC)
        | if access & FILE_WRITE_DATA != 0 {
            PROT_WRITE
        } else {
            0
        };
    page_protection(maximum, false)
}

/// Makes the formerly out-of-file tail of an existing VMA accessible after the
/// backing file grows through `ftruncate`.
///
/// Linux VMAs retain their requested length independently of the file's current
/// length. A page wholly beyond EOF faults with `SIGBUS`, but the same page
/// becomes an ordinary file-backed page if the file is later extended into it.
/// Windows sections cannot contain such a tail, so `map_file` reserves it as a
/// placeholder. This function replaces exactly the newly backed placeholder
/// pages with views from a section created at the file's new size.
pub(crate) fn refresh_file_mappings(fd: i32, _new_length: u64) -> Result<(), i32> {
    let _fork_mapping_transaction =
        kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let opened = match fs::verity::Opened::from_fd(fd) {
        Ok(opened) => opened,
        Err(kinakaze_vfs::ENOTTY) => return Ok(()),
        Err(error) => return Err(error),
    };
    if opened.is_directory() {
        return Ok(());
    }
    let _inode = fs::verity::lock(opened.borrowed_handle())?;
    let identity = verity::native_id(opened.borrowed_handle())?;
    let new_length = fs::verity::authoritative_size(opened.borrowed_handle())?;

    loop {
        let candidate = {
            let registry = mappings().lock().map_err(|_| EIO)?;
            registry.iter().find_map(|(&start, mapping)| {
                if mapping.kind != MappingKind::Placeholder {
                    return None;
                }
                if let Some(origin) = &mapping.file_origin {
                    if origin.owner.identity() != identity || mapping.verity.is_some() {
                        return None;
                    }
                    let offset = origin.owner.offset(start).ok()?;
                    let backed = page_rounded_length(
                        new_length.saturating_sub(offset).min(mapping.length as u64) as usize,
                    )
                    .ok()?;
                    if backed == 0 {
                        return None;
                    }
                    let page = memory_geometry().0;
                    // A PAGE_NOACCESS section view cannot always regain COW
                    // access on Windows. Keep PROT_NONE EOF pages reserved and
                    // instantiate them only when mprotect grants access.
                    let from = (start..start + backed.min(mapping.length))
                        .step_by(page)
                        .find(|&at| origin.owner.protection(at).ok() != Some(PROT_NONE))?;
                    let linux_protection = origin.owner.protection(from).ok()?;
                    let mut length = backed.min(mapping.length) - (from - start);
                    for delta in (page..length).step_by(page) {
                        if origin.owner.protection(from + delta).ok()? != linux_protection {
                            length = delta;
                            break;
                        }
                    }
                    return Some((
                        from as *mut c_void,
                        length,
                        origin.owner.offset(from).ok()?,
                        page_protection(linux_protection, !origin.owner.shared()).ok()?,
                        origin.owner.shared(),
                        Some(origin.owner.clone()),
                        if origin.owner.shared() {
                            shared_section_protection(origin.owner.handle(), linux_protection)
                                .ok()?
                        } else {
                            section_protection(linux_protection, true)
                        },
                    ));
                }
                let info = mapping.file?;
                if info.fd != fd || info.generation != opened.generation() {
                    return None;
                }
                let available = new_length.saturating_sub(info.file_offset);
                let available = available.min(info.mapping_length as u64) as usize;
                let backed_length = page_rounded_length(available).ok()?;
                let backed_end = info
                    .origin
                    .checked_add(backed_length.min(info.mapping_length))?;
                if start >= backed_end {
                    return None;
                }
                let fragment_end = start.checked_add(mapping.length)?;
                let end = fragment_end.min(backed_end);
                let offset = info.file_offset.checked_add((start - info.origin) as u64)?;
                Some((
                    start as *mut c_void,
                    end - start,
                    offset,
                    info.protection,
                    info.shared,
                    None,
                    info.protection,
                ))
            })
        };
        let Some((address, length, offset, protection, shared, owner, section_protection)) =
            candidate
        else {
            return Ok(());
        };

        // A zero maximum size binds the new section to the file's current
        // length. The inode is pinned independently of the fd; a shared view
        // retains this new section for exact same-object fork restoration.
        let section = unsafe {
            CreateFileMappingW(
                owner
                    .as_ref()
                    .map_or(opened.borrowed_handle(), |owner| owner.handle()),
                ptr::null(),
                section_protection,
                0,
                0,
                ptr::null(),
            )
        };
        if section.is_null() {
            return Err(last_errno());
        }
        let retained = if shared {
            match BackingRef::new_retained_shared(
                section,
                page_rounded_length(new_length as usize)?,
                section_protection,
            ) {
                Ok(backing) => Some(backing),
                Err(error) => {
                    unsafe { CloseHandle(section) };
                    return Err(error);
                }
            }
        } else {
            None
        };
        let replaced = replace_placeholder_view(
            section,
            address,
            length,
            offset,
            protection,
            shared,
            retained.as_ref(),
        );
        if retained.is_none() {
            unsafe { CloseHandle(section) };
        }
        match replaced? {
            Some(mapped) if mapped == address => {}
            _ => return Err(EIO),
        }
    }
}

/// Records a live mapping so `munmap` can release it correctly.
fn record_mapping(
    user: *mut c_void,
    base: *mut c_void,
    length: usize,
    kind: MappingKind,
    shared: bool,
    file: Option<FileMappingInfo>,
) {
    let mapping = Mapping {
        base,
        length,
        kind,
        shared,
        backing: None,
        backing_offset: 0,
        view_protection: PAGE_NOACCESS,
        file,
        verity: None,
        native_inode: None,
        file_origin: None,
    };
    if let Ok(mut registry) = mappings().lock() {
        insert_mapping_fragment(&mut registry, user as usize, mapping);
    }
    if !shared {
        if crate::fork_trace_enabled() {
            eprintln!(
                "kinakaze libc: pid {} record_mapping user={:p} base={:p} len={:#x} shared={}",
                std::process::id(),
                user,
                base,
                length,
                shared
            );
        }
    }
}

/// `munmap`.
///
/// Linux lets a caller unmap any page-aligned sub-range of anything, splitting
/// mappings as needed. The registry therefore stores the live fragments
/// separately from `Mapping::base`, which always remains the address returned by
/// the Windows allocation API. This distinction matters after trimming the start
/// of a reservation: `VirtualFree(MEM_RELEASE)` still requires the original base.
///
/// A sub-range of an anonymous `MAP_PRIVATE` mapping is decommitted with
/// `MEM_DECOMMIT`, which leaves the address reserved but makes every page in the
/// range fault on access. That is what Linux's `munmap` observably does to the
/// unmapped pages, so a caller trimming an allocation gets the behaviour it
/// expects; what differs is that the address space stays reserved, so a later
/// `mmap` without `MAP_FIXED` will not be placed there.
///
/// A sub-range of a *view* cannot be expressed at all — `UnmapViewOfFile` takes no
/// length — so it reports `EINVAL` rather than unmapping more than was asked for.
/// Silently releasing the whole view would leave the caller's remaining pages
/// invalid with no indication.
///
/// # Safety
///
/// `address` must be a mapping from `mmap64` and must not be accessed afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_munmap(address: *mut c_void, length: usize) -> c_int {
    let result = unmap(address, length);
    if mmap_trace_enabled() {
        match &result {
            Ok(()) => eprintln!("kinakaze: munmap({address:p}, {length:#x}) = 0"),
            Err(error) => {
                eprintln!("kinakaze: munmap({address:p}, {length:#x}) = -1 errno={error}")
            }
        }
    }
    posix(result)
}

/// The body of `munmap`.
fn unmap(address: *mut c_void, length: usize) -> Result<(), i32> {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;
    let result = unmap_impl(address, length);
    let refreshed = verity::refresh_fault_index();
    result.and(refreshed)
}

fn unmap_impl(address: *mut c_void, length: usize) -> Result<(), i32> {
    let (page, _) = memory_geometry();
    if address.is_null() || length == 0 || address as usize % page != 0 {
        return Err(EINVAL);
    }
    // Linux rounds the length up to a whole page.
    let length = length
        .checked_add(page - 1)
        .ok_or(EINVAL)?
        .div_euclid(page)
        .checked_mul(page)
        .ok_or(EINVAL)?;
    let request_start = address as usize;
    let request_end = request_start.checked_add(length).ok_or(EINVAL)?;
    let _fork_mapping_transaction =
        kinakaze_runtime::begin_fork_mapping_transaction().ok_or(ENOMEM)?;

    // A retained section view can be split losslessly: temporarily restore its
    // placeholder, remap the untouched sides from the same section offsets, and
    // leave the requested middle as a pure placeholder for the release pass.
    loop {
        let partial = {
            let registry = mappings().lock().map_err(|_| EIO)?;
            registry.iter().find_map(|(&start, mapping)| {
                if mapping.kind != MappingKind::PlaceholderView
                    || !mapping
                        .backing
                        .as_ref()
                        .is_some_and(|backing| backing.is_remappable() || backing.is_cow())
                {
                    return None;
                }
                let end = start.saturating_add(mapping.length);
                let from = start.max(request_start);
                let to = end.min(request_end);
                (from < to && (from != start || to != end)).then(|| (from, to - from))
            })
        };
        let Some((from, fragment_length)) = partial else {
            break;
        };
        let prepared = {
            let mut registry = mappings().lock().map_err(|_| EIO)?;
            prepare_placeholder_locked(&mut registry, from, fragment_length)?
        };
        if prepared.is_none() {
            return Err(EIO);
        }
        // Dropping `previous` deliberately relinquishes the unmapped middle's
        // reference; the remapped sides retain theirs in the registry.
        drop(prepared);
    }

    // Unlike section views and ordinary reservations, a placeholder can be
    // split without disturbing either side. Carve every partially touched
    // placeholder before subtracting the range from the logical registry.
    loop {
        let partial = {
            let registry = mappings().lock().map_err(|_| EIO)?;
            registry.iter().find_map(|(&start, mapping)| {
                if mapping.kind != MappingKind::Placeholder {
                    return None;
                }
                let end = start.saturating_add(mapping.length);
                let from = start.max(request_start);
                let to = end.min(request_end);
                (from < to && (from != start || to != end)).then(|| (from, to - from))
            })
        };
        let Some((from, fragment_length)) = partial else {
            break;
        };
        if !carve_placeholder(from as *mut c_void, fragment_length)? {
            return Err(EIO);
        }
    }

    let mut registry = mappings().lock().map_err(|_| EIO)?;

    // Snapshot every live fragment touched by the request. Unknown pages are
    // deliberately ignored: Linux munmap succeeds when a range includes holes.
    let overlaps: Vec<(usize, Mapping, usize, usize)> = registry
        .iter()
        .filter_map(|(&start, mapping)| {
            let end = start.saturating_add(mapping.length);
            let overlap_start = start.max(request_start);
            let overlap_end = end.min(request_end);
            (overlap_start < overlap_end)
                .then(|| (start, mapping.clone(), overlap_start, overlap_end))
        })
        .collect();

    // Windows cannot partially unmap a section view. Validate every view before
    // changing the registry so an EINVAL never leaves half of the request done.
    if overlaps.iter().any(|(start, mapping, from, to)| {
        matches!(
            mapping.kind,
            MappingKind::View | MappingKind::PlaceholderView
        ) && (*from != *start || *to != start.saturating_add(mapping.length))
    }) {
        return Err(EINVAL);
    }

    // Subtract the requested interval from every affected live fragment. Both
    // halves retain the original allocation base; only the HashMap key denotes
    // the Linux-visible start of a fragment.
    for (start, mapping, from, to) in &overlaps {
        registry.remove(start);
        unregister_fork_fragment(*start, mapping);
        if *start < *from {
            let left_len = *from - *start;
            insert_mapping_fragment(
                &mut registry,
                *start,
                Mapping {
                    length: left_len,
                    ..mapping.clone()
                },
            );
        }
        let mapping_end = start.saturating_add(mapping.length);
        if *to < mapping_end {
            let right_len = mapping_end - *to;
            insert_mapping_fragment(
                &mut registry,
                *to,
                Mapping {
                    length: right_len,
                    ..mapping.clone()
                },
            );
        }
    }

    // A reservation is released only after its final live fragment disappears.
    // Until then, decommit precisely the removed pages so accesses fault as they
    // do after Linux munmap. Multiple removed fragments can share one base.
    let affected_reservations: HashSet<usize> = overlaps
        .iter()
        .filter(|(_, mapping, _, _)| {
            matches!(
                mapping.kind,
                MappingKind::Reserved | MappingKind::PlaceholderPrivate
            )
        })
        .map(|(_, mapping, _, _)| mapping.base as usize)
        .collect();
    let surviving_reservations: HashSet<usize> = registry
        .values()
        .filter(|mapping| {
            matches!(
                mapping.kind,
                MappingKind::Reserved | MappingKind::PlaceholderPrivate
            )
        })
        .map(|mapping| mapping.base as usize)
        .collect();

    for base in &affected_reservations {
        if !surviving_reservations.contains(base) {
            let mapping = overlaps
                .iter()
                .find(|(_, mapping, _, _)| mapping.base as usize == *base)
                .expect("affected reservation has an overlap")
                .1
                .clone();
            release(mapping)?;
        }
    }
    for (_, mapping, from, to) in &overlaps {
        if matches!(
            mapping.kind,
            MappingKind::Reserved | MappingKind::PlaceholderPrivate
        ) && surviving_reservations.contains(&(mapping.base as usize))
        {
            release_partial(mapping.clone(), *from as *mut c_void, *to - *from)?;
        } else if matches!(
            mapping.kind,
            MappingKind::View | MappingKind::PlaceholderView | MappingKind::Placeholder
        ) {
            release(mapping.clone())?;
        } else if mapping.kind.is_borrowed() {
            release_partial(mapping.clone(), *from as *mut c_void, *to - *from)?;
        }
    }

    Ok(())
}

/// Flush shared mappings using the retained mount policy, even after fd close.
pub(crate) fn sync_mappings(address: usize, length: usize, flags: c_int) -> Result<(), i32> {
    let page = memory_geometry().0;
    if address % page != 0 || flags & !7 != 0 || flags & 5 == 5 {
        return Err(EINVAL);
    }
    if length == 0 {
        return Ok(());
    }
    let end = address
        .checked_add(page_rounded_length(length)?)
        .ok_or(ENOMEM)?;
    let registry = mappings().lock().map_err(|_| EIO)?;
    let mut selected = registry
        .iter()
        .filter_map(|(start, m)| {
            let from = address.max(*start);
            let to = end.min(start.saturating_add(m.length));
            (from < to).then_some((from, to, m))
        })
        .collect::<Vec<_>>();
    selected.sort_by_key(|m| m.0);
    let mut covered = address;
    for (from, to, _) in &selected {
        if *from > covered {
            return Err(ENOMEM);
        }
        covered = covered.max(*to);
    }
    if covered != end {
        return Err(ENOMEM);
    }
    for (from, to, mapping) in selected {
        if !mapping.shared {
            continue;
        }
        if let Some((volume, since)) = mapping
            .file_origin
            .as_ref()
            .and_then(|i| i.owner.volatile())
        {
            kinakaze_vfs::mount::overlay::check_volatile_epoch(volume, since)?;
            continue;
        }
        if flags & 4 != 0
            && matches!(
                mapping.kind,
                MappingKind::View | MappingKind::PlaceholderView
            )
        {
            if unsafe { FlushViewOfFile(from as _, to - from) } == 0 {
                return Err(last_errno());
            }
        }
    }
    Ok(())
}

/// Releases a whole mapping.
fn release(mapping: Mapping) -> Result<(), i32> {
    match mapping.kind {
        MappingKind::View => {
            if mapping.shared
                && mapping
                    .file_origin
                    .as_ref()
                    .and_then(|i| i.owner.volatile())
                    .is_none()
            {
                // A shared view's dirty pages are written back before the view
                // goes away. Windows flushes lazily otherwise, so a caller that
                // unmaps and immediately reads the file through a descriptor
                // could see stale bytes.
                // SAFETY: the view is live until it is unmapped below.
                unsafe { FlushViewOfFile(mapping.base, 0) };
            }
            // SAFETY: `base` is the address MapViewOfFileEx returned.
            if unsafe { UnmapViewOfFile(mapping.base) } == 0 {
                return Err(last_errno());
            }
            Ok(())
        }
        MappingKind::PlaceholderView => {
            if mapping.shared
                && mapping
                    .file_origin
                    .as_ref()
                    .and_then(|i| i.owner.volatile())
                    .is_none()
            {
                unsafe { FlushViewOfFile(mapping.base, 0) };
            }
            // First restore the exact placeholder, then release it. This keeps
            // the view lifecycle symmetrical and never leaves a transient hole
            // while the mapping registry still owns the address.
            if unsafe {
                windows_sys::Win32::System::Memory::UnmapViewOfFile2(
                    GetCurrentProcess(),
                    windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: mapping.base,
                    },
                    MEM_PRESERVE_PLACEHOLDER,
                )
            } == 0
            {
                return Err(last_errno());
            }
            if unsafe { VirtualFree(mapping.base, 0, MEM_RELEASE) } == 0 {
                return Err(last_errno());
            }
            Ok(())
        }
        MappingKind::Reserved | MappingKind::PlaceholderPrivate | MappingKind::Placeholder => {
            // MEM_RELEASE requires a zero size and the original base address.
            // SAFETY: `base` is the address VirtualAlloc returned.
            if unsafe { VirtualFree(mapping.base, 0, MEM_RELEASE) } == 0 {
                return Err(last_errno());
            }
            Ok(())
        }
        MappingKind::BorrowedReservation => {
            // The allocation base belongs to the host; only return these pages
            // to its still-live reservation.
            if unsafe { VirtualFree(mapping.base, mapping.length, MEM_DECOMMIT) } == 0 {
                return Err(last_errno());
            }
            Ok(())
        }
        MappingKind::BorrowedView => {
            let mut old_protection = 0;
            if unsafe {
                windows_sys::Win32::System::Memory::VirtualProtect(
                    mapping.base,
                    mapping.length,
                    PAGE_NOACCESS,
                    &mut old_protection,
                )
            } == 0
            {
                return Err(last_errno());
            }
            Ok(())
        }
    }
}

/// Releases part of a mapping, as far as Windows allows.
fn release_partial(mapping: Mapping, address: *mut c_void, length: usize) -> Result<(), i32> {
    match mapping.kind {
        MappingKind::Reserved | MappingKind::PlaceholderPrivate => {
            // Decommitting makes the range fault on access, which is what the
            // caller asked for. The reservation stays, which is the divergence
            // documented on `munmap`.
            // SAFETY: the range lies inside a live reservation.
            if unsafe { VirtualFree(address, length, MEM_DECOMMIT) } == 0 {
                return Err(last_errno());
            }
            Ok(())
        }
        MappingKind::BorrowedReservation => {
            if unsafe { VirtualFree(address, length, MEM_DECOMMIT) } == 0 {
                return Err(last_errno());
            }
            Ok(())
        }
        MappingKind::BorrowedView => {
            let mut old_protection = 0;
            if unsafe {
                windows_sys::Win32::System::Memory::VirtualProtect(
                    address,
                    length,
                    PAGE_NOACCESS,
                    &mut old_protection,
                )
            } == 0
            {
                return Err(last_errno());
            }
            Ok(())
        }
        // `UnmapViewOfFile` releases the whole view or nothing, so a partial
        // request cannot be honoured. Refusing is the only truthful answer.
        MappingKind::View | MappingKind::PlaceholderView | MappingKind::Placeholder => Err(EINVAL),
    }
}

// ---------------------------------------------------------------------------
// Unlocked stdio.
//
// glibc's `*_unlocked` forms exist so a caller that has taken `flockfile` can
// skip the per-operation lock. This layer has no `flockfile`: a stream's mutex is
// taken inside each operation and is not reachable from outside, so there is no
// lock for these to skip. Each therefore forwards to the locked implementation,
// which is both correct and the only implementation that could be correct — an
// actually-unsynchronized path would corrupt the stream's buffer under the
// concurrency the locked one already handles.
//
// The forwarding is deliberate and not a placeholder. These are the real
// definitions of the unlocked calls on this platform.
// ---------------------------------------------------------------------------

/// `getc_unlocked`, which is `fgetc`. See the section note.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getc_unlocked(file: *mut File) -> c_int {
    crate::stdio::fgetc(file)
}

/// `getchar_unlocked`, which is `getc_unlocked` on `stdin`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getchar_unlocked() -> c_int {
    crate::stdio::fgetc(crate::stdio::exports::kinakaze_stdin())
}

/// `putc_unlocked`, which is `fputc`. See the section note.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_putc_unlocked(character: c_int, file: *mut File) -> c_int {
    crate::stdio::fputc(character, file)
}

/// `putchar_unlocked`, which is `putc_unlocked` on `stdout`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_putchar_unlocked(character: c_int) -> c_int {
    crate::stdio::fputc(character, crate::stdio::exports::kinakaze_stdout())
}

/// `fputc_unlocked`, which is `fputc`. See the section note.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fputc_unlocked(character: c_int, file: *mut File) -> c_int {
    crate::stdio::fputc(character, file)
}

/// `feof_unlocked`, which is `feof`. See the section note.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_feof_unlocked(file: *mut File) -> c_int {
    crate::stdio::feof(file)
}

/// `ferror_unlocked`, which is `ferror`. See the section note.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ferror_unlocked(file: *mut File) -> c_int {
    crate::stdio::ferror(file)
}

/// `fileno_unlocked`, which is `fileno`. See the section note.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fileno_unlocked(file: *mut File) -> c_int {
    crate::stdio::fileno(file)
}

/// `fputs_unlocked`, which is `fputs`. See the section note.
///
/// # Safety
///
/// `text` must be a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fputs_unlocked(
    text: *const c_char,
    file: *mut File,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::stdio::fputs(text, file) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn fputs_unlocked(text: *const c_char, file: *mut File) -> c_int {
    unsafe { kinakaze_abi_fputs_unlocked(text, file) }
}

/// `fgets_unlocked`, which is `fgets`. See the section note.
///
/// # Safety
///
/// `buffer` must be writable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgets_unlocked(
    buffer: *mut c_char,
    size: c_int,
    file: *mut File,
) -> *mut c_char {
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::stdio::fgets(buffer, size, file) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn fgets_unlocked(
    buffer: *mut c_char,
    size: c_int,
    file: *mut File,
) -> *mut c_char {
    unsafe { kinakaze_abi_fgets_unlocked(buffer, size, file) }
}

// ---------------------------------------------------------------------------
// Line reading.
// ---------------------------------------------------------------------------

/// `getdelim`.
///
/// The buffer protocol is the part callers get wrong and this must get right.
/// `*line` is grown with this library's `realloc` and both `*line` and `*capacity`
/// are updated in place, so the guest may `free` the buffer or pass it back for the
/// next line. The allocation comes from the same allocator `free` releases, which
/// is why the active C allocator is used rather than a Rust `Vec` that would
/// be freed by an allocator the guest never saw.
///
/// Returns the byte count including the delimiter, or -1 at end of file. At end of
/// file errno is left exactly as it was: a caller distinguishes EOF from an error
/// by checking `ferror`, and clobbering errno here would break that.
///
/// # Safety
///
/// `line` must point at a pointer that is null or a live allocation from the
/// active C allocator, and `capacity` at the matching size.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getdelim(
    line: *mut *mut c_char,
    capacity: *mut usize,
    delimiter: c_int,
    file: *mut File,
) -> isize {
    if line.is_null() || capacity.is_null() || file.is_null() {
        set_errno(EINVAL);
        return -1;
    }
    // SAFETY: forwarded from this function's contract.
    unsafe { crate::stdio::line::getdelim(file, line, capacity, delimiter as u8) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn getdelim(
    line: *mut *mut c_char,
    capacity: *mut usize,
    delimiter: c_int,
    file: *mut File,
) -> isize {
    unsafe { kinakaze_abi_getdelim(line, capacity, delimiter, file) }
}

/// `getline`, which is `getdelim` with a newline delimiter.
///
/// # Safety
///
/// Same contract as [`kinakaze_abi_getdelim`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getline(
    line: *mut *mut c_char,
    capacity: *mut usize,
    file: *mut File,
) -> isize {
    // SAFETY: forwarded from this function's contract.
    unsafe { kinakaze_abi_getdelim(line, capacity, c_int::from(b'\n'), file) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn getline(
    line: *mut *mut c_char,
    capacity: *mut usize,
    file: *mut File,
) -> isize {
    unsafe { kinakaze_abi_getline(line, capacity, file) }
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn kinakaze_abi___getdelim(
    line: *mut *mut c_char,
    capacity: *mut usize,
    delimiter: c_int,
    file: *mut File,
) -> isize {
    core::arch::naked_asm!(
        "mov r8, [rsp]",
        "jmp {body}",
        body = sym getdelim_with_caller,
    )
}

unsafe extern "sysv64" fn getdelim_with_caller(
    line: *mut *mut c_char,
    capacity: *mut usize,
    delimiter: c_int,
    file: *mut File,
    caller: usize,
) -> isize {
    if crate::stdio::trace_enabled() {
        use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
        let mut info = core::mem::MaybeUninit::<MEMORY_BASIC_INFORMATION>::uninit();
        let queried = unsafe {
            VirtualQuery(
                caller as *const c_void,
                info.as_mut_ptr(),
                core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried != 0 {
            let info = unsafe { info.assume_init() };
            let base = info.AllocationBase as usize;
            crate::stdio::trace_event(format_args!(
                "__getdelim caller={caller:#x} base={base:#x} offset={:#x}",
                caller.wrapping_sub(base)
            ));
        }
    }
    unsafe { kinakaze_abi_getdelim(line, capacity, delimiter, file) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __getdelim(
    line: *mut *mut c_char,
    capacity: *mut usize,
    delimiter: c_int,
    file: *mut File,
) -> isize {
    unsafe { kinakaze_abi___getdelim(line, capacity, delimiter, file) }
}

// ---------------------------------------------------------------------------
// Large-file stdio spellings and buffering.
//
// On x86_64 `off_t` is already 64 bits, so `fseeko64` and `fseeko` describe
// byte-for-byte identical calls. These are forwarders rather than second copies,
// following the convention [`crate::fsextra`] uses for the same reason: two
// implementations of one call can drift, and one of them will be the one a guest
// happens to reach.
// ---------------------------------------------------------------------------

/// `fseeko64`, identical to `fseek` on x86_64.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fseeko64(file: *mut File, offset: i64, whence: c_int) -> c_int {
    crate::stdio::fseek(file, offset, whence)
}

/// `ftello64`, identical to `ftell` on x86_64.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ftello64(file: *mut File) -> i64 {
    crate::stdio::ftell(file)
}

/// `setbuf`, which is `setvbuf` with the two modes it can express.
///
/// A non-null buffer selects full buffering and a null one selects none. The
/// caller's buffer itself is ignored, as it is in `setvbuf`: this implementation
/// owns its buffering, and writing into the caller's array would mean maintaining
/// two representations of the same bytes.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setbuf(file: *mut File, buffer: *mut c_char) {
    // _IOFBF is 0 and _IONBF is 2, matching the constants `setvbuf` takes.
    let mode = if buffer.is_null() { 2 } else { 0 };
    crate::stdio::setvbuf(file, ptr::null_mut(), mode, crate::stdio::BUFSIZ);
}

/// `freopen64`, identical to `freopen` on x86_64.
///
/// The stream object is reused, which is the whole point of the call: after
/// `freopen(path, "r", stdin)` the guest's `stdin` must still be `stdin`. That is
/// done by opening the new file, moving it onto the stream's existing descriptor
/// with `dup2`, and closing the temporary. The `FILE` never moves, so a guest
/// holding a copy of the pointer — or the `stdin` data symbol the loader copied
/// out of this DLL — keeps working.
///
/// A null `path` is the POSIX form that changes mode on the same file. The name is
/// recovered from the open handle, since a `FILE` records no path.
///
/// # Safety
///
/// `path` must be null or a null-terminated string, `mode` must be
/// null-terminated, and `file` must be a stream from this library.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_freopen64(
    path: *const c_char,
    mode: *const c_char,
    file: *mut File,
) -> *mut File {
    if mode.is_null() || file.is_null() {
        set_errno(EFAULT);
        return ptr::null_mut();
    }
    // The stream's own descriptor is the one the reopened file must land on.
    let target = crate::stdio::fileno(file);
    if target < 0 {
        set_errno(EBADF);
        return ptr::null_mut();
    }
    // Buffered output is flushed before the descriptor changes underneath it, or
    // those bytes would be written to the new file.
    // SAFETY: `file` was checked non-null and the caller guarantees it is a
    // stream from this library.
    unsafe { crate::stdio::fflush(file) };

    // A null path reopens the same file. Its name comes from the handle, because
    // a `FILE` carries no path of its own.
    let reopened = if path.is_null() {
        match kinakaze_vfs::get(target)
            .and_then(|entry| handle_path(entry.raw as *mut c_void))
            .map(|windows| windows_to_linux(&windows))
        {
            Ok(name) => {
                let Ok(name) = std::ffi::CString::new(name) else {
                    set_errno(EINVAL);
                    return ptr::null_mut();
                };
                // SAFETY: an owned CString is null-terminated, and `mode` is
                // guaranteed by this function's contract.
                unsafe { crate::stdio::fopen(name.as_ptr(), mode) }
            }
            Err(error) => {
                set_errno(error);
                return ptr::null_mut();
            }
        }
    } else {
        // SAFETY: forwarded from this function's contract.
        unsafe { crate::stdio::fopen(path, mode) }
    };
    if reopened.is_null() {
        // `fopen` already set errno. POSIX says the original stream is closed
        // even when the reopen fails, which is what makes `freopen` unusable for
        // "try this file, else keep the old one".
        let _ = kinakaze_vfs::close(target);
        return ptr::null_mut();
    }

    let source = crate::stdio::fileno(reopened);
    // Move the new file onto the stream's descriptor number. `dup2` closes the
    // old one, which is what releases the file the stream had open.
    if kinakaze_abi_dup2(source, target) < 0 {
        // errno is already set by dup2. The temporary stream still owns the new
        // file, so it is closed rather than leaked.
        // SAFETY: `reopened` came from `fopen` and is not used again.
        unsafe { crate::stdio::fclose(reopened) };
        return ptr::null_mut();
    }
    // The temporary stream has served its purpose. Closing it releases the
    // descriptor `fopen` allocated; the duplicate on `target` keeps the file open.
    // SAFETY: `reopened` came from `fopen` and is not used again.
    unsafe { crate::stdio::fclose(reopened) };

    // The stream's sticky flags and pushback describe the file that is gone. A
    // seek to the current position clears both, which is what `fseek` does, and
    // leaves the position where `dup2` put it.
    let position = fs::lseek(target, 0, SEEK_CUR).unwrap_or(0);
    if crate::stdio::fseek(file, position as i64, SEEK_SET) != 0 {
        // A stream on something unseekable, which is legal: clear the indicators
        // directly instead.
        crate::stdio::clearerr(file);
    }
    crate::stdio::reset_after_reopen(file, unsafe { CStr::from_ptr(mode) }.to_bytes());
    file
}

// ---------------------------------------------------------------------------
// Temporary names.
//
// The security property these calls exist for is unpredictability: an attacker
// who can guess the next name can pre-create it as a symlink and win the race.
// So the six characters come from `BCryptGenRandom` and not from a counter, a
// pid or a clock reading, and `mkstemp64` creates with O_EXCL so that even a
// guessed name loses.
// ---------------------------------------------------------------------------

/// Characters a generated suffix is drawn from.
///
/// 62 symbols, matching glibc, so a name is safe on a case-sensitive filesystem
/// and legal on Windows.
const TEMPLATE_ALPHABET: &[u8; 62] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Trailing `X` characters a template must end with.
const TEMPLATE_MARKS: usize = 6;

/// Attempts before a generator gives up.
///
/// glibc tries 62^3 times. That is unreachable in practice — a collision needs an
/// adversary creating names as fast as they are generated — and this bound keeps a
/// full or unwritable directory from spinning for minutes before reporting the
/// error that was true on the first attempt.
const TEMPLATE_ATTEMPTS: usize = 4096;

/// Fills `bytes` with cryptographic random data.
fn random_bytes(bytes: &mut [u8]) -> Result<(), i32> {
    // The second flag is BCRYPT_USE_SYSTEM_PREFERRED_RNG, which is what lets the
    // algorithm handle be null.
    const USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    // SAFETY: the buffer is writable for its own length and a null algorithm
    // handle is documented with this flag.
    let status = unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            USE_SYSTEM_PREFERRED_RNG,
        )
    };
    // An NTSTATUS is negative on failure. There is no fallback: a generator that
    // silently produced predictable names would defeat the only reason these
    // functions exist.
    if status < 0 { Err(EIO) } else { Ok(()) }
}

/// Validates a template and returns the offset of its `XXXXXX`.
///
/// # Safety
///
/// `template` must be a writable, null-terminated string.
unsafe fn template_marks(template: *mut c_char) -> Result<usize, i32> {
    unsafe { template_marks_with_suffix(template, 0) }
}

unsafe fn template_marks_with_suffix(template: *mut c_char, suffix: c_int) -> Result<usize, i32> {
    if template.is_null() {
        return Err(EFAULT);
    }
    // SAFETY: the caller guarantees a null-terminated string.
    let bytes = unsafe { CStr::from_ptr(template) }.to_bytes();
    let suffix = usize::try_from(suffix).map_err(|_| EINVAL)?;
    let end = bytes.len().checked_sub(suffix).ok_or(EINVAL)?;
    let start = end.checked_sub(TEMPLATE_MARKS).ok_or(EINVAL)?;
    if bytes[start..end] != *b"XXXXXX" {
        return Err(EINVAL);
    }
    Ok(start)
}

/// Writes a fresh random suffix into a template in place.
///
/// # Safety
///
/// `template` must be writable for at least `start + TEMPLATE_MARKS` bytes.
unsafe fn write_suffix(template: *mut c_char, start: usize) -> Result<(), i32> {
    let mut random = [0u8; TEMPLATE_MARKS];
    random_bytes(&mut random)?;
    for (index, byte) in random.iter().enumerate() {
        // The modulo is very slightly biased — 256 is not a multiple of 62 — which
        // costs at most a fraction of a bit across six characters and is what
        // glibc's own generator does. It is noted rather than hidden.
        let character = TEMPLATE_ALPHABET[(*byte as usize) % TEMPLATE_ALPHABET.len()];
        // SAFETY: the caller guarantees the six bytes at `start` are writable.
        unsafe { *template.add(start + index) = character as c_char };
    }
    Ok(())
}

/// Reads a template back as a guest path.
///
/// # Safety
///
/// `template` must be a null-terminated string.
unsafe fn template_path(template: *const c_char) -> Result<&'static str, i32> {
    // SAFETY: forwarded from this function's contract.
    unsafe { borrow_path(template) }
}

/// `mkstemp64`, identical to `mkstemp` on x86_64.
///
/// The file is created with `O_EXCL`, which is what makes the call safe rather
/// than merely unpredictable: if a name is guessed and pre-created, the create
/// fails and another name is tried instead of the attacker's file being opened.
/// The mode is 0600, so the file is not readable by others — as far as the host
/// expresses that; see [`crate::fsextra`] on what a Windows file can store of a
/// POSIX mode.
///
/// # Safety
///
/// `template` must be a writable, null-terminated string ending in `XXXXXX`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkstemp64(template: *mut c_char) -> c_int {
    unsafe { create_temporary_file(template, 0, 0) }
}

/// One creation path for every LP64 temporary-file spelling.
unsafe fn create_temporary_file(template: *mut c_char, suffix: c_int, flags: c_int) -> c_int {
    // SAFETY: forwarded from this function's contract.
    let start = match unsafe { template_marks_with_suffix(template, suffix) } {
        Ok(start) => start,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    for _ in 0..TEMPLATE_ATTEMPTS {
        // SAFETY: `start` came from the validated template.
        if let Err(error) = unsafe { write_suffix(template, start) } {
            set_errno(error);
            return -1;
        }
        // SAFETY: the template is still null-terminated; only the six marks moved.
        let path = match unsafe { template_path(template) } {
            Ok(path) => path,
            Err(error) => {
                set_errno(error);
                return -1;
            }
        };
        match fs::open(
            path,
            (flags & !fs::O_ACCMODE) | fs::O_RDWR | fs::O_CREAT | fs::O_EXCL,
            0o600,
        ) {
            Ok(fd) => return fd,
            // The name was taken. Try another rather than reporting failure.
            Err(EEXIST) => continue,
            Err(error) => {
                set_errno(error);
                return -1;
            }
        }
    }
    // Every attempt collided, which means something other than chance is at work.
    set_errno(EEXIST);
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkostemp(template: *mut c_char, flags: c_int) -> c_int {
    unsafe { create_temporary_file(template, 0, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn mkostemp(template: *mut c_char, flags: c_int) -> c_int {
    unsafe { kinakaze_abi_mkostemp(template, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkostemp64(
    template: *mut c_char,
    flags: c_int,
) -> c_int {
    unsafe { create_temporary_file(template, 0, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkstemps(template: *mut c_char, suffix: c_int) -> c_int {
    unsafe { create_temporary_file(template, suffix, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkstemps64(
    template: *mut c_char,
    suffix: c_int,
) -> c_int {
    unsafe { create_temporary_file(template, suffix, 0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkostemps(
    template: *mut c_char,
    suffix: c_int,
    flags: c_int,
) -> c_int {
    unsafe { create_temporary_file(template, suffix, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkostemps64(
    template: *mut c_char,
    suffix: c_int,
    flags: c_int,
) -> c_int {
    unsafe { create_temporary_file(template, suffix, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn mkostemp64(template: *mut c_char, flags: c_int) -> c_int {
    unsafe { kinakaze_abi_mkostemp64(template, flags) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mq_getattr(mqdes: c_int, mqstat: *mut c_void) -> c_int {
    crate::sysadmin::mqueue::mq_getattr(mqdes, mqstat as usize)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn mq_getattr(mqdes: c_int, mqstat: *mut c_void) -> c_int {
    unsafe { kinakaze_abi_mq_getattr(mqdes, mqstat) }
}

/// `mkdtemp`, which creates a directory and returns the template.
///
/// # Safety
///
/// `template` must be a writable, null-terminated string ending in `XXXXXX`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mkdtemp(template: *mut c_char) -> *mut c_char {
    // SAFETY: forwarded from this function's contract.
    let start = match unsafe { template_marks(template) } {
        Ok(start) => start,
        Err(error) => {
            set_errno(error);
            return ptr::null_mut();
        }
    };
    for _ in 0..TEMPLATE_ATTEMPTS {
        // SAFETY: `start` came from the validated template.
        if let Err(error) = unsafe { write_suffix(template, start) } {
            set_errno(error);
            return ptr::null_mut();
        }
        // SAFETY: the template is still null-terminated.
        let path = match unsafe { template_path(template) } {
            Ok(path) => path,
            Err(error) => {
                set_errno(error);
                return ptr::null_mut();
            }
        };
        // `mkdir` is already exclusive: creating a directory that exists fails,
        // which gives this the same guarantee O_EXCL gives `mkstemp`.
        match fs::mkdir(path, 0o700) {
            Ok(()) => return template,
            Err(EEXIST) => continue,
            Err(error) => {
                set_errno(error);
                return ptr::null_mut();
            }
        }
    }
    set_errno(EEXIST);
    ptr::null_mut()
}

/// `mktemp`, which only generates a name.
///
/// Deprecated for a reason this implementation cannot fix: between the moment this
/// returns a free name and the moment the caller creates it, anyone may create it
/// first. `mkstemp64` exists because it closes that window by creating the file
/// itself, and it should be used instead.
///
/// The name is checked to be free, so what is returned is at least true at the
/// instant it is returned. On failure the template is set to the empty string,
/// which is what POSIX specifies and what distinguishes "no name available" from
/// a name the caller might otherwise use.
///
/// # Safety
///
/// `template` must be a writable, null-terminated string ending in `XXXXXX`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mktemp(template: *mut c_char) -> *mut c_char {
    // SAFETY: forwarded from this function's contract.
    let start = match unsafe { template_marks(template) } {
        Ok(start) => start,
        Err(error) => {
            set_errno(error);
            // POSIX: the template becomes an empty string on failure.
            if !template.is_null() {
                // SAFETY: the caller guarantees a writable string.
                unsafe { *template = 0 };
            }
            return template;
        }
    };
    for _ in 0..TEMPLATE_ATTEMPTS {
        // SAFETY: `start` came from the validated template.
        if let Err(error) = unsafe { write_suffix(template, start) } {
            set_errno(error);
            break;
        }
        // SAFETY: the template is still null-terminated.
        let Ok(path) = (unsafe { template_path(template) }) else {
            break;
        };
        // A name that does not resolve to an existing file is what this call
        // promises. `stat` failing with ENOENT is exactly that.
        if fs::stat(path).is_err() {
            return template;
        }
    }
    set_errno(EEXIST);
    // SAFETY: the caller guarantees a writable string.
    unsafe { *template = 0 };
    template
}

// ---------------------------------------------------------------------------
// Positional and descriptor-to-descriptor transfers.
// ---------------------------------------------------------------------------

/// `pread64`, identical to `pread` on x86_64.
///
/// The defining property is that the file position does not move. On Windows that
/// is only expressible through an `OVERLAPPED` carrying the offset, which is what
/// the VFS's transfer path already does for every seekable descriptor: it keeps
/// the position in the table and passes it per request, so a read at an explicit
/// offset is a read with the table's position left alone.
///
/// Native files are atomically pinned before I/O. The verified-data boundary
/// uses an independently reopened positional handle, including for inherited
/// synchronous files; it never saves/restores the original kernel seek pointer.
///
/// A synthetic `/proc` file has no handle at all and no positional read in the
/// VFS's interface, so it is served by seeking, reading and seeking back. Same
/// caveat for concurrent position-changing access.
///
/// # Safety
///
/// `buffer` must be writable for `count` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pread64(
    fd: c_int,
    buffer: *mut c_void,
    count: usize,
    offset: i64,
) -> isize {
    if buffer.is_null() && count != 0 {
        set_errno(EFAULT);
        return -1;
    }
    if offset < 0 {
        set_errno(EINVAL);
        return -1;
    }
    if count == 0 {
        return 0;
    }
    // SAFETY: forwarded from this function's contract.
    match unsafe { read_at(fd, buffer.cast::<u8>(), count, offset as u64) } {
        Ok(read) => read as isize,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Reads at an explicit offset without disturbing the descriptor's position.
///
/// # Safety
///
/// `buffer` must be writable for `count` bytes.
unsafe fn read_at(fd: c_int, buffer: *mut u8, count: usize, offset: u64) -> Result<usize, i32> {
    if matches!(
        kinakaze_vfs::get(fd)?.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return kinakaze_vfs::tmpfs::read(
            fd,
            unsafe { std::slice::from_raw_parts_mut(buffer, count) },
            Some(offset),
        );
    }
    loop {
        let (entry, pin) = match kinakaze_vfs::pin_native_fd(fd, |entry| {
            if entry.flags.contains(FdFlags::PATH_ONLY)
                || (entry.flags.contains(FdFlags::WRITE_ACCESS)
                    && !entry.flags.contains(FdFlags::READ_ACCESS))
            {
                return Err(EBADF);
            }
            if !entry.flags.contains(FdFlags::SEEKABLE) {
                return Err(ESPIPE);
            }
            if matches!(entry.kind, FdKind::Directory | FdKind::SyntheticDirectory) {
                return Err(kinakaze_vfs::EISDIR);
            }
            if matches!(
                entry.kind,
                FdKind::Synthetic | FdKind::CgroupFile | FdKind::ProcSysctl
            ) {
                return Err(kinakaze_vfs::ENOTTY);
            }
            Ok(())
        }) {
            Ok(pinned) => pinned,
            Err(kinakaze_vfs::ENOTTY) => {
                return unsafe { read_synthetic_at(fd, buffer, count, offset) };
            }
            Err(error) => return Err(error),
        };
        let outcome = unsafe { pin.read_once(FdEntry { offset, ..entry }, buffer, count) };
        // No signal handler may inherit a transient, unregistered OwnedHandle
        // via the suspended syscall stack. Each restart atomically pins anew.
        drop(pin);
        if outcome == Err(kinakaze_vfs::EINTR)
            && kinakaze_vfs::signal::deliver_pending() == kinakaze_vfs::signal::Delivery::Restart
        {
            continue;
        }
        return outcome;
    }
}

unsafe fn read_synthetic_at(
    fd: c_int,
    buffer: *mut u8,
    count: usize,
    offset: u64,
) -> Result<usize, i32> {
    let entry = kinakaze_vfs::get(fd)?;
    if !entry.flags.contains(FdFlags::SEEKABLE) {
        // A pipe, socket or console has no position to read at.
        return Err(ESPIPE);
    }

    // A synthetic file's bytes live in a VFS side table with no positional
    // accessor, so its position is moved and put back.
    if matches!(
        entry.kind,
        FdKind::Synthetic | FdKind::CgroupFile | FdKind::ProcSysctl
    ) {
        let saved = entry.offset;
        fs::lseek(fd, offset as i64, SEEK_SET)?;
        // SAFETY: the caller guarantees the buffer.
        let slice = unsafe { core::slice::from_raw_parts_mut(buffer, count) };
        let outcome = kinakaze_vfs::read(fd, slice);
        // Restored on both paths, so a failed read does not leave the descriptor
        // somewhere the caller did not put it.
        let _ = fs::lseek(fd, saved as i64, SEEK_SET);
        return outcome;
    }
    Err(EBADF)
}

#[cfg(test)]
/// One attempt with a caller-owned native pin; never dispatches guest signals.
unsafe fn read_pinned_at(
    entry: FdEntry,
    buffer: *mut u8,
    count: usize,
    offset: u64,
) -> Result<usize, i32> {
    // The overlapped path takes its offset from the entry, and the entry is a
    // copy, so overriding it here reads elsewhere without touching the table.
    // `platform_read` does not advance the stored position, which is exactly the
    // `pread` contract.
    let positioned = FdEntry { offset, ..entry };
    if entry.kind == FdKind::File || entry.flags.contains(FdFlags::OVERLAPPED) {
        // SAFETY: the caller guarantees `count` writable bytes.
        return unsafe { kinakaze_vfs::platform_read_pinned_once(positioned, buffer, count) };
    }

    // A synchronous handle: the kernel file pointer moves, so it is put back.
    let handle = entry.raw as *mut c_void;
    let mut saved = 0i64;
    // SAFETY: the handle is live and `saved` is a writable local.
    let queried = unsafe { SetFilePointerEx(handle, 0, &raw mut saved, FILE_CURRENT) };
    // SAFETY: as above; the transfer reads at the offset carried in `positioned`.
    let outcome = unsafe { kinakaze_vfs::platform_read_pinned_once(positioned, buffer, count) };
    if queried != 0 {
        // SAFETY: the handle is live and the saved position was valid.
        unsafe { SetFilePointerEx(handle, saved, ptr::null_mut(), FILE_BEGIN) };
    }
    outcome
}

/// `sendfile64`, identical to `sendfile` on x86_64.
///
/// This is a read/write loop, and that is the honest implementation rather than a
/// fallback. Windows has no primitive that copies between two arbitrary
/// descriptors: `TransmitFile` is the only zero-copy path and it requires the
/// destination to be a Winsock socket, `CopyFileEx` works on paths rather than
/// handles, and neither covers the file-to-file and pipe-to-file cases a guest
/// actually uses.
///
/// `TransmitFile` is deliberately not used even for the socket case it does cover.
/// Every socket this layer owns is non-blocking underneath, with blocking
/// semantics and signal interruption reconstructed inside
/// [`kinakaze_vfs::socket`]; a `TransmitFile` issued around that layer would
/// return `WSAEWOULDBLOCK` on any transfer large enough to matter, and making it
/// work would mean rebuilding the readiness and `EINTR` handling that the write
/// path already has. The copy loop goes through `kinakaze_vfs::write`, so a
/// `sendfile` to a socket is interruptible and restartable exactly like a `write`
/// to it. The cost is one buffer copy, which is the price of correctness here.
///
/// When `offset` is non-null the input is read from that position and the position
/// is written back, leaving the input descriptor's own offset untouched, which is
/// what Linux does.
///
/// # Safety
///
/// `offset` must be null or a writable `off_t`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sendfile64(
    out_fd: c_int,
    in_fd: c_int,
    offset: *mut i64,
    count: usize,
) -> isize {
    // SAFETY: forwarded from this function's contract.
    match unsafe { copy_descriptor(out_fd, in_fd, offset, count) } {
        Ok(copied) => copied as isize,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}

/// Buffer size for one pass of the copy loop.
///
/// 64 KiB is large enough that the syscall overhead disappears against the copy
/// and small enough to sit on the heap without a visible allocation cost.
const COPY_CHUNK: usize = 65536;

/// The body of `sendfile64`.
///
/// # Safety
///
/// `offset` must be null or writable.
unsafe fn copy_descriptor(
    out_fd: c_int,
    in_fd: c_int,
    offset: *mut i64,
    count: usize,
) -> Result<usize, i32> {
    // Both descriptors are validated before anything is transferred.
    let input = kinakaze_vfs::get(in_fd)?;
    let _ = kinakaze_vfs::get(out_fd)?;
    // Linux requires the input support mmap-like reads, which in practice means a
    // regular file; a pipe as input is refused with EINVAL there too.
    if !input.flags.contains(FdFlags::SEEKABLE) {
        return Err(EINVAL);
    }

    // SAFETY: the caller guarantees `offset` is null or readable.
    let mut position = if offset.is_null() {
        None
    } else {
        let start = unsafe { *offset };
        if start < 0 {
            return Err(EINVAL);
        }
        Some(start as u64)
    };

    let mut buffer = vec![0u8; COPY_CHUNK.min(count.max(1))];
    let mut copied = 0usize;
    while copied < count {
        let wanted = (count - copied).min(buffer.len());
        let read = match position {
            // An explicit position: read there and leave the descriptor's own
            // offset alone.
            // SAFETY: the buffer is writable for `wanted` bytes.
            Some(at) => unsafe { read_at(in_fd, buffer.as_mut_ptr(), wanted, at) }?,
            None => kinakaze_vfs::read(in_fd, &mut buffer[..wanted])?,
        };
        if read == 0 {
            // End of input, which is not an error: the count is what was copied.
            break;
        }
        // The write is looped because a short write is legal, and stopping at one
        // would silently lose bytes that were read.
        let mut written = 0;
        while written < read {
            match kinakaze_vfs::write(out_fd, &buffer[written..read]) {
                Ok(0) => return Err(EIO),
                Ok(amount) => written += amount,
                Err(error) => {
                    // Bytes already transferred are reported, as POSIX requires,
                    // and the failure is only returned when nothing moved.
                    if copied + written > 0 {
                        if let Some(at) = position.as_mut() {
                            *at += written as u64;
                            // SAFETY: the caller guarantees `offset` is writable.
                            unsafe { *offset = *at as i64 };
                        }
                        return Ok(copied + written);
                    }
                    return Err(error);
                }
            }
        }
        copied += written;
        if let Some(at) = position.as_mut() {
            *at += written as u64;
        }
        if read < wanted {
            // A short read means end of input for a regular file.
            break;
        }
    }

    if let Some(at) = position {
        // SAFETY: the caller guarantees `offset` is writable when non-null.
        unsafe { *offset = at as i64 };
    }
    Ok(copied)
}

/// `fchdir`.
///
/// Retain the directory object in the guest fs_struct, including an old-root
/// O_PATH descriptor which is no longer reachable through the current root.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fchdir(fd: c_int) -> c_int {
    posix(crate::fs::restart_metadata(|| change_directory(fd)))
}
fn change_directory(fd: c_int) -> Result<(), i32> {
    fs::fchdir(fd)
}

// ---------------------------------------------------------------------------
// Timestamps.
//
// `SetFileTime` is the whole mechanism. Two things need care: the special
// nanosecond values, which select "now" and "leave alone" rather than being
// times, and the access rights, since a handle opened for reading cannot write
// attributes.
// ---------------------------------------------------------------------------

/// Seconds between the Windows FILETIME epoch (1601) and the Unix epoch (1970).
const FILETIME_TO_UNIX_SECONDS: i64 = 11_644_473_600;

/// `UTIME_NOW`: use the current time for this component.
const UTIME_NOW: i64 = (1 << 30) - 1;
/// `UTIME_OMIT`: leave this component unchanged.
const UTIME_OMIT: i64 = (1 << 30) - 2;

/// The Linux `struct timespec`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TimeSpec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// The Linux `struct timeval`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TimeVal {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

/// What to write for one timestamp component.
#[derive(Clone, Copy)]
enum Stamp {
    /// Write this exact time.
    At(FileTime),
    /// Write the current time.
    Now,
    /// Leave the component as it is.
    Omit,
}

/// The current time as a FILETIME.
fn now_filetime() -> FileTime {
    let mut time = FileTime::default();
    // SAFETY: `time` is a writable local; the call cannot fail.
    unsafe { GetSystemTimeAsFileTime(&raw mut time) };
    time
}

/// Converts Unix seconds and nanoseconds into a FILETIME.
fn unix_to_filetime(seconds: i64, nanoseconds: i64) -> Result<FileTime, i32> {
    let ticks = seconds
        .checked_add(FILETIME_TO_UNIX_SECONDS)
        .and_then(|shifted| shifted.checked_mul(10_000_000))
        .and_then(|ticks| ticks.checked_add(nanoseconds / 100))
        .ok_or(EOVERFLOW)?;
    // A time before 1601 cannot be represented at all.
    if ticks < 0 {
        return Err(EINVAL);
    }
    let ticks = ticks as u64;
    Ok(FileTime {
        low: ticks as u32,
        high: (ticks >> 32) as u32,
    })
}

/// Reads a `struct timespec` as a stamp, honouring the special nanoseconds.
///
/// Getting these wrong is the failure this function exists to prevent: 1073741823
/// and 1073741822 are `UTIME_NOW` and `UTIME_OMIT`, and treating either as a real
/// nanosecond count writes a timestamp roughly a second off, silently, on a file
/// the caller asked not to touch.
fn stamp_from_timespec(time: TimeSpec) -> Result<Stamp, i32> {
    match time.tv_nsec {
        UTIME_NOW => Ok(Stamp::Now),
        UTIME_OMIT => Ok(Stamp::Omit),
        nanoseconds if (0..1_000_000_000).contains(&nanoseconds) => {
            Ok(Stamp::At(unix_to_filetime(time.tv_sec, nanoseconds)?))
        }
        // Out of range and not one of the special values.
        _ => Err(EINVAL),
    }
}

/// Writes access and modification times through a handle.
///
/// # Safety
///
/// `handle` must be a live file or directory handle.
unsafe fn write_times(handle: *mut c_void, access: Stamp, modification: Stamp) -> Result<(), i32> {
    let now = now_filetime();
    let resolve = |stamp: Stamp| match stamp {
        Stamp::At(time) => Some(time),
        Stamp::Now => Some(now),
        Stamp::Omit => None,
    };
    let access = resolve(access);
    let modification = resolve(modification);
    // Public utimensat handles the both-omitted fast path before pathname
    // lookup, as Linux requires. Keep this internal helper harmless as well.
    if access.is_none() && modification.is_none() {
        return Ok(());
    }
    // A null pointer is how SetFileTime is told to leave a component alone, which
    // maps onto UTIME_OMIT exactly.
    let access_pointer = access.as_ref().map_or(ptr::null(), |time| &raw const *time);
    let write_pointer = modification
        .as_ref()
        .map_or(ptr::null(), |time| &raw const *time);
    // SAFETY: the handle is live and both pointers are null or address locals
    // that outlive the call. The creation time is left alone: POSIX has no
    // concept of it and overwriting it would destroy information.
    if unsafe { SetFileTime(handle, ptr::null(), access_pointer, write_pointer) } == 0 {
        return Err(last_errno());
    }
    Ok(())
}

/// Opens a path for the sole purpose of writing its timestamps.
///
/// `FILE_WRITE_ATTRIBUTES` is the only right needed, and asking for no more means
/// a read-only file's times can still be set, which POSIX allows.
/// `FILE_FLAG_BACKUP_SEMANTICS` is what makes a directory openable at all, and
/// directories are a case `utimensat` is routinely used on.
fn open_for_times(path: &str, follow: bool) -> Result<TimeHandle, i32> {
    let resolved = kinakaze_vfs::mount::overlay::prepare_write(path, follow, false)?;
    let encoded = kinakaze_vfs::path::wide_path(&resolved)?;
    let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
    if !follow {
        // AT_SYMLINK_NOFOLLOW: act on the link, not its target.
        flags |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    // SAFETY: `encoded` is a null-terminated wide string outliving the call.
    let handle = unsafe {
        CreateFileW(
            encoded.as_ptr(),
            FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(last_errno());
    }
    Ok(TimeHandle {
        raw: handle,
        _write_path: Some(resolved),
        metadata: None,
    })
}

/// A handle opened only to stamp a file, closed on every path out.
struct TimeHandle {
    raw: *mut c_void,
    // Keep overlay write ownership alive until the timestamp handle closes.
    _write_path: Option<kinakaze_vfs::mount::overlay::WritePath>,
    metadata: Option<kinakaze_vfs::mount::overlay::MetadataHandle>,
}

impl Drop for TimeHandle {
    fn drop(&mut self) {
        // SAFETY: this type owns the handle it was given.
        if self.metadata.is_none() {
            unsafe { CloseHandle(self.raw) };
        }
    }
}

/// Stamps the descriptor's inode with a handle carrying attribute access.
///
/// A descriptor opened `O_RDONLY` carries no `FILE_WRITE_ATTRIBUTES`, and POSIX
/// still allows `futimens` on it. ReOpenFile checks the additional access against
/// the same file object. Resolving its old pathname would hit a different inode
/// after rename/replacement and cannot address an unlinked open file at all.
fn open_descriptor_for_times(fd: c_int, allow_path: bool) -> Result<TimeHandle, i32> {
    let entry = kinakaze_vfs::get(fd)?;
    if !allow_path && entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    if let Some(handle) = kinakaze_vfs::mount::overlay::metadata_handle(fd, true, allow_path)? {
        use std::os::windows::io::AsRawHandle;
        return Ok(TimeHandle {
            raw: handle.as_raw_handle(),
            _write_path: None,
            metadata: Some(handle),
        });
    }
    // A synthetic /proc file has no host file to stamp. Reporting success would
    // claim a timestamp was written somewhere it cannot be.
    if matches!(
        entry.kind,
        FdKind::Synthetic | FdKind::SyntheticDirectory | FdKind::CgroupFile
    ) {
        return Err(EPERM);
    }
    if entry.raw == 0 {
        return Err(EBADF);
    }
    // SAFETY: the installed handle names the object being stamped. No pathname
    // lookup or compatibility retry is used to obtain the required access.
    let handle = unsafe {
        ReOpenFile(
            entry.raw as *mut c_void,
            FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(last_errno());
    }
    Ok(TimeHandle {
        raw: handle,
        _write_path: None,
        metadata: None,
    })
}

/// `utimensat`.
///
/// Honours directory-relative lookup, `AT_FDCWD`, `AT_SYMLINK_NOFOLLOW` and
/// Linux's `AT_EMPTY_PATH`, using the common directory-descriptor resolver.
///
/// A null `times` means "both to now", which is the `touch` case.
///
/// # Safety
///
/// `path` must be null or a null-terminated string, and `times` must be null or
/// point at two `struct timespec`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_utimensat(
    dirfd: c_int,
    path: *const c_char,
    times: *const TimeSpec,
    flags: c_int,
) -> c_int {
    // Linux's raw syscall returns before even checking flags/path when neither
    // timestamp is to change. Seconds are ignored for these special values.
    if !times.is_null()
        && unsafe { (*times).tv_nsec == UTIME_OMIT && (*times.add(1)).tv_nsec == UTIME_OMIT }
    {
        return 0;
    }
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        set_errno(EINVAL);
        return -1;
    }
    let follow = flags & AT_SYMLINK_NOFOLLOW == 0;

    // Linux's extension: a null path, or AT_EMPTY_PATH, makes `dirfd` the target
    // itself. glibc's `futimens` is implemented on exactly this.
    if path.is_null() || flags & AT_EMPTY_PATH != 0 {
        if path.is_null() && flags != 0 {
            set_errno(EINVAL);
            return -1;
        }
        if path.is_null() && dirfd == AT_FDCWD {
            set_errno(EFAULT);
            return -1;
        }
        // An empty string with AT_EMPTY_PATH also names the descriptor.
        // SAFETY: the caller guarantees a null-terminated string when non-null.
        let empty = path.is_null() || unsafe { CStr::from_ptr(path) }.to_bytes().is_empty();
        if empty {
            if dirfd != AT_FDCWD
                && kinakaze_vfs::get(dirfd).is_ok_and(|e| {
                    matches!(
                        e.kind,
                        FdKind::TmpfsFile
                            | FdKind::TmpfsDirectory
                            | FdKind::MessageQueue
                            | FdKind::SysfsFile
                    )
                })
            {
                if path.is_null()
                    && kinakaze_vfs::get(dirfd).is_ok_and(|e| e.flags.contains(FdFlags::PATH_ONLY))
                {
                    return posix(Err(EBADF));
                }
                return posix(unsafe { tmpfs_times(None, dirfd, follow, times) });
            }
            let handle = if dirfd == AT_FDCWD {
                open_for_times(".", follow)
            } else {
                open_descriptor_for_times(dirfd, !path.is_null())
            };
            // SAFETY: the caller provides the timespec array. Linux resolves
            // the target before validating nanosecond values.
            return posix(handle.and_then(|handle| unsafe { stamp_timespec(handle, times) }));
        }
    }

    // SAFETY: forwarded from this function's contract.
    let path = match unsafe { borrow_path(path) } {
        Ok(path) => path,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    if path.is_empty() {
        set_errno(kinakaze_vfs::ENOENT);
        return -1;
    }
    if let Ok(target) = crate::fsextra::resolve_at(dirfd, path) {
        if kinakaze_vfs::tmpfs::owns(&target) {
            return posix(unsafe { tmpfs_times(Some(&target), dirfd, follow, times) });
        }
    }
    posix(
        crate::fsextra::resolve_at(dirfd, path)
            .and_then(|path| open_for_times(&path, follow))
            // SAFETY: forwarded from this function's contract.
            .and_then(|handle| unsafe { stamp_timespec(handle, times) }),
    )
}

unsafe fn tmpfs_times(
    path: Option<&str>,
    fd: i32,
    follow: bool,
    times: *const TimeSpec,
) -> Result<(), i32> {
    let values = if times.is_null() {
        [[0, UTIME_NOW]; 2]
    } else {
        unsafe {
            [
                [(*times).tv_sec, (*times).tv_nsec],
                [(*times.add(1)).tv_sec, (*times.add(1)).tv_nsec],
            ]
        }
    };
    kinakaze_vfs::tmpfs::set_times(path, fd, follow, values)
}

/// Validates timestamps only after resolving a live target, as Linux does.
unsafe fn stamp_timespec(handle: TimeHandle, times: *const TimeSpec) -> Result<(), i32> {
    // SAFETY: the caller supplies two readable timespecs or NULL.
    let (access, modification) = unsafe { read_timespec_pair(times) }?;
    // SAFETY: the owned handle stays live throughout the write.
    unsafe { write_times(handle.raw, access, modification) }
}

/// Stamps a path.
fn stamp_path(path: &str, follow: bool, access: Stamp, modification: Stamp) -> Result<(), i32> {
    let handle = open_for_times(path, follow)?;
    // SAFETY: the handle is live until the guard drops.
    unsafe { write_times(handle.raw, access, modification) }
}

/// Reads the two-element `struct timespec` array `utimensat` takes.
///
/// # Safety
///
/// `times` must be null or point at two readable `struct timespec`.
unsafe fn read_timespec_pair(times: *const TimeSpec) -> Result<(Stamp, Stamp), i32> {
    if times.is_null() {
        // A null array sets both to the current time.
        return Ok((Stamp::Now, Stamp::Now));
    }
    // SAFETY: the caller guarantees two readable structs.
    let access = unsafe { *times };
    // SAFETY: as above.
    let modification = unsafe { *times.add(1) };
    Ok((
        stamp_from_timespec(access)?,
        stamp_from_timespec(modification)?,
    ))
}

/// `futimens`.
///
/// # Safety
///
/// `times` must be null or point at two `struct timespec`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_futimens(fd: c_int, times: *const TimeSpec) -> c_int {
    // glibc rejects negative fd values before entering the raw syscall.
    if fd < 0 {
        set_errno(EBADF);
        return -1;
    }
    // SAFETY: forwarded from this function's contract; NULL selects fd mode.
    unsafe { kinakaze_abi_utimensat(fd, ptr::null(), times, 0) }
}

/// Timestamp an open inode; converting through futimens preserves unlink semantics.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_futimes(fd: c_int, times: *const TimeVal) -> c_int {
    if fd < 0 {
        set_errno(EBADF);
        return -1;
    }
    unsafe { kinakaze_abi_futimesat(fd, ptr::null(), times) }
}

/// Microsecond timestamps relative to an open directory, or to fd itself for NULL.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_futimesat(
    fd: c_int,
    path: *const c_char,
    times: *const TimeVal,
) -> c_int {
    if times.is_null() {
        return unsafe { kinakaze_abi_utimensat(fd, path, ptr::null(), 0) };
    }
    let times = unsafe { core::slice::from_raw_parts(times, 2) };
    if times
        .iter()
        .any(|time| !(0..1_000_000).contains(&time.tv_usec))
    {
        set_errno(EINVAL);
        return -1;
    }
    let converted = [
        TimeSpec {
            tv_sec: times[0].tv_sec,
            tv_nsec: times[0].tv_usec * 1000,
        },
        TimeSpec {
            tv_sec: times[1].tv_sec,
            tv_nsec: times[1].tv_usec * 1000,
        },
    ];
    unsafe { kinakaze_abi_utimensat(fd, path, converted.as_ptr(), 0) }
}

/// `utimes`, the microsecond-resolution predecessor of `utimensat`.
///
/// `struct timeval` has no `UTIME_NOW` or `UTIME_OMIT`: a null array means both to
/// now, and any array given sets both. The microseconds are scaled to nanoseconds,
/// which loses nothing — Windows keeps 100-nanosecond ticks, finer than either.
///
/// # Safety
///
/// `path` must be a null-terminated string and `times` null or two
/// `struct timeval`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_utimes(
    path: *const c_char,
    times: *const TimeVal,
) -> c_int {
    // SAFETY: forwarded from this function's contract.
    let path = match unsafe { borrow_path(path) } {
        Ok(path) => path,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    let (access, modification) = if times.is_null() {
        (Stamp::Now, Stamp::Now)
    } else {
        // SAFETY: the caller guarantees two readable structs.
        let access = unsafe { *times };
        // SAFETY: as above.
        let modification = unsafe { *times.add(1) };
        // POSIX requires a microsecond field in range; out of range is EINVAL
        // rather than a silently wrapped timestamp.
        if !(0..1_000_000).contains(&access.tv_usec)
            || !(0..1_000_000).contains(&modification.tv_usec)
        {
            set_errno(EINVAL);
            return -1;
        }
        let converted = unix_to_filetime(access.tv_sec, access.tv_usec * 1000).and_then(|access| {
            unix_to_filetime(modification.tv_sec, modification.tv_usec * 1000)
                .map(|modification| (access, modification))
        });
        match converted {
            Ok((access, modification)) => (Stamp::At(access), Stamp::At(modification)),
            Err(error) => {
                set_errno(error);
                return -1;
            }
        }
    };
    // `utimes` follows symlinks; `lutimes` is the form that does not.
    posix(stamp_path(path, true, access, modification))
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn utimes(path: *const c_char, times: *const TimeVal) -> c_int {
    unsafe { kinakaze_abi_utimes(path, times) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn lutimes(path: *const c_char, times: *const TimeVal) -> c_int {
    unsafe { kinakaze_abi_lutimes(path, times) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lutimes(
    path: *const c_char,
    times: *const TimeVal,
) -> c_int {
    let path = match unsafe { borrow_path(path) } {
        Ok(path) => path,
        Err(error) => {
            set_errno(error);
            return -1;
        }
    };
    let (access, modification) = if times.is_null() {
        (Stamp::Now, Stamp::Now)
    } else {
        let access = unsafe { *times };
        let modification = unsafe { *times.add(1) };
        if !(0..1_000_000).contains(&access.tv_usec)
            || !(0..1_000_000).contains(&modification.tv_usec)
        {
            set_errno(EINVAL);
            return -1;
        }
        let converted = unix_to_filetime(access.tv_sec, access.tv_usec * 1000).and_then(|access| {
            unix_to_filetime(modification.tv_sec, modification.tv_usec * 1000)
                .map(|modification| (access, modification))
        });
        match converted {
            Ok((access, modification)) => (Stamp::At(access), Stamp::At(modification)),
            Err(error) => {
                set_errno(error);
                return -1;
            }
        }
    };
    posix(stamp_path(path, false, access, modification))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdio::EOF;
    use kinakaze_vfs::{EAGAIN, ENOTDIR};

    /// A file in the host temporary directory, removed when the test ends.
    ///
    /// Tests here work against real files rather than fixtures: the whole point is
    /// to check what Windows actually does with an offset, a lock or a timestamp,
    /// and a mock would only confirm this module's own assumptions.
    struct TempFile {
        guest: String,
        native: PathBuf,
    }

    impl TempFile {
        /// Creates a uniquely named file and returns it with its guest path.
        fn new(tag: &str) -> Self {
            let mut random = [0u8; 8];
            random_bytes(&mut random).expect("system RNG");
            let suffix: String = random
                .iter()
                .map(|byte| {
                    char::from(TEMPLATE_ALPHABET[(*byte as usize) % TEMPLATE_ALPHABET.len()])
                })
                .collect();
            let native = std::env::temp_dir().join(format!("kinakaze-{tag}-{suffix}"));
            let guest = windows_to_linux(&native);
            Self { guest, native }
        }

        /// The guest path, which is what the ABI functions take.
        fn path(&self) -> &str {
            &self.guest
        }

        /// A null-terminated copy of the guest path.
        fn c_path(&self) -> std::ffi::CString {
            std::ffi::CString::new(self.guest.clone()).expect("path has no interior NUL")
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.native);
            let _ = std::fs::remove_dir_all(&self.native);
        }
    }

    /// Opens a guest path through the VFS, panicking with the errno on failure.
    fn open(path: &str, flags: c_int) -> c_int {
        fs::open(path, flags, 0o644).unwrap_or_else(|error| panic!("open {path}: errno {error}"))
    }

    /// Writes bytes to a descriptor, insisting all of them land.
    fn write_all(fd: c_int, bytes: &[u8]) {
        let mut written = 0;
        while written < bytes.len() {
            written += kinakaze_vfs::write(fd, &bytes[written..]).expect("write");
        }
    }

    #[test]
    fn struct_layouts_match_the_linux_abi() {
        // Guest code indexes these directly, so every size here is ABI.
        assert_eq!(size_of::<PollFd>(), 8, "struct pollfd is int + 2 shorts");
        assert_eq!(align_of::<PollFd>(), 4);
        let poll = PollFd::default();
        let base = (&raw const poll) as usize;
        assert_eq!((&raw const poll.events) as usize - base, 4);
        assert_eq!((&raw const poll.revents) as usize - base, 6);

        assert_eq!(size_of::<TimeSpec>(), 16);
        assert_eq!(size_of::<TimeVal>(), 16);
        // Two shorts, then 8-byte-aligned offsets, then a pid, then padding.
        assert_eq!(size_of::<Flock>(), 32);
        assert_eq!(align_of::<Flock>(), 8);
        let lock = Flock::default();
        let base = (&raw const lock) as usize;
        assert_eq!((&raw const lock.l_whence) as usize - base, 2);
        assert_eq!((&raw const lock.l_start) as usize - base, 8);
        assert_eq!((&raw const lock.l_len) as usize - base, 16);
        assert_eq!((&raw const lock.l_pid) as usize - base, 24);
    }

    #[test]
    fn fifo_readiness_filters_interest_but_always_reports_hangup_and_error() {
        let sample = kinakaze_vfs::fifo::Readiness {
            readable: true,
            writable: true,
            hangup: true,
            error: true,
        };
        assert_eq!(fifo_readiness(sample, 0), POLLHUP | POLLERR);
        assert_eq!(fifo_readiness(sample, POLLIN), POLLIN | POLLHUP | POLLERR);
        assert_eq!(fifo_readiness(sample, POLLOUT), POLLOUT | POLLHUP | POLLERR);
        assert_eq!(
            fifo_readiness(sample, POLLIN | POLLOUT),
            POLLIN | POLLOUT | POLLHUP | POLLERR
        );
        assert_eq!(fifo_readiness(Default::default(), POLLIN | POLLOUT), 0);
    }

    #[test]
    fn inode_fifo_libc_status_ioctl_and_evented_poll() {
        struct OwnedFd(i32);
        impl Drop for OwnedFd {
            fn drop(&mut self) {
                let _ = kinakaze_vfs::close(self.0);
            }
        }
        let fifo = TempFile::new("libc-inode-fifo");
        fs::create_fifo(fifo.path(), 0o600).unwrap();
        let reader = OwnedFd(open(fifo.path(), fs::O_RDONLY | fs::O_NONBLOCK));
        assert_eq!(kinakaze_vfs::get(reader.0).unwrap().kind, FdKind::Fifo);
        let writer = OwnedFd(open(fifo.path(), fs::O_WRONLY | fs::O_NONBLOCK));
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(reader.0, F_GETFL, 0) },
            fs::O_RDONLY | fs::O_NONBLOCK
        );
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(writer.0, F_GETFL, 0) },
            fs::O_WRONLY | fs::O_NONBLOCK
        );
        let capacity =
            kinakaze_vfs::fifo::capacity(reader.0, kinakaze_vfs::get(reader.0).unwrap()).unwrap();
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(reader.0, F_GETPIPE_SZ, 0) },
            capacity as c_int
        );
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(writer.0, F_SETPIPE_SZ, capacity - 1) },
            capacity as c_int
        );
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(writer.0, F_SETPIPE_SZ, capacity * 2) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EOPNOTSUPP);
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(writer.0, F_SETPIPE_SZ, usize::MAX) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        let duplicate = OwnedFd(unsafe { kinakaze_abi_fcntl64(reader.0, F_DUPFD_CLOEXEC, 0) });
        assert!(duplicate.0 >= 0);
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(duplicate.0, F_SETFL, fs::O_APPEND as usize) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(reader.0, F_GETFL, 0) },
            fs::O_RDONLY | fs::O_APPEND
        );
        assert_eq!(
            unsafe {
                kinakaze_abi_fcntl64(reader.0, F_SETFL, (fs::O_APPEND | fs::O_NONBLOCK) as usize)
            },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(duplicate.0, F_GETFL, 0) },
            fs::O_RDONLY | fs::O_APPEND | fs::O_NONBLOCK
        );

        let mut descriptors = [PollFd {
            fd: reader.0,
            events: POLLIN,
            revents: 0,
        }];
        assert_eq!(poll_wait(&mut descriptors, 0), Ok(0));
        let write_fd = writer.0;
        let producer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(40));
            kinakaze_vfs::write(write_fd, b"fifo")
        });
        let waited = poll_wait(&mut descriptors, 1_000);
        assert_eq!(producer.join().unwrap(), Ok(4));
        assert_eq!(waited, Ok(1));
        assert_eq!(descriptors[0].revents, POLLIN);
        let mut queued = -1;
        assert_eq!(
            unsafe {
                crate::term::kinakaze_abi_ioctl(
                    reader.0,
                    crate::term::FIONREAD,
                    (&raw mut queued).cast(),
                )
            },
            0
        );
        assert_eq!(queued, 4);
        let mut aliases = [
            PollFd {
                fd: reader.0,
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: reader.0,
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: writer.0,
                events: POLLOUT,
                revents: 0,
            },
        ];
        assert_eq!(poll_wait(&mut aliases, 0), Ok(3));
        assert_eq!(aliases[0].revents, POLLIN);
        assert_eq!(aliases[1].revents, POLLIN);
        assert_eq!(aliases[2].revents, POLLOUT);
        let mut received = [0; 4];
        assert_eq!(kinakaze_vfs::read(reader.0, &mut received), Ok(4));
        assert_eq!(&received, b"fifo");
        assert_eq!(poll_wait(&mut descriptors, 40), Ok(0));
        drop(writer);
        descriptors[0].events = 0;
        assert_eq!(poll_wait(&mut descriptors, 1_000), Ok(1));
        assert_eq!(
            descriptors[0].revents, POLLHUP,
            "HUP is independent of requested interest"
        );
        assert_eq!(
            unsafe {
                crate::term::kinakaze_abi_ioctl(
                    reader.0,
                    crate::term::FIONREAD,
                    (&raw mut queued).cast(),
                )
            },
            0
        );
        assert_eq!(queued, 0);
        let duplex = OwnedFd(open(fifo.path(), fs::O_RDWR | fs::O_NONBLOCK));
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(duplex.0, F_GETFL, 0) },
            fs::O_RDWR | fs::O_NONBLOCK
        );
    }

    #[test]
    fn map_failed_is_not_null() {
        // Every caller tests `== MAP_FAILED`. A null would pass that test and be
        // used as a mapping, so this is worth pinning.
        assert_eq!(MAP_FAILED as usize, usize::MAX);
        assert!(!MAP_FAILED.is_null());
    }

    fn enable_verity(fd: c_int) -> Result<(), i32> {
        let mut arg = [0u8; 128];
        arg[..4].copy_from_slice(&1u32.to_le_bytes());
        arg[4..8].copy_from_slice(&1u32.to_le_bytes());
        arg[8..12].copy_from_slice(&4096u32.to_le_bytes());
        let result =
            unsafe { crate::term::kinakaze_abi_ioctl(fd, 0x4080_6685, arg.as_mut_ptr().cast()) };
        if result == 0 {
            Ok(())
        } else {
            Err(crate::kinakaze_errno())
        }
    }

    fn verity_file(tag: &str, bytes: &[u8]) -> (TempFile, c_int) {
        let file = TempFile::new(tag);
        let writable = open(file.path(), fs::O_CREAT | fs::O_EXCL | fs::O_RDWR);
        write_all(writable, bytes);
        kinakaze_vfs::close(writable).unwrap();
        let fd = open(file.path(), fs::O_RDONLY);
        fs::verity::enable(fd, 1, 4096, &[]).expect("enable real fs-verity backend");
        (file, fd)
    }

    #[test]
    fn verity_ioctl_measure_metadata_flags_and_readonly_enforcement() {
        let (file, fd) = verity_file("verity-ioctl", b"protected content");
        let mut digest = [0u8; 68];
        digest[2..4].copy_from_slice(&64u16.to_le_bytes());
        assert_eq!(
            unsafe { crate::term::kinakaze_abi_ioctl(fd, 0xc004_6686, digest.as_mut_ptr().cast()) },
            0
        );
        assert_eq!(&digest[..4], &[1, 0, 32, 0]);
        digest[2..4].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(
            unsafe { crate::term::kinakaze_abi_ioctl(fd, 0xc004_6686, digest.as_mut_ptr().cast()) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), EOVERFLOW);
        let mut flags = [0u8; 8];
        flags[4..].fill(0x5a);
        assert_eq!(
            unsafe { crate::term::kinakaze_abi_ioctl(fd, 0x8008_6601, flags.as_mut_ptr().cast()) },
            0
        );
        assert_eq!(
            u32::from_le_bytes(flags[..4].try_into().unwrap()),
            0x10_0000
        );
        assert_eq!(&flags[4..], &[0x5a; 4]);
        flags[..4].fill(0);
        assert_eq!(
            unsafe { crate::term::kinakaze_abi_ioctl(fd, 0x4008_6602, flags.as_mut_ptr().cast()) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), EPERM);
        let mut descriptor = [0u8; 256];
        let mut request = [
            2u64,
            0,
            descriptor.len() as u64,
            descriptor.as_mut_ptr() as u64,
            0,
        ];
        assert_eq!(
            unsafe {
                crate::term::kinakaze_abi_ioctl(fd, 0xc028_6687, request.as_mut_ptr().cast())
            },
            256
        );
        assert_eq!(descriptor[0], 1);
        assert_eq!(
            u64::from_le_bytes(descriptor[8..16].try_into().unwrap()),
            17
        );
        assert_eq!(fs::fstat(fd).unwrap().st_size, 17);
        assert_eq!(fs::open(file.path(), fs::O_RDWR, 0o644), Err(EPERM));
        assert_eq!(kinakaze_abi_posix_fallocate64(fd, 0, 2), EBADF);
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn verity_mapping_private_discard_shared_protection_and_eof() {
        let page = memory_geometry().0;
        let (_file, fd) = verity_file("verity-map", &vec![0x42; page + 17]);
        let private = map(
            ptr::null_mut(),
            page * 3,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE,
            fd,
            0,
        )
        .unwrap();
        let shared = map(ptr::null_mut(), page * 3, PROT_READ, MAP_SHARED, fd, 0).unwrap();
        assert_eq!(
            map(ptr::null_mut(), page, PROT_WRITE, MAP_SHARED, fd, 0),
            Err(kinakaze_vfs::EACCES)
        );
        kinakaze_vfs::close(fd).unwrap();
        unsafe {
            *private.cast::<u8>() = 9;
        }
        assert_eq!(unsafe { *shared.cast::<u8>() }, 0x42);
        madvise_impl(private, page, MADV_DONTNEED).unwrap();
        assert_eq!(unsafe { *private.cast::<u8>() }, 0x42);
        assert_eq!(unsafe { *private.cast::<u8>().add(page + 17) }, 0);
        assert_eq!(
            unsafe { kinakaze_abi_mprotect(shared, page, PROT_WRITE) },
            -1
        );
        assert_eq!(crate::kinakaze_errno(), kinakaze_vfs::EACCES);
        let tail = unsafe { private.cast::<u8>().add(page * 2) }.cast();
        assert_eq!(
            unsafe { kinakaze_abi_mprotect(tail, page, PROT_READ | PROT_WRITE) },
            0
        );
        assert_eq!(
            verity::kinakaze_abi_mapping_fault_signal(tail as usize, 0),
            7
        );
        {
            let _registry = mappings().lock().unwrap();
            assert_eq!(
                verity::kinakaze_abi_mapping_fault_signal(tail as usize, 0),
                7,
                "synchronous fault must not need the mapping mutex"
            );
        }
        assert_eq!(unsafe { kinakaze_abi_mprotect(tail, page, PROT_NONE) }, 0);
        assert_eq!(
            verity::kinakaze_abi_mapping_fault_signal(tail as usize, 0),
            0
        );
        assert_eq!(unsafe { kinakaze_abi_mprotect(tail, page, PROT_READ) }, 0);
        assert_eq!(
            verity::kinakaze_abi_mapping_fault_signal(tail as usize, 0),
            7
        );
        assert_eq!(
            verity::kinakaze_abi_mapping_fault_signal(tail as usize, 1),
            0
        );
        assert_eq!(unsafe { kinakaze_abi_munmap(private, page * 3) }, 0);
        assert_eq!(unsafe { kinakaze_abi_munmap(shared, page * 3) }, 0);
    }

    #[test]
    fn verity_mapping_pin_survives_close_reuse_for_native_and_verified_paths() {
        let (page, granularity) = memory_geometry();
        for enabled in [false, true] {
            let original = TempFile::new("verity-map-pinned-original");
            let replacement = TempFile::new("verity-map-pinned-replacement");
            std::fs::write(&original.native, vec![0x46; page]).unwrap();
            std::fs::write(&replacement.native, vec![0xb9; page]).unwrap();
            let fd = open(original.path(), fs::O_RDONLY);
            let second = open(replacement.path(), fs::O_RDONLY);
            if enabled {
                fs::verity::enable(fd, 1, 4096, &[]).unwrap();
            } else {
                // Opposite states expose any split classification/native-open:
                // the replacement must never be mapped with the original pin.
                fs::verity::enable(second, 1, 4096, &[]).unwrap();
            }
            let opened = fs::verity::Opened::from_fd(fd).unwrap();
            let identity = verity::native_id(opened.borrowed_handle()).unwrap();
            let generation = opened.generation();
            kinakaze_vfs::close(fd).unwrap();
            assert_eq!(kinakaze_abi_dup2(second, fd), fd);
            assert_ne!(kinakaze_vfs::get(fd).unwrap().generation, generation);
            let mapped = map_opened_file(
                ptr::null_mut(),
                page * 2,
                PROT_READ,
                false,
                false,
                fd,
                &opened,
                0,
                granularity,
            )
            .unwrap();
            drop(opened);
            kinakaze_vfs::close(fd).unwrap();
            kinakaze_vfs::close(second).unwrap();
            assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x46);
            {
                let registry = mappings().lock().unwrap();
                let state = registry.get(&(mapped as usize)).unwrap();
                assert_eq!(state.verity.is_some(), enabled);
                if !enabled {
                    assert_eq!(state.native_inode, Some(identity));
                    assert_eq!(state.file.as_ref().unwrap().generation, generation);
                }
            }
            assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page * 2) }, 0);
        }
    }

    #[test]
    fn verity_mapping_fixed_partial_unmap_keeps_neighbors_and_clean_bytes() {
        let page = memory_geometry().0;
        let (_file, fd) = verity_file("verity-fixed", &vec![0x76; page * 3]);
        let mapped = map(
            ptr::null_mut(),
            page * 3,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE,
            fd,
            0,
        )
        .unwrap();
        let middle = unsafe { mapped.cast::<u8>().add(page) }.cast();
        assert_eq!(unsafe { kinakaze_abi_mprotect(middle, page, PROT_READ) }, 0);
        assert_eq!(
            unsafe { kinakaze_abi_mprotect(middle, page, PROT_READ | PROT_WRITE) },
            0
        );
        unsafe {
            *middle.cast::<u8>() = 0x23;
        }
        madvise_impl(middle, page, MADV_DONTNEED).unwrap();
        assert_eq!(unsafe { *middle.cast::<u8>() }, 0x76);
        let replacement = map(
            middle,
            page,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
            -1,
            0,
        )
        .unwrap();
        assert_eq!(replacement, middle);
        assert_eq!(unsafe { *middle.cast::<u8>() }, 0);
        assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x76);
        assert_eq!(unsafe { *mapped.cast::<u8>().add(page * 2) }, 0x76);
        assert_eq!(unsafe { kinakaze_abi_munmap(middle, page) }, 0);
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page * 3) }, 0);
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn verity_mapping_tampered_page_is_bus_fault_without_exposing_bytes() {
        use std::io::{Seek, SeekFrom, Write};
        let page = memory_geometry().0;
        let (file, fd) = verity_file("verity-tamper", &vec![0x17; page * 2]);
        let mut native = std::fs::OpenOptions::new()
            .write(true)
            .open(&file.native)
            .unwrap();
        native.seek(SeekFrom::Start(page as u64)).unwrap();
        native.write_all(&[0x99]).unwrap();
        drop(native);
        let mapped = map(ptr::null_mut(), page * 2, PROT_READ, MAP_PRIVATE, fd, 0).unwrap();
        assert_eq!(unsafe { *mapped.cast::<u8>() }, 0x17);
        assert_eq!(
            verity::kinakaze_abi_mapping_fault_signal(mapped as usize + page, 0),
            7
        );
        assert_eq!(
            unsafe { kinakaze_abi_mprotect(mapped, page * 2, PROT_READ | PROT_WRITE) },
            0
        );
        assert_eq!(
            verity::kinakaze_abi_mapping_fault_signal(mapped as usize + page, 0),
            7
        );
        let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
        unsafe {
            VirtualQuery(
                (mapped as usize + page) as *const c_void,
                &mut info,
                size_of::<MemoryBasicInformation>(),
            );
        }
        assert_eq!(info.protect, PAGE_NOACCESS);
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page * 2) }, 0);
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn verity_enable_rejects_existing_unverified_local_mapping_after_fd_close() {
        let file = TempFile::new("verity-existing-map");
        let write_fd = open(file.path(), fs::O_CREAT | fs::O_RDWR);
        write_all(write_fd, b"map before verity");
        kinakaze_vfs::close(write_fd).unwrap();
        let fd = open(file.path(), fs::O_RDONLY);
        let mapped = map(ptr::null_mut(), 4096, PROT_READ, MAP_PRIVATE, fd, 0).unwrap();
        kinakaze_vfs::close(fd).unwrap();
        let second = open(file.path(), fs::O_RDONLY);
        assert_eq!(
            verity::reject_unverified_mappings(second),
            Err(kinakaze_vfs::EBUSY)
        );
        assert_eq!(enable_verity(second), Err(kinakaze_vfs::EBUSY));
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, 4096) }, 0);
        verity::reject_unverified_mappings(second).unwrap();
        assert_eq!(enable_verity(second), Ok(()));
        kinakaze_vfs::close(second).unwrap();
    }

    fn mapping_lease_lock(handle: *mut c_void, exclusive: bool) -> Result<(), u32> {
        unsafe extern "system" {
            fn LockFileEx(
                file: *mut c_void,
                flags: u32,
                reserved: u32,
                low: u32,
                high: u32,
                overlapped: *mut Overlapped,
            ) -> i32;
        }
        let mut at = Overlapped::at(1u64 << 62);
        if unsafe { LockFileEx(handle, 1 | if exclusive { 2 } else { 0 }, 0, 1, 0, &mut at) } == 0 {
            Err(unsafe { GetLastError() })
        } else {
            Ok(())
        }
    }

    fn open_mapping_lease_file(path: &Path) -> *mut c_void {
        let encoded = kinakaze_vfs::path::wide_path(path).expect("valid mapping lease path");
        let handle = unsafe {
            CreateFileW(
                encoded.as_ptr(),
                0x8000_0000,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                0x4000_0000,
                ptr::null_mut(),
            )
        };
        assert_ne!(
            handle,
            INVALID_HANDLE_VALUE,
            "readonly open failed {}",
            unsafe { GetLastError() }
        );
        handle
    }

    #[test]
    fn verity_native_mapping_lease_process_helper() {
        let Some(path) = std::env::var_os("KINAKAZE_VERITY_LEASE_PROBE_PATH") else {
            return;
        };
        let handle = open_mapping_lease_file(Path::new(&path));
        let shared = std::env::var_os("KINAKAZE_VERITY_LEASE_PROBE_SHARED").is_some();
        let result = mapping_lease_lock(handle, !shared);
        println!("LEASE_RESULT={}", result.err().unwrap_or(0));
        if let Ok(target) = std::env::var("KINAKAZE_VERITY_LEASE_PROBE_TARGET_PID") {
            let target = target.parse::<u32>().unwrap();
            let process =
                unsafe { windows_sys::Win32::System::Threading::OpenProcess(0x0040, 0, target) };
            assert!(!process.is_null());
            let mut transferred = ptr::null_mut();
            assert_ne!(
                unsafe {
                    DuplicateHandle(
                        GetCurrentProcess(),
                        handle,
                        process,
                        &mut transferred,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                },
                0
            );
            println!("LEASE_TRANSFERRED_HANDLE={}", transferred as usize);
            unsafe {
                CloseHandle(process);
            }
        }
        if shared {
            std::process::exit(if result.is_ok() { 23 } else { 24 });
        }
        unsafe {
            CloseHandle(handle);
        }
    }

    fn remote_mapping_lease(path: &Path, shared: bool, transfer: bool) -> std::process::Output {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "fdio::tests::verity_native_mapping_lease_process_helper",
                "--nocapture",
            ])
            .env("KINAKAZE_VERITY_LEASE_PROBE_PATH", path)
            .creation_flags(0x0800_0000);
        if shared {
            command.env("KINAKAZE_VERITY_LEASE_PROBE_SHARED", "1");
        }
        if transfer {
            command.env(
                "KINAKAZE_VERITY_LEASE_PROBE_TARGET_PID",
                std::process::id().to_string(),
            );
        }
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        let waited = unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(child.as_raw_handle(), 5_000)
        };
        if waited != 0 {
            let _ = child.kill();
            let _ = child.wait();
            panic!("native lease helper exceeded bounded wait: {waited}");
        }
        child.wait_with_output().unwrap()
    }

    #[test]
    fn verity_native_readonly_mapping_lease_lifetime_and_crossprocess_probe() {
        let file = TempFile::new("verity-native-lease");
        std::fs::write(&file.native, vec![0x64; 4096]).unwrap();
        let lease = open_mapping_lease_file(&file.native);
        assert_eq!(
            mapping_lease_lock(lease, false),
            Ok(()),
            "readonly handle permits shared lease"
        );
        let mut duplicate = ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    lease,
                    GetCurrentProcess(),
                    &mut duplicate,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let section =
            unsafe { CreateFileMappingW(lease, ptr::null(), PAGE_READONLY, 0, 0, ptr::null()) };
        assert!(!section.is_null());
        let view = unsafe { MapViewOfFileEx(section, FILE_MAP_READ, 0, 0, 4096, ptr::null_mut()) };
        assert!(!view.is_null());
        unsafe {
            CloseHandle(section);
            CloseHandle(lease);
        }
        let blocked = remote_mapping_lease(&file.native, false, false);
        assert!(
            blocked.status.success(),
            "{}",
            String::from_utf8_lossy(&blocked.stderr)
        );
        assert!(
            String::from_utf8_lossy(&blocked.stdout).contains("LEASE_RESULT=33"),
            "{}",
            String::from_utf8_lossy(&blocked.stdout)
        );
        unsafe {
            CloseHandle(duplicate);
        }
        let released = remote_mapping_lease(&file.native, false, false);
        assert!(released.status.success());
        assert!(
            String::from_utf8_lossy(&released.stdout).contains("LEASE_RESULT=0"),
            "{}",
            String::from_utf8_lossy(&released.stdout)
        );
        assert_eq!(
            unsafe { *view.cast::<u8>() },
            0x64,
            "view outlives lease unless owner retains handle"
        );
        let dead_owner = remote_mapping_lease(&file.native, true, false);
        assert_eq!(dead_owner.status.code(), Some(23));
        let after_death = remote_mapping_lease(&file.native, false, false);
        assert!(String::from_utf8_lossy(&after_death.stdout).contains("LEASE_RESULT=0"));
        unsafe {
            UnmapViewOfFile(view);
        }
        let transfer = remote_mapping_lease(&file.native, true, true);
        assert_eq!(transfer.status.code(), Some(23));
        let output = String::from_utf8_lossy(&transfer.stdout);
        let inherited = output
            .lines()
            .find_map(|line| line.strip_prefix("LEASE_TRANSFERRED_HANDLE="))
            .expect("child publishes transferred native handle")
            .parse::<usize>()
            .unwrap();
        let after_transfer = remote_mapping_lease(&file.native, false, false);
        let held = String::from_utf8_lossy(&after_transfer.stdout).contains("LEASE_RESULT=33");
        // Windows byte-range locks belong to the locking process as well as
        // its file object: DuplicateHandle into another process does NOT
        // transfer the source process's lease when that process terminates.
        let reacquired = mapping_lease_lock(inherited as *mut c_void, false);
        let after_reacquire = remote_mapping_lease(&file.native, false, false);
        unsafe {
            CloseHandle(inherited as *mut c_void);
        }
        assert!(
            !held,
            "cross-process transfer probe changed native lease semantics: {}",
            String::from_utf8_lossy(&after_transfer.stdout)
        );
        assert_eq!(reacquired, Ok(()));
        assert!(String::from_utf8_lossy(&after_reacquire.stdout).contains("LEASE_RESULT=33"));
        let final_release = remote_mapping_lease(&file.native, false, false);
        assert!(String::from_utf8_lossy(&final_release.stdout).contains("LEASE_RESULT=0"));
    }

    #[test]
    fn verity_fault_index_readers_and_protection_publish_concurrently() {
        let page = memory_geometry().0;
        let (_file, fd) = verity_file("verity-fault-index", b"verified");
        let mapped = map(ptr::null_mut(), page * 2, PROT_READ, MAP_PRIVATE, fd, 0).unwrap();
        let tail = mapped as usize + page;
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(move || {
                    for _ in 0..25_000 {
                        assert!(matches!(
                            verity::kinakaze_abi_mapping_fault_signal(tail, 0),
                            0 | 7
                        ));
                    }
                });
            }
            for iteration in 0..200 {
                let protection = if iteration & 1 == 0 {
                    PROT_NONE
                } else {
                    PROT_READ
                };
                assert_eq!(
                    unsafe { kinakaze_abi_mprotect(tail as *mut c_void, page, protection) },
                    0
                );
            }
        });
        assert_eq!(verity::kinakaze_abi_mapping_fault_signal(tail, 0), 7);
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page * 2) }, 0);
        assert_eq!(verity::kinakaze_abi_mapping_fault_signal(tail, 0), 0);
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    #[ignore = "explicit disposable fixture preparation for a real guest fs-verity probe"]
    fn verity_prepare_disposable_guest_fixtures() {
        use std::io::{Seek, SeekFrom, Write};
        let mut nonce = [0u8; 8];
        random_bytes(&mut nonce).unwrap();
        let nonce = u64::from_ne_bytes(nonce);
        let root = std::env::temp_dir().join(format!(
            "kinakaze-verity-guest-{}-{nonce:x}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        for (name, bytes) in [
            ("good", vec![0x42u8; 4096 + 17]),
            ("corrupt", vec![0x65u8; 4096 * 3]),
            ("empty", Vec::new()),
        ] {
            let native = root.join(name);
            std::fs::write(&native, bytes).unwrap();
            let guest = windows_to_linux(&native);
            let fd = fs::open(&guest, fs::O_RDONLY, 0).unwrap();
            fs::verity::enable(fd, 1, 4096, &[]).unwrap();
            kinakaze_vfs::close(fd).unwrap();
            if name == "corrupt" {
                let mut tamper = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&native)
                    .unwrap();
                tamper.seek(SeekFrom::Start(4096)).unwrap();
                tamper.write_all(&[0x99]).unwrap();
                tamper.sync_all().unwrap();
            }
        }
        println!("VERITY_FIXTURE_ROOT={}", root.display());
    }

    #[test]
    fn pread_reads_at_an_offset_without_moving_the_position() {
        let file = TempFile::new("pread");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"0123456789ABCDEF");

        // Put the position somewhere recognizable, then read elsewhere.
        let before = fs::lseek(fd, 4, SEEK_SET).expect("seek");
        assert_eq!(before, 4);

        let mut buffer = [0u8; 4];
        // SAFETY: the buffer is a writable local of the stated length.
        let read = unsafe {
            kinakaze_abi_pread64(fd, buffer.as_mut_ptr().cast::<c_void>(), buffer.len(), 10)
        };
        assert_eq!(read, 4, "pread should return the full count");
        assert_eq!(&buffer, b"ABCD", "pread read from the wrong offset");

        // The defining property: the position is exactly where it was.
        let after = fs::lseek(fd, 0, SEEK_CUR).expect("seek");
        assert_eq!(after, 4, "pread must not move the file offset");

        // And an ordinary read still continues from there.
        let mut next = [0u8; 2];
        assert_eq!(kinakaze_vfs::read(fd, &mut next).expect("read"), 2);
        assert_eq!(&next, b"45", "the position was disturbed after all");

        // Past the end of file is zero bytes, not an error.
        // SAFETY: the buffer is a writable local.
        let past = unsafe {
            kinakaze_abi_pread64(fd, buffer.as_mut_ptr().cast::<c_void>(), buffer.len(), 4096)
        };
        assert_eq!(past, 0, "reading past EOF is end of file, not failure");

        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn pread_refuses_a_descriptor_with_no_position() {
        let mut fds = [0 as c_int; 2];
        // SAFETY: two writable locals.
        assert_eq!(unsafe { kinakaze_abi_pipe(fds.as_mut_ptr()) }, 0);
        let mut buffer = [0u8; 4];
        set_errno(0);
        // SAFETY: the buffer is a writable local.
        let read =
            unsafe { kinakaze_abi_pread64(fds[0], buffer.as_mut_ptr().cast::<c_void>(), 4, 0) };
        assert_eq!(read, -1);
        assert_eq!(
            kinakaze_tls::errno(),
            ESPIPE,
            "a pipe has no offset to read at"
        );
        let _ = kinakaze_vfs::close(fds[0]);
        let _ = kinakaze_vfs::close(fds[1]);
    }

    #[test]
    fn dup_produces_an_independent_descriptor_on_the_same_file() {
        let file = TempFile::new("dup");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"shared contents");

        let copy = kinakaze_abi_dup(fd);
        assert!(copy >= 0, "dup failed with errno {}", kinakaze_tls::errno());
        assert_ne!(copy, fd, "dup must return a new descriptor");

        // The duplicate starts where the original was, which is at the end after
        // the write above.
        let original = fs::lseek(fd, 0, SEEK_CUR).expect("seek");
        assert_eq!(
            fs::lseek(copy, 0, SEEK_CUR).expect("seek"),
            original,
            "the duplicate should start at the original's position"
        );

        // It reads the same file.
        fs::lseek(copy, 0, SEEK_SET).expect("seek");
        let mut buffer = [0u8; 6];
        assert_eq!(kinakaze_vfs::read(copy, &mut buffer).expect("read"), 6);
        assert_eq!(&buffer, b"shared");

        // Closing one leaves the other usable, which is what proves the handles
        // are genuinely separate rather than one value stored twice.
        assert!(kinakaze_vfs::close(copy).is_ok());
        assert!(
            fs::lseek(fd, 0, SEEK_SET).is_ok(),
            "closing the duplicate closed the original's handle"
        );

        set_errno(0);
        assert_eq!(kinakaze_abi_dup(9999), -1, "dup of a bad fd must fail");
        assert_eq!(kinakaze_tls::errno(), EBADF);

        let _ = kinakaze_vfs::close(fd);
    }

    static DUP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn dup2_lands_on_the_requested_descriptor_and_closes_what_was_there() {
        let _guard = DUP_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let first = TempFile::new("dup2-a");
        let second = TempFile::new("dup2-b");
        let source = open(first.path(), fs::O_RDWR | fs::O_CREAT);
        let target = open(second.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(source, b"AAAA");
        write_all(target, b"BBBB");

        // dup2 onto a live descriptor: the target must become the source's file.
        assert_eq!(
            kinakaze_abi_dup2(source, target),
            target,
            "dup2 must return newfd"
        );
        fs::lseek(target, 0, SEEK_SET).expect("seek");
        let mut buffer = [0u8; 4];
        assert_eq!(kinakaze_vfs::read(target, &mut buffer).expect("read"), 4);
        assert_eq!(
            &buffer, b"AAAA",
            "dup2 did not rebind the target descriptor"
        );

        // The source is untouched.
        fs::lseek(source, 0, SEEK_SET).expect("seek");
        assert_eq!(kinakaze_vfs::read(source, &mut buffer).expect("read"), 4);
        assert_eq!(&buffer, b"AAAA");

        let _ = kinakaze_vfs::close(target);
        let _ = kinakaze_vfs::close(source);
    }

    #[test]
    fn dup2_preserves_handleless_character_devices() {
        let _guard = DUP_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let source = open("/dev/null", fs::O_RDWR);
        let replaced = TempFile::new("dup2-dev-null-target");
        let target = open(replaced.path(), fs::O_RDWR | fs::O_CREAT);

        assert_eq!(kinakaze_abi_dup2(source, target), target);
        kinakaze_vfs::close(source).unwrap();
        assert_eq!(kinakaze_vfs::get(target).unwrap().kind, FdKind::Null);
        assert_eq!(kinakaze_vfs::write(target, b"discarded").unwrap(), 9);
        let mut byte = [0u8; 1];
        assert_eq!(kinakaze_vfs::read(target, &mut byte).unwrap(), 0);

        kinakaze_vfs::close(target).unwrap();
    }

    #[test]
    fn dup2_copies_writable_proc_sysctl_identity() {
        let _guard = DUP_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let path = "/proc/sys/net/ipv4/ip_forward";
        kinakaze_vfs::procfs::write_file(path, b"0\n", 0).unwrap();
        let source = open(path, fs::O_WRONLY);
        let replaced = TempFile::new("dup2-proc-sysctl-target");
        let target = open(replaced.path(), fs::O_RDWR | fs::O_CREAT);

        assert_eq!(kinakaze_abi_dup2(source, target), target);
        kinakaze_vfs::close(source).unwrap();
        assert_eq!(kinakaze_vfs::get(target).unwrap().kind, FdKind::ProcSysctl);
        assert_eq!(kinakaze_vfs::write(target, b"1\n"), Ok(2));
        assert_eq!(
            kinakaze_vfs::procfs::read_file(path).unwrap(),
            b"1\n".to_vec()
        );

        kinakaze_vfs::close(target).unwrap();
        kinakaze_vfs::procfs::write_file(path, b"0\n", 0).unwrap();
    }

    #[test]
    fn dup2_copies_unix_socket_side_table_state() {
        let _guard = DUP_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (source, peer) =
            kinakaze_vfs::unix::socketpair(kinakaze_vfs::socket::SOCK_STREAM).unwrap();
        let replaced = TempFile::new("dup2-unix-target");
        let target = open(replaced.path(), fs::O_RDWR | fs::O_CREAT);

        assert_eq!(kinakaze_abi_dup2(source, target), target);
        // `xmove_fd`, used by BusyBox's TLS helper, closes the source
        // immediately after dup2. The target must retain a complete socket
        // endpoint rather than just its raw named-pipe handle.
        kinakaze_vfs::close(source).unwrap();
        assert_eq!(kinakaze_vfs::write(target, b"tls").unwrap(), 3);
        let mut buffer = [0u8; 3];
        assert_eq!(kinakaze_vfs::read(peer, &mut buffer).unwrap(), 3);
        assert_eq!(&buffer, b"tls");

        kinakaze_vfs::close(target).unwrap();
        kinakaze_vfs::close(peer).unwrap();
    }

    #[test]
    fn dup2_onto_itself_is_a_noop_that_preserves_the_descriptor() {
        let _guard = DUP_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let file = TempFile::new("dup2-self");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"survives");

        // The detail shells depend on: `exec 3>&3` must not close fd 3.
        assert_eq!(kinakaze_abi_dup2(fd, fd), fd);
        fs::lseek(fd, 0, SEEK_SET).expect("seek");
        let mut buffer = [0u8; 8];
        assert_eq!(
            kinakaze_vfs::read(fd, &mut buffer).expect("read"),
            8,
            "dup2(fd, fd) closed the descriptor it was asked to keep"
        );
        assert_eq!(&buffer, b"survives");

        // A closed fd is still EBADF even in the self case.
        set_errno(0);
        assert_eq!(kinakaze_abi_dup2(9999, 9999), -1);
        assert_eq!(kinakaze_tls::errno(), EBADF);

        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn f_dupfd_respects_the_floor_and_descriptor_flags() {
        let file = TempFile::new("dupfd");
        let fd = open(file.path(), fs::O_RDONLY | fs::O_CREAT);

        // Tests in this process deliberately share the real descriptor table,
        // so another parallel test may already own the floor. The VFS unit test
        // covers exact lowest-slot selection on an isolated table; this test
        // covers the libc integration and flag contract.
        let floor = 400;
        // SAFETY: F_DUPFD takes an integer argument, not a pointer.
        let copy = unsafe { kinakaze_abi_fcntl64(fd, F_DUPFD, floor as usize) };
        assert!(
            copy >= floor,
            "F_DUPFD returned {copy}, below the requested floor {floor}"
        );
        // A duplicate never inherits FD_CLOEXEC.
        // SAFETY: F_GETFD takes no argument.
        assert_eq!(unsafe { kinakaze_abi_fcntl64(copy, F_GETFD, 0) }, 0);

        // F_DUPFD_CLOEXEC sets it.
        // SAFETY: as above.
        let cloexec = unsafe { kinakaze_abi_fcntl64(fd, F_DUPFD_CLOEXEC, floor as usize) };
        assert!(cloexec >= floor);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(cloexec, F_GETFD, 0) },
            FD_CLOEXEC,
            "F_DUPFD_CLOEXEC must set the flag on the copy"
        );

        let _ = kinakaze_vfs::close(cloexec);
        let _ = kinakaze_vfs::close(copy);
        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn fcntl_reports_and_sets_the_descriptor_flag() {
        let file = TempFile::new("fcntl-fd");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);

        // Opened without O_CLOEXEC, so the flag is clear.
        // SAFETY: F_GETFD takes no argument.
        assert_eq!(unsafe { kinakaze_abi_fcntl64(fd, F_GETFD, 0) }, 0);

        // Setting it is honoured for real: the handle's inherit bit is what
        // decides whether a fork sees the descriptor, and it is writable.
        // SAFETY: F_SETFD takes an integer.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(fd, F_SETFD, FD_CLOEXEC as usize) },
            0
        );
        // SAFETY: F_GETFD takes no argument.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(fd, F_GETFD, 0) },
            FD_CLOEXEC,
            "F_SETFD did not take effect"
        );
        // And clearing it again.
        // SAFETY: F_SETFD takes an integer.
        assert_eq!(unsafe { kinakaze_abi_fcntl64(fd, F_SETFD, 0) }, 0);
        // SAFETY: F_GETFD takes no argument.
        assert_eq!(unsafe { kinakaze_abi_fcntl64(fd, F_GETFD, 0) }, 0);

        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn fcntl_reports_the_real_access_mode() {
        let file = TempFile::new("fcntl-fl");
        // Create it first so the read-only open below has something to open.
        let created = open(file.path(), fs::O_WRONLY | fs::O_CREAT);
        let _ = kinakaze_vfs::close(created);

        for (flags, expected) in [
            (fs::O_RDONLY, fs::O_RDONLY),
            (fs::O_WRONLY, fs::O_WRONLY),
            (fs::O_RDWR, fs::O_RDWR),
        ] {
            let fd = open(file.path(), flags);
            // SAFETY: F_GETFL takes no argument.
            let reported = unsafe { kinakaze_abi_fcntl64(fd, F_GETFL, 0) };
            assert_eq!(
                reported & fs::O_ACCMODE,
                expected,
                "F_GETFL reported the wrong access mode for open flags {flags:o}"
            );
            // The round trip every program makes must succeed: reading the flags
            // and writing them straight back changes nothing.
            // SAFETY: F_SETFL takes an integer.
            assert_eq!(
                unsafe { kinakaze_abi_fcntl64(fd, F_SETFL, reported as usize) },
                0,
                "the F_GETFL/F_SETFL round trip must succeed"
            );
            let _ = kinakaze_vfs::close(fd);
        }

        // O_APPEND is read from the table, so it must survive the trip too.
        let fd = open(file.path(), fs::O_WRONLY | fs::O_APPEND);
        // SAFETY: F_GETFL takes no argument.
        let reported = unsafe { kinakaze_abi_fcntl64(fd, F_GETFL, 0) };
        assert_ne!(
            reported & fs::O_APPEND,
            0,
            "F_GETFL should report O_APPEND on a descriptor opened with it"
        );
        // F_SETFL changes the shared open-description flags just as Linux does.
        // SAFETY: F_SETFL takes an integer.
        let cleared =
            unsafe { kinakaze_abi_fcntl64(fd, F_SETFL, (reported & !fs::O_APPEND) as usize) };
        assert_eq!(cleared, 0, "clearing O_APPEND must succeed");
        // SAFETY: F_GETFL takes no argument.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(fd, F_GETFL, 0) } & fs::O_APPEND,
            0
        );

        // Setting O_NONBLOCK is likewise observable through F_GETFL.
        // SAFETY: F_SETFL takes an integer.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(fd, F_SETFL, fs::O_NONBLOCK as usize) },
            0
        );
        // SAFETY: F_GETFL takes no argument.
        assert_ne!(
            unsafe { kinakaze_abi_fcntl64(fd, F_GETFL, 0) } & fs::O_NONBLOCK,
            0
        );
        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn f_setfl_makes_an_empty_pipe_nonblocking() {
        let mut fds = [-1; 2];
        // SAFETY: `fds` has room for the two descriptors returned by pipe.
        assert_eq!(unsafe { kinakaze_abi_pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: F_SETFL takes an integer.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(fds[0], F_SETFL, fs::O_NONBLOCK as usize) },
            0
        );

        let mut byte = [0u8; 1];
        assert_eq!(kinakaze_vfs::read(fds[0], &mut byte), Err(EAGAIN));
        assert_eq!(kinakaze_vfs::write(fds[1], b"x"), Ok(1));
        assert_eq!(kinakaze_vfs::read(fds[0], &mut byte), Ok(1));
        assert_eq!(byte, [b'x']);

        let _ = kinakaze_vfs::close(fds[0]);
        let _ = kinakaze_vfs::close(fds[1]);
    }

    #[test]
    fn fcntl_locks_and_unlocks_a_byte_range() {
        let file = TempFile::new("fcntl-lock");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, &[0u8; 256]);

        let mut lock = Flock {
            l_type: F_WRLCK,
            l_whence: SEEK_SET as i16,
            l_start: 0,
            l_len: 64,
            l_pid: 0,
        };
        // SAFETY: `lock` is a writable local of the right type.
        let taken = unsafe { kinakaze_abi_fcntl64(fd, F_SETLK, (&raw mut lock) as usize) };
        assert_eq!(
            taken,
            0,
            "F_SETLK failed with errno {}",
            kinakaze_tls::errno()
        );

        // POSIX locks belong to the process, not the individual open handle.
        let other = open(file.path(), fs::O_RDWR);
        let mut probe = Flock {
            l_type: F_WRLCK,
            l_whence: SEEK_SET as i16,
            l_start: 0,
            l_len: 64,
            l_pid: 0,
        };
        set_errno(0);
        // SAFETY: `probe` is a writable local.
        let conflict = unsafe { kinakaze_abi_fcntl64(other, F_SETLK, (&raw mut probe) as usize) };
        assert_eq!(conflict, 0, "a process must not conflict with its own lock");

        // F_GETLK ignores the calling process's locks and changes only l_type.
        let mut query = Flock {
            l_type: F_WRLCK,
            l_whence: SEEK_SET as i16,
            l_start: 0,
            l_len: 64,
            l_pid: 0,
        };
        // SAFETY: `query` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(other, F_GETLK, (&raw mut query) as usize) },
            0
        );
        assert_eq!(query.l_type, F_UNLCK);
        assert_eq!(query.l_pid, 0);

        // An unrelated range is free, and F_GETLK says so.
        let mut free = Flock {
            l_type: F_WRLCK,
            l_whence: SEEK_SET as i16,
            l_start: 128,
            l_len: 64,
            l_pid: 0,
        };
        // SAFETY: `free` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(other, F_GETLK, (&raw mut free) as usize) },
            0
        );
        assert_eq!(
            free.l_type, F_UNLCK,
            "F_GETLK should report an unlocked range as free"
        );

        // Releasing lets the other descriptor take it.
        lock.l_type = F_UNLCK;
        // SAFETY: `lock` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(fd, F_SETLK, (&raw mut lock) as usize) },
            0
        );
        probe.l_type = F_WRLCK;
        // SAFETY: `probe` is a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_fcntl64(other, F_SETLK, (&raw mut probe) as usize) },
            0,
            "the range should be available once the first lock is released"
        );
        probe.l_type = F_UNLCK;
        // SAFETY: `probe` is a writable local.
        unsafe { kinakaze_abi_fcntl64(other, F_SETLK, (&raw mut probe) as usize) };

        let _ = kinakaze_vfs::close(other);
        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn pipe_installs_two_working_ends() {
        let mut fds = [0 as c_int; 2];
        // SAFETY: two writable locals.
        assert_eq!(unsafe { kinakaze_abi_pipe(fds.as_mut_ptr()) }, 0);
        let [read, write] = fds;
        assert!(read >= 0 && write >= 0);
        assert_ne!(read, write);

        // Both are in the table, as pipes.
        assert_eq!(
            kinakaze_vfs::get(read).expect("read end").kind,
            FdKind::Pipe
        );
        assert_eq!(
            kinakaze_vfs::get(write).expect("write end").kind,
            FdKind::Pipe
        );

        write_all(write, b"through the pipe");
        let mut buffer = [0u8; 16];
        let mut filled = 0;
        while filled < buffer.len() {
            let count = kinakaze_vfs::read(read, &mut buffer[filled..]).expect("read");
            assert_ne!(count, 0, "the pipe reported EOF with a writer still open");
            filled += count;
        }
        assert_eq!(&buffer, b"through the pipe");

        // A pipe has no file position.
        assert_eq!(fs::lseek(read, 0, SEEK_CUR), Err(ESPIPE));

        // F_GETPIPE_SZ reports the real capacity rather than a constant.
        // SAFETY: F_GETPIPE_SZ takes no argument.
        let capacity = unsafe { kinakaze_abi_fcntl64(write, F_GETPIPE_SZ, 0) };
        assert!(
            capacity > 0,
            "F_GETPIPE_SZ should report the buffer the pipe was created with"
        );

        let _ = kinakaze_vfs::close(read);
        let _ = kinakaze_vfs::close(write);

        // A null array is EFAULT, not a crash.
        set_errno(0);
        // SAFETY: passing null is exactly what is being tested.
        assert_eq!(unsafe { kinakaze_abi_pipe(ptr::null_mut()) }, -1);
        assert_eq!(kinakaze_tls::errno(), EFAULT);
    }

    #[test]
    fn poll_reports_a_pty_readable_only_when_a_line_is_complete() {
        use kinakaze_vfs::fs::{O_NOCTTY, O_RDWR};

        let master = kinakaze_vfs::tty::open_master(O_RDWR | O_NOCTTY).unwrap();
        kinakaze_vfs::tty::set_slave_lock(master, false).unwrap();
        let slave = kinakaze_vfs::tty::open_slave(
            kinakaze_vfs::tty::pty_number(master).unwrap(),
            O_RDWR | O_NOCTTY,
        )
        .unwrap();

        let mut watch = [PollFd {
            fd: slave,
            events: POLLIN,
            revents: 0,
        }];
        // SAFETY: one writable pollfd.
        assert_eq!(unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 0) }, 0);

        // Half a line: queued, but poll must not claim it is readable, or the
        // read that follows would block inside a non-blocking loop.
        kinakaze_vfs::write(master, b"typing").unwrap();
        watch[0].revents = 0;
        // SAFETY: as above.
        assert_eq!(unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 0) }, 0);

        // The terminator makes it readable, and the blocking wait ends on the
        // terminal's own event rather than after the re-examination interval.
        kinakaze_vfs::write(master, b"\r").unwrap();
        watch[0].revents = 0;
        // SAFETY: as above.
        assert_eq!(unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 1000) }, 1);
        assert_eq!(watch[0].revents & POLLIN, POLLIN);

        // FIONREAD agrees with poll: seven bytes, the line plus its newline.
        let mut available = 0 as c_int;
        // SAFETY: FIONREAD takes a writable int.
        assert_eq!(
            unsafe {
                crate::term::kinakaze_abi_ioctl(
                    slave,
                    crate::term::FIONREAD,
                    (&raw mut available).cast(),
                )
            },
            0
        );
        assert_eq!(available, 7);

        let _ = kinakaze_vfs::close(slave);
        let _ = kinakaze_vfs::close(master);
    }

    #[test]
    fn pselect_swaps_the_signal_mask_for_the_duration_of_the_wait() {
        // A mask that blocks nothing, installed over one that blocks SIGUSR1.
        let blocked = 1u64 << (kinakaze_vfs::signal::SIGUSR1 as u64 - 1);
        let original = kinakaze_vfs::signal::swap_blocked_mask(blocked);

        let mut read_set = FdSet { fds_bits: [0; 16] };
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: 1_000_000,
        };
        let empty: u64 = 0;
        // SAFETY: every pointer is a readable or writable local.
        let result = unsafe {
            kinakaze_abi_pselect(
                0,
                &raw mut read_set,
                ptr::null_mut(),
                ptr::null_mut(),
                &raw const timeout,
                &raw const empty,
            )
        };
        assert_eq!(
            result, 0,
            "an empty pselect should time out and report zero"
        );
        // The caller's mask is back: leaving it swapped would silently change
        // every later wait on this thread.
        assert_eq!(
            kinakaze_vfs::signal::blocked_mask(),
            blocked,
            "pselect did not restore the caller's signal mask"
        );

        // A negative nanosecond field is EINVAL rather than an enormous wait.
        let invalid = Timespec {
            tv_sec: 0,
            tv_nsec: -1,
        };
        // SAFETY: as above.
        assert_eq!(
            unsafe {
                kinakaze_abi_pselect(
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &raw const invalid,
                    ptr::null(),
                )
            },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        kinakaze_vfs::signal::swap_blocked_mask(original);
    }

    #[test]
    fn poll_distinguishes_a_pipe_with_data_from_one_without() {
        let mut fds = [0 as c_int; 2];
        // SAFETY: two writable locals.
        assert_eq!(unsafe { kinakaze_abi_pipe(fds.as_mut_ptr()) }, 0);
        let [read, write] = fds;

        // Empty pipe: not readable. This is the case the VFS's epoll cannot
        // answer — it reports every non-socket ready — so it is the reason this
        // module queries pipe state for itself.
        let mut watch = [PollFd {
            fd: read,
            events: POLLIN,
            revents: 0x7f,
        }];
        // SAFETY: one writable entry.
        let ready = unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 0) };
        assert_eq!(ready, 0, "an empty pipe must not report readable");
        assert_eq!(watch[0].revents, 0, "revents must be cleared on every call");

        // The write end is writable, since the pipe is empty.
        let mut writable = [PollFd {
            fd: write,
            events: POLLOUT,
            revents: 0,
        }];
        // SAFETY: one writable entry.
        assert_eq!(unsafe { kinakaze_abi_poll(writable.as_mut_ptr(), 1, 0) }, 1);
        assert_eq!(writable[0].revents & POLLOUT, POLLOUT);

        // With data in it, readable.
        write_all(write, b"x");
        // SAFETY: one writable entry.
        let ready = unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 0) };
        assert_eq!(ready, 1, "a pipe holding data must report readable");
        assert_eq!(watch[0].revents & POLLIN, POLLIN);

        // Draining it makes it unreadable again, which proves the answer comes
        // from the pipe and is not a constant.
        let mut byte = [0u8; 1];
        assert_eq!(kinakaze_vfs::read(read, &mut byte).expect("read"), 1);
        // SAFETY: one writable entry.
        assert_eq!(unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 0) }, 0);

        // Closing the writer is a hangup, reported whether or not it was asked
        // for.
        let _ = kinakaze_vfs::close(write);
        // SAFETY: one writable entry.
        let ready = unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 0) };
        assert_eq!(ready, 1, "a closed writer must wake the reader");
        assert_ne!(
            watch[0].revents & POLLHUP,
            0,
            "the reader should see POLLHUP once the writer is gone"
        );

        let _ = kinakaze_vfs::close(read);
    }

    #[test]
    fn poll_unions_interests_for_duplicate_socket_slots() {
        let (socket, peer) =
            kinakaze_vfs::unix::socketpair(kinakaze_vfs::socket::SOCK_STREAM).unwrap();
        let mut watch = [
            PollFd {
                fd: socket,
                events: POLLIN,
                revents: 0,
            },
            PollFd {
                fd: socket,
                events: POLLOUT,
                revents: 0,
            },
        ];

        // One epoll registration backs both pollfd slots. Its interest must be
        // the union, and an event requested only by the second slot must not be
        // discarded merely because the registration cookie names the first.
        // SAFETY: both pollfd entries are writable.
        assert_eq!(unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 2, 0) }, 1);
        assert_eq!(watch[0].revents, 0);
        assert_eq!(watch[1].revents & POLLOUT, POLLOUT);

        kinakaze_vfs::write(peer, b"x").unwrap();
        watch.iter_mut().for_each(|entry| entry.revents = 0);
        // SAFETY: both pollfd entries remain writable.
        assert_eq!(unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 2, 0) }, 2);
        assert_eq!(watch[0].revents & POLLIN, POLLIN);
        assert_eq!(watch[1].revents & POLLOUT, POLLOUT);

        let _ = kinakaze_vfs::close(peer);
        let _ = kinakaze_vfs::close(socket);
    }

    #[test]
    fn poll_reports_connected_socket_writability_as_a_level() {
        use std::os::windows::io::IntoRawSocket;

        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        server.set_nonblocking(true).unwrap();
        let fd = kinakaze_vfs::install(
            server.into_raw_socket() as usize,
            FdKind::Socket,
            FdFlags::NONE,
        )
        .unwrap();

        for _ in 0..3 {
            let mut watch = [PollFd {
                fd,
                events: POLLOUT,
                revents: 0,
            }];
            // SAFETY: one writable pollfd entry.
            assert_eq!(unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 0) }, 1);
            assert_eq!(watch[0].revents & POLLOUT, POLLOUT);
        }

        kinakaze_vfs::close(fd).unwrap();
        drop(client);
    }

    #[test]
    fn socket_revents_from_a_blocking_wait_are_counted_on_the_next_pass() {
        let entries = [
            PollFd {
                fd: 3,
                events: POLLIN,
                revents: POLLIN,
            },
            PollFd {
                fd: 4,
                events: POLLIN,
                revents: 0,
            },
        ];

        // `PollWatcher::collect` refuses to overwrite a nonzero revents slot.
        // The next pass must therefore count this preserved result before doing
        // its zero-timeout probe, in addition to preclassified invalid fds.
        assert_eq!(preserved_ready_count(&entries, &[0, 1], 1), 2);
    }

    #[test]
    fn poll_reports_a_regular_file_ready_and_a_bad_fd_invalid() {
        let file = TempFile::new("poll-file");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);

        // A regular file is always ready, in Linux too.
        let mut watch = [
            PollFd {
                fd,
                events: POLLIN | POLLOUT,
                revents: 0,
            },
            // A negative fd is skipped without being an error, which is how
            // callers disable a slot.
            PollFd {
                fd: -1,
                events: POLLIN,
                revents: 0x7f,
            },
            // A closed fd reports POLLNVAL whether or not it was requested.
            PollFd {
                fd: 9999,
                events: 0,
                revents: 0,
            },
        ];
        // SAFETY: three writable entries.
        let ready = unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 3, 0) };
        assert_eq!(ready, 2, "the file and the invalid descriptor should count");
        assert_eq!(watch[0].revents, POLLIN | POLLOUT);
        assert_eq!(watch[1].revents, 0, "a negative fd must be left alone");
        assert_eq!(watch[2].revents, POLLNVAL);

        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn poll_returns_zero_when_a_timeout_expires() {
        let mut fds = [0 as c_int; 2];
        // SAFETY: two writable locals.
        assert_eq!(unsafe { kinakaze_abi_pipe(fds.as_mut_ptr()) }, 0);
        let mut watch = [PollFd {
            fd: fds[0],
            events: POLLIN,
            revents: 0,
        }];

        let started = std::time::Instant::now();
        // SAFETY: one writable entry.
        let mut ready = unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 40) };
        if ready < 0 && kinakaze_tls::errno() == kinakaze_vfs::EINTR {
            ready = unsafe { kinakaze_abi_poll(watch.as_mut_ptr(), 1, 40) };
        }
        let waited = started.elapsed();
        assert_eq!(ready, 0, "nothing became ready, so the count is zero");
        // The wait really happened rather than returning immediately. The bound is
        // loose because a loaded machine can oversleep.
        assert!(
            waited >= std::time::Duration::from_millis(25),
            "poll returned after {waited:?}, which is too soon for a 40ms timeout"
        );

        let _ = kinakaze_vfs::close(fds[0]);
        let _ = kinakaze_vfs::close(fds[1]);
    }

    #[test]
    fn getline_grows_the_buffer_across_several_lines() {
        let file = TempFile::new("getline");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        // A short line, then one far longer than the initial allocation, so the
        // doubling path really runs.
        let long = "y".repeat(400);
        write_all(fd, format!("short\n{long}\nlast").as_bytes());
        let _ = kinakaze_vfs::close(fd);

        let path = file.c_path();
        // SAFETY: both arguments are null-terminated.
        let stream = unsafe { crate::stdio::fopen(path.as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null(), "fopen failed");

        let mut line: *mut c_char = ptr::null_mut();
        let mut capacity = 0usize;

        // First line, into a buffer this call has to allocate.
        // SAFETY: both out-parameters are writable locals; `line` starts null,
        // which `getline` is required to accept.
        let read = unsafe { kinakaze_abi_getline(&raw mut line, &raw mut capacity, stream) };
        assert_eq!(read, 6, "the newline counts towards the length");
        assert!(!line.is_null(), "getline must allocate a buffer");
        let first_capacity = capacity;
        assert!(capacity >= 7, "capacity must cover the line and its NUL");
        // SAFETY: `getline` NUL-terminates the buffer.
        assert_eq!(unsafe { CStr::from_ptr(line) }.to_bytes(), b"short\n");

        // Second line forces the buffer to grow, and both the pointer and the
        // capacity must be updated in place.
        // SAFETY: as above.
        let read = unsafe { kinakaze_abi_getline(&raw mut line, &raw mut capacity, stream) };
        assert_eq!(read, 401);
        assert!(
            capacity > first_capacity,
            "the buffer should have grown from {first_capacity} to hold a 400-byte line"
        );
        // SAFETY: as above.
        let text = unsafe { CStr::from_ptr(line) }.to_bytes();
        assert_eq!(text.len(), 401);
        assert!(text.starts_with(b"yyy") && text.ends_with(b"\n"));

        // A final line with no newline is still returned.
        // SAFETY: as above.
        let read = unsafe { kinakaze_abi_getline(&raw mut line, &raw mut capacity, stream) };
        assert_eq!(read, 4);
        // SAFETY: as above.
        assert_eq!(unsafe { CStr::from_ptr(line) }.to_bytes(), b"last");

        // End of file is -1 with errno untouched, which is how a caller tells EOF
        // from a failure.
        set_errno(0);
        // SAFETY: as above.
        let read = unsafe { kinakaze_abi_getline(&raw mut line, &raw mut capacity, stream) };
        assert_eq!(read, -1);
        assert_eq!(
            kinakaze_tls::errno(),
            0,
            "getline must not set errno at end of file"
        );

        // The buffer came from this library's allocator, so the guest's `free`
        // releases it.
        // SAFETY: `line` is an allocation from `kinakaze_realloc`.
        unsafe { crate::kinakaze_free(line.cast::<c_void>()) };
        // SAFETY: the stream came from `fopen` and is not used again.
        unsafe { crate::stdio::fclose(stream) };
    }

    #[test]
    fn getdelim_splits_on_the_requested_byte() {
        let file = TempFile::new("getdelim");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"alpha:beta:");
        let _ = kinakaze_vfs::close(fd);

        let path = file.c_path();
        // SAFETY: both arguments are null-terminated.
        let stream = unsafe { crate::stdio::fopen(path.as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null());

        let mut line: *mut c_char = ptr::null_mut();
        let mut capacity = 0usize;
        // SAFETY: writable locals.
        let read = unsafe {
            kinakaze_abi_getdelim(&raw mut line, &raw mut capacity, c_int::from(b':'), stream)
        };
        assert_eq!(read, 6);
        // SAFETY: the buffer is NUL-terminated.
        assert_eq!(unsafe { CStr::from_ptr(line) }.to_bytes(), b"alpha:");

        // SAFETY: `line` is an allocation from this library.
        unsafe { crate::kinakaze_free(line.cast::<c_void>()) };
        // SAFETY: the stream is not used again.
        unsafe { crate::stdio::fclose(stream) };
    }

    #[test]
    fn mkstemp_creates_distinct_files_that_did_not_exist() {
        let directory = std::env::temp_dir();
        let template = |name: &str| {
            let path = windows_to_linux(&directory.join(name));
            std::ffi::CString::new(path).expect("no interior NUL")
        };

        let mut first = template("kinakaze-mkstemp-XXXXXX").into_bytes_with_nul();
        let mut second = template("kinakaze-mkstemp-XXXXXX").into_bytes_with_nul();

        // SAFETY: both are writable, null-terminated, and end in XXXXXX.
        let first_fd = unsafe { kinakaze_abi_mkstemp64(first.as_mut_ptr().cast::<c_char>()) };
        assert!(
            first_fd >= 0,
            "mkstemp failed with errno {}",
            kinakaze_tls::errno()
        );
        // SAFETY: as above.
        let second_fd = unsafe { kinakaze_abi_mkstemp64(second.as_mut_ptr().cast::<c_char>()) };
        assert!(second_fd >= 0);

        // The two names must differ, or the second call opened the first file.
        assert_ne!(
            first, second,
            "two mkstemp calls produced the same name, so the suffix is not random"
        );
        // The template is rewritten in place with no X left.
        assert!(!first.windows(6).any(|window| window == b"XXXXXX"));

        // Both are real, writable files.
        write_all(first_fd, b"first");
        write_all(second_fd, b"second");

        let name_of = |bytes: &[u8]| {
            let text = CStr::from_bytes_until_nul(bytes).expect("terminator");
            text.to_str().expect("utf-8").to_string()
        };
        let first_path = name_of(&first);
        let second_path = name_of(&second);
        assert!(
            fs::stat(&first_path).is_ok(),
            "mkstemp did not create the file"
        );
        assert!(fs::stat(&second_path).is_ok());

        let _ = kinakaze_vfs::close(first_fd);
        let _ = kinakaze_vfs::close(second_fd);
        for path in [&first_path, &second_path] {
            if let Ok(native) = kinakaze_vfs::resolve_linux_path(path) {
                let _ = std::fs::remove_file(native);
            }
        }

        // A template without the six marks is rejected rather than guessed at.
        let mut bad = template("kinakaze-mkstemp-bad").into_bytes_with_nul();
        set_errno(0);
        // SAFETY: writable and null-terminated.
        assert_eq!(
            unsafe { kinakaze_abi_mkstemp64(bad.as_mut_ptr().cast::<c_char>()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn temporary_suffix_flags_and_plain_entry_share_creation_semantics() {
        let path = windows_to_linux(&std::env::temp_dir().join("kinakaze-suffix-XXXXXX.conf"));
        let mut template = std::ffi::CString::new(path).unwrap().into_bytes_with_nul();
        let fd = unsafe {
            kinakaze_abi_mkostemps(
                template.as_mut_ptr().cast(),
                5,
                fs::O_CLOEXEC | fs::O_APPEND,
            )
        };
        assert!(fd >= 0, "errno={}", kinakaze_tls::errno());
        assert_eq!(unsafe { kinakaze_abi_fcntl64(fd, F_GETFD, 0) } & 1, 1);
        assert_ne!(
            unsafe { kinakaze_abi_fcntl64(fd, F_GETFL, 0) } & fs::O_APPEND,
            0
        );
        let name = CStr::from_bytes_until_nul(&template)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(name.ends_with(".conf"));
        assert_eq!(fs::stat(name).unwrap().st_mode & 0o777, 0o600);
        write_all(fd, b"suffix-data");
        kinakaze_vfs::close(fd).unwrap();
        fs::unlink(name).unwrap();

        for (pattern, suffix) in [
            (b"badXXXXXX.dat\0".as_slice(), -1),
            (b"short\0", 1),
            (b"badYYYYYY.dat\0", 4),
            (b"XXXXXX\0", i32::MAX),
        ] {
            let mut template = pattern.to_vec();
            assert_eq!(
                unsafe { kinakaze_abi_mkstemps(template.as_mut_ptr().cast(), suffix) },
                -1
            );
            assert_eq!(kinakaze_tls::errno(), EINVAL);
            assert_eq!(template, pattern);
        }
        let mut invalid = *b"no-marks-here\0";
        unsafe extern "sysv64" {
            fn kinakaze_abi_mkstemp(template: *mut c_char) -> c_int;
        }
        assert_eq!(
            unsafe { kinakaze_abi_mkstemp(invalid.as_mut_ptr().cast()) },
            -1
        );
        assert_eq!(invalid, *b"no-marks-here\0");
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn mkdtemp_creates_a_directory_and_mktemp_only_a_name() {
        let directory = std::env::temp_dir();
        let mut template =
            std::ffi::CString::new(windows_to_linux(&directory.join("kinakaze-mkdtemp-XXXXXX")))
                .expect("no interior NUL")
                .into_bytes_with_nul();

        // SAFETY: writable, null-terminated, ending in XXXXXX.
        let result = unsafe { kinakaze_abi_mkdtemp(template.as_mut_ptr().cast::<c_char>()) };
        assert_eq!(
            result,
            template.as_mut_ptr().cast::<c_char>(),
            "mkdtemp returns the template it rewrote"
        );
        let path = CStr::from_bytes_until_nul(&template)
            .expect("terminator")
            .to_str()
            .expect("utf-8")
            .to_string();
        let info = fs::stat(&path).expect("mkdtemp did not create the directory");
        assert_eq!(
            info.st_mode & fs::S_IFMT,
            fs::S_IFDIR,
            "mkdtemp must create a directory, not a file"
        );
        if let Ok(native) = kinakaze_vfs::resolve_linux_path(&path) {
            let _ = std::fs::remove_dir_all(native);
        }

        // mktemp generates a name and creates nothing, which is exactly why it is
        // deprecated.
        let mut name =
            std::ffi::CString::new(windows_to_linux(&directory.join("kinakaze-mktemp-XXXXXX")))
                .expect("no interior NUL")
                .into_bytes_with_nul();
        // SAFETY: writable, null-terminated, ending in XXXXXX.
        unsafe { kinakaze_abi_mktemp(name.as_mut_ptr().cast::<c_char>()) };
        let generated = CStr::from_bytes_until_nul(&name)
            .expect("terminator")
            .to_str()
            .expect("utf-8")
            .to_string();
        assert!(
            !generated.is_empty(),
            "mktemp cleared the template, which means it found no free name"
        );
        assert!(
            fs::stat(&generated).is_err(),
            "mktemp must not create anything"
        );
    }

    #[test]
    fn anonymous_mmap_round_trips_through_memory() {
        let (page, _) = memory_geometry();
        let length = page * 3;
        // SAFETY: an anonymous mapping needs no descriptor; -1 is the conventional
        // fd for it.
        let mapped = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                length,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(
            mapped,
            MAP_FAILED,
            "anonymous mmap failed with errno {}",
            kinakaze_tls::errno()
        );
        assert!(!mapped.is_null());
        // A mapping is page-aligned.
        assert_eq!(mapped as usize % page, 0);

        // Write across the whole region and read it back, which is what proves the
        // pages are really committed rather than merely reserved.
        // SAFETY: the mapping is `length` writable bytes.
        let bytes = unsafe { core::slice::from_raw_parts_mut(mapped.cast::<u8>(), length) };
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        assert!(
            bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == (index % 251) as u8)
        );
        // The last byte of the last page is inside the mapping.
        assert_eq!(bytes[length - 1], ((length - 1) % 251) as u8);

        // SAFETY: `mapped` came from `mmap64` and is not used again.
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, length) }, 0);

        // A zero length is EINVAL, as it is on Linux.
        set_errno(0);
        // SAFETY: the call is expected to fail without mapping anything.
        let bad = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                0,
                PROT_READ,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_eq!(bad, MAP_FAILED);
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        // Neither MAP_SHARED nor MAP_PRIVATE is also EINVAL.
        set_errno(0);
        // SAFETY: as above.
        let bad =
            unsafe { kinakaze_abi_mmap64(ptr::null_mut(), page, PROT_READ, MAP_ANONYMOUS, -1, 0) };
        assert_eq!(bad, MAP_FAILED);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn anonymous_munmap_preserves_allocation_base_across_trims() {
        let (page, _) = memory_geometry();
        let length = page * 9;
        // V8 reserves inaccessible address space, trims both ends, and later
        // releases the surviving middle. This sequence proves the registry does
        // not replace the Windows allocation base with a fragment start.
        let mapped = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                length,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(mapped, MAP_FAILED);

        let start = mapped as usize;
        // SAFETY: every call names a page-aligned sub-range of `mapped`, and no
        // removed range is accessed afterwards.
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page * 3) }, 0);
        assert_eq!(
            unsafe { kinakaze_abi_munmap((start + page * 8) as *mut c_void, page) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_munmap((start + page * 3) as *mut c_void, page * 5) },
            0
        );

        let registry = mappings().lock().expect("mapping registry");
        assert!(
            registry
                .keys()
                .all(|fragment| *fragment < start || *fragment >= start + length),
            "the final munmap must remove every fragment of the reservation"
        );
    }

    #[test]
    fn anonymous_munmap_splits_an_interior_hole() {
        let (page, _) = memory_geometry();
        let length = page * 5;
        let mapped = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                length,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(mapped, MAP_FAILED);
        let start = mapped as usize;

        // Remove the middle first, then independently remove both surviving
        // fragments. The Windows reservation is released only by the last call.
        assert_eq!(
            unsafe { kinakaze_abi_munmap((start + page * 2) as *mut c_void, page) },
            0
        );
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page * 2) }, 0);
        assert_eq!(
            unsafe { kinakaze_abi_munmap((start + page * 3) as *mut c_void, page * 2) },
            0
        );

        let registry = mappings().lock().expect("mapping registry");
        assert!(
            registry
                .keys()
                .all(|fragment| *fragment < start || *fragment >= start + length),
            "the split reservation must not leave a stale live fragment"
        );
    }

    #[test]
    fn private_file_writes_and_permission_changes_do_not_modify_the_file() {
        let file = TempFile::new("mmap-private-cow");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        let length = memory_geometry().0 * 2;
        write_all(fd, &vec![17u8; length]);
        for initial in [PROT_READ | PROT_WRITE, PROT_READ, PROT_NONE] {
            let mapped = unsafe {
                kinakaze_abi_mmap64(ptr::null_mut(), length, initial, MAP_PRIVATE, fd, 0)
            };
            assert_ne!(
                mapped,
                MAP_FAILED,
                "initial={initial}, errno={}",
                kinakaze_tls::errno()
            );
            assert_eq!(
                unsafe { kinakaze_abi_mprotect(mapped, length, PROT_READ | PROT_WRITE) },
                0
            );
            unsafe { mapped.cast::<u8>().write_volatile(43) };
            assert_eq!(
                unsafe { kinakaze_abi_mprotect(mapped, length, PROT_NONE) },
                0
            );
            assert_eq!(
                unsafe { kinakaze_abi_mprotect(mapped, length, PROT_READ | PROT_WRITE) },
                0
            );
            assert_eq!(unsafe { mapped.cast::<u8>().read_volatile() }, 43);
            // The second page was still clean at the permission transition.
            unsafe { mapped.cast::<u8>().add(length / 2).write_volatile(47) };
            assert_eq!(unsafe { kinakaze_abi_munmap(mapped, length) }, 0);
            let mut on_disk = vec![0u8; length];
            assert_eq!(
                unsafe { kinakaze_abi_pread64(fd, on_disk.as_mut_ptr().cast(), length, 0) },
                length as isize
            );
            assert!(on_disk.iter().all(|&byte| byte == 17));
        }
        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn shared_file_mapping_sees_and_publishes_writes() {
        let file = TempFile::new("mmap-file");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"mapped file contents padded out for a whole page");

        let length = 16;
        // SAFETY: the descriptor is a live file and the offset is page-aligned.
        let mapped = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                length,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        assert_ne!(
            mapped,
            MAP_FAILED,
            "file mmap failed with errno {}",
            kinakaze_tls::errno()
        );

        // SAFETY: the mapping covers `length` bytes.
        let view = unsafe { core::slice::from_raw_parts_mut(mapped.cast::<u8>(), length) };
        assert_eq!(&view[..6], b"mapped", "the mapping did not show the file");

        // A shared mapping publishes writes back to the file.
        view[..6].copy_from_slice(b"CHANGE");
        // SAFETY: `mapped` came from `mmap64` and is not used again.
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, length) }, 0);

        let mut buffer = [0u8; 6];
        // SAFETY: the buffer is a writable local.
        let read = unsafe {
            kinakaze_abi_pread64(fd, buffer.as_mut_ptr().cast::<c_void>(), buffer.len(), 0)
        };
        assert_eq!(read, 6);
        assert_eq!(
            &buffer, b"CHANGE",
            "a MAP_SHARED write should reach the file"
        );

        // A mapping offset past the end of file cannot be created on Windows; see
        // `map_file`.
        set_errno(0);
        // SAFETY: the call is expected to fail without mapping anything.
        let past = unsafe {
            kinakaze_abi_mmap64(ptr::null_mut(), length, PROT_READ, MAP_SHARED, fd, 1 << 20)
        };
        assert_eq!(past, MAP_FAILED);

        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn map_fixed_replaces_placeholder_with_anonymous_and_file_pages() {
        let (page, granularity) = memory_geometry();
        assert!(page < granularity, "the test needs a sub-64 KiB offset");

        let file = TempFile::new("mmap-fixed-placeholder");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        let mut contents = vec![0u8; page * 4];
        for (index, byte) in contents[page..page * 3].iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        write_all(fd, &contents);
        let _ = kinakaze_vfs::close(fd);
        let fd = open(file.path(), fs::O_RDONLY);

        // This is HotSpot's CDS layout in miniature: reserve one inaccessible
        // window, commit a small header page, then map the archive immediately
        // after it from a page-aligned (but not 64 KiB-aligned) file offset.
        let reserved = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                page * 4,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(reserved, MAP_FAILED);
        let start = reserved as usize;

        let header = unsafe {
            kinakaze_abi_mmap64(
                reserved,
                page,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                -1,
                0,
            )
        };
        assert_eq!(header, reserved, "fixed anonymous replacement moved");
        unsafe { *header.cast::<u8>() = 0x5a };

        let archive_address = (start + page) as *mut c_void;
        let archive = unsafe {
            kinakaze_abi_mmap64(
                archive_address,
                page * 2,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_FIXED,
                fd,
                page as i64,
            )
        };
        assert_eq!(
            archive,
            archive_address,
            "placeholder file replacement failed with errno {}",
            kinakaze_tls::errno()
        );

        let bytes = unsafe { core::slice::from_raw_parts_mut(archive.cast::<u8>(), page * 2) };
        assert_eq!(bytes[0], 0);
        assert_eq!(bytes[250], 250);
        bytes[0] = 0xee;

        // MAP_PRIVATE must remain copy-on-write even though MapViewOfFile3 did
        // the placement.
        let mut on_disk = [0xffu8; 1];
        assert_eq!(
            unsafe {
                kinakaze_abi_pread64(
                    fd,
                    on_disk.as_mut_ptr().cast::<c_void>(),
                    on_disk.len(),
                    page as i64,
                )
            },
            1
        );
        assert_eq!(on_disk[0], 0);

        assert_eq!(unsafe { kinakaze_abi_munmap(archive, page * 2) }, 0);
        assert_eq!(unsafe { kinakaze_abi_munmap(header, page) }, 0);
        assert_eq!(
            unsafe { kinakaze_abi_munmap((start + page * 3) as *mut c_void, page) },
            0
        );
        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn fixed_prot_none_decommits_a_borrowed_host_reservation() {
        let (page, _) = memory_geometry();
        let reservation_length = page * 4;
        // Model a native Windows thread stack: the host owns one surrounding
        // reservation and the Linux guest may only commit/decommit pages in it.
        let reservation = unsafe {
            VirtualAlloc(
                ptr::null_mut(),
                reservation_length,
                MEM_RESERVE,
                PAGE_NOACCESS,
            )
        };
        assert!(!reservation.is_null());
        let guard = (reservation as usize + page) as *mut c_void;
        let guard_length = page * 2;

        let committed = unsafe {
            kinakaze_abi_mmap64(
                guard,
                guard_length,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                -1,
                0,
            )
        };
        assert_eq!(committed, guard);
        unsafe { committed.cast::<u8>().write(0x5a) };
        assert_eq!(
            mappings()
                .lock()
                .expect("mapping registry")
                .get(&(guard as usize))
                .map(|mapping| mapping.kind),
            Some(MappingKind::BorrowedReservation)
        );

        let inaccessible = unsafe {
            kinakaze_abi_mmap64(
                guard,
                guard_length,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED | MAP_NORESERVE,
                -1,
                0,
            )
        };
        assert_eq!(inaccessible, guard);
        assert!(
            !mappings()
                .lock()
                .expect("mapping registry")
                .contains_key(&(guard as usize)),
            "decommitted borrowed pages must not retain guest ownership metadata"
        );

        let mut info = MemoryBasicInformation {
            base_address: ptr::null_mut(),
            allocation_base: ptr::null_mut(),
            allocation_protect: 0,
            partition_id: 0,
            _pad: 0,
            region_size: 0,
            state: 0,
            protect: 0,
            type_: 0,
        };
        assert_ne!(
            unsafe { VirtualQuery(guard, &mut info, size_of::<MemoryBasicInformation>(),) },
            0
        );
        assert_eq!(info.allocation_base, reservation);
        assert_eq!(info.state, MEM_RESERVE_STATE);

        // HotSpot may attach the same native thread again. Recommitting the
        // exact pages must work without changing who owns the reservation.
        let recommitted = unsafe {
            kinakaze_abi_mmap64(
                guard,
                guard_length,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                -1,
                0,
            )
        };
        assert_eq!(recommitted, guard);
        unsafe { recommitted.cast::<u8>().write(0xa5) };
        assert_eq!(
            unsafe {
                kinakaze_abi_mmap64(
                    guard,
                    guard_length,
                    PROT_NONE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED | MAP_NORESERVE,
                    -1,
                    0,
                )
            },
            guard
        );

        // Only the host owner releases the allocation base.
        assert_ne!(unsafe { VirtualFree(reservation, 0, MEM_RELEASE) }, 0);
    }

    #[test]
    fn map_fixed_file_splits_an_existing_anonymous_section_without_copying_sides() {
        let (page, _) = memory_geometry();
        let file = TempFile::new("mmap-fixed-nested-section");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        let contents = vec![0x3cu8; page * 2];
        write_all(fd, &contents);
        let _ = kinakaze_vfs::close(fd);
        let fd = open(file.path(), fs::O_RDONLY);

        let reserved = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                page * 4,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(reserved, MAP_FAILED);
        let anonymous = unsafe {
            kinakaze_abi_mmap64(
                reserved,
                page * 4,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                -1,
                0,
            )
        };
        assert_eq!(anonymous, reserved);
        let start = anonymous as usize;
        unsafe {
            *(start as *mut u8) = 0x51;
            *((start + page * 3) as *mut u8) = 0x72;
        }

        let middle = (start + page) as *mut c_void;
        let archive = unsafe {
            kinakaze_abi_mmap64(
                middle,
                page * 2,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_FIXED,
                fd,
                0,
            )
        };
        assert_eq!(
            archive,
            middle,
            "nested replacement failed with errno {}",
            kinakaze_tls::errno()
        );
        unsafe {
            assert_eq!(*(start as *const u8), 0x51);
            assert_eq!(*((start + page * 3) as *const u8), 0x72);
            assert_eq!(*(middle as *const u8), 0x3c);
        }

        assert_eq!(unsafe { kinakaze_abi_munmap(middle, page * 2) }, 0);
        assert_eq!(
            unsafe { kinakaze_abi_munmap(start as *mut c_void, page) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_munmap((start + page * 3) as *mut c_void, page) },
            0
        );
        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn map_fixed_anonymous_replaces_a_committed_placeholder_with_zero_pages() {
        let (page, _) = memory_geometry();
        let reserved = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                page * 4,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(reserved, MAP_FAILED);
        let committed = unsafe {
            kinakaze_abi_mmap64(
                reserved,
                page * 4,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                -1,
                0,
            )
        };
        assert_eq!(committed, reserved);
        let start = committed as usize;
        unsafe {
            ptr::write_bytes(committed.cast::<u8>(), 0xa5, page * 4);
        }

        let middle = (start + page) as *mut c_void;
        let replaced = unsafe {
            kinakaze_abi_mmap64(
                middle,
                page * 2,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                -1,
                0,
            )
        };
        assert_eq!(replaced, middle);
        unsafe {
            assert_eq!(*(start as *const u8), 0xa5);
            assert_eq!(*((start + page * 4 - 1) as *const u8), 0xa5);
            assert!(
                core::slice::from_raw_parts(middle.cast::<u8>(), page * 2)
                    .iter()
                    .all(|byte| *byte == 0)
            );
        }
        let registry = mappings().lock().expect("mapping registry");
        assert!(registry.iter().all(|(&mapping_start, mapping)| {
            mapping_start < start
                || mapping_start >= start + page * 4
                || !mapping.kind.is_borrowed()
        }));
        drop(registry);

        assert_eq!(unsafe { kinakaze_abi_munmap(committed, page * 4) }, 0);
    }

    #[test]
    fn mprotect_commits_a_placeholder_subrange() {
        let (page, _) = memory_geometry();
        let reserved = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                page * 3,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(reserved, MAP_FAILED);

        let middle = (reserved as usize + page) as *mut c_void;
        assert_eq!(
            unsafe { kinakaze_abi_mprotect(middle, page, PROT_READ | PROT_WRITE) },
            0
        );
        unsafe {
            *middle.cast::<u8>() = 0x7b;
            assert_eq!(*middle.cast::<u8>(), 0x7b);
        }
        let registry = mappings().lock().expect("mapping registry");
        let committed = registry
            .get(&(middle as usize))
            .expect("committed placeholder fragment");
        assert_eq!(committed.kind, MappingKind::PlaceholderView);
        assert!(committed.backing.is_some());
        drop(registry);

        assert_eq!(unsafe { kinakaze_abi_munmap(reserved, page * 3) }, 0);
    }

    #[test]
    fn mmap_rejects_a_descriptor_that_cannot_be_mapped() {
        let mut fds = [0 as c_int; 2];
        // SAFETY: two writable locals.
        assert_eq!(unsafe { kinakaze_abi_pipe(fds.as_mut_ptr()) }, 0);
        set_errno(0);
        // SAFETY: the call is expected to fail without mapping anything.
        let mapped =
            unsafe { kinakaze_abi_mmap64(ptr::null_mut(), 4096, PROT_READ, MAP_SHARED, fds[0], 0) };
        assert_eq!(mapped, MAP_FAILED, "a pipe has no pages to map");
        assert_eq!(kinakaze_tls::errno(), ENODEV);
        let _ = kinakaze_vfs::close(fds[0]);
        let _ = kinakaze_vfs::close(fds[1]);
    }

    #[test]
    fn sendfile_copies_between_descriptors_and_honours_an_offset() {
        let source = TempFile::new("sendfile-in");
        let destination = TempFile::new("sendfile-out");
        let in_fd = open(source.path(), fs::O_RDWR | fs::O_CREAT);
        let out_fd = open(destination.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(in_fd, b"0123456789");
        fs::lseek(in_fd, 0, SEEK_SET).expect("seek");

        // With a null offset the input's own position is used and advanced.
        // SAFETY: a null offset is the documented "use the descriptor's position".
        let copied = unsafe { kinakaze_abi_sendfile64(out_fd, in_fd, ptr::null_mut(), 4) };
        assert_eq!(copied, 4);
        assert_eq!(
            fs::lseek(in_fd, 0, SEEK_CUR).expect("seek"),
            4,
            "a null offset should advance the input descriptor"
        );

        // With an explicit offset the descriptor's position is left alone and the
        // offset is written back.
        let mut offset = 6i64;
        // SAFETY: `offset` is a writable local.
        let copied = unsafe { kinakaze_abi_sendfile64(out_fd, in_fd, &raw mut offset, 4) };
        assert_eq!(copied, 4);
        assert_eq!(offset, 10, "the offset should advance by what was copied");
        assert_eq!(
            fs::lseek(in_fd, 0, SEEK_CUR).expect("seek"),
            4,
            "an explicit offset must not move the input descriptor"
        );

        // The destination holds the first four bytes then the bytes at offset 6.
        let mut buffer = [0u8; 8];
        // SAFETY: the buffer is a writable local.
        let read =
            unsafe { kinakaze_abi_pread64(out_fd, buffer.as_mut_ptr().cast::<c_void>(), 8, 0) };
        assert_eq!(read, 8);
        assert_eq!(&buffer, b"01236789");

        // Asking for more than the input holds copies what is there.
        let mut offset = 0i64;
        // SAFETY: `offset` is a writable local.
        let copied = unsafe { kinakaze_abi_sendfile64(out_fd, in_fd, &raw mut offset, 4096) };
        assert_eq!(copied, 10, "a short input is not an error");

        let _ = kinakaze_vfs::close(in_fd);
        let _ = kinakaze_vfs::close(out_fd);
    }

    #[test]
    fn posix_fallocate_returns_an_errno_and_extends_the_file() {
        let file = TempFile::new("fallocate");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"start");

        // Success is 0, and the file really grows.
        assert_eq!(kinakaze_abi_posix_fallocate64(fd, 0, 4096), 0);
        assert_eq!(
            fs::fstat(fd).expect("fstat").st_size,
            4096,
            "posix_fallocate should extend the file to offset+len"
        );
        // The existing bytes survive.
        let mut buffer = [0u8; 5];
        // SAFETY: the buffer is a writable local.
        unsafe { kinakaze_abi_pread64(fd, buffer.as_mut_ptr().cast::<c_void>(), 5, 0) };
        assert_eq!(&buffer, b"start");

        // A range already inside the file is a no-op and must not truncate.
        assert_eq!(kinakaze_abi_posix_fallocate64(fd, 0, 100), 0);
        assert_eq!(fs::fstat(fd).expect("fstat").st_size, 4096);

        // The POSIX quirk this function exists to get right: the error comes back
        // as the return value, and errno is not touched.
        set_errno(0);
        assert_eq!(
            kinakaze_abi_posix_fallocate64(fd, -1, 10),
            EINVAL,
            "posix_fallocate returns the errno rather than setting it"
        );
        assert_eq!(kinakaze_abi_posix_fallocate64(fd, 0, 0), EINVAL);
        assert_eq!(kinakaze_abi_posix_fallocate64(9999, 0, 10), EBADF);
        assert_eq!(
            kinakaze_tls::errno(),
            0,
            "posix_fallocate must leave errno alone"
        );

        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn futimens_writes_the_times_it_is_given_and_omits_what_it_should() {
        let file = TempFile::new("futimens");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"stamped");

        // A time far enough in the past to be unmistakable.
        let access = 1_000_000_000i64;
        let modification = 1_100_000_000i64;
        let times = [
            TimeSpec {
                tv_sec: access,
                tv_nsec: 0,
            },
            TimeSpec {
                tv_sec: modification,
                tv_nsec: 0,
            },
        ];
        // SAFETY: two readable locals.
        assert_eq!(
            unsafe { kinakaze_abi_futimens(fd, times.as_ptr()) },
            0,
            "futimens failed with errno {}",
            kinakaze_tls::errno()
        );

        let info = fs::fstat(fd).expect("fstat");
        assert_eq!(info.st_atime, access, "the access time was not written");
        assert_eq!(
            info.st_mtime, modification,
            "the modification time was not written"
        );

        // UTIME_OMIT must leave a component exactly as it was. Getting this wrong
        // silently rewrites a timestamp the caller asked not to touch.
        let omit = [
            TimeSpec {
                tv_sec: 0,
                tv_nsec: UTIME_OMIT,
            },
            TimeSpec {
                tv_sec: 1_200_000_000,
                tv_nsec: 0,
            },
        ];
        // SAFETY: two readable locals.
        assert_eq!(unsafe { kinakaze_abi_futimens(fd, omit.as_ptr()) }, 0);
        let info = fs::fstat(fd).expect("fstat");
        assert_eq!(
            info.st_atime, access,
            "UTIME_OMIT must not change the access time"
        );
        assert_eq!(info.st_mtime, 1_200_000_000);

        // UTIME_NOW uses the current time.
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64;
        let now = [
            TimeSpec {
                tv_sec: 0,
                tv_nsec: UTIME_NOW,
            },
            TimeSpec {
                tv_sec: 0,
                tv_nsec: UTIME_NOW,
            },
        ];
        // SAFETY: two readable locals.
        assert_eq!(unsafe { kinakaze_abi_futimens(fd, now.as_ptr()) }, 0);
        let info = fs::fstat(fd).expect("fstat");
        assert!(
            (info.st_mtime - before).abs() <= 5,
            "UTIME_NOW wrote {} which is not close to now ({before})",
            info.st_mtime
        );

        // A null array is both to now, which is the `touch` case.
        // SAFETY: null is the documented "both to now".
        assert_eq!(unsafe { kinakaze_abi_futimens(fd, ptr::null()) }, 0);

        // An out-of-range nanosecond that is not one of the special values is
        // EINVAL rather than a wrapped timestamp.
        let bad = [
            TimeSpec {
                tv_sec: 0,
                tv_nsec: 2_000_000_000,
            },
            TimeSpec::default(),
        ];
        set_errno(0);
        // SAFETY: two readable locals.
        assert_eq!(unsafe { kinakaze_abi_futimens(fd, bad.as_ptr()) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        let _ = kinakaze_vfs::close(fd);
    }

    #[test]
    fn utimensat_and_utimes_stamp_a_path() {
        let file = TempFile::new("utimes");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        let _ = kinakaze_vfs::close(fd);
        let path = file.c_path();

        let times = [
            TimeVal {
                tv_sec: 1_300_000_000,
                tv_usec: 500_000,
            },
            TimeVal {
                tv_sec: 1_310_000_000,
                tv_usec: 0,
            },
        ];
        // SAFETY: the path is null-terminated and there are two readable locals.
        assert_eq!(
            unsafe { kinakaze_abi_utimes(path.as_ptr(), times.as_ptr()) },
            0,
            "utimes failed with errno {}",
            kinakaze_tls::errno()
        );
        let info = fs::stat(file.path()).expect("stat");
        assert_eq!(info.st_atime, 1_300_000_000);
        assert_eq!(info.st_mtime, 1_310_000_000);

        // utimensat against AT_FDCWD with an absolute path.
        let spec = [
            TimeSpec {
                tv_sec: 1_400_000_000,
                tv_nsec: 0,
            },
            TimeSpec {
                tv_sec: 1_410_000_000,
                tv_nsec: 0,
            },
        ];
        // SAFETY: the path is null-terminated and there are two readable locals.
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(AT_FDCWD, path.as_ptr(), spec.as_ptr(), 0) },
            0
        );
        let info = fs::stat(file.path()).expect("stat");
        assert_eq!(info.st_atime, 1_400_000_000);
        assert_eq!(info.st_mtime, 1_410_000_000);

        // An out-of-range microsecond is refused.
        let bad = [
            TimeVal {
                tv_sec: 0,
                tv_usec: 2_000_000,
            },
            TimeVal::default(),
        ];
        set_errno(0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_utimes(path.as_ptr(), bad.as_ptr()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        // An unknown flag is EINVAL, not ignored.
        set_errno(0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(AT_FDCWD, path.as_ptr(), spec.as_ptr(), 0x4000) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn utimensat_resolves_directory_handles_and_does_not_follow_final_links() {
        let directory = TempFile::new("utimensat-dir");
        let moved = TempFile::new("utimensat-moved");
        std::fs::create_dir(&directory.native).unwrap();
        let child = directory.native.join("entry");
        std::fs::write(&child, b"original").unwrap();
        let dirfd = open(directory.path(), fs::O_RDONLY | fs::O_DIRECTORY);
        let link = std::ffi::CString::new(format!("{}/link", directory.path())).unwrap();
        assert_eq!(
            unsafe { crate::fsextra::kinakaze_abi_symlink(c"entry".as_ptr(), link.as_ptr()) },
            0
        );
        let times = [TimeSpec {
            tv_sec: 1_400_000_000,
            tv_nsec: 100_000_000,
        }; 2];
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(dirfd, c"entry".as_ptr(), times.as_ptr(), 0) },
            0
        );
        let child_guest = windows_to_linux(&child);
        let target_before = fs::stat(&child_guest).unwrap();
        let link_times = [TimeSpec {
            tv_sec: 1_410_000_000,
            tv_nsec: 200_000_000,
        }; 2];
        assert_eq!(
            unsafe {
                kinakaze_abi_utimensat(
                    dirfd,
                    c"link".as_ptr(),
                    link_times.as_ptr(),
                    AT_SYMLINK_NOFOLLOW,
                )
            },
            0
        );
        let link_stat = fs::lstat(link.to_str().unwrap()).unwrap();
        assert_eq!(link_stat.st_mtime, 1_410_000_000);
        assert_eq!(link_stat.st_mtime_nsec, 200_000_000);
        assert_ne!(link_stat.st_ino, 0);
        assert_ne!(link_stat.st_ino, target_before.st_ino);
        assert_eq!(link_stat.st_mode, fs::S_IFLNK | 0o777);
        assert_eq!(link_stat.st_size, 5);
        assert_eq!(
            fs::stat(&child_guest).unwrap().st_mtime,
            target_before.st_mtime
        );

        // A directory descriptor follows its inode when that directory moves;
        // a replacement at the former name must not receive this timestamp.
        std::fs::rename(&directory.native, &moved.native).unwrap();
        std::fs::create_dir(&directory.native).unwrap();
        std::fs::write(&child, b"replacement").unwrap();
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(dirfd, c"entry".as_ptr(), link_times.as_ptr(), 0) },
            0
        );
        assert_eq!(
            fs::stat(&format!("{}/entry", moved.path()))
                .unwrap()
                .st_mtime,
            1_410_000_000
        );
        assert_ne!(fs::stat(&child_guest).unwrap().st_mtime, 1_410_000_000);

        for (fd, name, expected) in [
            (999999, c"entry", EBADF),
            (dirfd, c"", kinakaze_vfs::ENOENT),
        ] {
            assert_eq!(
                unsafe { kinakaze_abi_utimensat(fd, name.as_ptr(), times.as_ptr(), 0) },
                -1
            );
            assert_eq!(kinakaze_tls::errno(), expected);
        }
        let plain = open(&child_guest, fs::O_RDONLY);
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(plain, c"child".as_ptr(), times.as_ptr(), 0) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), ENOTDIR);
        kinakaze_vfs::close(plain).unwrap();
        kinakaze_vfs::close(dirfd).unwrap();
        for path in [child, moved.native.join("entry"), moved.native.join("link")] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(&directory.native).unwrap();
        std::fs::remove_dir(&moved.native).unwrap();
    }

    #[test]
    fn futimens_keeps_the_open_inode_after_unlink_and_replacement() {
        let file = TempFile::new("futimens-unlinked");
        std::fs::write(&file.native, b"old inode").unwrap();
        let fd = open(file.path(), fs::O_RDONLY);
        fs::unlink(file.path()).unwrap();
        std::fs::write(&file.native, b"new inode").unwrap();
        let before = fs::stat(file.path()).unwrap();
        let times = [TimeSpec {
            tv_sec: 1_430_000_000,
            tv_nsec: 300_000_000,
        }; 2];
        assert_eq!(
            unsafe { kinakaze_abi_futimens(fd, times.as_ptr()) },
            0,
            "unlinked inode timestamp: errno {}",
            kinakaze_tls::errno()
        );
        assert_eq!(fs::fstat(fd).unwrap().st_mtime, 1_430_000_000);
        assert_eq!(fs::stat(file.path()).unwrap().st_mtime, before.st_mtime);
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn utimensat_empty_path_omit_and_null_path_flags_follow_linux() {
        let file = TempFile::new("utimensat-empty");
        std::fs::write(&file.native, b"path-only inode").unwrap();
        let fd = open(file.path(), fs::O_PATH);
        let times = [TimeSpec {
            tv_sec: 1_440_000_000,
            tv_nsec: 0,
        }; 2];
        assert_eq!(unsafe { kinakaze_abi_futimens(fd, times.as_ptr()) }, -1);
        assert_eq!(kinakaze_tls::errno(), EBADF);
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(fd, c"".as_ptr(), times.as_ptr(), AT_EMPTY_PATH) },
            0
        );
        assert_eq!(fs::stat(file.path()).unwrap().st_mtime, 1_440_000_000);
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(fd, ptr::null(), times.as_ptr(), AT_SYMLINK_NOFOLLOW) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        let omit = [TimeSpec {
            tv_sec: i64::MAX,
            tv_nsec: UTIME_OMIT,
        }; 2];
        // Linux must not inspect even an invalid filename pointer in this case.
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(-1, 1usize as *const c_char, omit.as_ptr(), -1) },
            0
        );
        assert_eq!(unsafe { kinakaze_abi_futimens(-1, omit.as_ptr()) }, -1);
        assert_eq!(kinakaze_tls::errno(), EBADF);
        let invalid = [TimeSpec {
            tv_sec: 0,
            tv_nsec: -1,
        }; 2];
        assert_eq!(
            unsafe { kinakaze_abi_futimens(999999, invalid.as_ptr()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EBADF);
        let absent = std::ffi::CString::new(format!("{}.missing", file.path())).unwrap();
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(AT_FDCWD, absent.as_ptr(), invalid.as_ptr(), 0) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ENOENT);
        assert_eq!(
            unsafe { kinakaze_abi_utimensat(fd, c"".as_ptr(), invalid.as_ptr(), AT_EMPTY_PATH) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        kinakaze_vfs::close(fd).unwrap();
    }

    #[test]
    fn fchdir_moves_guest_fs_context_without_mutating_native_process_cwd() {
        let original = fs::getcwd();
        let host_before = std::env::current_dir().unwrap();
        // A real directory to move into: the host temporary directory.
        let native = std::env::temp_dir();
        let guest = windows_to_linux(&native);
        let fd = open(&guest, fs::O_RDONLY | fs::O_DIRECTORY);

        assert_eq!(
            kinakaze_abi_fchdir(fd),
            0,
            "fchdir failed with errno {}",
            kinakaze_tls::errno()
        );
        // The guest's own notion of the current directory moved.
        let moved = fs::getcwd();
        assert!(
            moved.eq_ignore_ascii_case(&guest)
                || moved.eq_ignore_ascii_case(guest.trim_end_matches('/')),
            "fchdir put the guest at {moved:?}, expected {guest:?}"
        );
        // The native process cwd is shared by unrelated guest fs_structs.
        let host = std::env::current_dir().expect("current directory");
        assert!(
            host == host_before,
            "guest fchdir changed the process-wide Windows cwd to {host:?}"
        );

        // A non-directory descriptor is ENOTDIR.
        let file = TempFile::new("fchdir");
        let plain = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        set_errno(0);
        assert_eq!(kinakaze_abi_fchdir(plain), -1);
        assert_eq!(kinakaze_tls::errno(), ENOTDIR);

        set_errno(0);
        assert_eq!(kinakaze_abi_fchdir(9999), -1);
        assert_eq!(kinakaze_tls::errno(), EBADF);

        let _ = kinakaze_vfs::close(plain);
        let _ = kinakaze_vfs::close(fd);
        // Put the guest back so other tests are not affected by the move.
        let _ = fs::chdir(&original);
    }

    #[test]
    fn unlocked_stdio_matches_its_locked_counterpart() {
        let file = TempFile::new("unlocked");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"ab");
        let _ = kinakaze_vfs::close(fd);

        let path = file.c_path();
        // SAFETY: both arguments are null-terminated.
        let stream = unsafe { crate::stdio::fopen(path.as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null());

        // The unlocked reader is the locked reader; this checks it really reads.
        assert_eq!(kinakaze_abi_getc_unlocked(stream), i32::from(b'a'));
        assert_eq!(kinakaze_abi_getc_unlocked(stream), i32::from(b'b'));
        assert_eq!(kinakaze_abi_getc_unlocked(stream), EOF);
        assert_ne!(
            kinakaze_abi_feof_unlocked(stream),
            0,
            "feof_unlocked should report the end of file the read reached"
        );
        assert_eq!(kinakaze_abi_ferror_unlocked(stream), 0);
        // fileno_unlocked names the same descriptor as fileno.
        assert_eq!(
            kinakaze_abi_fileno_unlocked(stream),
            crate::stdio::fileno(stream)
        );

        // SAFETY: the stream is not used again.
        unsafe { crate::stdio::fclose(stream) };
    }

    #[test]
    fn freopen_reuses_the_stream_on_a_new_file() {
        let first = TempFile::new("freopen-a");
        let second = TempFile::new("freopen-b");
        for (file, contents) in [(&first, b"first file"), (&second, b"secondfile")] {
            let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
            write_all(fd, contents);
            let _ = kinakaze_vfs::close(fd);
        }

        let first_path = first.c_path();
        // SAFETY: both arguments are null-terminated.
        let stream = unsafe { crate::stdio::fopen(first_path.as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null());
        let descriptor = crate::stdio::fileno(stream);

        let mut byte = [0u8; 5];
        // SAFETY: the buffer is a writable local and the stream is live.
        unsafe { crate::stdio::fread(byte.as_mut_ptr().cast::<c_void>(), 1, 5, stream) };
        assert_eq!(&byte, b"first");

        let second_path = second.c_path();
        // SAFETY: all three arguments satisfy the contract.
        let reopened =
            unsafe { kinakaze_abi_freopen64(second_path.as_ptr(), c"r".as_ptr(), stream) };
        assert_eq!(
            reopened, stream,
            "freopen must reuse the stream object so `stdin` stays `stdin`"
        );
        assert_eq!(
            crate::stdio::fileno(stream),
            descriptor,
            "freopen should keep the stream on its own descriptor number"
        );

        // The stream now reads the second file from its beginning.
        let mut text = [0u8; 6];
        // SAFETY: the buffer is a writable local and the stream is live.
        let read = unsafe { crate::stdio::fread(text.as_mut_ptr().cast::<c_void>(), 1, 6, stream) };
        assert_eq!(read, 6);
        assert_eq!(
            &text, b"second",
            "freopen did not rebind the stream to the new file"
        );

        // SAFETY: the stream is not used again.
        unsafe { crate::stdio::fclose(stream) };
    }

    #[test]
    fn fseeko64_and_ftello64_agree_with_the_stream_position() {
        let file = TempFile::new("fseeko");
        let fd = open(file.path(), fs::O_RDWR | fs::O_CREAT);
        write_all(fd, b"0123456789");
        let _ = kinakaze_vfs::close(fd);

        let path = file.c_path();
        // SAFETY: both arguments are null-terminated.
        let stream = unsafe { crate::stdio::fopen(path.as_ptr(), c"r".as_ptr()) };
        assert!(!stream.is_null());

        assert_eq!(kinakaze_abi_ftello64(stream), 0);
        assert_eq!(kinakaze_abi_fseeko64(stream, 4, SEEK_SET), 0);
        assert_eq!(kinakaze_abi_ftello64(stream), 4);
        assert_eq!(kinakaze_abi_fseeko64(stream, 2, SEEK_CUR), 0);
        assert_eq!(kinakaze_abi_ftello64(stream), 6);
        assert_eq!(kinakaze_abi_fseeko64(stream, 0, SEEK_END), 0);
        assert_eq!(kinakaze_abi_ftello64(stream), 10);

        // setbuf selects a discipline and must not fail on a valid stream.
        kinakaze_abi_setbuf(stream, ptr::null_mut());
        assert_eq!(kinakaze_abi_fseeko64(stream, 0, SEEK_SET), 0);
        assert_eq!(kinakaze_abi_ftello64(stream), 0);

        // SAFETY: the stream is not used again.
        unsafe { crate::stdio::fclose(stream) };
    }

    #[test]
    fn mprotect_permission_transitions() {
        let (page, _) = memory_geometry();
        let mapped = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                page,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(mapped, MAP_FAILED);

        // Write to page
        let slice = unsafe { core::slice::from_raw_parts_mut(mapped.cast::<u8>(), page) };
        slice[0] = 42;
        slice[page - 1] = 99;

        // Change to PROT_READ
        assert_eq!(unsafe { kinakaze_abi_mprotect(mapped, page, PROT_READ) }, 0);
        assert_eq!(slice[0], 42);
        assert_eq!(slice[page - 1], 99);

        // Change back to PROT_READ | PROT_WRITE
        assert_eq!(
            unsafe { kinakaze_abi_mprotect(mapped, page, PROT_READ | PROT_WRITE) },
            0
        );
        slice[0] = 100;
        assert_eq!(slice[0], 100);

        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page) }, 0);
    }

    #[test]
    fn madvise_posix_behaviors() {
        let (page, _) = memory_geometry();
        let mapped = unsafe {
            kinakaze_abi_mmap64(
                ptr::null_mut(),
                page * 2,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(mapped, MAP_FAILED);

        // Standard advice codes
        const MADV_NORMAL: c_int = 0;
        const MADV_RANDOM: c_int = 1;
        const MADV_SEQUENTIAL: c_int = 2;
        const MADV_WILLNEED: c_int = 3;
        const MADV_DONTNEED: c_int = 4;
        const MADV_FREE: c_int = 8;

        assert_eq!(
            unsafe { kinakaze_abi_madvise(mapped, page * 2, MADV_NORMAL) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_madvise(mapped, page * 2, MADV_RANDOM) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_madvise(mapped, page * 2, MADV_SEQUENTIAL) },
            0
        );
        assert_eq!(
            unsafe { kinakaze_abi_madvise(mapped, page * 2, MADV_WILLNEED) },
            0
        );
        let bytes = unsafe { core::slice::from_raw_parts_mut(mapped.cast::<u8>(), page * 2) };
        bytes.fill(0xa5);
        assert_eq!(
            unsafe { kinakaze_abi_madvise(mapped, page * 2, MADV_DONTNEED) },
            0
        );
        assert!(
            bytes.iter().all(|byte| *byte == 0),
            "MADV_DONTNEED must expose fresh zero pages, not stale allocator state"
        );
        assert_eq!(
            unsafe { kinakaze_abi_madvise(mapped, page * 2, MADV_FREE) },
            -1,
            "an unimplemented lazy-free promise must be refused"
        );
        assert_eq!(crate::kinakaze_errno(), EINVAL);

        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page * 2) }, 0);
    }

    #[test]
    fn file_vma_past_eof_keeps_its_linux_length_for_advice() {
        let (page, _) = memory_geometry();
        let guest_path = format!("/tmp/kinakaze-madvise-eof-{}", std::process::id());
        let host_path = kinakaze_vfs::resolve_linux_path(&guest_path).unwrap();
        std::fs::create_dir_all(host_path.parent().unwrap()).unwrap();
        std::fs::write(&host_path, vec![0x5a; page]).unwrap();

        let fd = kinakaze_vfs::fs::open(&guest_path, kinakaze_vfs::fs::O_RDWR, 0).unwrap();
        let mapped =
            unsafe { kinakaze_abi_mmap64(ptr::null_mut(), page * 2, PROT_READ, MAP_SHARED, fd, 0) };
        assert_ne!(mapped, MAP_FAILED);
        assert_eq!(
            unsafe { kinakaze_abi_madvise(mapped, page * 2, MADV_RANDOM) },
            0,
            "the inaccessible tail is still part of the Linux VMA"
        );
        assert_eq!(unsafe { *(mapped.cast::<u8>()) }, 0x5a);
        assert_eq!(crate::fs::ftruncate(fd, (page * 2) as i64), 0);
        assert_eq!(
            unsafe { *(mapped.cast::<u8>().add(page)) },
            0,
            "a page enters the existing VMA when ftruncate extends the file into it"
        );
        assert_eq!(unsafe { kinakaze_abi_munmap(mapped, page * 2) }, 0);
        kinakaze_vfs::close(fd).unwrap();
        std::fs::remove_file(host_path).unwrap();
    }

    #[test]
    fn vectored_io_readv_writev() {
        let mut fds = [0 as c_int; 2];
        assert_eq!(unsafe { kinakaze_abi_pipe(fds.as_mut_ptr()) }, 0);

        let buf1 = b"Hello, ";
        let buf2 = b"vectored ";
        let buf3 = b"I/O!";

        let out_iov = [
            IoVec {
                iov_base: buf1.as_ptr() as *mut c_void,
                iov_len: buf1.len(),
            },
            IoVec {
                iov_base: buf2.as_ptr() as *mut c_void,
                iov_len: buf2.len(),
            },
            IoVec {
                iov_base: buf3.as_ptr() as *mut c_void,
                iov_len: buf3.len(),
            },
        ];

        let written = unsafe { kinakaze_abi_writev(fds[1], out_iov.as_ptr(), 3) };
        assert_eq!(written, (buf1.len() + buf2.len() + buf3.len()) as isize);

        let mut in_buf1 = [0u8; 10];
        let mut in_buf2 = [0u8; 20];
        let in_iov = [
            IoVec {
                iov_base: in_buf1.as_mut_ptr().cast(),
                iov_len: in_buf1.len(),
            },
            IoVec {
                iov_base: in_buf2.as_mut_ptr().cast(),
                iov_len: in_buf2.len(),
            },
        ];

        let read = unsafe { kinakaze_abi_readv(fds[0], in_iov.as_ptr(), 2) };
        assert_eq!(read, written);

        assert_eq!(&in_buf1, b"Hello, vec");
        assert_eq!(&in_buf2[..10], b"tored I/O!");

        let _ = kinakaze_vfs::close(fds[0]);
        let _ = kinakaze_vfs::close(fds[1]);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_eventfd_read(fd: c_int, value: *mut u64) -> c_int {
    if value.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    let n = unsafe { crate::kinakaze_abi_read(fd, value.cast(), 8) };
    if n == 8 { 0 } else { -1 }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_eventfd_write(fd: c_int, value: u64) -> c_int {
    let n = unsafe { crate::kinakaze_abi_write(fd, &raw const value as *const c_void, 8) };
    if n == 8 { 0 } else { -1 }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_splice(
    fd_in: c_int,
    _off_in: *mut i64,
    fd_out: c_int,
    _off_out: *mut i64,
    len: usize,
    flags: core::ffi::c_uint,
) -> isize {
    if len == 0 {
        return 0;
    }
    if flags & !0x0f != 0 {
        set_errno(EINVAL);
        return -1;
    }
    for fd in [fd_in, fd_out] {
        if let Err(error) = kinakaze_vfs::get(fd) {
            set_errno(error);
            return -1;
        }
    }
    // These descriptor backends have no transactional splice operation. A
    // read-then-write emulation can consume bytes that a short/failed write
    // never delivers. Linux returns EINVAL for a missing splice capability;
    // reject before touching bytes or offsets so callers can safely copy.
    set_errno(EINVAL);
    -1
}
