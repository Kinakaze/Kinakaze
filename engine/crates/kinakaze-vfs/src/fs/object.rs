//! Inode-owned native handles for path walking and internal metadata streams.
//! Paths are used only to enter the host filesystem or for diagnostics; child
//! and stream lookup is relative to an already open object. No global cache or
//! guest descriptor state is introduced here.

use super::{NativeIoStatus, complete_native_io};
use crate::{EINVAL, EIO, ENAMETOOLONG, ENOENT, FdEntry, FdFlags, FdKind, errno_from_win32};
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, GENERIC_READ, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, GetFileSizeEx, OPEN_EXISTING,
    SYNCHRONIZE, SetFileTime,
};

const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const FLAGS: u32 = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OVERLAPPED;

#[cfg(test)]
mod lifecycle_tests;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: HANDLE,
    name: *mut UnicodeString,
    attributes: u32,
    security: *mut core::ffi::c_void,
    qos: *mut core::ffi::c_void,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        handle: *mut HANDLE,
        access: u32,
        attrs: *const ObjectAttributes,
        status: *mut NativeIoStatus,
        allocation: *const i64,
        file_attributes: u32,
        share: u32,
        disposition: u32,
        options: u32,
        ea: *const u8,
        ea_length: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
    fn NtQueryInformationFile(
        file: HANDLE,
        io: *mut NativeIoStatus,
        buffer: *mut core::ffi::c_void,
        length: u32,
        class: u32,
    ) -> i32;
    fn NtQueryDirectoryFile(
        file: HANDLE,
        event: HANDLE,
        apc: *const u8,
        context: *const u8,
        io: *mut NativeIoStatus,
        buffer: *mut u8,
        length: u32,
        class: u32,
        single: u8,
        pattern: *const UnicodeString,
        restart: u8,
    ) -> i32;
}

/// Owned object reference. Separate reopen/stream handles carry data I/O, so
/// sharing an object reference does not share a mutable seek position or pending
/// request. Every object has SYNCHRONIZE access for retiring its own native open.
#[derive(Debug)]
pub(crate) struct Object(usize);
impl Drop for Object {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.raw()) };
    }
}

fn wide(name: &OsStr) -> Result<Vec<u16>, i32> {
    let mut name: Vec<u16> = name.encode_wide().collect();
    if name.contains(&0) {
        return Err(EINVAL);
    }
    name.push(0);
    Ok(name)
}

impl Object {
    /// Reopen a persisted NTFS/ReFS file identifier, including after rename.
    /// The volume hint does not select a pathname for the resulting object.
    pub(crate) fn by_id(hint: &Path, id: u64, access: u32) -> Result<Self, i32> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ID_DESCRIPTOR, FileIdType, OpenFileById,
        };
        let volume = Self::open(hint, FILE_READ_ATTRIBUTES)?;
        let mut descriptor: FILE_ID_DESCRIPTOR = unsafe { std::mem::zeroed() };
        descriptor.dwSize = std::mem::size_of_val(&descriptor) as u32;
        descriptor.Type = FileIdType;
        descriptor.Anonymous.FileId = id as i64;
        Self::owned(unsafe {
            OpenFileById(
                volume.raw(),
                &descriptor,
                access | SYNCHRONIZE,
                SHARE,
                ptr::null(),
                FLAGS,
            )
        })
    }
    pub(crate) fn owned(handle: HANDLE) -> Result<Self, i32> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(Self(handle as usize))
        }
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }

    /// Duplicate the same native open, preserving sharing locks and leases.
    pub(crate) fn duplicate(handle: HANDLE) -> Result<Self, i32> {
        use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut duplicate = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                handle,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        Self::owned(duplicate)
    }

    /// Query rights granted to this particular handle, not rights that a new
    /// open by the current host identity might acquire. Object basic information
    /// is a non-I/O kernel handle query and cannot race another file request's
    /// completion status or seek position.
    pub(crate) fn granted_access(handle: HANDLE) -> Result<u32, i32> {
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtQueryObject(
                handle: HANDLE,
                class: u32,
                buffer: *mut core::ffi::c_void,
                length: u32,
                returned: *mut u32,
            ) -> i32;
        }
        // PUBLIC_OBJECT_BASIC_INFORMATION: granted access is the second ULONG.
        let mut storage = [0u64; 7];
        let status = unsafe {
            NtQueryObject(
                handle,
                0,
                storage.as_mut_ptr().cast(),
                std::mem::size_of_val(&storage) as u32,
                ptr::null_mut(),
            )
        };
        if status < 0 {
            return Err(errno_from_win32(unsafe { RtlNtStatusToDosError(status) }));
        }
        Ok((storage[0] >> 32) as u32)
    }
    /// Count handles to this owned object, not references to unrelated memory.
    pub(crate) fn handle_count(&self) -> Result<u32, i32> {
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtQueryObject(
                handle: HANDLE,
                class: u32,
                buffer: *mut core::ffi::c_void,
                length: u32,
                returned: *mut u32,
            ) -> i32;
        }
        let mut information = [0u32; 14];
        let status = unsafe {
            NtQueryObject(
                self.raw(),
                0,
                information.as_mut_ptr().cast(),
                size_of_val(&information) as u32,
                ptr::null_mut(),
            )
        };
        if status < 0 {
            return Err(errno_from_win32(unsafe { RtlNtStatusToDosError(status) }));
        }
        Ok(information[2])
    }

    /// Pin a native filesystem descriptor while close/dup2 cannot reclaim its
    /// slot. Only the nonblocking kernel duplication holds the table guard;
    /// subsequent filesystem requests use independent reopened handles.
    pub(crate) fn from_fd(fd: i32) -> Result<Self, i32> {
        Self::from_fd_with_entry(fd).map(|(object, _)| object)
    }

    /// Pin the inode and its descriptor properties under the same table guard.
    /// A separate `get` followed by duplication can otherwise combine the old
    /// descriptor's access mode with a recycled descriptor's native object.
    pub(crate) fn from_fd_with_entry(fd: i32) -> Result<(Self, FdEntry), i32> {
        Self::from_fd_checked(fd, |_| Ok(()))
    }

    /// Validate descriptor-specific ABI rules and pin the same entry atomically.
    /// The validator runs under the fd-table read guard, so it must not perform
    /// I/O or reenter the table. No caller may validate a separate borrowed `get`.
    pub(crate) fn from_fd_checked(
        fd: i32,
        validate: impl FnOnce(FdEntry) -> Result<(), i32>,
    ) -> Result<(Self, FdEntry), i32> {
        use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        if fd < 0 {
            return Err(crate::EBADF);
        }
        let table = crate::table().read().map_err(|_| EIO)?;
        let entry = table
            .slots
            .get(fd as usize)
            .and_then(|slot| *slot)
            .ok_or(crate::EBADF)?;
        validate(entry)?;
        if !matches!(entry.kind, FdKind::File | FdKind::Directory) {
            return Err(crate::EOPNOTSUPP);
        }
        let mut duplicate = ptr::null_mut();
        let process = unsafe { GetCurrentProcess() };
        if unsafe {
            DuplicateHandle(
                process,
                entry.raw as HANDLE,
                process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        Ok((Self::owned(duplicate)?, entry))
    }

    pub(crate) fn open(path: &Path, access: u32) -> Result<Self, i32> {
        let name = crate::path::wide_path(path)?;
        Self::owned(unsafe {
            CreateFileW(
                name.as_ptr(),
                access | SYNCHRONIZE,
                SHARE,
                ptr::null(),
                OPEN_EXISTING,
                FLAGS,
                ptr::null_mut(),
            )
        })
    }

    /// Reopen an opaque kernel handle; the kernel validates invalid handles.
    /// Callers retain the handle to preserve inode identity across this call.
    pub(crate) fn reopen(file: HANDLE, access: u32) -> Result<Self, i32> {
        // SAFETY: The name and output storage are owned here. The native handle
        // is an opaque kernel key and is never dereferenced in host memory.
        unsafe { Self::relative(file, vec![0], access, 1) }
    }

    /// Reopen the same inode while excluding data writers, including writable
    /// mappings whose original file descriptor has already been closed. This
    /// native share reservation is released automatically on process death.
    /// # Safety
    /// The borrowed inode handle remains live for the duration of the open.
    pub(crate) unsafe fn reopen_deny_write(file: HANDLE, access: u32) -> Result<Self, i32> {
        unsafe {
            Self::relative_with_share(
                file,
                vec![0],
                access,
                1,
                FILE_SHARE_READ | FILE_SHARE_DELETE,
            )
        }
    }

    pub(crate) fn into_raw(self) -> HANDLE {
        let handle = self.raw();
        std::mem::forget(self);
        handle
    }

    /// Open a stored child name. Guest escaping and mount/symlink policy belong
    /// to the walker; no native slash, stream separator or dot traversal enters
    /// this one-component operation.
    pub(crate) fn child(&self, stored: &OsStr, access: u32) -> Result<Self, i32> {
        let name = wide(stored)?;
        if name.len() < 2
            || name == [46, 0]
            || name == [46, 46, 0]
            || name[..name.len() - 1]
                .iter()
                .any(|&c| matches!(c, 47 | 92 | 58))
        {
            return Err(EINVAL);
        }
        unsafe { Self::relative(self.raw(), name, access, 1) }
    }

    /// # Safety
    /// `file` remains live. `name` is an internal native stream name, beginning
    /// with ':', not a guest pathname. Works for both file and directory inodes.
    pub(crate) unsafe fn stream(file: HANDLE, name: &OsStr, access: u32) -> Result<Self, i32> {
        let name = wide(name)?;
        if name.len() < 3
            || name[0] != 58
            || name[..name.len() - 1].iter().any(|&c| matches!(c, 47 | 92))
        {
            return Err(EINVAL);
        }
        unsafe { Self::relative(file, name, access, 1) }
    }

    /// # Safety
    /// The borrowed file is a live unpublished inode owned by the transaction.
    /// A failed creation is cleaned up when that transaction removes its inode.
    pub(crate) unsafe fn create_stream(
        file: HANDLE,
        name: &OsStr,
        access: u32,
    ) -> Result<Self, i32> {
        let name = wide(name)?;
        if name.len() < 3
            || name[0] != 58
            || name[..name.len() - 1].iter().any(|&c| matches!(c, 47 | 92))
        {
            return Err(EINVAL);
        }
        unsafe { Self::relative(file, name, access, 2) }
    }

    unsafe fn relative(
        parent: HANDLE,
        name: Vec<u16>,
        access: u32,
        disposition: u32,
    ) -> Result<Self, i32> {
        unsafe { Self::relative_with_share(parent, name, access, disposition, SHARE) }
    }

    pub(crate) fn create_directory_child(
        &self,
        stored: &OsStr,
        existing: bool,
    ) -> Result<Self, i32> {
        let name = wide(stored)?;
        if name.len() < 2
            || name == [46, 0]
            || name == [46, 46, 0]
            || name[..name.len() - 1]
                .iter()
                .any(|&c| matches!(c, 47 | 92 | 58))
        {
            return Err(EINVAL);
        }
        unsafe {
            Self::relative_options(
                self.raw(),
                name,
                FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | 1,
                if existing { 3 } else { 2 },
                SHARE,
                0x0020_0001,
            )
        }
    }
    pub(crate) fn create_regular_child(&self, stored: &OsStr, access: u32) -> Result<Self, i32> {
        let name = wide(stored)?;
        if name.len() < 2
            || name == [46, 0]
            || name == [46, 46, 0]
            || name[..name.len() - 1]
                .iter()
                .any(|&c| matches!(c, 47 | 92 | 58))
        {
            return Err(EINVAL);
        }
        unsafe { Self::relative_options(self.raw(), name, access, 2, SHARE, 0x0020_0040) }
    }
    unsafe fn relative_with_share(
        parent: HANDLE,
        name: Vec<u16>,
        access: u32,
        disposition: u32,
        share: u32,
    ) -> Result<Self, i32> {
        unsafe { Self::relative_options(parent, name, access, disposition, share, 0x0020_0000) }
    }
    unsafe fn relative_options(
        parent: HANDLE,
        mut name: Vec<u16>,
        access: u32,
        disposition: u32,
        share: u32,
        options: u32,
    ) -> Result<Self, i32> {
        let length = u16::try_from((name.len() - 1) * 2).map_err(|_| ENAMETOOLONG)?;
        let maximum_length = u16::try_from(name.len() * 2).map_err(|_| ENAMETOOLONG)?;
        let mut string = UnicodeString {
            length,
            maximum_length,
            buffer: name.as_mut_ptr(),
        };
        let attrs = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root: parent,
            name: &mut string,
            attributes: 0,
            security: ptr::null_mut(),
            qos: ptr::null_mut(),
        };
        let mut io = NativeIoStatus::default();
        let mut handle = ptr::null_mut();
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                access | SYNCHRONIZE,
                &attrs,
                &mut io,
                ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                share,
                disposition,
                options,
                ptr::null(),
                0,
            )
        };
        if status < 0 {
            if status as u32 == 0xc000_0043 && share & FILE_SHARE_WRITE == 0 {
                return Err(26); // ETXTBSY: an existing data writer or mapping.
            }
            return Err(errno_from_win32(unsafe { RtlNtStatusToDosError(status) }));
        }
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            // A successful/pending native open without a handle is a broken
            // kernel/API invariant; no buffer can safely unwind if still pending.
            if status == 0x103 {
                std::process::abort();
            }
            return Err(EIO);
        }
        let object = Self(handle as usize);
        unsafe { complete_native_io(handle, &mut io, status)? };
        Ok(object)
    }

    /// Current diagnostic pathname, not an authority for opening the inode.
    pub(crate) fn path(&self) -> Result<PathBuf, i32> {
        super::path_from_handle(self.raw()).ok_or(EIO)
    }

    /// An independent directory scan on the pinned inode. This is a real
    /// directory read (and may update atime), not component pathname lookup.
    pub(crate) fn entries(&self) -> Result<Vec<OsString>, i32> {
        self.entries_with_atime(false)
    }

    pub(crate) fn maintenance_entries(&self) -> Result<Vec<OsString>, i32> {
        self.entries_with_atime(true)
    }

    fn entries_with_atime(&self, suppress_atime: bool) -> Result<Vec<OsString>, i32> {
        let query = {
            Self::reopen(
                self.raw(),
                FILE_READ_ATTRIBUTES
                    | FILE_LIST_DIRECTORY
                    | if suppress_atime {
                        FILE_WRITE_ATTRIBUTES
                    } else {
                        0
                    },
            )?
        };
        if suppress_atime {
            query.suppress_atime()?;
        }
        let mut storage = vec![0u64; 4096];
        let mut first = true;
        let mut result = Vec::new();
        loop {
            let mut io = NativeIoStatus::default();
            let status = unsafe {
                NtQueryDirectoryFile(
                    query.raw(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    &mut io,
                    storage.as_mut_ptr().cast(),
                    (storage.len() * 8) as u32,
                    12,
                    0,
                    ptr::null(),
                    u8::from(first),
                )
            };
            let status = unsafe { super::complete_native_status(query.raw(), &mut io, status)? };
            first = false;
            if status as u32 == 0x8000_0006 {
                return Ok(result);
            }
            if status < 0 {
                return Err(errno_from_win32(unsafe { RtlNtStatusToDosError(status) }));
            }
            if io.information == 0 || io.information > storage.len() * 8 {
                return Err(EIO);
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), io.information)
            };
            let mut cursor = 0;
            loop {
                let record = &bytes[cursor..];
                if record.len() < 12 {
                    return Err(EIO);
                }
                let next = u32::from_le_bytes(record[0..4].try_into().unwrap()) as usize;
                let length = u32::from_le_bytes(record[8..12].try_into().unwrap()) as usize;
                if length % 2 != 0 || length > record.len() - 12 {
                    return Err(EIO);
                }
                let name: Vec<u16> = record[12..12 + length]
                    .chunks_exact(2)
                    .map(|v| u16::from_le_bytes([v[0], v[1]]))
                    .collect();
                let name = OsString::from_wide(&name);
                if name != "." && name != ".." {
                    result.push(name);
                }
                if next == 0 {
                    break;
                }
                if next < 12 + length || next >= record.len() {
                    return Err(EIO);
                }
                cursor += next;
            }
        }
    }

    /// Enumerate streams on this inode without reopening its pathname. The
    /// private query handle keeps pending I/O independent of shared references.
    pub(crate) fn streams(&self) -> Result<Vec<(OsString, u64)>, i32> {
        let query = Self::reopen(self.raw(), FILE_READ_ATTRIBUTES)?;
        let mut storage = vec![0u64; 512];
        loop {
            let length = u32::try_from(storage.len() * 8).map_err(|_| crate::EOVERFLOW)?;
            let mut io = NativeIoStatus::default();
            let status = unsafe {
                NtQueryInformationFile(
                    query.raw(),
                    &mut io,
                    storage.as_mut_ptr().cast(),
                    length,
                    22,
                )
            };
            let status = unsafe { super::complete_native_status(query.raw(), &mut io, status)? };
            if matches!(status as u32, 0x8000_0005 | 0xc000_0004 | 0xc000_0023) {
                let extra = storage.len();
                if extra > u32::MAX as usize / 16 {
                    return Err(crate::EOVERFLOW);
                }
                storage
                    .try_reserve_exact(extra)
                    .map_err(|_| crate::ENOMEM)?;
                storage.resize(extra * 2, 0);
                continue;
            }
            if status < 0 {
                return Err(errno_from_win32(unsafe { RtlNtStatusToDosError(status) }));
            }
            if io.information > length as usize {
                return Err(EIO);
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), io.information)
            };
            let mut cursor = 0;
            let mut streams = Vec::new();
            while cursor < bytes.len() {
                let record = &bytes[cursor..];
                if record.len() < 24 {
                    return Err(EIO);
                }
                let next = u32::from_le_bytes(record[0..4].try_into().unwrap()) as usize;
                let size = u32::from_le_bytes(record[4..8].try_into().unwrap()) as usize;
                let length = i64::from_le_bytes(record[8..16].try_into().unwrap());
                if size % 2 != 0 || size > record.len() - 24 || length < 0 {
                    return Err(EIO);
                }
                let name: Vec<u16> = record[24..24 + size]
                    .chunks_exact(2)
                    .map(|v| u16::from_le_bytes([v[0], v[1]]))
                    .collect();
                streams.push((OsString::from_wide(&name), length as u64));
                if next == 0 {
                    break;
                }
                if next < 24 + size || next > record.len() {
                    return Err(EIO);
                }
                cursor += next;
            }
            return Ok(streams);
        }
    }

    pub(crate) fn suppress_atime(&self) -> Result<(), i32> {
        let disabled = FILETIME {
            dwLowDateTime: u32::MAX,
            dwHighDateTime: u32::MAX,
        };
        if unsafe { SetFileTime(self.raw(), ptr::null(), &disabled, ptr::null()) } == 0 {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(())
        }
    }

    /// Metadata transactions must not project their hidden reads/appends as a
    /// guest access or modification timestamp change.
    pub(crate) fn suppress_data_times(&self) -> Result<(), i32> {
        let disabled = FILETIME {
            dwLowDateTime: u32::MAX,
            dwHighDateTime: u32::MAX,
        };
        if unsafe { SetFileTime(self.raw(), ptr::null(), &disabled, &disabled) } == 0 {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(())
        }
    }

    pub(crate) fn read_at(&self, offset: u64, bytes: &mut [u8]) -> Result<usize, i32> {
        if crate::interrupt::current().is_null() {
            return Err(EIO);
        }
        let entry = FdEntry {
            raw: self.0,
            kind: FdKind::File,
            flags: FdFlags::OVERLAPPED.union(FdFlags::SEEKABLE),
            generation: 0,
            description_id: 0,
            offset,
        };
        unsafe { crate::platform::transfer_once(&entry, bytes.as_mut_ptr(), bytes.len(), true) }
    }

    /// Internal positional write, with interruption returned before guest signal
    /// handlers are dispatched and before the caller's transaction is unwound.
    pub(crate) fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<usize, i32> {
        if crate::interrupt::current().is_null() {
            return Err(EIO);
        }
        let entry = FdEntry {
            raw: self.0,
            kind: FdKind::File,
            flags: FdFlags::OVERLAPPED.union(FdFlags::SEEKABLE),
            generation: 0,
            description_id: 0,
            offset,
        };
        unsafe {
            crate::platform::transfer_once(&entry, bytes.as_ptr().cast_mut(), bytes.len(), false)
        }
    }

    /// Change native EOF without changing any shared file position.
    pub(crate) fn set_length(&self, length: u64) -> Result<(), i32> {
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
        let length = i64::try_from(length).map_err(|_| crate::EOVERFLOW)?;
        let mut io = NativeIoStatus::default();
        let status = unsafe {
            NtSetInformationFile(
                self.raw(),
                &mut io,
                (&length as *const i64).cast(),
                std::mem::size_of_val(&length) as u32,
                20, // FileEndOfFileInformation
            )
        };
        unsafe { complete_native_io(self.raw(), &mut io, status) }
    }

    /// Flush this private asynchronous inode handle, retiring cancellation before
    /// the native IO status or transaction buffers can go out of scope.
    pub(crate) fn flush(&self) -> Result<(), i32> {
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtFlushBuffersFile(file: HANDLE, io: *mut NativeIoStatus) -> i32;
        }
        let mut io = NativeIoStatus::default();
        let status = unsafe { NtFlushBuffersFile(self.raw(), &mut io) };
        let result = unsafe { complete_native_io(self.raw(), &mut io, status) };
        if let Err(error) = result {
            crate::mount::overlay::record_io_error(self.raw(), error);
        }
        result
    }
}

/// Read a legacy record for explicit offline migration, never live inode stat.
/// Absence is distinct from an I/O/access error or a corrupt oversized record.
/// # Safety
/// The borrowed inode handle must remain live until the stream has been opened.
pub(super) unsafe fn metadata_stream(
    file: HANDLE,
    name: &str,
    limit: usize,
) -> Result<Option<Vec<u8>>, i32> {
    let stream = match unsafe {
        Object::stream(file, OsStr::new(name), GENERIC_READ | FILE_WRITE_ATTRIBUTES)
    } {
        Ok(stream) => stream,
        Err(ENOENT) => return Ok(None),
        Err(error) => return Err(error),
    };
    stream.suppress_atime()?;
    let mut length = 0i64;
    if unsafe { GetFileSizeEx(stream.raw(), &mut length) } == 0 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let length = usize::try_from(length).map_err(|_| EIO)?;
    if length > limit {
        return Err(EIO);
    }
    let mut bytes = vec![0; length];
    let mut offset = 0;
    while offset < length {
        let count = stream.read_at(offset as u64, &mut bytes[offset..])?;
        if count == 0 {
            return Err(EIO);
        }
        offset += count;
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{self, S_IFCHR, S_IFLNK, S_IFMT};
    use windows_sys::Win32::Storage::FileSystem::FILE_READ_EA;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("kinakaze-inode-{}-{stamp}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn readonly_unlink_detaches_name_and_retains_open_inode_attributes() {
        let f = Fixture::new();
        let path = f.path("readonly-unlink");
        std::fs::write(&path, b"original").unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let original =
            Object::open(&path, GENERIC_READ | FILE_READ_ATTRIBUTES | FILE_READ_EA).unwrap();
        let delete = Object::open(&path, windows_sys::Win32::Storage::FileSystem::DELETE).unwrap();
        fs::unlink_inode(delete.raw()).unwrap();
        drop(delete);
        assert!(
            !path.exists(),
            "POSIX unlink must detach the name immediately"
        );
        let mut bytes = [0; 8];
        assert_eq!(original.read_at(0, &mut bytes).unwrap(), 8);
        assert_eq!(&bytes, b"original");
        let mut info = core::mem::MaybeUninit::<
            windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION,
        >::uninit();
        assert_ne!(
            unsafe {
                windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle(
                    original.raw(),
                    info.as_mut_ptr(),
                )
            },
            0
        );
        let info = unsafe { info.assume_init() };
        assert_ne!(
            info.dwFileAttributes
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY,
            0
        );
        std::fs::write(&path, b"replacement").unwrap();
        assert_eq!(original.read_at(0, &mut bytes).unwrap(), 8);
        assert_eq!(&bytes, b"original");
    }

    #[test]
    fn stat_metadata_survives_unlink_and_name_replacement() {
        let f = Fixture::new();
        let path = f.path("file");
        std::fs::write(&path, b"original").unwrap();
        fs::set_mode_host_path(&path, 0o640).unwrap();
        let original = Object::open(&path, FILE_READ_ATTRIBUTES | FILE_READ_EA).unwrap();
        let before = fs::stat_handle(original.raw(), false).unwrap();
        fs::unlink(&crate::path::to_guest_path(&path)).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        fs::set_mode_host_path(&path, 0o600).unwrap();
        let after = fs::stat_handle(original.raw(), false).unwrap();
        assert_eq!(after.st_ino, before.st_ino);
        assert_eq!(after.st_mode & 0o7777, 0o640);
        assert_eq!(after.st_size, 8);
        fs::set_mode_handle(original.raw(), 0o444).unwrap();
        assert_eq!(
            fs::stat_handle(original.raw(), false).unwrap().st_mode & 0o7777,
            0o444
        );
        fs::set_mode_handle(original.raw(), 0o640).unwrap();
        assert_eq!(
            fs::stat_handle(
                Object::open(&path, FILE_READ_ATTRIBUTES).unwrap().raw(),
                false
            )
            .unwrap()
            .st_mode
                & 0o7777,
            0o600
        );
    }

    #[test]
    fn symlink_and_device_records_follow_the_inode_after_rename() {
        let f = Fixture::new();
        let link = f.path("link");
        crate::path::create_emulated_symlink(&link, "../目标").unwrap();
        let object = Object::open(&link, FILE_READ_ATTRIBUTES).unwrap();
        std::fs::rename(&link, f.path("moved-link")).unwrap();
        crate::path::create_emulated_symlink(&link, "replacement").unwrap();
        assert_eq!(
            fs::symlink_target_handle(object.raw()).unwrap().as_deref(),
            Some("../目标")
        );
        let stat = fs::stat_handle(object.raw(), false).unwrap();
        assert_eq!(stat.st_mode & S_IFMT, S_IFLNK);
        assert_eq!(stat.st_size, "../目标".len() as i64);
        let device = f.path("device");
        fs::create_device(&crate::path::to_guest_path(&device), S_IFCHR | 0o600, 0x103).unwrap();
        let object = Object::open(&device, FILE_READ_ATTRIBUTES).unwrap();
        std::fs::rename(&device, f.path("moved-device")).unwrap();
        std::fs::write(&device, b"not a device").unwrap();
        let stat = fs::stat_handle(object.raw(), false).unwrap();
        assert_eq!(stat.st_mode & S_IFMT, S_IFCHR);
        assert_eq!(stat.st_rdev, 0x103);
    }

    #[test]
    fn malformed_metadata_is_not_replaced_by_guessed_host_permissions() {
        let f = Fixture::new();
        std::fs::write(f.path("file"), b"data").unwrap();
        let object = Object::open(&f.path("file"), FILE_READ_ATTRIBUTES).unwrap();
        let writer = Object::reopen(
            object.raw(),
            windows_sys::Win32::Storage::FileSystem::FILE_WRITE_EA,
        )
        .unwrap();
        for value in [b"bad".as_slice(), b"BAD!0000", b"CYINODE2"] {
            super::super::ea::write(&writer, super::super::inode::EA_NAME, value).unwrap();
            assert!(matches!(fs::stat_handle(object.raw(), false), Err(EIO)));
        }
    }

    #[test]
    fn native_ea_update_works_on_a_readonly_inode_and_preserves_guest_xattrs() {
        let f = Fixture::new();
        let path = f.path("readonly");
        std::fs::write(&path, b"data").unwrap();
        crate::xattr::Attributes::open_host(&path, true)
            .unwrap()
            .set(b"user.keep", b"value", 0)
            .unwrap();
        fs::set_mode_host_path(&path, 0o400).unwrap();
        let object = Object::open(&path, FILE_READ_ATTRIBUTES).unwrap();
        fs::inode::update(object.raw(), |record| {
            record.mode = Some(0o440);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::stat_handle(object.raw(), false).unwrap().st_mode & 0o7777,
            0o440
        );
        assert_eq!(
            crate::xattr::Attributes::open_host(&path, false)
                .unwrap()
                .get(b"user.keep")
                .unwrap(),
            b"value"
        );
        fs::set_mode_host_path(&path, 0o640).unwrap();
    }

    #[test]
    fn large_symlink_record_grows_the_native_ea_query_buffer() {
        let f = Fixture::new();
        let target = "component/".repeat(350);
        crate::path::create_emulated_symlink(&f.path("link"), &target).unwrap();
        let object = Object::open(&f.path("link"), FILE_READ_ATTRIBUTES).unwrap();
        fs::unlink(&crate::path::to_guest_path(&f.path("link"))).unwrap();
        assert_eq!(
            fs::symlink_target_handle(object.raw()).unwrap(),
            Some(target.clone())
        );
        assert_eq!(
            fs::stat_handle(object.raw(), false).unwrap().st_size,
            target.len() as i64
        );
    }

    #[test]
    fn pinned_descriptor_survives_guest_close_and_inode_name_replacement() {
        let f = Fixture::new();
        let path = f.path("link");
        crate::path::create_emulated_symlink(&path, "old target").unwrap();
        let guest = crate::path::to_guest_path(&path);
        let fd = fs::open(&guest, fs::O_PATH | fs::O_NOFOLLOW, 0).unwrap();
        let pinned = Object::from_fd(fd).unwrap();
        crate::close(fd).unwrap();
        fs::unlink(&guest).unwrap();
        crate::path::create_emulated_symlink(&path, "new target").unwrap();
        assert_eq!(fs::readlink_handle(pinned.raw()).unwrap(), "old target");
        assert_eq!(fs::read_link_host_path(&path).unwrap(), "new target");
        assert!(matches!(Object::from_fd(-1), Err(crate::EBADF)));
    }

    #[test]
    fn directory_enumeration_reopens_the_inode_and_spans_native_buffers() {
        let f = Fixture::new();
        let dir = f.path("dir");
        std::fs::create_dir(&dir).unwrap();
        let mut expected = std::collections::BTreeSet::new();
        for index in 0..1000 {
            let name = format!("entry-{index:04}-{}", "x".repeat(30));
            std::fs::write(dir.join(&name), b"").unwrap();
            expected.insert(OsString::from(name));
        }
        let object = Object::open(&dir, FILE_READ_ATTRIBUTES).unwrap();
        std::fs::rename(&dir, f.path("renamed")).unwrap();
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("wrong"), b"").unwrap();
        assert_eq!(
            object
                .entries()
                .unwrap()
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        assert_eq!(object.entries().unwrap().len(), 1000);
        assert!(matches!(
            object.child(OsStr::new(".."), FILE_READ_ATTRIBUTES),
            Err(EINVAL)
        ));
    }

    #[test]
    fn guest_readdir_keeps_an_open_directory_outside_a_new_root() {
        let f = Fixture::new();
        let directory = f.path("volume");
        std::fs::create_dir_all(directory.join("subdir")).unwrap();
        std::fs::write(directory.join("file"), b"data").unwrap();
        crate::path::create_emulated_symlink(&directory.join("link"), "file").unwrap();
        let root = f.path("container");
        std::fs::create_dir(&root).unwrap();
        let pinned = Object::open(&directory, FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY).unwrap();
        std::fs::rename(&directory, f.path("renamed-volume")).unwrap();
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("wrong"), b"").unwrap();
        let raw = pinned.into_raw() as usize;
        // An isolated fs_struct leaves other tests' root unchanged.
        std::thread::spawn(move || {
            crate::fs_context::unshare();
            crate::path::set_system_root(root);
            let fd =
                crate::install(raw, crate::FdKind::Directory, crate::FdFlags::READ_ACCESS).unwrap();
            let entries = fs::read_directory_fd(fd).unwrap();
            crate::close(fd).unwrap();
            let names: std::collections::BTreeSet<_> =
                entries.iter().map(|entry| entry.name.as_str()).collect();
            assert_eq!(
                names,
                [".", "..", "file", "link", "subdir"].into_iter().collect()
            );
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.name == "subdir" && entry.is_directory)
            );
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.name == "link" && entry.is_symlink)
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn metadata_queries_preserve_directory_atime_after_the_last_handle_closes() {
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_TRAVERSE, GetFileInformationByHandle,
        };
        let expected = 132_000_000_000_000_000u64;
        let stamp = FILETIME {
            dwLowDateTime: expected as u32,
            dwHighDateTime: (expected >> 32) as u32,
        };
        // A real readdir (10) is deliberately absent: reading the directory is
        // allowed to update atime. These operations inspect metadata/names only.
        for operation in [0, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13] {
            let f = Fixture::new();
            std::fs::create_dir(f.path("dir")).unwrap();
            std::fs::write(f.path("dir/child"), b"data").unwrap();
            fs::set_mode_host_path(&f.path("dir"), 0o751).unwrap();
            {
                let writer = Object::open(&f.path("dir"), FILE_WRITE_ATTRIBUTES).unwrap();
                assert_ne!(
                    unsafe { SetFileTime(writer.raw(), ptr::null(), &stamp, ptr::null()) },
                    0
                );
            }
            {
                let access = match operation {
                    2 => FILE_TRAVERSE,
                    3 => GENERIC_READ,
                    12 => FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
                    _ => FILE_READ_ATTRIBUTES | FILE_READ_EA,
                };
                let object = Object::open(&f.path("dir"), access).unwrap();
                match operation {
                    4 => {
                        let _ = object
                            .child(OsStr::new("child"), FILE_READ_ATTRIBUTES)
                            .unwrap();
                    }
                    5 => {
                        let _ = Object::reopen(object.raw(), FILE_READ_ATTRIBUTES | FILE_READ_EA)
                            .unwrap();
                    }
                    6 => {
                        let _ = fs::stat_handle(object.raw(), false).unwrap();
                    }
                    7 => {
                        let _ = object.path().unwrap();
                    }
                    8 => {
                        let _ =
                            unsafe { crate::xattr::Attributes::from_handle(object.raw(), false) }
                                .unwrap()
                                .snapshot()
                                .unwrap();
                    }
                    9 => {
                        let _ = object.streams().unwrap();
                    }
                    11 => {
                        assert!(matches!(
                            object.child(OsStr::new("missing"), FILE_READ_ATTRIBUTES),
                            Err(ENOENT)
                        ));
                    }
                    12 => {
                        object.suppress_atime().unwrap();
                        assert!(matches!(
                            object.child(OsStr::new("missing"), FILE_READ_ATTRIBUTES),
                            Err(ENOENT)
                        ));
                    }
                    13 => {
                        assert!(matches!(
                            Object::open(&f.path("dir/missing"), FILE_READ_ATTRIBUTES),
                            Err(ENOENT)
                        ));
                    }
                    _ => {}
                }
            }
            let observer = Object::open(&f.path("dir"), FILE_READ_ATTRIBUTES).unwrap();
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            assert_ne!(
                unsafe { GetFileInformationByHandle(observer.raw(), &mut info) },
                0
            );
            let actual = ((info.ftLastAccessTime.dwHighDateTime as u64) << 32)
                | info.ftLastAccessTime.dwLowDateTime as u64;
            assert_eq!(
                actual, expected,
                "metadata operation {operation} changed atime"
            );
        }
    }

    #[test]
    fn stream_enumeration_grows_its_buffer_and_stays_on_the_same_inode() {
        let f = Fixture::new();
        std::fs::write(f.path("file"), b"main").unwrap();
        let mut expected = std::collections::BTreeSet::from([OsString::from("::$DATA")]);
        for index in 0..120 {
            let name = format!("stream-{index:03}-{}", "s".repeat(35));
            std::fs::write(f.path(&format!("file:{name}")), b"stream").unwrap();
            expected.insert(OsString::from(format!(":{name}:$DATA")));
        }
        let object = Object::open(&f.path("file"), FILE_READ_ATTRIBUTES).unwrap();
        std::fs::rename(f.path("file"), f.path("renamed")).unwrap();
        std::fs::write(f.path("file"), b"replacement").unwrap();
        let streams = object.streams().unwrap();
        assert_eq!(
            streams
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        for (name, length) in streams {
            assert_eq!(length, if name == "::$DATA" { 4 } else { 6 });
        }
    }
}
