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
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_WRITE_EA,
    GetFileInformationByHandle,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForMultipleObjects, WaitForSingleObject,
};

use crate::{EBADF, EEXIST, EINVAL, EIO, ENOSPC, EOPNOTSUPP, errno_from_win32};
#[cfg(test)]
use crate::{FdFlags, FdKind};

pub const ENODATA: i32 = 61;
pub const XATTR_CREATE: i32 = 1;
pub const XATTR_REPLACE: i32 = 2;
pub const XATTR_SIZE_MAX: usize = 65_536;
const EA_NAME: &[u8] = b"KINAKAZE.LINUX.XATTRS";
const MAGIC: &[u8; 8] = b"CYXATTR1";
// An NT EA record, including its header, must fit in the filesystem EA buffer.
const MAX_RECORD_SIZE: usize = 65_535;
type Table = BTreeMap<Vec<u8>, Vec<u8>>;

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
/// The lock name derives only from native identity. Retain this key only while
/// an owned inode handle prevents file-id reuse; no mutable metadata is cached.
pub(crate) struct InodeKey(Vec<u16>);
/// A retained named mutex capability. The caller keeps its native inode alive
/// throughout this object's lifetime, preventing identity reuse. This object
/// stores no metadata and is rebuilt rather than inherited by fork caches.
pub(crate) struct InodeMutex(Handle);
// Windows mutex handles may be used concurrently by multiple native threads.
// Ownership belongs to an acquiring thread, not to the handle itself.
unsafe impl Send for InodeMutex {}
unsafe impl Sync for InodeMutex {}

pub(crate) struct InodeMutexGuard<'a> {
    mutex: &'a InodeMutex,
    // ReleaseMutex must execute on the same native thread as acquisition.
    _thread: std::marker::PhantomData<*mut ()>,
}
impl InodeMutex {
    pub(crate) fn open(key: &InodeKey) -> Result<Self, i32> {
        Handle::new(unsafe { CreateMutexW(ptr::null(), 0, key.0.as_ptr()) }).map(Self)
    }
    pub(crate) fn acquire(&self) -> Result<InodeMutexGuard<'_>, i32> {
        acquire_mutex(self.0.0)?;
        Ok(InodeMutexGuard {
            mutex: self,
            _thread: std::marker::PhantomData,
        })
    }
}
impl Drop for InodeMutexGuard<'_> {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.mutex.0.0) };
    }
}
impl InodeKey {
    pub(crate) fn from_information(info: &BY_HANDLE_FILE_INFORMATION) -> Self {
        Self(
            format!(
                r"Local\kinakaze.xattr.v1.{:08x}.{:08x}{:08x}",
                info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow,
            )
            .encode_utf16()
            .chain(Some(0))
            .collect(),
        )
    }
    pub(crate) fn from_handle(file: HANDLE) -> Result<Self, i32> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file, &mut info) } == 0 {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        Ok(Self::from_information(&info))
    }
    pub(crate) fn acquire(&self) -> Result<InodeLock, i32> {
        InodeLock::acquire_name(&self.0)
    }
}
impl InodeLock {
    pub(crate) fn acquire(file: HANDLE) -> Result<Self, i32> {
        InodeKey::from_handle(file)?.acquire()
    }
    fn acquire_name(name: &[u16]) -> Result<Self, i32> {
        // A newly created mutex can be owned atomically by its creator. Most
        // inode transactions are uncontended and close the last handle on
        // return, so avoid a separate kernel wait for that common case.
        // Existing objects ignore initial ownership: retain the recursive,
        // abandoned-owner and interruptible contention paths below.
        let mutex = Handle::new(unsafe { CreateMutexW(ptr::null(), 1, name.as_ptr()) })?;
        if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            return Ok(Self(mutex));
        }
        acquire_mutex(mutex.0)?;
        Ok(Self(mutex))
    }
}

fn acquire_mutex(mutex: HANDLE) -> Result<(), i32> {
    match unsafe { WaitForSingleObject(mutex, 0) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(()),
        WAIT_TIMEOUT => {}
        _ => return Err(EIO),
    }
    let interrupt = crate::interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    let handles = [mutex, interrupt];
    loop {
        crate::signal::register_waiter();
        if crate::signal::interrupt_pending() {
            crate::signal::unregister_waiter();
            return Err(crate::EINTR);
        }
        let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        crate::signal::unregister_waiter();
        match result {
            WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(()),
            value if value == WAIT_OBJECT_0 + 1 => {} // stale or blocked signal: recheck
            _ => return Err(EIO),
        }
    }
}

#[cfg(test)]
mod retained_mutex_tests;
impl Drop for InodeLock {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.0) };
    }
}

/// An independently opened metadata handle; never changes a guest fd's offset.
pub struct Attributes(AttributeHandle);
enum AttributeHandle {
    Native(Handle),
    Mounted(crate::mount::overlay::MetadataHandle),
}
impl Attributes {
    fn raw(&self) -> HANDLE {
        use std::os::windows::io::AsRawHandle;
        match &self.0 {
            AttributeHandle::Native(handle) => handle.0,
            AttributeHandle::Mounted(handle) => handle.as_raw_handle(),
        }
    }
    fn access(write: bool) -> u32 {
        FILE_READ_ATTRIBUTES | FILE_READ_EA | if write { FILE_WRITE_EA } else { 0 }
    }

    /// `path` has already been resolved with the desired final-symlink rule.
    pub fn open_host(path: &Path, write: bool) -> Result<Self, i32> {
        let object = crate::fs::object::Object::open(path, Self::access(write))?;
        Ok(Self(AttributeHandle::Native(Handle(object.into_raw()))))
    }

    pub fn from_fd(fd: i32, write: bool) -> Result<Self, i32> {
        Self::from_descriptor(fd, write, false)
    }

    fn from_descriptor(fd: i32, write: bool, allow_path: bool) -> Result<Self, i32> {
        let handle =
            crate::mount::overlay::metadata_handle(fd, write, allow_path)?.ok_or(EOPNOTSUPP)?;
        Ok(Self(AttributeHandle::Mounted(handle)))
    }

    /// Follow procfs fd magic links to the pinned inode, not its display name.
    /// No guest descriptor is allocated, so metadata lookup works at RLIMIT_NOFILE.
    pub fn from_fd_link(path: &str, follow: bool, write: bool) -> Result<Option<Self>, i32> {
        if crate::tmpfs::owns(path) {
            return Ok(None);
        }
        let Some((pid, fd)) = crate::procfs::fd_magic_link(path) else {
            return Ok(None);
        };
        let (canonical, _scope) = crate::procfs::instance::enter(path)?;
        crate::procfs::metadata(&canonical)?;
        if !follow {
            return Err(EOPNOTSUPP);
        }
        if pid != crate::job::process_id() {
            return Err(EOPNOTSUPP);
        }
        Self::from_descriptor(fd, write, true)
            .map(Some)
            .map_err(|error| if error == EBADF { crate::ENOENT } else { error })
    }

    pub fn is_overlay(&self) -> bool {
        matches!(&self.0, AttributeHandle::Mounted(handle) if handle.is_overlay())
    }

    /// An independent metadata open of the same inode, never its current name.
    /// # Safety
    /// The borrowed handle must remain live throughout the native reopen.
    pub(crate) unsafe fn from_handle(handle: HANDLE, write: bool) -> Result<Self, i32> {
        let object = crate::fs::object::Object::reopen(handle, Self::access(write))?;
        Ok(Self(AttributeHandle::Native(Handle(object.into_raw()))))
    }

    /// Query the same inode as the EA handle, including emulated symlink type.
    pub fn metadata(&self) -> Result<crate::fs::Stat, i32> {
        crate::fs::stat_handle(self.raw(), false)
    }

    fn read_table(&self) -> Result<Table, i32> {
        Self::read_private_table(self.raw())
    }

    fn read_private_table(handle: HANDLE) -> Result<Table, i32> {
        match crate::fs::ea::read_private(handle, EA_NAME)? {
            Some(bytes) => decode(&bytes),
            None => Ok(Table::new()),
        }
    }

    fn write_table(&self, table: &Table) -> Result<(), i32> {
        crate::fs::ea::write_private(self.raw(), EA_NAME, &encode(table)?)
    }

    /// The caller already holds the inode mutex and owns this metadata open.
    /// Keep chown's privilege removal in the same transaction without another
    /// native reopen, recursive mutex acquisition, or maximum-size EA buffer.
    pub(crate) fn remove_private_locked(
        object: &crate::fs::object::Object,
        name: &[u8],
    ) -> Result<(), i32> {
        validate_name(name)?;
        let mut table = Self::read_private_table(object.raw())?;
        table.remove(name).ok_or(ENODATA)?;
        crate::fs::ea::write_private(object.raw(), EA_NAME, &encode(&table)?)
    }

    pub fn get(&self, name: &[u8]) -> Result<Vec<u8>, i32> {
        validate_name(name)?;
        let _lock = InodeLock::acquire(self.raw())?;
        self.read_table()?.remove(name).ok_or(ENODATA)
    }

    pub fn list(&self) -> Result<Vec<Vec<u8>>, i32> {
        let _lock = InodeLock::acquire(self.raw())?;
        Ok(self.read_table()?.into_keys().collect())
    }

    /// Read a coherent set of attributes with one EA query and one inode lock.
    /// Internal filesystem consumers must not assemble related metadata from
    /// repeated independent getxattr calls that can observe different updates.
    pub(crate) fn snapshot(&self) -> Result<BTreeMap<Vec<u8>, Vec<u8>>, i32> {
        let _lock = InodeLock::acquire(self.raw())?;
        self.read_table()
    }

    /// Replace the complete attribute set of a newly staged, unpublished inode.
    pub(crate) fn replace_snapshot(
        &self,
        attributes: &BTreeMap<Vec<u8>, Vec<u8>>,
    ) -> Result<(), i32> {
        let _lock = InodeLock::acquire(self.raw())?;
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
        let _lock = InodeLock::acquire(self.raw())?;
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
        let _lock = InodeLock::acquire(self.raw())?;
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
    fn nested_inode_locks_keep_exclusion_until_the_outer_guard_is_released() {
        let f = Fixture::new();
        let path = f.0.join("recursive-lock");
        std::fs::write(&path, b"data").unwrap();
        let attrs = Attributes::open_host(&path, false).unwrap();
        let outer = InodeLock::acquire(attrs.raw()).unwrap();
        let inner = InodeLock::acquire(attrs.raw()).unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (owned_tx, owned_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let attrs = Attributes::open_host(&path, false).unwrap();
            ready_tx.send(()).unwrap();
            let _lock = InodeLock::acquire(attrs.raw()).unwrap();
            owned_tx.send(()).unwrap();
        });
        ready_rx.recv().unwrap();
        drop(inner);
        let before_release = owned_rx.recv_timeout(std::time::Duration::from_millis(50));
        drop(outer);
        // Release before asserting so even a failure cannot strand the waiter.
        let after_release = owned_rx.recv_timeout(std::time::Duration::from_secs(5));
        waiter.join().unwrap();
        assert_eq!(
            before_release,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        assert_eq!(after_release, Ok(()));
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
            let _lock = InodeLock::acquire(attrs.raw()).unwrap();
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
        let interrupted = matches!(InodeLock::acquire(attrs.raw()), Err(crate::EINTR));
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 0);
        let delivery = crate::signal::deliver_pending();
        crate::signal::sigaction(12, Some(old)).unwrap();
        crate::signal::swap_blocked_mask(mask);
        assert!(interrupted);
        assert_eq!(delivery, crate::signal::Delivery::Interrupted);
        assert_eq!(CALLS.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(InodeLock::acquire(attrs.raw()).is_ok());
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
        let _signals = crate::signal::test_lock();
        let old_action = crate::signal::sigaction(
            crate::signal::SIGCHLD,
            Some(crate::signal::Action::default()),
        )
        .unwrap();
        crate::signal::raise_signal(crate::signal::SIGCHLD).unwrap();
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
        assert_eq!(
            crate::signal::take_pending(1 << (crate::signal::SIGCHLD - 1)).map(|info| info.signal),
            Some(crate::signal::SIGCHLD)
        );
        crate::signal::sigaction(crate::signal::SIGCHLD, Some(old_action)).unwrap();
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
