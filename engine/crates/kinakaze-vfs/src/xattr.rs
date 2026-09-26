//! Persistent Linux extended attributes backed by one native filesystem EA.
//!
//! Windows EA names are case-insensitive and values cannot be empty (an empty
//! value deletes an EA). A versioned record stores the exact Linux byte names
//! and values, including empty values, inside a single native EA. This is inode
//! metadata, not a pathname sidecar: hard links, rename and open/unlinked files
//! retain the same attributes. Filesystems without EAs return EOPNOTSUPP.
//!
//! Updates are one NtSetEaFile operation. An inode-keyed kernel mutex serializes
//! read/modify/write across processes; no process-local registry needs copying
//! across fork. The underlying filesystem's EA space limit remains observable
//! as ENOSPC, just as different Linux filesystems have different xattr limits.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA,
    GetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::{EBADF, EEXIST, EINVAL, EIO, ENOSPC, EOPNOTSUPP, FdFlags, FdKind, errno_from_win32};

pub const ENODATA: i32 = 61;
pub const XATTR_CREATE: i32 = 1;
pub const XATTR_REPLACE: i32 = 2;
pub const XATTR_SIZE_MAX: usize = 65_536;
const EA_NAME: &[u8] = b"KINAKAZE.LINUX.XATTRS";
const MAGIC: &[u8; 8] = b"CYXATTR1";
// An NT EA record, including its header, must fit in the filesystem EA buffer.
const MAX_RECORD_SIZE: usize = 65_535;
type Table = BTreeMap<Vec<u8>, Vec<u8>>;

use crate::fs::NativeIoStatus as IoStatus;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryEaFile(
        file: HANDLE,
        io: *mut IoStatus,
        buffer: *mut c_void,
        length: u32,
        single: u8,
        names: *const c_void,
        names_length: u32,
        index: *const u32,
        restart: u8,
    ) -> i32;
    fn NtSetEaFile(file: HANDLE, io: *mut IoStatus, buffer: *const c_void, length: u32) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

fn nt_errno(status: i32) -> i32 {
    match status as u32 {
        0xc000_004f | 0xc000_00bb => EOPNOTSUPP, // EAS_NOT_SUPPORTED / NOT_SUPPORTED
        0xc000_0050 => ENOSPC,                   // EA_TOO_LARGE
        _ => errno_from_win32(unsafe { RtlNtStatusToDosError(status) }),
    }
}

struct Handle(HANDLE);
impl Handle {
    fn new(handle: HANDLE) -> Result<Self, i32> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(crate) struct InodeLock(Handle);
impl InodeLock {
    pub(crate) fn acquire(file: HANDLE) -> Result<Self, i32> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file, &mut info) } == 0 {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let name: Vec<u16> = format!(
            r"Local\kinakaze.xattr.v1.{:08x}.{:08x}{:08x}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow,
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let mutex = Handle::new(unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) })?;
        match unsafe { WaitForSingleObject(mutex.0, 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(Self(mutex)),
            WAIT_TIMEOUT => {}
            _ => return Err(EIO),
        }
        let interrupt = crate::interrupt::current();
        if interrupt.is_null() {
            return Err(EIO);
        }
        let handles = [mutex.0, interrupt];
        loop {
            crate::signal::register_waiter();
            if crate::signal::pending() & !crate::signal::blocked_mask() != 0 {
                crate::signal::unregister_waiter();
                return Err(crate::EINTR);
            }
            let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            crate::signal::unregister_waiter();
            match result {
                WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(Self(mutex)),
                value if value == WAIT_OBJECT_0 + 1 => {} // stale or blocked signal: recheck
                _ => return Err(EIO),
            }
        }
    }
}
impl Drop for InodeLock {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.0) };
    }
}

/// An independently opened metadata handle; never changes a guest fd's offset.
pub struct Attributes(Handle, Option<crate::mount::overlay::MetadataHandle>);
impl Attributes {
    fn access(write: bool) -> u32 {
        FILE_READ_ATTRIBUTES | FILE_READ_EA | if write { FILE_WRITE_EA } else { 0 }
    }

    /// `path` has already been resolved with the desired final-symlink rule.
    pub fn open_host(path: &Path, write: bool) -> Result<Self, i32> {
        let object = crate::fs::object::Object::open(path, Self::access(write))?;
        Ok(Self(Handle(object.into_raw()), None))
    }

    pub fn from_fd(fd: i32, write: bool) -> Result<Self, i32> {
        use std::os::windows::io::AsRawHandle;
        if let Some(handle) = crate::mount::overlay::metadata_handle(fd, write, false)? {
            let mut attributes = unsafe { Self::from_handle(handle.as_raw_handle(), write)? };
            attributes.1 = Some(handle);
            return Ok(attributes);
        }
        let entry = crate::get(fd)?;
        if entry.flags.contains(FdFlags::PATH_ONLY) {
            return Err(EBADF);
        }
        if !matches!(entry.kind, FdKind::File | FdKind::Directory) {
            return Err(EOPNOTSUPP);
        }
        unsafe { Self::from_handle(entry.raw as HANDLE, write) }
    }

    /// An independent metadata open of the same inode, never its current name.
    /// # Safety
    /// The borrowed handle must remain live throughout the native reopen.
    pub(crate) unsafe fn from_handle(handle: HANDLE, write: bool) -> Result<Self, i32> {
        let object = crate::fs::object::Object::reopen(handle, Self::access(write))?;
        Ok(Self(Handle(object.into_raw()), None))
    }

    /// Query the same inode as the EA handle, including emulated symlink type.
    pub fn metadata(&self) -> Result<crate::fs::Stat, i32> {
        crate::fs::stat_handle(self.0.0, false)
    }

    fn read_table(&self) -> Result<Table, i32> {
        // FILE_GET_EA_INFORMATION: next offset, name length, name + NUL.
        let mut names = vec![0u8; 5 + EA_NAME.len() + 1];
        names[4] = EA_NAME.len() as u8;
        names[5..5 + EA_NAME.len()].copy_from_slice(EA_NAME);
        let mut buffer = vec![0u8; 65_536];
        let mut io = IoStatus::default();
        let status = unsafe {
            NtQueryEaFile(
                self.0.0,
                &mut io,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                1,
                names.as_ptr().cast(),
                names.len() as u32,
                ptr::null(),
                1,
            )
        };
        let status = unsafe { crate::fs::complete_native_status(self.0.0, &mut io, status)? };
        if matches!(status as u32, 0xc000_0051 | 0xc000_0052 | 0x8000_0012) {
            return Ok(Table::new());
        }
        if status < 0 {
            return Err(nt_errno(status));
        }
        if status != 0 || io.information < 8 || io.information > buffer.len() {
            return Err(EIO);
        }
        let length = u16::from_le_bytes([buffer[6], buffer[7]]) as usize;
        let name_end = 8 + buffer[5] as usize;
        let end = name_end + 1 + length;
        if end > io.information || buffer.get(8..name_end) != Some(EA_NAME) || buffer[name_end] != 0
        {
            return Err(EIO);
        }
        // Querying an absent named EA can return a record with a zero value.
        if length == 0 {
            return Ok(Table::new());
        }
        decode(&buffer[name_end + 1..end])
    }

    fn write_table(&self, table: &Table) -> Result<(), i32> {
        let value = encode(table)?;
        let mut buffer = vec![0u8; 8 + EA_NAME.len() + 1 + value.len()];
        if buffer.len() > MAX_RECORD_SIZE {
            return Err(ENOSPC);
        }
        buffer[5] = EA_NAME.len() as u8;
        buffer[6..8].copy_from_slice(&(value.len() as u16).to_le_bytes());
        buffer[8..8 + EA_NAME.len()].copy_from_slice(EA_NAME);
        buffer[9 + EA_NAME.len()..].copy_from_slice(&value);
        let mut io = IoStatus::default();
        let status = unsafe {
            NtSetEaFile(
                self.0.0,
                &mut io,
                buffer.as_ptr().cast(),
                buffer.len() as u32,
            )
        };
        let status = unsafe { crate::fs::complete_native_status(self.0.0, &mut io, status)? };
        if status == 0 {
            Ok(())
        } else {
            Err(nt_errno(status))
        }
    }

    pub fn get(&self, name: &[u8]) -> Result<Vec<u8>, i32> {
        validate_name(name)?;
        let _lock = InodeLock::acquire(self.0.0)?;
        self.read_table()?.remove(name).ok_or(ENODATA)
    }

    pub fn list(&self) -> Result<Vec<Vec<u8>>, i32> {
        let _lock = InodeLock::acquire(self.0.0)?;
        Ok(self.read_table()?.into_keys().collect())
    }

    /// Read a coherent set of attributes with one EA query and one inode lock.
    /// Internal filesystem consumers must not assemble related metadata from
    /// repeated independent getxattr calls that can observe different updates.
    pub(crate) fn snapshot(&self) -> Result<BTreeMap<Vec<u8>, Vec<u8>>, i32> {
        let _lock = InodeLock::acquire(self.0.0)?;
        self.read_table()
    }

    /// Replace the complete attribute set of a newly staged, unpublished inode.
    pub(crate) fn replace_snapshot(
        &self,
        attributes: &BTreeMap<Vec<u8>, Vec<u8>>,
    ) -> Result<(), i32> {
        let _lock = InodeLock::acquire(self.0.0)?;
        self.write_table(attributes)
    }

    pub fn set(&self, name: &[u8], value: &[u8], flags: i32) -> Result<(), i32> {
        validate_name(name)?;
        if flags & !(XATTR_CREATE | XATTR_REPLACE) != 0 {
            return Err(EINVAL);
        }
        if value.len() > XATTR_SIZE_MAX {
            return Err(7);
        } // E2BIG
        let _lock = InodeLock::acquire(self.0.0)?;
        let mut table = self.read_table()?;
        let exists = table.contains_key(name);
        if flags & XATTR_CREATE != 0 && exists {
            return Err(EEXIST);
        }
        if flags & XATTR_REPLACE != 0 && !exists {
            return Err(ENODATA);
        }
        table.insert(name.to_vec(), value.to_vec());
        self.write_table(&table)
    }

    pub fn remove(&self, name: &[u8]) -> Result<(), i32> {
        validate_name(name)?;
        let _lock = InodeLock::acquire(self.0.0)?;
        let mut table = self.read_table()?;
        table.remove(name).ok_or(ENODATA)?;
        self.write_table(&table)
    }
}

fn validate_name(name: &[u8]) -> Result<(), i32> {
    if name.is_empty() || name.len() > 255 || name.contains(&0) {
        return Err(crate::ERANGE);
    }
    Ok(())
}

fn encode(table: &Table) -> Result<Vec<u8>, i32> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&(table.len() as u32).to_le_bytes());
    for (name, value) in table {
        validate_name(name)?;
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(value);
        if bytes.len() + 9 + EA_NAME.len() > MAX_RECORD_SIZE {
            return Err(ENOSPC);
        }
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<Table, i32> {
    if bytes.len() < 12 || bytes.get(..8) != Some(MAGIC) {
        return Err(EIO);
    }
    let count = u32::from_le_bytes(bytes[8..12].try_into().map_err(|_| EIO)?) as usize;
    let mut cursor = 12;
    let mut table = Table::new();
    for _ in 0..count {
        let header = bytes.get(cursor..cursor + 6).ok_or(EIO)?;
        let name_len = u16::from_le_bytes(header[..2].try_into().map_err(|_| EIO)?) as usize;
        let value_len = u32::from_le_bytes(header[2..6].try_into().map_err(|_| EIO)?) as usize;
        cursor += 6;
        let name = bytes.get(cursor..cursor + name_len).ok_or(EIO)?;
        validate_name(name).map_err(|_| EIO)?;
        cursor += name_len;
        let value = bytes.get(cursor..cursor + value_len).ok_or(EIO)?;
        cursor += value_len;
        if table.insert(name.to_vec(), value.to_vec()).is_some() {
            return Err(EIO);
        }
    }
    if cursor != bytes.len() {
        return Err(EIO);
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root =
                std::env::temp_dir().join(format!("kinakaze-xattr-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn contended_inode_lock_returns_eintr_before_dispatching_a_handler() {
        let _signals = crate::signal::test_lock();
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        unsafe extern "sysv64" fn handler(_: i32) {
            CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let f = Fixture::new();
        let path = f.0.join("locked");
        std::fs::write(&path, b"data").unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let other_path = path.clone();
        let holder = std::thread::spawn(move || {
            let attrs = Attributes::open_host(&other_path, false).unwrap();
            let _lock = InodeLock::acquire(attrs.0.0).unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        ready_rx.recv().unwrap();
        let attrs = Attributes::open_host(&path, false).unwrap();
        let old = crate::signal::sigaction(
            12,
            Some(crate::signal::Action {
                disposition: crate::signal::Disposition::Handle(handler, 0),
                ..crate::signal::Action::default()
            }),
        )
        .unwrap();
        let mask = crate::signal::swap_blocked_mask(0);
        CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(!crate::interrupt::current().is_null());
        crate::signal::raise_thread_signal(crate::interrupt::current_thread_id(), 12).unwrap();
        let interrupted = matches!(InodeLock::acquire(attrs.0.0), Err(crate::EINTR));
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
        let delivery = crate::signal::deliver_pending();
        crate::signal::sigaction(12, Some(old)).unwrap();
        crate::signal::swap_blocked_mask(mask);
        assert!(interrupted);
        assert_eq!(delivery, crate::signal::Delivery::Interrupted);
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(InodeLock::acquire(attrs.0.0).is_ok());
    }

    #[test]
    fn native_ea_preserves_case_empty_values_and_hardlink_identity() {
        let f = Fixture::new();
        let path = f.0.join("file");
        let alias = f.0.join("alias");
        std::fs::write(&path, b"unchanged payload").unwrap();
        let attrs = Attributes::open_host(&path, true).unwrap();
        assert_eq!(attrs.get(b"user.key"), Err(ENODATA));
        attrs.set(b"user.key", b"", XATTR_CREATE).unwrap();
        attrs
            .set(b"user.Key", b"binary\0data", XATTR_CREATE)
            .unwrap();
        assert_eq!(attrs.set(b"user.key", b"bad", XATTR_CREATE), Err(EEXIST));
        assert_eq!(
            attrs.set(b"user.absent", b"bad", XATTR_REPLACE),
            Err(ENODATA)
        );
        std::fs::hard_link(&path, &alias).unwrap();
        let second = Attributes::open_host(&alias, true).unwrap();
        assert_eq!(second.get(b"user.key").unwrap(), b"");
        assert_eq!(second.get(b"user.Key").unwrap(), b"binary\0data");
        second.remove(b"user.key").unwrap();
        assert_eq!(attrs.get(b"user.key"), Err(ENODATA));
        assert_eq!(attrs.list().unwrap(), vec![b"user.Key".to_vec()]);
        assert_eq!(std::fs::read(&path).unwrap(), b"unchanged payload");
        std::fs::rename(&alias, f.0.join("renamed")).unwrap();
        assert_eq!(second.get(b"user.Key").unwrap(), b"binary\0data");
    }

    #[test]
    fn concurrent_native_ea_updates_do_not_lose_other_names() {
        let f = Fixture::new();
        let path = f.0.join("file");
        std::fs::write(&path, b"").unwrap();
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let attrs = Attributes::open_host(&path, true).unwrap();
                    for iteration in 0..20u8 {
                        attrs
                            .set(format!("user.worker{worker}").as_bytes(), &[iteration], 0)
                            .unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let attrs = Attributes::open_host(&path, false).unwrap();
        assert_eq!(attrs.list().unwrap().len(), 8);
        for worker in 0..8 {
            assert_eq!(
                attrs
                    .get(format!("user.worker{worker}").as_bytes())
                    .unwrap(),
                [19]
            );
        }
    }

    #[test]
    fn corrupt_ea_records_are_errors_not_empty_attribute_lists() {
        assert_eq!(decode(b""), Err(EIO));
        let mut table = Table::new();
        table.insert(b"user.a".to_vec(), vec![0, 1, 255]);
        let encoded = encode(&table).unwrap();
        assert_eq!(decode(&encoded), Ok(table));
        for end in 0..encoded.len() {
            assert_eq!(decode(&encoded[..end]), Err(EIO));
        }
    }

    #[test]
    fn descriptor_attributes_survive_posix_unlink_and_name_replacement() {
        use std::os::windows::io::IntoRawHandle;
        let f = Fixture::new();
        let path = f.0.join("file");
        std::fs::write(&path, b"old inode").unwrap();
        let raw = std::fs::File::open(&path).unwrap().into_raw_handle();
        let fd = crate::install(raw as usize, FdKind::File, FdFlags::NONE).unwrap();
        Attributes::from_fd(fd, true)
            .unwrap()
            .set(b"user.a", b"old", 0)
            .unwrap();
        crate::fs::unlink(&crate::to_guest_path(&path)).unwrap();
        std::fs::write(&path, b"new inode").unwrap();
        let old = Attributes::from_fd(fd, true).unwrap();
        assert_eq!(old.get(b"user.a").unwrap(), b"old");
        old.set(b"user.b", b"after unlink", 0).unwrap();
        assert_eq!(
            Attributes::from_fd(fd, false)
                .unwrap()
                .get(b"user.b")
                .unwrap(),
            b"after unlink"
        );
        assert_eq!(
            Attributes::open_host(&path, false).unwrap().get(b"user.a"),
            Err(ENODATA)
        );
        crate::close(fd).unwrap();
    }

    #[test]
    fn native_copy_preserves_attributes_without_aliasing_source() {
        let f = Fixture::new();
        let source = f.0.join("source");
        let target = f.0.join("target");
        std::fs::write(&source, b"data").unwrap();
        let attrs = Attributes::open_host(&source, true).unwrap();
        attrs.set(b"user.copy", b"before", 0).unwrap();
        std::fs::copy(&source, &target).unwrap();
        let copy = Attributes::open_host(&target, true).unwrap();
        assert_eq!(copy.get(b"user.copy").unwrap(), b"before");
        copy.set(b"user.copy", b"after", 0).unwrap();
        assert_eq!(attrs.get(b"user.copy").unwrap(), b"before");
    }

    #[test]
    fn native_ea_child_process_writer() {
        let Some(path) = std::env::var_os("KINAKAZE_XATTR_TEST_FILE") else {
            return;
        };
        let worker = std::env::var("KINAKAZE_XATTR_TEST_WORKER").unwrap();
        let attrs = Attributes::open_host(Path::new(&path), true).unwrap();
        for iteration in 0..40u8 {
            attrs
                .set(format!("user.process{worker}").as_bytes(), &[iteration], 0)
                .unwrap();
        }
    }

    #[test]
    fn independent_processes_serialize_updates_on_the_same_inode() {
        let f = Fixture::new();
        let path = f.0.join("shared");
        std::fs::write(&path, b"").unwrap();
        let executable = std::env::current_exe().unwrap();
        let children: Vec<_> = (0..4)
            .map(|worker| {
                std::process::Command::new(&executable)
                    .args(["--exact", "xattr::tests::native_ea_child_process_writer"])
                    .env("KINAKAZE_XATTR_TEST_FILE", &path)
                    .env("KINAKAZE_XATTR_TEST_WORKER", worker.to_string())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let attrs = Attributes::open_host(&path, false).unwrap();
        assert_eq!(attrs.list().unwrap().len(), 4);
        for worker in 0..4 {
            assert_eq!(
                attrs
                    .get(format!("user.process{worker}").as_bytes())
                    .unwrap(),
                [39]
            );
        }
    }
}
