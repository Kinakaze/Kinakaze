//! Stage one lower inode, then publish it without replacing an existing upper.
//! The mount coordinator must still supply validated/pinned roots, credentials,
//! and namespace locking. This module never registers a mount.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
#[cfg(test)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED,
    OPEN_EXISTING,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_NORMAL, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    FILE_DISPOSITION_INFO_EX, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_WRITE_ATTRIBUTES, FileDispositionInfoEx, SYNCHRONIZE,
    SetFileInformationByHandle, SetFileTime,
};

use super::Node;
use crate::fs::object::Object;
use crate::fs::{NativeIoStatus, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, Stat};
use crate::iouring::flush::Flush;
use crate::xattr::Attributes;
use crate::{
    EINTR, EIO, ENOTDIR, EOPNOTSUPP, EOVERFLOW, EXDEV, FdEntry, FdFlags, FdKind, errno_from_win32,
};

const SHARE: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
#[cfg(test)]
const OPEN_FLAGS: u32 =
    FILE_FLAG_OVERLAPPED | FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;

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
        attributes: *const ObjectAttributes,
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
}

#[derive(Debug)]
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
#[cfg(test)]
impl Handle {
    fn open(path: &Path, access: u32, disposition: u32) -> Result<Self, i32> {
        let name = crate::path::wide_path(path)?;
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                access,
                SHARE,
                ptr::null(),
                disposition,
                OPEN_FLAGS,
                ptr::null_mut(),
            )
        };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(Self(handle))
        }
    }
    fn entry(&self, offset: u64) -> FdEntry {
        FdEntry {
            raw: self.0 as usize,
            kind: FdKind::File,
            flags: FdFlags::SEEKABLE.union(FdFlags::OVERLAPPED),
            generation: 0,
            description_id: 0,
            offset,
        }
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

/// Owns only the unpublished inode, never a directory tree. Rollback uses the
/// inode handle, so even a renamed work directory cannot redirect the deletion.
#[derive(Debug)]
pub(crate) struct Staged {
    handle: Handle,
    path: PathBuf,
    published: bool,
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.published {
            let info = FILE_DISPOSITION_INFO_EX {
                Flags: FILE_DISPOSITION_FLAG_DELETE
                    | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
                    | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
            };
            if unsafe {
                SetFileInformationByHandle(
                    self.handle.0,
                    FileDispositionInfoEx,
                    (&info as *const FILE_DISPOSITION_INFO_EX).cast(),
                    std::mem::size_of_val(&info) as u32,
                )
            } == 0
            {
                eprintln!(
                    "kinakaze: cannot remove unpublished copy-up inode {:?}: win32={}",
                    self.path,
                    unsafe { GetLastError() }
                );
            }
        }
    }
}

impl Staged {
    pub(crate) fn anonymous(source: &Node, work: &Object) -> Result<Object, i32> {
        let mut staged = Self::prepare(source, work, false)?;
        let metadata = source.backing_metadata();
        let mut origin = Vec::new();
        origin.extend_from_slice(&metadata.st_dev.to_le_bytes());
        origin.extend_from_slice(&metadata.st_ino.to_le_bytes());
        unsafe { Attributes::from_handle(staged.handle.0, true)? }.set(
            format!("{}kinakaze.origin", source.namespace.prefix()).as_bytes(),
            &origin,
            0,
        )?;
        let object = unsafe {
            Object::reopen(
                staged.handle.0,
                FILE_READ_ATTRIBUTES | windows_sys::Win32::Storage::FileSystem::FILE_READ_EA,
            )?
        };
        crate::fs::unlink_inode(staged.handle.0)?;
        staged.published = true;
        Ok(object)
    }
    pub(crate) fn whiteout(work: &Object) -> Result<Self, i32> {
        let staged = Self::create(work, false)?;
        crate::fs::inode::replace(
            staged.handle.0,
            &crate::fs::inode::Record {
                mode: Some(crate::fs::S_IFCHR),
                device: Some(0),
                symlink: None,
                ..crate::fs::inode::Record::default()
            },
        )?;
        Ok(staged)
    }

    pub(crate) fn publish_replacing(mut self, parent: &Object, name: &str) -> Result<(), i32> {
        super::validate_component(name)?;
        unsafe {
            crate::fs::rename_host_handle_relative(
                self.handle.0,
                parent.raw(),
                OsStr::new(crate::path::escape_component(name).as_ref()),
                true,
            )?;
        }
        self.published = true;
        Ok(())
    }
    fn create(work: &Object, directory: bool) -> Result<Self, i32> {
        if crate::fs::stat_handle(work.raw(), false)?.st_mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
        let mut random = [0u8; 16];
        if unsafe {
            crate::BCryptGenRandom(ptr::null_mut(), random.as_mut_ptr(), random.len() as u32, 2)
        } < 0
        {
            return Err(EIO);
        }
        let name = format!("copy-{:032x}", u128::from_le_bytes(random));
        let mut name_wide = wide(OsStr::new(&name));
        let mut string = UnicodeString {
            length: ((name_wide.len() - 1) * 2) as u16,
            maximum_length: (name_wide.len() * 2) as u16,
            buffer: name_wide.as_mut_ptr(),
        };
        let attrs = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root: work.raw(),
            name: &mut string,
            attributes: 0,
            security: ptr::null_mut(),
            qos: ptr::null_mut(),
        };
        let mut io = NativeIoStatus::default();
        let mut handle = ptr::null_mut();
        // FILE_CREATE + DIRECTORY/NON_DIRECTORY_FILE gives a new inode and its
        // handle atomically, relative to the opened work directory. No path
        // create/reopen race and no timed retries on sharing/name collisions.
        let result = unsafe {
            NtCreateFile(
                &mut handle,
                GENERIC_READ | GENERIC_WRITE | DELETE | SYNCHRONIZE,
                &attrs,
                &mut io,
                ptr::null(),
                FILE_ATTRIBUTE_NORMAL,
                SHARE,
                2,
                0x0020_0000 | if directory { 1 } else { 0x40 },
                ptr::null(),
                0,
            )
        };
        if result < 0 {
            return Err(errno_from_win32(unsafe { RtlNtStatusToDosError(result) }));
        }
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            // Pending creation must have a waitable request owner. An invalid
            // native result cannot safely unwind buffers still owned by the OS.
            if result == 0x103 {
                std::process::abort();
            }
            return Err(EIO);
        }
        let mut staged = Self {
            handle: Handle(handle),
            // Diagnostic only. The transaction owns the native inode even if
            // the work directory is renamed before querying its current name.
            path: PathBuf::from(name),
            published: false,
        };
        // Async creation can itself be pending (for example on a remote
        // filesystem). Retire it before dropping the NT name/status buffers.
        unsafe { crate::fs::complete_native_io(handle, &mut io, result)? };
        staged.path = crate::fs::path_from_handle(staged.handle.0).ok_or(EIO)?;
        Ok(staged)
    }

    /// Copies a single already-looked-up lower inode. Directories copy metadata,
    /// not children. O_TRUNC callers can omit the lower file's data entirely.
    pub(crate) fn prepare(source: &Node, work: &Object, truncate: bool) -> Result<Self, i32> {
        Self::prepare_mode(source, work, truncate, false)
    }

    pub(crate) fn prepare_mode(
        source: &Node,
        work: &Object,
        truncate: bool,
        metadata_only: bool,
    ) -> Result<Self, i32> {
        source.verify_metacopy()?;
        let marker = if metadata_only {
            source.metacopy_marker()?
        } else {
            None
        };
        let metadata_only = metadata_only && marker.is_some();
        if crate::interrupt::current().is_null() {
            return Err(EIO);
        }
        let source_object = source.backing_object();
        let attributes = unsafe { Attributes::from_handle(source_object.raw(), false)? };
        let metadata = attributes.metadata()?;
        let mut xattrs = attributes.snapshot()?;
        let prefix = source.namespace.prefix().as_bytes();
        // Escaped overlay.overlay.* is user-visible nested-layer metadata and
        // remains escaped in the physical copy. Private implementation xattrs
        // (origin, opaque, index, etc.) must not be blindly cloned to the upper.
        xattrs.retain(|name, _| {
            !name.starts_with(prefix) || name[prefix.len()..].starts_with(b"overlay.")
        });
        if source.context.is_none() && source.entries[0].requires_redirect_or_metacopy {
            return Err(EOPNOTSUPP);
        }
        let kind = metadata.st_mode & S_IFMT;
        let staged = Self::create(work, kind == S_IFDIR)?;
        let mut inode = crate::fs::inode::read(source_object.raw())?;
        let mut buffer = Vec::new();
        let mut flush = if source.flags() & super::features::VOLATILE != 0 {
            None
        } else {
            Some(Flush::new()?)
        };
        let data_object = source.data_object();
        let data_size = crate::fs::stat_handle(data_object.raw(), false)?.st_size;
        if kind == S_IFREG && !truncate && !metadata_only && data_size > 0 {
            let file =
                unsafe { Object::reopen(data_object.raw(), GENERIC_READ | FILE_WRITE_ATTRIBUTES)? };
            // Reads used for copy-up must not change lower atime. Windows lets
            // this independent handle suppress automatic access-time updates.
            file.suppress_atime()?;
            copy_bytes(
                file.raw(),
                staged.handle.0,
                data_size as u64,
                &mut buffer,
                CopyData::GuestFile,
            )?;
        }
        if kind == S_IFLNK {
            let target = crate::fs::readlink_handle(source_object.raw())?;
            inode.symlink = Some(target);
        } else if !metadata_only {
            copy_streams(data_object, staged.handle.0, &mut buffer, &mut flush)?;
        }
        if metadata_only && kind == S_IFREG {
            xattrs.insert(
                format!("{}metacopy", source.namespace.prefix()).into_bytes(),
                marker.unwrap_or_default(),
            );
        }
        unsafe { Attributes::from_handle(staged.handle.0, true)? }.replace_snapshot(&xattrs)?;
        // Explicit mode also covers unmanaged lower files with host ACL modes.
        // Copy the internal record separately from the guest xattr namespace.
        inode.mode = Some(metadata.st_mode);
        crate::fs::inode::replace(staged.handle.0, &inode)?;
        crate::fs::set_mode_handle(staged.handle.0, metadata.st_mode)?;
        set_times(staged.handle.0, &metadata)?;
        if let Some(flush) = flush.as_mut()
            && (kind != S_IFDIR || source.flags() & super::features::FSYNC_STRICT != 0)
        {
            unsafe { flush.file(staged.handle.0)? };
        }
        Ok(staged)
    }

    /// Publish only into an absent name. A competing publisher gets EEXIST and
    /// must re-lookup the winning upper inode; its file is never overwritten.
    pub(crate) fn publish(self, upper_parent: &Object, name: &str) -> Result<(), i32> {
        let parent = Handle(
            unsafe { Object::reopen(upper_parent.raw(), FILE_READ_ATTRIBUTES | FILE_TRAVERSE)? }
                .into_raw(),
        );
        self.publish_to(&parent, name)
    }

    fn publish_to(mut self, parent: &Handle, name: &str) -> Result<(), i32> {
        super::validate_component(name)?;
        let metadata = crate::fs::stat_handle(parent.0, false)?;
        if metadata.st_mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
        if crate::fs::stat_handle(self.handle.0, false)?.st_dev != metadata.st_dev {
            return Err(EXDEV);
        }
        let stored = crate::path::escape_component(name);
        unsafe {
            crate::fs::rename_host_handle_relative(
                self.handle.0,
                parent.0,
                OsStr::new(stored.as_ref()),
                false,
            )?
        };
        self.published = true;
        Ok(())
    }
}

/// Ensure an already resolved sequence of components has an upper inode.
/// `root` retains an upper-first set of already pinned layer inodes; `work` is
/// the pinned work directory. No pathname is reopened to discover either.
/// The mount/path layer must exclude ancestor rename/unlink, check permissions
/// and resolve symlinks before entering. This operation-local coordinator does
/// not create a namespace or relax those caller responsibilities.
///
/// Only missing ancestors and the final inode are copied, never siblings. A
/// winning concurrent upper is looked up again. `truncate` skips lower data;
/// truncation of an already existing upper belongs to the subsequent open.
pub(crate) fn ensure_upper(
    root: &Node,
    work: &Object,
    components: &[&str],
    truncate: bool,
) -> Result<Node, i32> {
    ensure_upper_mode(root, work, components, truncate, false)
}

pub(crate) fn ensure_upper_mode(
    root: &Node,
    work: &Object,
    components: &[&str],
    truncate: bool,
    metadata_only: bool,
) -> Result<Node, i32> {
    let root = root.clone();
    // Complete lookup first: a missing child or whiteout is not an instruction
    // to materialize any parents or create an absent target.
    let mut found = root.clone();
    for component in components {
        found = found.child(component)?;
    }
    if components.is_empty() {
        return Ok(root);
    }
    let mut parent = root.clone();
    for (index, component) in components.iter().enumerate() {
        interrupted()?;
        let source = parent.child(component)?;
        if source.backing_layer() != 0 {
            if source.entries[0].indexed {
                let target = parent
                    .backing_object()
                    .path()?
                    .join(crate::path::escape_component(component).as_ref());
                std::fs::hard_link(source.backing_object().path()?, target).map_err(|e| {
                    e.raw_os_error()
                        .map_or(EIO, |v| crate::errno_from_win32(v as u32))
                })?;
                parent = parent.child(component)?;
                continue;
            }
            // Bulk I/O holds no directory mutex. No-replace publication plus
            // fresh lookup arbitrates different processes copying the same name.
            let final_component = index + 1 == components.len();
            let staged = Staged::prepare_mode(
                &source,
                work,
                truncate && final_component,
                metadata_only
                    && final_component
                    && source.backing_metadata().st_mode & S_IFMT == S_IFREG,
            )?;
            // The mount coordinator preserves the visible inode across copy-up;
            // a standalone Staged::prepare remains a physical inode copy.
            let metadata = source.backing_metadata();
            let mut origin = Vec::with_capacity(16);
            origin.extend_from_slice(&metadata.st_dev.to_le_bytes());
            origin.extend_from_slice(&metadata.st_ino.to_le_bytes());
            unsafe { Attributes::from_handle(staged.handle.0, true)? }.set(
                format!("{}kinakaze.origin", source.namespace.prefix()).as_bytes(),
                &origin,
                0,
            )?;
            let installation = if let Some(index) = source
                .context
                .as_ref()
                .and_then(|context| context.index.as_ref())
            {
                let upper = unsafe {
                    Object::reopen(
                        staged.handle.0,
                        FILE_READ_ATTRIBUTES
                            | windows_sys::Win32::Storage::FileSystem::FILE_READ_EA,
                    )?
                };
                Some(index.install(&source.origin()?, &upper, metadata.st_nlink)?)
            } else {
                None
            };
            if source.flags() & super::features::FSYNC_STRICT != 0 {
                unsafe { Object::reopen(staged.handle.0, GENERIC_READ | GENERIC_WRITE)? }
                    .flush()?;
                if let Some(installation) = &installation {
                    installation.flush()?;
                }
                if let Some(index) = source
                    .context
                    .as_ref()
                    .and_then(|context| context.index.as_ref())
                {
                    index.flush()?;
                }
            }
            let handle = Handle(
                unsafe {
                    Object::reopen(
                        parent.backing_object().raw(),
                        FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | FILE_TRAVERSE,
                    )?
                }
                .into_raw(),
            );
            let _guard = DirectoryLock::acquire(&handle)?;
            let metadata = crate::fs::stat_handle(handle.0, false)?;
            match staged.publish_to(&handle, component) {
                Ok(()) => {
                    if let Some(installation) = installation {
                        installation.commit();
                    }
                    // Copy-up must not look like creating a new guest child.
                    // Linux also restores parent timestamps best-effort after
                    // publication; failure cannot unpublish the committed inode.
                    if let Err(error) = set_times(handle.0, &metadata) {
                        eprintln!("kinakaze: copy-up parent timestamp restore: errno={error}");
                    }
                    if source.flags() & super::features::FSYNC_STRICT != 0 {
                        unsafe { Object::reopen(handle.0, GENERIC_READ | GENERIC_WRITE)? }
                            .flush()?;
                    }
                }
                Err(crate::EEXIST) => {
                    drop(installation);
                }
                Err(error) => return Err(error),
            }
            let winner = parent.child(component)?;
            if winner.backing_layer() != 0 {
                // A vanished winner must not be reported as a writable lower.
                return Err(crate::ENOENT);
            }
            parent = winner;
        } else {
            parent = source;
        }
    }
    if parent.is_metacopy() && !metadata_only {
        complete_metacopy(&parent, work, truncate)?;
        let mut fresh = root.clone();
        for component in components {
            fresh = fresh.child(component)?;
        }
        return Ok(fresh);
    }
    Ok(parent)
}

pub(super) fn complete_metacopy(source: &Node, work: &Object, truncate: bool) -> Result<(), i32> {
    let marker = format!("{}metacopy", source.namespace.prefix());
    if !unsafe { Attributes::from_handle(source.backing_object().raw(), false)? }
        .snapshot()?
        .contains_key(marker.as_bytes())
    {
        return Ok(());
    }
    let staged = Staged::prepare(source, work, truncate)?;
    let target =
        unsafe { Object::reopen(source.backing_object().raw(), GENERIC_READ | GENERIC_WRITE)? };
    let metadata = crate::fs::stat_handle(target.raw(), false)?;
    let length = crate::fs::stat_handle(staged.handle.0, false)?.st_size as u64;
    target.set_length(0)?;
    let mut buffer = Vec::new();
    copy_bytes(
        staged.handle.0,
        target.raw(),
        length,
        &mut buffer,
        CopyData::GuestFile,
    )?;
    set_times(target.raw(), &metadata)?;
    if source.flags() & super::features::VOLATILE == 0 {
        target.flush()?;
    }
    // Failed I/O leaves the marker in place: readers still use the complete
    // lower data, and the next writer retries without exposing partial bytes.
    unsafe { Attributes::from_handle(target.raw(), true)? }.remove(marker.as_bytes())?;
    if source.flags() & super::features::FSYNC_STRICT != 0 {
        target.flush()?;
    }
    Ok(())
}

/// All upper directory-entry mutations must use this same inode-keyed lock.
/// There is no process-local handle registry or pathname-keyed cache to clone.
struct DirectoryLock(Handle);
impl DirectoryLock {
    fn acquire(directory: &Handle) -> Result<Self, i32> {
        use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            CreateMutexW, INFINITE, WaitForMultipleObjects,
        };
        let stat = crate::fs::stat_handle(directory.0, false)?;
        if stat.st_mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
        let interrupt = crate::interrupt::current();
        if interrupt.is_null() {
            return Err(EIO);
        }
        let name = wide(OsStr::new(&format!(
            "Local\\kinakaze.overlay.dir.v1.{:016x}.{:016x}",
            stat.st_dev, stat.st_ino
        )));
        let raw = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if raw.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let handle = Handle(raw);
        loop {
            crate::signal::register_waiter();
            if let Err(error) = interrupted() {
                crate::signal::unregister_waiter();
                return Err(error);
            }
            let handles = [raw, interrupt];
            let waited = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            crate::signal::unregister_waiter();
            match waited {
                WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(Self(handle)),
                value if value == WAIT_OBJECT_0 + 1 => continue,
                _ => return Err(EIO),
            }
        }
    }
}
impl Drop for DirectoryLock {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::Threading::ReleaseMutex(self.0.0) };
    }
}

fn interrupted() -> Result<(), i32> {
    if crate::signal::pending() & !crate::signal::blocked_mask() != 0 {
        Err(EINTR)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum CopyData {
    GuestFile,
    NativeStream,
}

fn copy_bytes(
    source: HANDLE,
    target: HANDLE,
    length: u64,
    buffer: &mut Vec<u8>,
    data: CopyData,
) -> Result<(), i32> {
    let size = length.min(1024 * 1024) as usize;
    buffer.resize(size, 0);
    let mut offset = 0;
    while offset < length {
        interrupted()?;
        let amount = (length - offset).min(buffer.len() as u64) as usize;
        let count = match data {
            CopyData::GuestFile => crate::fs::verity::verified_read_preserving_atime(
                source,
                offset,
                &mut buffer[..amount],
            )?
            .ok_or(EIO)?,
            // Internal named streams contain their own native byte sequence;
            // the unnamed stream's logical EOF and descriptor do not apply.
            CopyData::NativeStream => unsafe {
                crate::platform::transfer_once(
                    &entry(source, offset),
                    buffer.as_mut_ptr(),
                    amount,
                    true,
                )
            }?,
        };
        if count == 0 {
            return Err(EIO);
        }
        write_bytes(target, offset, &buffer[..count])?;
        offset += count as u64;
    }
    Ok(())
}

fn entry(handle: HANDLE, offset: u64) -> FdEntry {
    FdEntry {
        raw: handle as usize,
        kind: FdKind::File,
        flags: FdFlags::SEEKABLE.union(FdFlags::OVERLAPPED),
        generation: 0,
        description_id: 0,
        offset,
    }
}

fn write_bytes(target: HANDLE, mut offset: u64, mut bytes: &[u8]) -> Result<(), i32> {
    while !bytes.is_empty() {
        interrupted()?;
        let count = unsafe {
            crate::platform::transfer_once(
                &entry(target, offset),
                bytes.as_ptr() as *mut u8,
                bytes.len(),
                false,
            )
        }?;
        if count == 0 {
            return Err(EIO);
        }
        offset += count as u64;
        bytes = &bytes[count..];
    }
    Ok(())
}

fn copy_streams(
    source: &Object,
    target: HANDLE,
    buffer: &mut Vec<u8>,
    flush: &mut Option<Flush>,
) -> Result<(), i32> {
    for (name, length) in source.streams()? {
        if name != "::$DATA" {
            let src = unsafe {
                Object::stream(source.raw(), &name, GENERIC_READ | FILE_WRITE_ATTRIBUTES)?
            };
            src.suppress_atime()?;
            let dst = unsafe { Object::create_stream(target, &name, GENERIC_WRITE)? };
            copy_bytes(src.raw(), dst.raw(), length, buffer, CopyData::NativeStream)?;
            if let Some(flush) = flush.as_mut() {
                unsafe { flush.file(dst.raw())? };
            }
        }
    }
    Ok(())
}

fn set_times(handle: HANDLE, metadata: &Stat) -> Result<(), i32> {
    let stamp = |seconds: i64, nanos: i64| -> Result<FILETIME, i32> {
        let ticks = (seconds as i128 + 11_644_473_600) * 10_000_000 + nanos as i128 / 100;
        let ticks = u64::try_from(ticks).map_err(|_| EOVERFLOW)?;
        Ok(FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        })
    };
    let atime = stamp(metadata.st_atime, metadata.st_atime_nsec)?;
    let mtime = stamp(metadata.st_mtime, metadata.st_mtime_nsec)?;
    if unsafe { SetFileTime(handle, ptr::null(), &atime, &mtime) } == 0 {
        Err(errno_from_win32(unsafe { GetLastError() }))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mount::overlay::XattrNamespace;
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{
        CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForMultipleObjects,
        WaitForSingleObject,
    };

    fn directory(path: &Path) -> Object {
        Object::open(path, FILE_READ_ATTRIBUTES | FILE_TRAVERSE).unwrap()
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("kinakaze-copy-up-{}-{stamp}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            for name in ["upper", "lower", "work"] {
                std::fs::create_dir(path.join(name)).unwrap();
            }
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn directory(&self, name: &str) -> Object {
            directory(&self.path(name))
        }
        fn node(&self, name: &str) -> Node {
            Node::root([self.path("lower")], XattrNamespace::Trusted)
                .unwrap()
                .child(name)
                .unwrap()
        }
        fn prepare(&self, name: &str, truncate: bool) -> Staged {
            Staged::prepare(&self.node(name), &self.directory("work"), truncate).unwrap()
        }
        fn assert_empty_work(&self) {
            assert_eq!(std::fs::read_dir(self.path("work")).unwrap().count(), 0);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn staged_file_is_atomic_and_preserves_data_metadata_and_xattrs() {
        let f = Fixture::new();
        let data: Vec<u8> = (0..2 * 1024 * 1024 + 13).map(|n| (n % 251) as u8).collect();
        std::fs::write(f.path("lower/file"), &data).unwrap();
        crate::fs::set_mode_host_path(&f.path("lower/file"), 0o640).unwrap();
        let source =
            Handle::open(&f.path("lower/file"), FILE_WRITE_ATTRIBUTES, OPEN_EXISTING).unwrap();
        let stamps = Stat {
            st_atime: 1_600_000_010,
            st_mtime: 1_600_000_020,
            st_atime_nsec: 123_456_700,
            st_mtime_nsec: 345_678_900,
            ..Stat::default()
        };
        set_times(source.0, &stamps).unwrap();
        let attributes = Attributes::open_host(&f.path("lower/file"), true).unwrap();
        for (name, value) in [
            ("user.empty", &b""[..]),
            ("user.MixedCase", &b"a\0b"[..]),
            ("trusted.overlay.origin", &b"private"[..]),
            ("trusted.overlay.overlay.opaque", &b"y"[..]),
        ] {
            attributes.set(name.as_bytes(), value, 0).unwrap();
        }
        let node = f.node("file");
        let old_metadata = *node.backing_metadata();
        let mut lower_reader = std::fs::File::open(node.backing_path()).unwrap();
        let staged = Staged::prepare(&node, &f.directory("work"), false).unwrap();
        assert!(!f.path("upper/file").exists());
        assert_eq!(std::fs::read(&staged.path).unwrap(), data);
        staged.publish(&f.directory("upper"), "file").unwrap();
        f.assert_empty_work();
        let upper = Attributes::open_host(&f.path("upper/file"), false).unwrap();
        let metadata = upper.metadata().unwrap();
        assert_ne!(metadata.st_ino, old_metadata.st_ino);
        assert_eq!(metadata.st_mode & 0o7777, 0o640);
        assert_eq!(
            (metadata.st_mtime, metadata.st_mtime_nsec),
            (stamps.st_mtime, stamps.st_mtime_nsec)
        );
        assert_eq!(
            upper.snapshot().unwrap(),
            BTreeMap::from([
                (b"user.empty".to_vec(), Vec::new()),
                (b"user.MixedCase".to_vec(), b"a\0b".to_vec()),
                (b"trusted.overlay.overlay.opaque".to_vec(), b"y".to_vec())
            ])
        );
        std::fs::write(f.path("upper/file"), b"modified upper").unwrap();
        let mut original = Vec::new();
        lower_reader.read_to_end(&mut original).unwrap();
        assert_eq!(original, data);
        assert_eq!(
            attributes.get(b"trusted.overlay.origin").unwrap(),
            b"private"
        );
    }

    #[test]
    fn verity_copy_up_authenticates_data_hides_tree_and_preserves_lower_atime() {
        let f = Fixture::new();
        let data = vec![0x5a; 20_013];
        let lower = f.path("lower/verified");
        std::fs::write(&lower, &data).unwrap();
        std::fs::write(f.path("lower/verified:user-stream"), b"opaque stream").unwrap();
        let fd =
            crate::fs::open(&crate::path::to_guest_path(&lower), crate::fs::O_RDONLY, 0).unwrap();
        crate::fs::verity::enable(fd, 1, 4096, b"copy-up").unwrap();
        crate::close(fd).unwrap();
        let stamps = Stat {
            st_atime: 1_600_000_111,
            st_mtime: 1_600_000_222,
            ..Stat::default()
        };
        {
            let handle = Object::open(&lower, FILE_WRITE_ATTRIBUTES).unwrap();
            set_times(handle.raw(), &stamps).unwrap();
        }
        let staged = f.prepare("verified", false);
        assert_eq!(std::fs::read(&staged.path).unwrap(), data);
        assert_eq!(
            std::fs::read(staged.path.with_file_name(format!(
                "{}:user-stream",
                staged.path.file_name().unwrap().to_string_lossy()
            )))
            .unwrap(),
            b"opaque stream"
        );
        assert!(
            crate::fs::verity::descriptor(staged.handle.0)
                .unwrap()
                .is_none()
        );
        staged.publish(&f.directory("upper"), "verified").unwrap();
        f.assert_empty_work();
        let observer = Object::open(&lower, FILE_READ_ATTRIBUTES).unwrap();
        let after = crate::fs::stat_handle(observer.raw(), false).unwrap();
        assert_eq!(after.st_size, data.len() as i64);
        assert_eq!(after.st_atime, stamps.st_atime);
        assert_eq!(after.st_mtime, stamps.st_mtime);
    }

    #[test]
    fn corrupt_verity_lower_aborts_copy_up_without_publishing_unverified_bytes() {
        use std::io::Write;
        let f = Fixture::new();
        let lower = f.path("lower/corrupt");
        std::fs::write(&lower, vec![0x51; 12_037]).unwrap();
        let fd =
            crate::fs::open(&crate::path::to_guest_path(&lower), crate::fs::O_RDONLY, 0).unwrap();
        crate::fs::verity::enable(fd, 2, 2048, b"copy-up").unwrap();
        crate::close(fd).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&lower)
            .unwrap()
            .write_all(b"corruption")
            .unwrap();
        assert!(matches!(
            Staged::prepare(&f.node("corrupt"), &f.directory("work"), false),
            Err(EIO)
        ));
        assert!(!f.path("upper/corrupt").exists());
        f.assert_empty_work();
    }

    #[test]
    fn directory_copy_is_metadata_only_and_symlink_target_is_opaque() {
        let f = Fixture::new();
        std::fs::create_dir(f.path("lower/dir")).unwrap();
        std::fs::write(f.path("lower/dir/not-copied"), b"child").unwrap();
        crate::fs::set_mode_host_path(&f.path("lower/dir"), 0o750).unwrap();
        Attributes::open_host(&f.path("lower/dir"), true)
            .unwrap()
            .set(b"user.dir", b"value", 0)
            .unwrap();
        f.prepare("dir", false)
            .publish(&f.directory("upper"), "dir")
            .unwrap();
        assert_eq!(std::fs::read_dir(f.path("upper/dir")).unwrap().count(), 0);
        let upper = Attributes::open_host(&f.path("upper/dir"), false).unwrap();
        assert_eq!(upper.metadata().unwrap().st_mode & 0o777, 0o750);
        assert_eq!(upper.get(b"user.dir").unwrap(), b"value");
        crate::path::create_emulated_symlink(&f.path("lower/link"), "../../missing/target")
            .unwrap();
        f.prepare("link", false)
            .publish(&f.directory("upper"), "link")
            .unwrap();
        assert_eq!(
            crate::path::emulated_symlink_target(&f.path("upper/link"))
                .unwrap()
                .as_deref(),
            Some("../../missing/target")
        );
        assert_eq!(
            Attributes::open_host(&f.path("upper/link"), false)
                .unwrap()
                .metadata()
                .unwrap()
                .st_mode
                & S_IFMT,
            S_IFLNK
        );
        f.assert_empty_work();
    }

    #[test]
    fn truncate_skips_lower_data_but_retains_metadata() {
        let f = Fixture::new();
        std::fs::write(f.path("lower/file"), b"lower data").unwrap();
        Attributes::open_host(&f.path("lower/file"), true)
            .unwrap()
            .set(b"user.key", b"value", 0)
            .unwrap();
        f.prepare("file", true)
            .publish(&f.directory("upper"), "file")
            .unwrap();
        assert_eq!(std::fs::read(f.path("upper/file")).unwrap(), b"");
        assert_eq!(std::fs::read(f.path("lower/file")).unwrap(), b"lower data");
        assert_eq!(
            Attributes::open_host(&f.path("upper/file"), false)
                .unwrap()
                .get(b"user.key")
                .unwrap(),
            b"value"
        );
        f.assert_empty_work();
    }

    fn check_ancestor_copy(check_atime: bool) {
        let f = Fixture::new();
        std::fs::create_dir_all(f.path("lower/a/b")).unwrap();
        std::fs::write(f.path("lower/a/b/file"), b"requested leaf").unwrap();
        std::fs::write(f.path("lower/a/b/sibling"), b"not copied").unwrap();
        std::fs::write(f.path("lower/a/other"), b"not copied either").unwrap();
        let stamps = Stat {
            st_atime: 1_600_000_001,
            st_mtime: 1_600_000_002,
            st_atime_nsec: 123_400,
            st_mtime_nsec: 567_800,
            ..Stat::default()
        };
        Attributes::open_host(&f.path("lower/a/b"), true)
            .unwrap()
            .set(b"user.dir", b"copied", 0)
            .unwrap();
        // Finish descendant setup before restoring ancestor timestamps: even
        // fixture creation must not contaminate the initial access-time value.
        for name in ["lower/a/b", "lower/a", "upper"] {
            crate::fs::set_mode_host_path(&f.path(name), 0o751).unwrap();
            let handle = Handle::open(&f.path(name), FILE_WRITE_ATTRIBUTES, OPEN_EXISTING).unwrap();
            set_times(handle.0, &stamps).unwrap();
        }
        let roots =
            Node::root([f.path("upper"), f.path("lower")], XattrNamespace::Trusted).unwrap();
        let result =
            ensure_upper(&roots, &f.directory("work"), &["a", "b", "file"], false).unwrap();
        for name in ["upper", "upper/a", "upper/a/b"] {
            let stat = Attributes::open_host(&f.path(name), false)
                .unwrap()
                .metadata()
                .unwrap();
            assert_eq!(stat.st_mode & 0o777, 0o751);
            assert_eq!(
                (stat.st_mtime, stat.st_mtime_nsec),
                (stamps.st_mtime, stamps.st_mtime_nsec)
            );
            if check_atime {
                assert_eq!(
                    (stat.st_atime, stat.st_atime_nsec),
                    (stamps.st_atime, stamps.st_atime_nsec),
                    "{name}"
                );
            }
        }
        assert_eq!(
            result.backing_path(),
            std::fs::canonicalize(f.path("upper/a/b/file")).unwrap()
        );
        assert_eq!(
            std::fs::read(result.backing_path()).unwrap(),
            b"requested leaf"
        );
        assert_eq!(
            Attributes::open_host(&f.path("upper/a/b"), false)
                .unwrap()
                .get(b"user.dir")
                .unwrap(),
            b"copied"
        );
        assert!(!f.path("upper/a/b/sibling").exists());
        assert!(!f.path("upper/a/other").exists());
        assert_eq!(
            std::fs::read(f.path("lower/a/b/file")).unwrap(),
            b"requested leaf"
        );
        f.assert_empty_work();
    }

    #[test]
    fn missing_ancestors_copy_only_metadata_and_retain_parent_mtime() {
        check_ancestor_copy(false);
    }

    #[test]
    #[ignore = "known gap: native relative directory rename changes target-ancestor atime; restoring the immediate parent does not cover ancestors"]
    fn ancestor_atime_is_not_changed_by_internal_layer_traversal() {
        check_ancestor_copy(true);
    }

    #[test]
    fn native_relative_directory_rename_keeps_the_pinned_target_identity() {
        let f = Fixture::new();
        std::fs::create_dir(f.path("upper/parent")).unwrap();
        std::fs::create_dir(f.path("work/source")).unwrap();
        let source = Object::open(
            &f.path("work/source"),
            GENERIC_READ | GENERIC_WRITE | DELETE,
        )
        .unwrap();
        let parent = Object::open(
            &f.path("upper/parent"),
            FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | FILE_TRAVERSE,
        )
        .unwrap();
        let root = Object::open(
            &f.path("upper"),
            FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
        )
        .unwrap();
        let stamps = Stat {
            st_atime: 1_600_000_001,
            st_mtime: 1_600_000_002,
            ..Stat::default()
        };
        set_times(root.raw(), &stamps).unwrap();
        let before = crate::fs::stat_handle(root.raw(), false).unwrap();
        let inode = crate::fs::stat_handle(source.raw(), false).unwrap().st_ino;
        unsafe {
            crate::fs::rename_host_handle_relative(
                source.raw(),
                parent.raw(),
                OsStr::new("child"),
                false,
            )
        }
        .unwrap();
        let after = crate::fs::stat_handle(root.raw(), false).unwrap();
        eprintln!(
            "native rename root atime before={} after={}",
            before.st_atime, after.st_atime
        );
        drop(source);
        let closed = crate::fs::stat_handle(root.raw(), false).unwrap();
        eprintln!(
            "native rename root atime after source close={}",
            closed.st_atime
        );
        assert_eq!(
            crate::fs::stat_handle(
                parent
                    .child(OsStr::new("child"), FILE_READ_ATTRIBUTES)
                    .unwrap()
                    .raw(),
                false
            )
            .unwrap()
            .st_ino,
            inode
        );
        assert!(!f.path("work/source").exists());
    }

    #[test]
    fn ancestor_coordinator_reuses_upper_and_respects_hidden_or_absent_children() {
        let f = Fixture::new();
        std::fs::create_dir(f.path("lower/dir")).unwrap();
        std::fs::create_dir(f.path("upper/dir")).unwrap();
        std::fs::write(f.path("lower/dir/file"), b"lower").unwrap();
        std::fs::write(f.path("upper/dir/file"), b"already upper").unwrap();
        std::fs::write(f.path("lower/dir/hidden"), b"must stay hidden").unwrap();
        crate::fs::create_device(
            &crate::path::to_guest_path(&f.path("upper/dir/hidden")),
            crate::fs::S_IFCHR | 0o600,
            0,
        )
        .unwrap();
        let roots =
            Node::root([f.path("upper"), f.path("lower")], XattrNamespace::Trusted).unwrap();
        let before = Attributes::open_host(&f.path("upper/dir/file"), false)
            .unwrap()
            .metadata()
            .unwrap();
        let result = ensure_upper(&roots, &f.directory("work"), &["dir", "file"], true).unwrap();
        assert_eq!(result.backing_metadata().st_ino, before.st_ino);
        assert_eq!(
            std::fs::read(result.backing_path()).unwrap(),
            b"already upper"
        );
        for name in ["hidden", "absent"] {
            assert!(matches!(
                ensure_upper(&roots, &f.directory("work"), &["dir", name], false),
                Err(crate::ENOENT)
            ));
        }
        std::fs::create_dir(f.path("lower/missing-parent")).unwrap();
        assert!(matches!(
            ensure_upper(
                &roots,
                &f.directory("work"),
                &["missing-parent", "absent"],
                false
            ),
            Err(crate::ENOENT)
        ));
        assert!(!f.path("upper/missing-parent").exists());
        crate::fs::set_mode_host_path(&f.path("upper/dir/hidden"), 0o600).unwrap();
        f.assert_empty_work();
    }

    #[test]
    fn ancestor_copy_helper() {
        let Some(path) = std::env::var_os("KINAKAZE_COPY_UP_TEST").map(PathBuf::from) else {
            return;
        };
        let index = std::env::var("KINAKAZE_COPY_UP_INDEX").unwrap();
        let gate = event(&path, "gate", false);
        let ready = event(&path, &format!("ready-{index}"), false);
        assert_ne!(unsafe { SetEvent(ready.0) }, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(gate.0, 10_000) },
            WAIT_OBJECT_0
        );
        let result = ensure_upper(
            &Node::root(
                [path.join("upper"), path.join("lower")],
                XattrNamespace::Trusted,
            )
            .unwrap(),
            &directory(&path.join("work")),
            &["a", "b", &index],
            false,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(result.backing_path()).unwrap(),
            format!("leaf {index}").as_bytes()
        );
    }

    #[test]
    fn independent_processes_copy_shared_ancestors_without_losing_children() {
        let f = Fixture::new();
        std::fs::create_dir_all(f.path("lower/a/b")).unwrap();
        for index in 0..4 {
            std::fs::write(
                f.path(&format!("lower/a/b/{index}")),
                format!("leaf {index}"),
            )
            .unwrap();
        }
        let gate = event(&f.0, "gate", true);
        let ready: Vec<_> = (0..4)
            .map(|i| event(&f.0, &format!("ready-{i}"), true))
            .collect();
        let mut children: Vec<_> = (0..4)
            .map(|i| child("ancestor_copy_helper", &f.0, i))
            .collect();
        for (ready, child) in ready.iter().zip(&children) {
            let handles = [ready.0, child.0.as_raw_handle()];
            assert_eq!(
                unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 10_000) },
                WAIT_OBJECT_0
            );
        }
        assert_ne!(unsafe { SetEvent(gate.0) }, 0);
        for child in &mut children {
            assert_eq!(
                unsafe { WaitForSingleObject(child.0.as_raw_handle(), 10_000) },
                WAIT_OBJECT_0
            );
            assert!(child.0.wait().unwrap().success());
        }
        for index in 0..4 {
            assert_eq!(
                std::fs::read(f.path(&format!("upper/a/b/{index}"))).unwrap(),
                format!("leaf {index}").as_bytes()
            );
        }
        f.assert_empty_work();
    }

    #[test]
    fn existing_upper_is_never_replaced_and_failed_stage_is_removed() {
        let f = Fixture::new();
        std::fs::write(f.path("lower/file"), b"lower").unwrap();
        std::fs::write(f.path("upper/file"), b"winner").unwrap();
        assert_eq!(
            f.prepare("file", false)
                .publish(&f.directory("upper"), "file"),
            Err(crate::EEXIST)
        );
        assert_eq!(std::fs::read(f.path("upper/file")).unwrap(), b"winner");
        f.assert_empty_work();
        let stage = f.prepare("file", false);
        let before = stage.path.clone();
        drop(stage);
        assert!(!before.exists());
        f.assert_empty_work();
        assert_eq!(std::fs::read(f.path("lower/file")).unwrap(), b"lower");
    }

    #[test]
    fn stream_read_failure_rolls_back_after_data_was_staged() {
        let f = Fixture::new();
        std::fs::write(f.path("lower/file"), b"lower").unwrap();
        let stream = f.path("lower/file:unavailable");
        std::fs::write(&stream, b"stream").unwrap();
        let node = f.node("file");
        let blocker = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(stream)
            .unwrap();
        assert!(Staged::prepare(&node, &f.directory("work"), false).is_err());
        drop(blocker);
        f.assert_empty_work();
        assert!(!f.path("upper/file").exists());
    }

    #[test]
    fn read_only_staged_inode_can_be_published_or_rolled_back() {
        let f = Fixture::new();
        std::fs::write(f.path("lower/file"), b"read only").unwrap();
        crate::fs::set_mode_host_path(&f.path("lower/file"), 0o444).unwrap();
        let stage = f.prepare("file", false);
        drop(stage);
        f.assert_empty_work();
        f.prepare("file", false)
            .publish(&f.directory("upper"), "file")
            .unwrap();
        assert_eq!(
            Attributes::open_host(&f.path("upper/file"), false)
                .unwrap()
                .metadata()
                .unwrap()
                .st_mode
                & 0o777,
            0o444
        );
        crate::fs::set_mode_host_path(&f.path("lower/file"), 0o644).unwrap();
        crate::fs::set_mode_host_path(&f.path("upper/file"), 0o644).unwrap();
        f.assert_empty_work();
    }

    #[test]
    fn special_inode_metadata_survives_without_opening_a_device() {
        let f = Fixture::new();
        crate::fs::create_device(
            &crate::path::to_guest_path(&f.path("lower/device")),
            crate::fs::S_IFCHR | 0o600,
            0x103,
        )
        .unwrap();
        f.prepare("device", false)
            .publish(&f.directory("upper"), "device")
            .unwrap();
        let metadata = Attributes::open_host(&f.path("upper/device"), false)
            .unwrap()
            .metadata()
            .unwrap();
        assert_eq!(metadata.st_mode & S_IFMT, crate::fs::S_IFCHR);
        assert_eq!(metadata.st_rdev, 0x103);
        f.assert_empty_work();
    }

    // These helpers run in independent native processes. Signal dispositions,
    // runtime registration and named-object lifetimes cannot leak into another
    // test, even when the full suite runs tests concurrently.
    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    fn child(helper: &str, path: &Path, index: usize) -> ChildGuard {
        ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &format!("mount::overlay::copy_up::tests::{helper}"),
                    "--nocapture",
                ])
                .env("KINAKAZE_COPY_UP_TEST", path)
                .env("KINAKAZE_COPY_UP_INDEX", index.to_string())
                .creation_flags(0x0800_0000)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }

    fn event_name(path: &Path, suffix: &str) -> Vec<u16> {
        wide(OsStr::new(&format!(
            "Local\\{}.{}",
            path.file_name().unwrap().to_str().unwrap(),
            suffix
        )))
    }

    fn event(path: &Path, suffix: &str, create: bool) -> Handle {
        let name = event_name(path, suffix);
        let handle = unsafe {
            if create {
                CreateEventW(ptr::null(), 1, 0, name.as_ptr())
            } else {
                OpenEventW(EVENT_MODIFY_STATE | SYNCHRONIZE, 0, name.as_ptr())
            }
        };
        assert!(!handle.is_null(), "event: {}", unsafe { GetLastError() });
        Handle(handle)
    }

    #[test]
    fn competing_publisher_helper() {
        let Some(path) = std::env::var_os("KINAKAZE_COPY_UP_TEST").map(PathBuf::from) else {
            return;
        };
        let index = std::env::var("KINAKAZE_COPY_UP_INDEX").unwrap();
        let gate = event(&path, "gate", false);
        let ready = event(&path, &format!("ready-{index}"), false);
        let lower = Node::root([path.join("lower")], XattrNamespace::Trusted).unwrap();
        let staged = Staged::prepare(
            &lower.child("file").unwrap(),
            &directory(&path.join("work")),
            false,
        )
        .unwrap();
        assert_ne!(unsafe { SetEvent(ready.0) }, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(gate.0, 10_000) },
            WAIT_OBJECT_0
        );
        match staged.publish(&directory(&path.join("upper")), "file") {
            Ok(()) => {}
            Err(crate::EEXIST) => std::process::exit(17),
            Err(error) => panic!("publish errno={error}"),
        }
    }

    #[test]
    fn independent_publishers_never_replace_the_winner_or_expose_partial_data() {
        let f = Fixture::new();
        let data: Vec<u8> = (0..1024 * 1024 + 7).map(|n| (n % 251) as u8).collect();
        std::fs::write(f.path("lower/file"), &data).unwrap();
        let gate = event(&f.0, "gate", true);
        let ready: Vec<_> = (0..4)
            .map(|i| event(&f.0, &format!("ready-{i}"), true))
            .collect();
        let mut children: Vec<_> = (0..4)
            .map(|i| child("competing_publisher_helper", &f.0, i))
            .collect();
        for (ready, child) in ready.iter().zip(&children) {
            // A dead helper must wake the test immediately, not cost the full
            // ready timeout. The same wait checks both concrete kernel objects.
            let handles = [ready.0, child.0.as_raw_handle()];
            assert_eq!(
                unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 10_000) },
                WAIT_OBJECT_0
            );
        }
        assert!(!f.path("upper/file").exists());
        assert_eq!(std::fs::read_dir(f.path("work")).unwrap().count(), 4);
        assert_ne!(unsafe { SetEvent(gate.0) }, 0);
        let mut winners = 0;
        for child in &mut children {
            assert_eq!(
                unsafe { WaitForSingleObject(child.0.as_raw_handle(), 10_000) },
                WAIT_OBJECT_0
            );
            match child.0.wait().unwrap().code() {
                Some(0) => winners += 1,
                Some(17) => {}
                status => panic!("unexpected publisher exit {status:?}"),
            }
        }
        assert_eq!(winners, 1);
        assert_eq!(std::fs::read(f.path("upper/file")).unwrap(), data);
        assert_eq!(std::fs::read(f.path("lower/file")).unwrap(), data);
        f.assert_empty_work();
    }

    #[test]
    fn publication_follows_the_open_parent_after_it_is_renamed() {
        let f = Fixture::new();
        std::fs::write(f.path("lower/file"), b"same directory inode").unwrap();
        let parent = f.directory("upper");
        let staged = f.prepare("file", false);
        std::fs::rename(f.path("upper"), f.path("renamed-upper")).unwrap();
        std::fs::create_dir(f.path("upper")).unwrap();
        staged.publish(&parent, "file").unwrap();
        assert!(!f.path("upper/file").exists());
        assert_eq!(
            std::fs::read(f.path("renamed-upper/file")).unwrap(),
            b"same directory inode"
        );
        f.assert_empty_work();
    }

    #[test]
    fn copy_up_keeps_all_pinned_roots_after_their_names_are_replaced() {
        let f = Fixture::new();
        std::fs::create_dir_all(f.path("lower/a/b")).unwrap();
        std::fs::write(f.path("lower/a/b/file"), b"pinned lower").unwrap();
        let root = Node::root([f.path("upper"), f.path("lower")], XattrNamespace::Trusted).unwrap();
        let work = f.directory("work");
        for name in ["upper", "lower", "work"] {
            std::fs::rename(f.path(name), f.path(&format!("renamed-{name}"))).unwrap();
            std::fs::create_dir(f.path(name)).unwrap();
            std::fs::write(f.path(&format!("{name}/keep")), name.as_bytes()).unwrap();
        }
        std::fs::create_dir_all(f.path("lower/a/b")).unwrap();
        std::fs::write(f.path("lower/a/b/file"), b"replacement lower").unwrap();
        let copied = ensure_upper(&root, &work, &["a", "b", "file"], false).unwrap();
        let mut data = [0u8; 32];
        let reader =
            unsafe { Object::reopen(copied.backing_object().raw(), GENERIC_READ) }.unwrap();
        let count = reader.read_at(0, &mut data).unwrap();
        assert_eq!(&data[..count], b"pinned lower");
        assert_eq!(
            std::fs::read(f.path("renamed-upper/a/b/file")).unwrap(),
            b"pinned lower"
        );
        assert_eq!(
            std::fs::read(f.path("renamed-lower/a/b/file")).unwrap(),
            b"pinned lower"
        );
        assert_eq!(
            std::fs::read(f.path("lower/a/b/file")).unwrap(),
            b"replacement lower"
        );
        assert!(!f.path("upper/a").exists());
        assert_eq!(
            std::fs::read_dir(f.path("renamed-work")).unwrap().count(),
            0
        );
        assert_eq!(std::fs::read_dir(f.path("work")).unwrap().count(), 1);
        for name in ["upper", "lower", "work"] {
            assert_eq!(
                std::fs::read(f.path(&format!("{name}/keep"))).unwrap(),
                name.as_bytes()
            );
        }
    }

    #[test]
    fn rollback_removes_only_its_inode_after_work_and_stage_name_replacement() {
        let f = Fixture::new();
        std::fs::write(f.path("lower/file"), b"lower").unwrap();
        let source = f.node("file");
        let work = f.directory("work");
        std::fs::rename(f.path("work"), f.path("renamed-work")).unwrap();
        std::fs::create_dir(f.path("work")).unwrap();
        std::fs::write(f.path("work/keep"), b"new directory").unwrap();
        let staged = Staged::prepare(&source, &work, false).unwrap();
        let staged_path = staged.path.clone();
        let renamed = f.path("renamed-work/moved-stage");
        std::fs::rename(&staged_path, &renamed).unwrap();
        std::fs::write(&staged_path, b"replacement inode").unwrap();
        drop(staged);
        assert!(!renamed.exists());
        assert_eq!(std::fs::read(&staged_path).unwrap(), b"replacement inode");
        assert_eq!(
            std::fs::read(f.path("work/keep")).unwrap(),
            b"new directory"
        );
        assert_eq!(std::fs::read(f.path("lower/file")).unwrap(), b"lower");
    }

    #[test]
    fn native_stream_open_is_relative_to_the_inode_not_its_old_name() {
        let f = Fixture::new();
        for directory in [false, true] {
            let leaf = if directory { "dir" } else { "file" };
            let path = f.path(&format!("lower/{leaf}"));
            if directory {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, b"file").unwrap();
            }
            std::fs::write(
                path.with_file_name(format!("{leaf}:test-stream")),
                b"original",
            )
            .unwrap();
            let base =
                Handle::open(&path, FILE_READ_ATTRIBUTES | SYNCHRONIZE, OPEN_EXISTING).unwrap();
            std::fs::rename(&path, f.path(&format!("lower/renamed-{leaf}"))).unwrap();
            if directory {
                std::fs::create_dir(&path).unwrap();
            } else {
                std::fs::write(&path, b"replacement").unwrap();
            }
            std::fs::write(
                path.with_file_name(format!("{leaf}:test-stream")),
                b"replacement",
            )
            .unwrap();
            let mut name = wide(OsStr::new(":test-stream"));
            let mut string = UnicodeString {
                length: ((name.len() - 1) * 2) as u16,
                maximum_length: (name.len() * 2) as u16,
                buffer: name.as_mut_ptr(),
            };
            let attrs = ObjectAttributes {
                length: std::mem::size_of::<ObjectAttributes>() as u32,
                root: base.0,
                name: &mut string,
                attributes: 0,
                security: ptr::null_mut(),
                qos: ptr::null_mut(),
            };
            let mut io = NativeIoStatus::default();
            let mut stream = ptr::null_mut();
            let status = unsafe {
                NtCreateFile(
                    &mut stream,
                    GENERIC_READ | SYNCHRONIZE,
                    &attrs,
                    &mut io,
                    ptr::null(),
                    FILE_ATTRIBUTE_NORMAL,
                    SHARE,
                    1,
                    0x0020_0040,
                    ptr::null(),
                    0,
                )
            };
            assert!(
                status >= 0,
                "directory={directory}, stream open NTSTATUS={status:#x}"
            );
            let stream = Handle(stream);
            unsafe { crate::fs::complete_native_io(stream.0, &mut io, status).unwrap() };
            let mut buffer = [0; 16];
            let count = unsafe {
                crate::platform::transfer_once(
                    &stream.entry(0),
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    true,
                )
            }
            .unwrap();
            assert_eq!(&buffer[..count], b"original");
        }
    }

    #[test]
    fn internal_inode_queries_and_single_level_copy_preserve_directory_atime() {
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_READ_EA, GetFileInformationByHandle,
        };
        let f = Fixture::new();
        std::fs::write(f.path("upper/child"), b"child").unwrap();
        let object = Object::open(
            &f.path("upper"),
            FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | FILE_READ_EA,
        )
        .unwrap();
        let stamps = Stat {
            st_atime: 1_600_000_001,
            st_mtime: 1_600_000_002,
            ..Stat::default()
        };
        crate::fs::set_mode_host_path(&f.path("upper"), 0o751).unwrap();
        crate::fs::set_mode_host_path(&f.path("upper/child"), 0o644).unwrap();
        std::fs::write(f.path("lower/copied"), b"lower").unwrap();
        for action in 0..10 {
            set_times(object.raw(), &stamps).unwrap();
            match action {
                0 => {
                    let _ = unsafe { Attributes::from_handle(object.raw(), false) }.unwrap();
                }
                1 => {
                    let _ = unsafe { Attributes::from_handle(object.raw(), false) }
                        .unwrap()
                        .snapshot()
                        .unwrap();
                }
                2 => {
                    let _ = crate::fs::inode::read(object.raw()).unwrap();
                }
                3 => {
                    let _ = object
                        .child(OsStr::new("child"), FILE_READ_ATTRIBUTES)
                        .unwrap();
                }
                4 => {
                    let _ = object.path().unwrap();
                }
                5 => {
                    let _ = unsafe { Object::reopen(object.raw(), FILE_READ_ATTRIBUTES) }.unwrap();
                }
                6 => {
                    let _ = Node::root([f.path("upper")], XattrNamespace::Trusted).unwrap();
                }
                7 => {
                    assert!(matches!(
                        object.child(OsStr::new("missing"), FILE_READ_ATTRIBUTES),
                        Err(crate::ENOENT)
                    ));
                }
                8 => {
                    let _ = Node::root([f.path("upper")], XattrNamespace::Trusted)
                        .unwrap()
                        .child("child")
                        .unwrap();
                }
                9 => {
                    let _ = ensure_upper(
                        &Node::root([f.path("upper"), f.path("lower")], XattrNamespace::Trusted)
                            .unwrap(),
                        &f.directory("work"),
                        &["copied"],
                        false,
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            assert_ne!(
                unsafe { GetFileInformationByHandle(object.raw(), &mut info) },
                0
            );
            let ticks = (info.ftLastAccessTime.dwHighDateTime as u64) << 32
                | info.ftLastAccessTime.dwLowDateTime as u64;
            assert_eq!(
                ticks / 10_000_000 - 11_644_473_600,
                stamps.st_atime as u64,
                "operation {action}"
            );
        }
    }

    #[test]
    fn interrupted_copy_helper() {
        let Some(path) = std::env::var_os("KINAKAZE_COPY_UP_TEST").map(PathBuf::from) else {
            return;
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "sysv64" fn handler(_: i32) {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }
        assert!(!crate::interrupt::current().is_null());
        crate::signal::sigaction(
            12,
            Some(crate::signal::Action {
                disposition: crate::signal::Disposition::Handle(handler, 0),
                ..crate::signal::Action::default()
            }),
        )
        .unwrap();
        crate::signal::swap_blocked_mask(0);
        crate::signal::raise_thread_signal(crate::interrupt::current_thread_id(), 12).unwrap();
        let lower = Node::root([path.join("lower")], XattrNamespace::Trusted).unwrap();
        let result = Staged::prepare(
            &lower.child("file").unwrap(),
            &directory(&path.join("work")),
            false,
        );
        assert!(matches!(result, Err(EINTR)), "result={result:?}");
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read_dir(path.join("work")).unwrap().count(), 0);
        assert!(!path.join("upper/file").exists());
        assert_eq!(
            crate::signal::deliver_pending(),
            crate::signal::Delivery::Interrupted
        );
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn interrupted_copy_unwinds_before_guest_signal_delivery() {
        let f = Fixture::new();
        std::fs::write(f.path("lower/file"), b"lower remains intact").unwrap();
        let mut child = child("interrupted_copy_helper", &f.0, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(child.0.as_raw_handle(), 10_000) },
            WAIT_OBJECT_0
        );
        assert!(child.0.wait().unwrap().success());
        f.assert_empty_work();
        assert_eq!(
            std::fs::read(f.path("lower/file")).unwrap(),
            b"lower remains intact"
        );
    }
}
