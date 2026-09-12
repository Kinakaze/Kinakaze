//! Kernel-domain mount metadata shared by independently hosted processes.
//!
//! Readers cache a decoded table against one atomic publication word. Writers
//! fill the inactive bank and publish only after it is complete. Abandoning the
//! native mutex cannot expose a partially updated table. The pagefile section
//! reserves its address range; only metadata pages actually used are committed.

use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEM_COMMIT, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, SEC_RESERVE, UnmapViewOfFile, VirtualAlloc,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, INFINITE, ReleaseMutex, WaitForSingleObject,
};

use super::MountPoint;
use crate::{EIO, ENOSPC, EOVERFLOW, errno_from_win32};

const MAGIC: u64 = u64::from_le_bytes(*b"CYMNT003");
const HEADER_SIZE: usize = 65536;
const BANK_SIZE: usize = 32 * 1024 * 1024;
const SECTION_SIZE: usize = HEADER_SIZE + BANK_SIZE * 2;
// Unix socket queue metadata is 80 bytes, or 176 with one ordinary credential.
// Keep small observations on the stack; large namespace tables still use one Vec.
const INLINE_SNAPSHOT_BYTES: usize = 192;

enum SnapshotBytes {
    Inline {
        bytes: [u8; INLINE_SNAPSHOT_BYTES],
        len: usize,
    },
    Heap(Vec<u8>),
}

impl SnapshotBytes {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Inline { bytes, len } => &bytes[..*len],
            Self::Heap(bytes) => bytes,
        }
    }

    fn into_vec(self) -> Vec<u8> {
        match self {
            Self::Inline { bytes, len } => bytes[..len].to_vec(),
            Self::Heap(bytes) => bytes,
        }
    }
}
static CURRENT: Mutex<Option<Arc<Store>>> = Mutex::new(None);
static LEADER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
thread_local! { static TASK: std::cell::RefCell<Option<Arc<Store>>> = const { std::cell::RefCell::new(None) }; }
pub(crate) fn inherit(store: Arc<Store>) {
    TASK.with(|slot| *slot.borrow_mut() = Some(store));
}
pub(crate) struct Scope(Option<Arc<Store>>);
impl Drop for Scope {
    fn drop(&mut self) {
        TASK.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}
/// Resolve one operation in a retained mount view without changing membership.
pub(crate) fn scoped(store: Arc<Store>) -> Scope {
    Scope(TASK.with(|slot| slot.borrow_mut().replace(store)))
}
pub(super) fn install(store: Arc<Store>) -> Result<(), i32> {
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let _ = LEADER.compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire);
    if LEADER.load(Ordering::Acquire) == tid {
        let mut current = CURRENT.lock().map_err(|_| EIO)?;
        if !kinakaze_runtime::job::set_mount_namespace(crate::job::process_id(), store.id()) {
            return Err(EIO);
        }
        *current = Some(store.clone());
    }
    inherit(store);
    Ok(())
}
static INITIAL: Mutex<Option<Arc<Store>>> = Mutex::new(None);

#[repr(C)]
struct Header {
    magic: AtomicU64,
    publication: AtomicU64,
    size: u64,
    domain: u64,
    namespace: u64,
    next_namespace: AtomicU64,
    owner: AtomicU64,
}

struct Handle(HANDLE);
impl Handle {
    fn new(raw: HANDLE) -> Result<Self, i32> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(errno_from_win32(unsafe { GetLastError() }))
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(crate) struct Store {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _section: Handle,
    mutex: Handle,
    // Section pages are never decommitted while this Store is alive. Remember
    // successful commits per view; a reopened Store starts conservatively at 0.
    committed_banks: [AtomicUsize; 2],
    pub(crate) cache: RwLock<Option<(u64, std::sync::Arc<Vec<MountPoint>>)>>,
    pub(crate) eventfd_events: std::sync::OnceLock<Result<crate::eventfd::ReadyEvents, i32>>,
}
// The view remains mapped while Store is live. Native mutations take the named
// mutex; cached Rust data uses its own RwLock and the publication is atomic.
unsafe impl Send for Store {}
unsafe impl Sync for Store {}

impl Drop for Store {
    fn drop(&mut self) {
        unsafe { UnmapViewOfFile(self.view) };
    }
}

struct Guard<'a>(&'a Store);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.mutex.0) };
    }
}

impl Store {
    fn acquire(&self) -> Result<Guard<'_>, i32> {
        match unsafe { WaitForSingleObject(self.mutex.0, INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Guard(self)),
            _ => Err(EIO),
        }
    }

    fn header(&self) -> &Header {
        unsafe { &*self.view.Value.cast::<Header>() }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.header().publication.load(Ordering::SeqCst)
    }

    fn bank(&self, revision: u64) -> *mut u8 {
        unsafe {
            self.view
                .Value
                .cast::<u8>()
                .add(HEADER_SIZE + (revision as usize & 1) * BANK_SIZE)
        }
    }

    fn commit_pages(&self, address: *mut u8, length: usize) -> Result<(), i32> {
        let bank = (0..2).find(|&index| self.bank(index as u64) == address);
        if let Some(index) = bank
            && self.committed_banks[index].load(Ordering::Acquire) >= length
        {
            return Ok(());
        }
        if unsafe { VirtualAlloc(address.cast(), length, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        if let Some(index) = bank {
            self.committed_banks[index].fetch_max(length, Ordering::Release);
        }
        Ok(())
    }

    /// Read shared words atomically: a second writer can recycle an old bank
    /// while a reader copies it. The publication check rejects that snapshot.
    ///
    /// Each word pairs Acquire with the writer's Release. If a reader sees
    /// any word from a reused bank, that writer's observation of the newer
    /// publication happens-before the reader's final publication load, which
    /// must then reject the old revision. A reader observing no reused words
    /// has the original complete snapshot. Relaxed payload accesses would
    /// lose that ordering; plain memcpy would also introduce a data race.
    fn copy_bank(&self, revision: u64) -> Result<SnapshotBytes, i32> {
        let bank = self.bank(revision);
        let len = unsafe { &*bank.cast::<AtomicU64>() }.load(Ordering::Acquire) as usize;
        if len > BANK_SIZE - 8 {
            return Err(EIO);
        }
        let mut bytes = if len <= INLINE_SNAPSHOT_BYTES {
            SnapshotBytes::Inline {
                bytes: [0; INLINE_SNAPSHOT_BYTES],
                len,
            }
        } else {
            SnapshotBytes::Heap(Vec::with_capacity(len))
        };
        let destination = match &mut bytes {
            SnapshotBytes::Inline { bytes, .. } => bytes.as_mut_ptr(),
            SnapshotBytes::Heap(bytes) => bytes.as_mut_ptr(),
        };
        let whole = len & !7;
        for offset in (0..whole).step_by(8) {
            let word =
                unsafe { &*bank.add(8 + offset).cast::<AtomicU64>() }.load(Ordering::Acquire);
            // SAFETY: capacity is at least len, and this complete word ends
            // within whole <= len. The destination is private stack/Vec
            // storage; the shared source is still read atomically.
            unsafe { ptr::write_unaligned(destination.add(offset).cast::<u64>(), word.to_le()) };
        }
        if whole != len {
            let word = unsafe { &*bank.add(8 + whole).cast::<AtomicU64>() }.load(Ordering::Acquire);
            // SAFETY: only the remaining 1..7 bytes are copied into capacity.
            unsafe {
                ptr::copy_nonoverlapping(
                    word.to_le_bytes().as_ptr(),
                    destination.add(whole),
                    len - whole,
                )
            };
        }
        // SAFETY: every byte below len was initialized by the loops above.
        if let SnapshotBytes::Heap(bytes) = &mut bytes {
            unsafe { bytes.set_len(len) };
        }
        Ok(bytes)
    }

    pub(crate) fn read(&self) -> Result<(u64, Vec<u8>), i32> {
        self.read_snapshot()
            .map(|(revision, bytes)| (revision, bytes.into_vec()))
    }

    /// Decode a stable owned snapshot without allocating a temporary small Vec.
    /// The callback runs once, only after publication validation, and may reenter.
    pub(crate) fn read_with<T>(
        &self,
        inspect: impl FnOnce(&[u8]) -> Result<T, i32>,
    ) -> Result<T, i32> {
        self.read_with_revision(inspect).map(|(_, value)| value)
    }

    /// Keep the version attached to the bytes that were decoded. A caller
    /// acquiring its mutation lock can reuse this value if publication has not
    /// changed, without copying and decoding the same ancillary records twice.
    pub(crate) fn read_with_revision<T>(
        &self,
        inspect: impl FnOnce(&[u8]) -> Result<T, i32>,
    ) -> Result<(u64, T), i32> {
        let (revision, bytes) = self.read_snapshot()?;
        inspect(bytes.as_slice()).map(|value| (revision, value))
    }

    fn read_snapshot(&self) -> Result<(u64, SnapshotBytes), i32> {
        loop {
            let revision = self.revision();
            let bytes = self.copy_bank(revision);
            // A fork child must not wait on a mutex held by a frozen parent
            // sibling. A stable publication gives a complete immutable value.
            if self.revision() == revision {
                return bytes.map(|bytes| (revision, bytes));
            }
            std::thread::yield_now();
        }
    }

    /// Inspect a committed value while excluding publishers. Resource owners
    /// use this boundary before retiring local references: an abandoned writer
    /// still exposes the last complete bank, never a half-published handle set.
    pub(crate) fn inspect_exclusive<T>(
        &self,
        try_only: bool,
        inspect: impl FnOnce(&[u8]) -> Result<T, i32>,
    ) -> Result<Option<T>, i32> {
        match unsafe { WaitForSingleObject(self.mutex.0, if try_only { 0 } else { INFINITE }) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => {
                let _guard = Guard(self);
                inspect(self.copy_bank(self.revision())?.as_slice()).map(Some)
            }
            windows_sys::Win32::Foundation::WAIT_TIMEOUT if try_only => Ok(None),
            _ => Err(EIO),
        }
    }

    pub(crate) fn update<T>(
        &self,
        action: impl FnOnce(&[u8]) -> Result<(Vec<u8>, T), i32>,
    ) -> Result<T, i32> {
        self.update_with_revision(action).map(|(_, result)| result)
    }

    /// Return the publication belonging to this result while still holding the
    /// publisher lock. Loading revision after update returns can mislabel an
    /// older decoded value with a concurrent writer's newer publication.
    pub(crate) fn update_with_revision<T>(
        &self,
        action: impl FnOnce(&[u8]) -> Result<(Vec<u8>, T), i32>,
    ) -> Result<(u64, T), i32> {
        self.update_then(action, |_| {})
    }

    /// Publish native readiness under the same mutex as the counter commit.
    /// The callback must not reenter this Store or call guest code.
    pub(crate) fn update_notified<T>(
        &self,
        action: impl FnOnce(&[u8]) -> Result<(Vec<u8>, T), i32>,
        notify: impl FnOnce(&[u8]),
    ) -> Result<T, i32> {
        self.update_then(action, notify).map(|(_, value)| value)
    }

    /// Publish already encoded bytes without allocating another owned buffer.
    pub(crate) fn replace(&self, bytes: &[u8]) -> Result<(), i32> {
        let guard = self.acquire()?;
        let revision = self.revision();
        // Replacement does not inspect the old value. Compare it in place
        // under the publisher lock, stopping at the first changed word instead
        // of copying (and for larger values allocating) a complete snapshot.
        if !self.bank_matches(&guard, revision, bytes)? {
            self.publish(&guard, revision, bytes)?;
        }
        Ok(())
    }

    fn bank_matches(&self, _guard: &Guard<'_>, revision: u64, bytes: &[u8]) -> Result<bool, i32> {
        let bank = self.bank(revision);
        let len = unsafe { &*bank.cast::<AtomicU64>() }.load(Ordering::Acquire) as usize;
        if len > BANK_SIZE - 8 {
            return Err(EIO);
        }
        if bytes.len() > BANK_SIZE - 8 {
            return Err(ENOSPC);
        }
        if len != bytes.len() {
            return Ok(false);
        }
        let mut chunks = bytes.chunks_exact(8);
        for (index, chunk) in chunks.by_ref().enumerate() {
            let word =
                unsafe { &*bank.add(8 + index * 8).cast::<AtomicU64>() }.load(Ordering::Acquire);
            if word != u64::from_le_bytes(chunk.try_into().unwrap()) {
                return Ok(false);
            }
        }
        let tail = chunks.remainder();
        if !tail.is_empty() {
            let word =
                unsafe { &*bank.add(8 + (len & !7)).cast::<AtomicU64>() }.load(Ordering::Acquire);
            if &word.to_le_bytes()[..tail.len()] != tail {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn update_then<T, B: AsRef<[u8]>>(
        &self,
        action: impl FnOnce(&[u8]) -> Result<(B, T), i32>,
        notify: impl FnOnce(&[u8]),
    ) -> Result<(u64, T), i32> {
        let guard = self.acquire()?;
        let revision = self.revision();
        let old = self.copy_bank(revision)?;
        let (bytes, result) = action(old.as_slice())?;
        let bytes = bytes.as_ref();
        if bytes.len() > BANK_SIZE - 8 {
            return Err(ENOSPC);
        }
        // Publication identifies content for decoded caches, not transaction
        // attempts. Do not commit/copy a bank or invalidate readers when an
        // action leaves the value unchanged (common in I/O bookkeeping).
        if bytes == old.as_slice() {
            notify(bytes);
            return Ok((revision, result));
        }
        let next = self.publish(&guard, revision, bytes)?;
        notify(bytes);
        Ok((next, result))
    }

    // The inactive bank and publication word remain one writer transaction.
    // Readers can still copy either bank concurrently: keep atomic word stores
    // and their release ordering, including when replacement skipped a copy.
    fn publish(&self, _guard: &Guard<'_>, revision: u64, bytes: &[u8]) -> Result<u64, i32> {
        let next = revision.checked_add(1).ok_or(EOVERFLOW)?;
        let bank = self.bank(next);
        self.commit_pages(bank, 8 + bytes.len().next_multiple_of(8))?;
        for (index, chunk) in bytes.chunks(8).enumerate() {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            unsafe { &*bank.add(8 + index * 8).cast::<AtomicU64>() }
                .store(u64::from_le_bytes(word), Ordering::Release);
        }
        unsafe { &*bank.cast::<AtomicU64>() }.store(bytes.len() as u64, Ordering::Release);
        self.header().publication.store(next, Ordering::SeqCst);
        Ok(next)
    }

    fn open(domain: u64) -> Result<Self, i32> {
        Self::open_namespace(domain, 1, true)
    }

    fn open_namespace(domain: u64, namespace: u64, create: bool) -> Result<Self, i32> {
        Self::open_named(domain, namespace, create, false)
    }

    fn open_named(domain: u64, namespace: u64, create: bool, object: bool) -> Result<Self, i32> {
        let wide = |suffix: &str| {
            let name = if object {
                format!(r"Local\kinakaze.mount-object.v1.{domain}.{namespace}.{suffix}")
            } else if namespace == 1 {
                format!(r"Local\kinakaze.mount.v2.{domain}.{suffix}")
            } else {
                format!(r"Local\kinakaze.mount.v2.{domain}.ns-{namespace}.{suffix}")
            };
            name.encode_utf16().chain(Some(0)).collect::<Vec<_>>()
        };
        let mutex = Handle::new(unsafe { CreateMutexW(ptr::null(), 0, wide("guard").as_ptr()) })?;
        let section = Handle::new(unsafe {
            if create {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_READWRITE | SEC_RESERVE,
                    0,
                    SECTION_SIZE as u32,
                    wide("state").as_ptr(),
                )
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide("state").as_ptr())
            }
        })?;
        let view = unsafe { MapViewOfFile(section.0, FILE_MAP_ALL_ACCESS, 0, 0, SECTION_SIZE) };
        if view.Value.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let store = Self {
            view,
            _section: section,
            mutex,
            committed_banks: [const { AtomicUsize::new(0) }; 2],
            cache: RwLock::new(None),
            eventfd_events: std::sync::OnceLock::new(),
        };
        store.commit_pages(store.view.Value.cast(), size_of::<Header>())?;
        if store.header().magic.load(Ordering::Acquire) == 0 {
            let _guard = store.acquire()?;
            let header = store.view.Value.cast::<Header>();
            if store.header().magic.load(Ordering::Acquire) == 0 {
                store.commit_pages(store.bank(0), 8)?;
                unsafe {
                    (&*store.bank(0).cast::<AtomicU64>()).store(0, Ordering::Relaxed);
                    ptr::addr_of_mut!((*header).size).write(SECTION_SIZE as u64);
                    ptr::addr_of_mut!((*header).domain).write(domain);
                    ptr::addr_of_mut!((*header).namespace).write(namespace);
                    (*header).next_namespace.store(2, Ordering::Relaxed);
                }
                store.header().magic.store(MAGIC, Ordering::Release);
            }
        }
        if store.header().magic.load(Ordering::Acquire) != MAGIC
            || store.header().size != SECTION_SIZE as u64
            || store.header().domain != domain
            || store.header().namespace != namespace
        {
            return Err(EIO);
        }
        Ok(store)
    }

    pub(crate) fn user_object(id: u64, create: bool) -> Result<Self, i32> {
        Self::open_named(kinakaze_runtime::authority::domain_id(), id, create, true)
    }

    pub(crate) fn id(&self) -> u64 {
        self.header().namespace
    }
    pub(crate) fn pin(&self) -> Result<crate::fs::object::Object, i32> {
        crate::fs::object::Object::duplicate(self._section.0)
    }
    pub(crate) fn owner(&self) -> u64 {
        self.header().owner.load(Ordering::Acquire).max(1)
    }

    pub(crate) fn descriptor(&self, flags: crate::FdFlags) -> Result<i32, i32> {
        self.descriptor_kind(crate::FdKind::MountNamespace, flags)
    }

    pub(crate) fn descriptor_kind(
        &self,
        kind: crate::FdKind,
        flags: crate::FdFlags,
    ) -> Result<i32, i32> {
        let mut raw = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self._section.0,
                GetCurrentProcess(),
                &mut raw,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        match crate::install(raw as usize, kind, flags) {
            Ok(fd) => Ok(fd),
            Err(error) => {
                unsafe { CloseHandle(raw) };
                Err(error)
            }
        }
    }
}

/// A configuration or detached tree is owned by its section descriptors, so
/// dup/fork/exec share its published state without copying a Rust side table.
pub(crate) fn new_object() -> Result<Store, i32> {
    let root = initial()?;
    let id = root
        .header()
        .next_namespace
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| {
            (id < u64::MAX - (1u64 << 32)).then_some(id + 1)
        })
        .map_err(|_| EOVERFLOW)?;
    Store::open_named(kinakaze_runtime::authority::domain_id(), id, true, true)
}

pub(crate) fn object_fd(fd: i32) -> Result<Store, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|entry| *entry)
        .ok_or(crate::EBADF)?;
    object_entry(entry)
}

/// The caller must hold the descriptor table lock, or own the raw handle.
pub(crate) fn object_entry(entry: crate::FdEntry) -> Result<Store, i32> {
    if !matches!(
        entry.kind,
        crate::FdKind::EventFd
            | crate::FdKind::FsContext
            | crate::FdKind::MountTree
            | crate::FdKind::Namespace
            | crate::FdKind::UserNamespace
            | crate::FdKind::TmpfsFile
            | crate::FdKind::TmpfsDirectory
            | crate::FdKind::MessageQueue
            | crate::FdKind::SysfsFile
    ) {
        return Err(crate::EINVAL);
    }
    let view =
        unsafe { MapViewOfFile(entry.raw as HANDLE, FILE_MAP_ALL_ACCESS, 0, 0, HEADER_SIZE) };
    if view.Value.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let header = unsafe { &*view.Value.cast::<Header>() };
    let result = if header.magic.load(Ordering::Acquire) == MAGIC
        && header.domain == kinakaze_runtime::authority::domain_id()
        && header.size == SECTION_SIZE as u64
        && header.namespace != 0
    {
        Store::open_named(header.domain, header.namespace, false, true)
    } else {
        Err(EIO)
    };
    unsafe { UnmapViewOfFile(view) };
    result
}

pub(crate) fn initial() -> Result<Arc<Store>, i32> {
    let mut slot = INITIAL.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        *slot = Some(Arc::new(Store::open(
            kinakaze_runtime::authority::domain_id(),
        )?));
    }
    Ok(Arc::clone(slot.as_ref().ok_or(EIO)?))
}

pub(crate) fn get() -> Result<Arc<Store>, i32> {
    if let Some(store) = TASK.with(|slot| slot.borrow().clone()) {
        return Ok(store);
    }
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let _ = LEADER.compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire);
    let mut slot = CURRENT.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        *slot = Some(initial()?);
    }
    let store = Arc::clone(slot.as_ref().ok_or(EIO)?);
    inherit(store.clone());
    Ok(store)
}

/// Copy before publication: a failed allocation never changes the caller.
pub(crate) fn prepare_unshare() -> Result<impl FnOnce() -> Result<(), i32>, i32> {
    prepare_owned(crate::user_namespace::id(crate::job::process_id())?)
}
pub(crate) fn prepare_owned(owner: u64) -> Result<impl FnOnce() -> Result<(), i32>, i32> {
    let _topology = topology_guard()?;
    let parent = get()?;
    let (_, bytes) = parent.read()?;
    let root = initial()?;
    let id = root
        .header()
        .next_namespace
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| {
            (id < u64::MAX - (1u64 << 32)).then_some(id + 1)
        })
        .map_err(|_| EOVERFLOW)?;
    let child = Arc::new(Store::open_namespace(
        kinakaze_runtime::authority::domain_id(),
        id,
        true,
    )?);
    child.update(|_| Ok((bytes, ())))?;
    child.header().owner.store(owner, Ordering::Release);
    Ok(move || {
        let _topology = _topology;
        install(child)
    })
}

/// Reopen an inherited membership already authenticated by the process table.
/// Exec/fork does not perform setns and needs no new CAP_SYS_ADMIN privilege.
pub(super) fn restore_membership(id: u64) -> Result<(), i32> {
    if id == 0 || kinakaze_runtime::job::mount_namespace(crate::job::process_id()) != Some(id) {
        return Err(crate::EPERM);
    }
    let _root = initial()?;
    let store = Arc::new(Store::open_namespace(
        kinakaze_runtime::authority::domain_id(),
        id,
        false,
    )?);
    install(store)
}

#[cfg(test)]
pub(crate) fn enter(id: u64) -> Result<(), i32> {
    install(prepare_enter(id)?)
}

pub(super) fn prepare_enter(id: u64) -> Result<Arc<Store>, i32> {
    if id == 0 {
        return Err(crate::EINVAL);
    }
    let _root = initial()?;
    let store = Arc::new(Store::open_namespace(
        kinakaze_runtime::authority::domain_id(),
        id,
        false,
    )?);
    if !crate::user_namespace::capable(store.owner(), 21) {
        return Err(crate::EPERM);
    }
    Ok(store)
}

pub(crate) fn for_process(pid: u32) -> Result<Arc<Store>, i32> {
    let id = kinakaze_runtime::job::mount_namespace(pid).ok_or(crate::ENOENT)?;
    let store = Arc::new(Store::open_namespace(
        kinakaze_runtime::authority::domain_id(),
        id,
        false,
    )?);
    Ok(store)
}
pub(crate) fn namespace(id: u64) -> Result<Store, i32> {
    Store::open_namespace(kinakaze_runtime::authority::domain_id(), id, false)
}

/// Serialize topology mutations across namespaces. Readers use each namespace's
/// atomic publication; they must not wait on a frozen parent's native mutex.
pub(crate) struct TopologyGuard(Handle);
impl Drop for TopologyGuard {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.0);
        }
    }
}
pub(crate) fn topology_guard() -> Result<TopologyGuard, i32> {
    let name = format!(
        r"Local\kinakaze.mount-topology.v1.{}",
        kinakaze_runtime::authority::domain_id()
    )
    .encode_utf16()
    .chain(Some(0))
    .collect::<Vec<_>>();
    let handle = Handle::new(unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) })?;
    match unsafe { WaitForSingleObject(handle.0, INFINITE) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(TopologyGuard(handle)),
        _ => Err(EIO),
    }
}
pub(crate) fn next_group() -> Result<u64, i32> {
    initial()?
        .header()
        .next_namespace
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
        .map_err(|_| EOVERFLOW)
}
pub(crate) fn live_namespaces() -> Result<Vec<Arc<Store>>, i32> {
    let root = initial()?;
    let end = root.header().next_namespace.load(Ordering::Acquire);
    let mut namespaces = vec![root];
    for id in 2..end {
        match namespace(id) {
            Ok(store) => namespaces.push(Arc::new(store)),
            Err(crate::ENOENT) => (),
            Err(error) => return Err(error),
        }
    }
    Ok(namespaces)
}
impl Store {
    /// Commit memory for both banks before publishing a multi-namespace event.
    pub(crate) fn reserve_update(&self, length: usize) -> Result<(), i32> {
        if length > BANK_SIZE - 8 {
            return Err(ENOSPC);
        }
        self.commit_pages(self.bank(0), 8 + length.next_multiple_of(8))?;
        self.commit_pages(self.bank(1), 8 + length.next_multiple_of(8))
    }
}

/// The fd owns a section reference, so its namespace survives the last member.
/// Pin under the fd-table lock to exclude close and numeric handle reuse.
pub(crate) fn descriptor_id(fd: i32) -> Result<u64, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|entry| *entry)
        .ok_or(crate::EBADF)?;
    if entry.kind != crate::FdKind::MountNamespace {
        return Err(crate::EINVAL);
    }
    let view =
        unsafe { MapViewOfFile(entry.raw as HANDLE, FILE_MAP_ALL_ACCESS, 0, 0, HEADER_SIZE) };
    if view.Value.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let header = unsafe { &*view.Value.cast::<Header>() };
    let result = if header.magic.load(Ordering::Acquire) == MAGIC
        && header.domain == kinakaze_runtime::authority::domain_id()
        && header.size == SECTION_SIZE as u64
        && header.namespace != 0
    {
        Ok(header.namespace)
    } else {
        Err(EIO)
    };
    unsafe { UnmapViewOfFile(view) };
    result
}

/// Registered VFS fork/exec restoration must discard parent-local addresses and
/// non-inherited native handles. Reopening the same section preserves the shared
/// mount table, rather than replacing it with the parent's serialized snapshot.
pub(crate) fn restore() {
    TASK.with(|slot| *slot.borrow_mut() = None);
    if let Ok(mut slot) = CURRENT.lock() {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    fn domain() -> u64 {
        0x8000_0000 | u64::from(std::process::id())
    }

    #[test]
    fn domain_high_bits_isolate_shared_namespaces() {
        let low = domain() ^ 0x0100_0000;
        let high = low | (1u64 << 48);
        let first = Store::open(low).unwrap();
        let second = Store::open(high).unwrap();
        first.update(|_| Ok((b"first".to_vec(), ()))).unwrap();
        assert!(second.read().unwrap().1.is_empty());
        second.update(|_| Ok((b"second".to_vec(), ()))).unwrap();
        assert_eq!(Store::open(low).unwrap().read().unwrap().1, b"first");
        assert_eq!(Store::open(high).unwrap().read().unwrap().1, b"second");
    }

    #[test]
    fn inline_snapshots_preserve_boundaries_and_can_reenter_without_locks() {
        let store = Store::open(domain() ^ 0x0200_0000).unwrap();
        for size in [0, 1, 7, 8, 79, 80, 127, 128, 129, 4097] {
            let value: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            store.update(|_| Ok((value.clone(), ()))).unwrap();
            let (_, snapshot) = store.read_snapshot().unwrap();
            assert_eq!(
                matches!(snapshot, SnapshotBytes::Inline { .. }),
                size <= INLINE_SNAPSHOT_BYTES
            );
            assert_eq!(snapshot.as_slice(), value);
            store
                .read_with(|bytes| {
                    assert_eq!(bytes, value);
                    store.update(|old| {
                        assert_eq!(old, bytes);
                        Ok((b"next".to_vec(), ()))
                    })?;
                    assert_eq!(
                        bytes, value,
                        "published changes cannot mutate an owned observation"
                    );
                    Ok(())
                })
                .unwrap();
            assert_eq!(store.read().unwrap().1, b"next");
        }
    }

    #[test]
    fn unchanged_transactions_preserve_publication_and_run_the_action() {
        let store = Store::open(domain() ^ 0x0800_0000).unwrap();
        store.update(|_| Ok((b"stable".to_vec(), ()))).unwrap();
        let revision = store.revision();
        let mut called = 0;
        let result = store
            .update(|old| {
                called += 1;
                Ok((old.to_vec(), 42))
            })
            .unwrap();
        assert_eq!((called, result), (1, 42));
        assert_eq!(store.read().unwrap(), (revision, b"stable".to_vec()));
        assert_eq!(
            store.update::<()>(|_| Err(crate::EACCES)),
            Err(crate::EACCES)
        );
        assert_eq!(store.revision(), revision);
        store.update(|_| Ok((b"changed".to_vec(), ()))).unwrap();
        assert_eq!(store.read().unwrap(), (revision + 1, b"changed".to_vec()));
        let (observed, result) = store
            .update_with_revision(|old| Ok((old.to_vec(), 42)))
            .unwrap();
        assert_eq!((observed, result), (revision + 1, 42));
        let (observed, result) = store
            .update_with_revision(|_| Ok((b"newer".to_vec(), 43)))
            .unwrap();
        assert_eq!((observed, result), (revision + 2, 43));
    }

    #[test]
    fn independent_views_grow_and_reuse_both_committed_banks() {
        let first = Store::open(domain() ^ 0x0400_0000).unwrap();
        let second = Store::open(domain() ^ 0x0400_0000).unwrap();
        for (index, size) in [80, 8193, 64, 65537, 4097, 1, 131073, 80]
            .into_iter()
            .enumerate()
        {
            let value = vec![index as u8 + 1; size];
            let (writer, reader) = if index % 2 == 0 {
                (&first, &second)
            } else {
                (&second, &first)
            };
            writer.replace(&value).unwrap();
            assert_eq!(reader.read().unwrap().1, value);
            let publication = writer.revision();
            writer.replace(&value).unwrap();
            assert_eq!(writer.revision(), publication);
        }
    }

    #[test]
    #[ignore = "explicit shared-metadata performance comparison"]
    fn shared_metadata_cost_probe() {
        use std::hint::black_box;
        use std::time::Instant;
        let store = Store::open(domain() ^ 0x4000_0000).unwrap();
        for size in [64, 80, 128, 129, 4096, 1024 * 1024] {
            store.update(|_| Ok((vec![7; size], ()))).unwrap();
            let iterations = (8 * 1024 * 1024 / size).clamp(128, 20_000);
            let modes = ["read", "read_with", "unchanged", "changed"];
            let mut timings = [[0.0f64; 7]; 4];
            for round in 0..8 {
                // Warm both paths, then rotate order between samples so clock
                // warmup or a publication immediately beforehand cannot always
                // favor the same API. This compares APIs in one binary, not a
                // historical-build or end-to-end socket throughput benchmark.
                for position in 0..modes.len() {
                    let mode_index = (round + position) % modes.len();
                    let mode = modes[mode_index];
                    let started = Instant::now();
                    for _ in 0..iterations {
                        if mode == "read" {
                            let (_, value) = store.read().unwrap();
                            assert_eq!(black_box(value).len(), size);
                        } else if mode == "read_with" {
                            store
                                .read_with(|bytes| {
                                    assert_eq!(black_box(bytes).len(), size);
                                    Ok(())
                                })
                                .unwrap();
                        } else {
                            store
                                .update(|old| {
                                    let mut next = old.to_vec();
                                    if mode == "changed" {
                                        next[0] = next[0].wrapping_add(1);
                                    }
                                    Ok((next, ()))
                                })
                                .unwrap();
                        }
                    }
                    if round != 0 {
                        timings[mode_index][round - 1] =
                            started.elapsed().as_nanos() as f64 / iterations as f64;
                    }
                }
            }
            for (mode, mut samples) in modes.into_iter().zip(timings) {
                samples.sort_by(f64::total_cmp);
                eprintln!(
                    "shared_metadata mode={mode} bytes={size} iterations={iterations} samples=7 ns_per_op_min={:.1} ns_per_op_median={:.1} ns_per_op_max={:.1}",
                    samples[0], samples[3], samples[6],
                );
            }
        }
    }

    #[test]
    fn replacement_preserves_byte_boundaries_and_content_revisions() {
        let store = Store::open(domain() ^ 0x4001_0000).unwrap();
        for size in (0..=17).chain([79, 80, 176, 192, 193, 4097, 65537]) {
            let mut value: Vec<_> = (0..size).map(|i| (i % 251) as u8).collect();
            store.replace(&value).unwrap();
            let revision = store.revision();
            store.replace(&value).unwrap();
            assert_eq!(store.read().unwrap(), (revision, value.clone()));
            if size != 0 {
                for index in [0, size / 2, size - 1] {
                    let revision = store.revision();
                    value[index] ^= 0x80;
                    store.replace(&value).unwrap();
                    assert_eq!(store.read().unwrap(), (revision + 1, value.clone()));
                    store.replace(&value).unwrap();
                    assert_eq!(store.revision(), revision + 1);
                }
            }
        }
    }

    #[test]
    fn replacement_errors_preserve_the_last_publication() {
        let store = Store::open(domain() ^ 0x4002_0000).unwrap();
        store.replace(b"first").unwrap();
        let before = store.read().unwrap();
        assert_eq!(store.replace(&vec![0; BANK_SIZE - 7]), Err(ENOSPC));
        assert_eq!(store.read().unwrap(), before);
        // Revision MAX selects bank 1, already populated by the first write.
        store.header().publication.store(u64::MAX, Ordering::SeqCst);
        store.replace(b"first").unwrap();
        assert_eq!(store.replace(b"changed"), Err(EOVERFLOW));
        assert_eq!(store.read().unwrap(), (u64::MAX, b"first".to_vec()));
    }

    #[test]
    #[ignore = "explicit replacement snapshot cost comparison"]
    fn replacement_cost_probe() {
        use std::hint::black_box;
        use std::time::Instant;
        let store = Store::open(domain() ^ 0x4003_0000).unwrap();
        for size in [80, 176, 4097, 1024 * 1024] {
            let mut bytes = vec![7; size];
            store.replace(&bytes).unwrap();
            let iterations = (4 * 1024 * 1024 / size).clamp(16, 8000);
            let modes = [
                "snapshot_unchanged",
                "replace_unchanged",
                "snapshot_changed",
                "replace_changed",
            ];
            let mut timings = [[0.0f64; 7]; 4];
            for round in 0..8 {
                for position in 0..modes.len() {
                    let mode = (round + position) % modes.len();
                    let started = Instant::now();
                    for _ in 0..iterations {
                        if mode >= 2 {
                            bytes[0] = bytes[0].wrapping_add(1);
                        }
                        if mode % 2 == 0 {
                            // The previous replace implementation: snapshot,
                            // borrowed replacement, comparison and publication.
                            store
                                .update_then(|_| Ok((black_box(&bytes), ())), |_| {})
                                .unwrap();
                        } else {
                            store.replace(black_box(&bytes)).unwrap();
                        }
                    }
                    if round != 0 {
                        timings[mode][round - 1] =
                            started.elapsed().as_nanos() as f64 / iterations as f64;
                    }
                }
            }
            for (mode, mut samples) in modes.into_iter().zip(timings) {
                samples.sort_by(f64::total_cmp);
                eprintln!(
                    "replace_metadata mode={mode} bytes={size} iterations={iterations} samples=7 ns_per_op_min={:.1} ns_per_op_median={:.1} ns_per_op_max={:.1}",
                    samples[0], samples[3], samples[6]
                );
            }
        }
    }

    #[test]
    fn abandoned_writer_helper() {
        let Ok(domain) = std::env::var("KINAKAZE_MOUNT_CRASH_TEST") else {
            return;
        };
        let store = Store::open(domain.parse().unwrap()).unwrap();
        let _guard = store.acquire().unwrap();
        let next = store.revision() + 1;
        let bank = store.bank(next);
        store.commit_pages(bank, 4096).unwrap();
        unsafe { &*bank.cast::<AtomicU64>() }.store(usize::MAX as u64, Ordering::SeqCst);
        // Deliberately abandon the named mutex before publication, simulating
        // process death while assembling an inactive transaction.
        std::process::exit(23);
    }

    #[test]
    fn a_dead_writer_does_not_publish_partial_mount_metadata() {
        let store = Store::open(domain()).unwrap();
        store.update(|_| Ok((b"committed".to_vec(), ()))).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mount::shared::tests::abandoned_writer_helper",
                "--nocapture",
            ])
            .env("KINAKAZE_MOUNT_CRASH_TEST", domain().to_string())
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert_eq!(child.status.code(), Some(23));
        assert_eq!(store.read().unwrap().1, b"committed");
        store
            .update(|old| {
                assert_eq!(old, b"committed");
                Ok((b"recovered".to_vec(), ()))
            })
            .unwrap();
        assert_eq!(store.read().unwrap().1, b"recovered");
        store.replace(b"replacement after abandonment").unwrap();
        assert_eq!(store.read().unwrap().1, b"replacement after abandonment");
    }

    #[test]
    fn readers_progress_while_an_unpublished_writer_is_suspended() {
        let store = Arc::new(Store::open(domain() ^ 0x1000_0000).unwrap());
        store.update(|_| Ok((b"visible".to_vec(), ()))).unwrap();
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer_store = Arc::clone(&store);
        let writer = std::thread::spawn(move || {
            let _guard = writer_store.acquire().unwrap();
            held_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        held_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (result_tx, result_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            result_tx.send(store.read()).unwrap();
        });
        let result = result_rx.recv_timeout(Duration::from_secs(1));
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        reader.join().unwrap();
        assert_eq!(result.unwrap().unwrap().1, b"visible");
    }

    #[test]
    fn concurrent_bank_reuse_never_returns_torn_records() {
        let store = Arc::new(Store::open(domain() ^ 0x2000_0000).unwrap());
        store.update(|_| Ok((vec![1; 32768], ()))).unwrap();
        let writer_store = Arc::clone(&store);
        let writer = std::thread::spawn(move || {
            for i in 1..=500usize {
                let bytes = vec![(i % 251) as u8; 8 + (i * 193) % 32768];
                if i % 2 == 0 {
                    writer_store.update(|_| Ok((bytes, ()))).unwrap();
                } else {
                    writer_store.replace(&bytes).unwrap();
                }
            }
        });
        for _ in 0..1000 {
            let (_, bytes) = store.read().unwrap();
            assert!(bytes.len() >= 8);
            assert!(bytes.iter().all(|byte| *byte == bytes[0]));
        }
        writer.join().unwrap();
    }

    fn generation_record(generation: u64) -> Vec<u8> {
        let words = 1 + (generation as usize % 17) * 257;
        (0..words)
            .flat_map(|index| (generation + index as u64).to_le_bytes())
            .collect()
    }

    #[test]
    fn independent_bank_writer_helper() {
        let Ok(domain) = std::env::var("KINAKAZE_BANK_REUSE_TEST") else {
            return;
        };
        let store = Store::open(domain.parse().unwrap()).unwrap();
        for generation in 1..=1000 {
            if generation % 2 == 0 {
                store
                    .update(|_| Ok((generation_record(generation), ())))
                    .unwrap();
            } else {
                store.replace(&generation_record(generation)).unwrap();
            }
        }
    }

    #[test]
    fn independent_process_bank_reuse_preserves_length_and_every_word() {
        let domain = domain() ^ 0x0400_0000;
        let store = Store::open(domain).unwrap();
        store.update(|_| Ok((generation_record(0), ()))).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mount::shared::tests::independent_bank_writer_helper",
            ])
            .env("KINAKAZE_BANK_REUSE_TEST", domain.to_string())
            .creation_flags(0x0800_0000)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        loop {
            let (_, bytes) = store.read().unwrap();
            assert!(bytes.len() >= 8);
            let generation = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            assert_eq!(bytes, generation_record(generation));
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if started.elapsed() > Duration::from_secs(10) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("independent bank writer did not complete");
            }
        }
        assert_eq!(store.read().unwrap().1, generation_record(1000));
    }
}
