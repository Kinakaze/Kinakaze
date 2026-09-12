//! Linux process-owned advisory record locks.
//!
//! Native byte-range locks are mandatory and owned by a Windows file object;
//! neither property implements F_SETLK. This registry instead shares inode,
//! Linux process identity and inclusive byte ranges between all hosted processes.
//! Queries have no locking side effect and ordinary file I/O is unaffected.
//!
//! The named section contains two banks. A transaction writes the inactive bank
//! and publishes it with one atomic store, so a process dying with the mutex held
//! cannot partially rewrite another process's locks. Only live records are copied.
//! Process death is checked against the PID registry (including its incarnation),
//! and blocking callers wait on a change event, the blocking process and signals.

use std::collections::{BTreeMap, BTreeSet};
use std::mem::{offset_of, size_of};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetProcessIdOfThread, INFINITE, OpenEventW,
    OpenProcess, OpenThread, PROCESS_SYNCHRONIZE, ReleaseMutex, SetEvent,
    THREAD_QUERY_LIMITED_INFORMATION, THREAD_SYNCHRONIZE, WaitForMultipleObjects,
    WaitForSingleObject,
};

use crate::{EAGAIN, EBADF, EINTR, EINVAL, EIO, FdEntry, FdFlags, FdKind, errno_from_win32};

pub const EDEADLK: i32 = 35;
pub const ENOLCK: i32 = 37;
const LOCK_CAPACITY: usize = 32_768;
const WAITER_CAPACITY: usize = 4_096;
const MAGIC: u64 = u64::from_le_bytes(*b"CYRECLK1");
const EVENT_PREFIX: &str = r"Local\kinakaze.record-lock.wait.v1";

/// Both endpoints are inclusive, including the byte at OFF_MAX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Range {
    pub start: u64,
    pub end: u64,
}

impl Range {
    pub fn new(start: u64, end: u64) -> Result<Self, i32> {
        if start > end || end > i64::MAX as u64 {
            Err(EINVAL)
        } else {
            Ok(Self { start, end })
        }
    }

    fn overlaps(self, other: Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Read,
    Write,
}

impl Kind {
    fn raw(self) -> u32 {
        match self {
            Self::Read => 0,
            Self::Write => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
struct Inode {
    volume: u64,
    id: [u8; 16],
}

impl Inode {
    fn from_entry(entry: FdEntry) -> Result<Self, i32> {
        if entry.flags.contains(FdFlags::PATH_ONLY)
            || !matches!(entry.kind, FdKind::File | FdKind::Directory)
            || entry.raw == 0
        {
            return Err(EBADF);
        }
        let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
        // The descriptor table, or its closing caller, pins this handle.
        if unsafe {
            GetFileInformationByHandleEx(
                entry.raw as HANDLE,
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        Ok(Self {
            volume: info.VolumeSerialNumber,
            id: info.FileId.Identifier,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
struct Owner {
    pid: u32,
    namespace: u32,
    born: u64,
}

impl Owner {
    fn from_entry(entry: kinakaze_runtime::job::Entry) -> Self {
        Self {
            pid: entry.namespace_pid,
            namespace: entry.namespace,
            born: entry.start_ticks,
        }
    }

    fn current() -> Result<Self, i32> {
        crate::job::ensure_registered();
        kinakaze_runtime::job::lookup_host(std::process::id())
            .map(Self::from_entry)
            .ok_or(EIO)
    }

    fn live_entry(self) -> Option<kinakaze_runtime::job::Entry> {
        kinakaze_runtime::job::lookup(self.pid).filter(|entry| {
            Self::from_entry(*entry) == self
                && entry.flags & kinakaze_runtime::job::FLAG_ZOMBIE == 0
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
struct Record {
    inode: Inode,
    owner: Owner,
    range: Range,
    kind: u32,
    reserved: u32,
}

impl Record {
    fn conflicts(self, wanted: Self) -> bool {
        self.inode == wanted.inode
            && self.owner != wanted.owner
            && self.range.overlaps(wanted.range)
            && (self.kind == Kind::Write.raw() || wanted.kind == Kind::Write.raw())
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Waiter {
    wanted: Record,
    host: u32,
    thread: u32,
}

impl Waiter {
    fn name(self) -> Vec<u16> {
        wide(&format!("{EVENT_PREFIX}.{}.{}", self.host, self.thread))
    }

    fn same_thread(self, other: Self) -> bool {
        self.host == other.host && self.thread == other.thread
    }
}

#[repr(C)]
struct Header {
    magic: u64,
    size: u64,
    active: AtomicU32,
    reserved: u32,
}

#[repr(C)]
struct Bank {
    lock_count: u32,
    waiter_count: u32,
    locks: [Record; LOCK_CAPACITY],
    waiters: [Waiter; WAITER_CAPACITY],
}

const SECTION_SIZE: usize = size_of::<Header>() + 2 * size_of::<Bank>();
static CACHE: AtomicUsize = AtomicUsize::new(0);

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
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

struct Shared {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _section: Handle,
    mutex: Handle,
}

impl Drop for Shared {
    fn drop(&mut self) {
        unsafe { UnmapViewOfFile(self.view) };
    }
}

struct Guard<'a>(&'a Shared);

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.mutex.0) };
    }
}

impl Shared {
    fn acquire(&self) -> Result<Guard<'_>, i32> {
        match unsafe { WaitForSingleObject(self.mutex.0, INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Guard(self)),
            _ => Err(EIO),
        }
    }

    fn header(&self) -> &Header {
        // The view is page-aligned; all shared mutations use the named mutex.
        unsafe { &*self.view.Value.cast::<Header>() }
    }

    fn bank(&self, index: u32) -> *mut Bank {
        debug_assert!(index < 2);
        unsafe {
            self.view
                .Value
                .cast::<u8>()
                .add(size_of::<Header>() + index as usize * size_of::<Bank>())
                .cast()
        }
    }

    fn load(&self) -> Result<State, i32> {
        let active = self.header().active.load(Ordering::Acquire);
        if active > 1 {
            return Err(EIO);
        }
        let bank = self.bank(active);
        let (locks, waiters) = unsafe {
            let lock_count = ptr::addr_of!((*bank).lock_count).read() as usize;
            let waiter_count = ptr::addr_of!((*bank).waiter_count).read() as usize;
            if lock_count > LOCK_CAPACITY || waiter_count > WAITER_CAPACITY {
                return Err(EIO);
            }
            (
                std::slice::from_raw_parts(ptr::addr_of!((*bank).locks).cast(), lock_count)
                    .to_vec(),
                std::slice::from_raw_parts(ptr::addr_of!((*bank).waiters).cast(), waiter_count)
                    .to_vec(),
            )
        };
        Ok(State {
            locks,
            waiters,
            dirty: false,
            wake: false,
        })
    }

    fn commit(&self, state: &State) -> Result<(), i32> {
        if state.locks.len() > LOCK_CAPACITY || state.waiters.len() > WAITER_CAPACITY {
            return Err(ENOLCK);
        }
        let next = 1 - self.header().active.load(Ordering::Acquire);
        let bank = self.bank(next);
        unsafe {
            ptr::copy_nonoverlapping(
                state.locks.as_ptr(),
                ptr::addr_of_mut!((*bank).locks).cast(),
                state.locks.len(),
            );
            ptr::copy_nonoverlapping(
                state.waiters.as_ptr(),
                ptr::addr_of_mut!((*bank).waiters).cast(),
                state.waiters.len(),
            );
            ptr::addr_of_mut!((*bank).lock_count).write(state.locks.len() as u32);
            ptr::addr_of_mut!((*bank).waiter_count).write(state.waiters.len() as u32);
        }
        self.header().active.store(next, Ordering::Release);
        Ok(())
    }
}

fn shared() -> Result<&'static Shared, i32> {
    let cached = CACHE.load(Ordering::Acquire);
    if cached != 0 {
        return Ok(unsafe { &*(cached as *const Shared) });
    }
    let mutex = Handle::new(unsafe {
        CreateMutexW(
            ptr::null(),
            0,
            wide(r"Local\kinakaze.record-lock.guard.v1").as_ptr(),
        )
    })?;
    let section = Handle::new(unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            SECTION_SIZE as u32,
            wide(r"Local\kinakaze.record-lock.v1").as_ptr(),
        )
    })?;
    let view = unsafe { MapViewOfFile(section.0, FILE_MAP_ALL_ACCESS, 0, 0, SECTION_SIZE) };
    if view.Value.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let candidate = Box::new(Shared {
        view,
        _section: section,
        mutex,
    });
    {
        let _guard = candidate.acquire()?;
        let header = candidate.view.Value.cast::<Header>();
        unsafe {
            if ptr::addr_of!((*header).magic).read() == 0 {
                // A newly created pagefile section is zero-filled. Publish magic last.
                ptr::addr_of_mut!((*header).size).write(SECTION_SIZE as u64);
                ptr::addr_of_mut!((*header).magic).write(MAGIC);
            }
            if (*header).magic != MAGIC || (*header).size != SECTION_SIZE as u64 {
                return Err(EIO);
            }
        }
    }
    // No local setup mutex can be inherited in a locked state by fork.
    let candidate = Box::into_raw(candidate);
    match CACHE.compare_exchange(0, candidate as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(unsafe { &*candidate }),
        Err(existing) => {
            unsafe { drop(Box::from_raw(candidate)) };
            Ok(unsafe { &*(existing as *const Shared) })
        }
    }
}

#[derive(Default)]
struct State {
    locks: Vec<Record>,
    waiters: Vec<Waiter>,
    dirty: bool,
    wake: bool,
}

impl State {
    fn conflict(&self, wanted: Record) -> Option<Record> {
        self.locks
            .iter()
            .copied()
            .find(|record| record.conflicts(wanted))
    }

    /// Replace only this owner's covered bytes, preserving other ranges/owners.
    fn replace(&mut self, wanted: Record, unlock: bool) -> Result<(), i32> {
        let mut replacement = Vec::with_capacity(self.locks.len() + 2);
        for old in self.locks.iter().copied() {
            if old.inode != wanted.inode
                || old.owner != wanted.owner
                || !old.range.overlaps(wanted.range)
            {
                replacement.push(old);
                continue;
            }
            if old.range.start < wanted.range.start {
                replacement.push(Record {
                    range: Range {
                        start: old.range.start,
                        end: wanted.range.start - 1,
                    },
                    ..old
                });
            }
            if old.range.end > wanted.range.end {
                replacement.push(Record {
                    range: Range {
                        start: wanted.range.end + 1,
                        end: old.range.end,
                    },
                    ..old
                });
            }
        }
        if !unlock {
            replacement.push(wanted);
        }
        replacement.sort_unstable_by_key(|r| (r.inode, r.owner, r.range.start));
        let mut merged: Vec<Record> = Vec::with_capacity(replacement.len());
        for record in replacement {
            if let Some(last) = merged.last_mut()
                && last.inode == record.inode
                && last.owner == record.owner
                && last.kind == record.kind
                && last.range.end + 1 >= record.range.start
            {
                last.range.end = last.range.end.max(record.range.end);
            } else {
                merged.push(record);
            }
        }
        if merged.len() > LOCK_CAPACITY {
            return Err(ENOLCK);
        }
        self.wake |= self.locks != merged;
        self.dirty |= self.wake;
        self.locks = merged;
        Ok(())
    }

    fn remove_waiter(&mut self, waiter: Waiter) {
        let old = self.waiters.len();
        self.waiters
            .retain(|candidate| !candidate.same_thread(waiter));
        self.dirty |= old != self.waiters.len();
    }

    fn would_deadlock(&self, wanted: Record) -> bool {
        let mut pending: Vec<Owner> = self
            .locks
            .iter()
            .filter(|record| record.conflicts(wanted))
            .map(|r| r.owner)
            .collect();
        let mut visited = BTreeSet::new();
        while let Some(owner) = pending.pop() {
            if owner == wanted.owner {
                return true;
            }
            if !visited.insert(owner) {
                continue;
            }
            for waiter in self.waiters.iter().filter(|w| w.wanted.owner == owner) {
                pending.extend(
                    self.locks
                        .iter()
                        .filter(|r| r.conflicts(waiter.wanted))
                        .map(|r| r.owner),
                );
            }
        }
        false
    }

    fn reap(&mut self) {
        let mut owners = BTreeMap::new();
        for owner in self
            .locks
            .iter()
            .map(|r| r.owner)
            .chain(self.waiters.iter().map(|w| w.wanted.owner))
        {
            owners.entry(owner).or_insert_with(|| owner.live_entry());
        }
        let before = self.locks.len();
        self.locks.retain(|r| owners[&r.owner].is_some());
        self.dirty |= before != self.locks.len();
        self.wake |= before != self.locks.len();
        let before = self.waiters.len();
        self.waiters.retain(|waiter| {
            let Some(entry) = owners[&waiter.wanted.owner] else {
                return false;
            };
            if entry.pid != waiter.host {
                return false;
            }
            let Ok(thread) = Handle::new(unsafe {
                OpenThread(
                    THREAD_SYNCHRONIZE | THREAD_QUERY_LIMITED_INFORMATION,
                    0,
                    waiter.thread,
                )
            }) else {
                return false;
            };
            unsafe {
                GetProcessIdOfThread(thread.0) == waiter.host
                    && WaitForSingleObject(thread.0, 0) == WAIT_TIMEOUT
            }
        });
        self.dirty |= before != self.waiters.len();
    }
}

fn notify(waiters: &[Waiter]) {
    for waiter in waiters {
        if let Ok(event) =
            Handle::new(unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, waiter.name().as_ptr()) })
        {
            unsafe { SetEvent(event.0) };
        }
    }
}

fn transaction<T>(body: impl FnOnce(&mut State) -> Result<T, i32>) -> Result<T, i32> {
    let shared = shared()?;
    let guard = shared.acquire()?;
    let mut state = shared.load()?;
    state.reap();
    let result = body(&mut state);
    if state.dirty {
        shared.commit(&state)?;
    }
    drop(guard);
    if state.wake {
        notify(&state.waiters);
    }
    result
}

fn descriptor(fd: i32) -> Result<(FdEntry, Inode), i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = *table
        .slots
        .get(fd as usize)
        .and_then(Option::as_ref)
        .ok_or(EBADF)?;
    Ok((entry, Inode::from_entry(entry)?))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Conflict {
    pub kind: Kind,
    pub range: Range,
    pub pid: u32,
}

pub fn query(fd: i32, range: Range, kind: Kind) -> Result<Option<Conflict>, i32> {
    Range::new(range.start, range.end)?;
    let (entry, inode) = descriptor(fd)?;
    let wanted = Record {
        inode,
        owner: Owner::current()?,
        range,
        kind: kind.raw(),
        reserved: 0,
    };
    transaction(|state| {
        if crate::get(fd)?.generation != entry.generation {
            return Err(EBADF);
        }
        Ok(state.conflict(wanted).map(|record| Conflict {
            kind: if record.kind == 0 {
                Kind::Read
            } else {
                Kind::Write
            },
            range: record.range,
            pid: record.owner.pid,
        }))
    })
}

/// `None` unlocks. A failed upgrade preserves the owner's previous locks.
pub fn set(fd: i32, range: Range, kind: Option<Kind>, wait: bool) -> Result<(), i32> {
    Range::new(range.start, range.end)?;
    let (entry, inode) = descriptor(fd)?;
    let wanted = Record {
        inode,
        owner: Owner::current()?,
        range,
        kind: kind.unwrap_or(Kind::Read).raw(),
        reserved: 0,
    };
    let waiter = Waiter {
        wanted,
        host: std::process::id(),
        thread: crate::interrupt::current_thread_id(),
    };
    // An uncontended F_SETLKW needs no wait handle. Create one before publishing
    // a waiter, under the same transaction, so an unlock cannot be lost.
    let mut event = None;
    loop {
        let blocker = transaction(|state| {
            if !crate::get(fd).is_ok_and(|current| current.generation == entry.generation) {
                state.remove_waiter(waiter);
                return Err(EBADF);
            }
            if kind.is_some()
                && let Some(blocker) = state.conflict(wanted)
            {
                if !wait {
                    return Err(EAGAIN);
                }
                if state.would_deadlock(wanted) {
                    state.remove_waiter(waiter);
                    return Err(EDEADLK);
                }
                if event.is_none() {
                    event = Some(Handle::new(unsafe {
                        CreateEventW(ptr::null(), 0, 0, waiter.name().as_ptr())
                    })?);
                }
                if !state
                    .waiters
                    .iter()
                    .any(|candidate| candidate.same_thread(waiter))
                {
                    if state.waiters.len() == WAITER_CAPACITY {
                        return Err(ENOLCK);
                    }
                    state.waiters.push(waiter);
                    // Publish wait dependencies without waking unchanged blockers.
                    state.dirty = true;
                }
                return Ok(Some(blocker.owner));
            }
            state.remove_waiter(waiter);
            state.replace(wanted, kind.is_none())?;
            Ok(None)
        })?;
        let Some(blocker) = blocker else {
            return Ok(());
        };
        let outcome = wait_for_change(event.as_ref().ok_or(EIO)?.0, blocker);
        if let Err(error) = outcome {
            transaction(|state| {
                state.remove_waiter(waiter);
                Ok(())
            })?;
            return Err(error);
        }
    }
}

fn wait_for_change(event: HANDLE, blocker: Owner) -> Result<(), i32> {
    let Some(entry) = blocker.live_entry() else {
        return Ok(());
    };
    let process = match Handle::new(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, entry.pid) }) {
        Ok(process) => process,
        Err(_) if blocker.live_entry().is_none() => return Ok(()),
        Err(error) => return Err(error),
    };
    let interrupt = crate::interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    crate::signal::register_waiter();
    let outcome = if crate::signal::deliver_pending() == crate::signal::Delivery::Interrupted {
        Err(EINTR)
    } else {
        let handles = [event, process.0, interrupt];
        match unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE) }
        {
            WAIT_OBJECT_0 => Ok(()),
            value if value == WAIT_OBJECT_0 + 1 => Ok(()),
            value if value == WAIT_OBJECT_0 + 2 => {
                if crate::signal::deliver_pending() == crate::signal::Delivery::Interrupted {
                    Err(EINTR)
                } else {
                    Ok(())
                }
            }
            _ => Err(EIO),
        }
    };
    crate::signal::unregister_waiter();
    outcome
}

/// Any close of this inode drops this process's POSIX locks, even a different fd.
pub(crate) fn descriptor_closed(entry: FdEntry) -> Result<(), i32> {
    if CACHE.load(Ordering::Acquire) == 0
        || !matches!(entry.kind, FdKind::File | FdKind::Directory)
        || entry.flags.contains(FdFlags::PATH_ONLY)
    {
        return Ok(());
    }
    // An old exec wrapper owns no Linux process identity and must not release
    // the locks now owned by its replacement image.
    let Some(owner) = kinakaze_runtime::job::lookup_host(std::process::id()).map(Owner::from_entry)
    else {
        return Ok(());
    };
    let inode = Inode::from_entry(entry)?;
    release_inodes(owner, &[inode])
}

fn release_inodes(owner: Owner, inodes: &[Inode]) -> Result<(), i32> {
    transaction(|state| {
        let before = state.locks.len();
        state
            .locks
            .retain(|record| record.owner != owner || !inodes.contains(&record.inode));
        state.dirty |= before != state.locks.len();
        state.wake |= before != state.locks.len();
        Ok(())
    })
}

/// Exec carries only the inodes closed by CLOEXEC, never a copy of shared locks.
pub(crate) fn serialize_exec_closed(entries: &[FdEntry]) -> Result<Vec<u8>, i32> {
    let mut inodes = BTreeSet::new();
    for entry in entries {
        if entry.flags.contains(FdFlags::CLOSE_ON_EXEC)
            && !entry.flags.contains(FdFlags::PATH_ONLY)
            && matches!(entry.kind, FdKind::File | FdKind::Directory)
        {
            inodes.insert(Inode::from_entry(*entry)?);
        }
    }
    let mut bytes = Vec::with_capacity(inodes.len() * size_of::<Inode>());
    for inode in inodes {
        bytes.extend_from_slice(&inode.volume.to_le_bytes());
        bytes.extend_from_slice(&inode.id);
    }
    Ok(bytes)
}

/// Called through the VFS fork registry after restoring the child's PID.
pub(crate) fn restore(payload: &[u8]) -> bool {
    if !payload.len().is_multiple_of(size_of::<Inode>()) {
        return false;
    }
    // Parent-only view and non-inheritable handles are not child resources.
    // The copied allocation is discarded, not dropped against invalid handles.
    CACHE.store(0, Ordering::Release);
    let inodes: Vec<Inode> = payload
        .chunks_exact(size_of::<Inode>())
        .map(|bytes| Inode {
            volume: u64::from_le_bytes(bytes[..8].try_into().unwrap()),
            id: bytes[8..24].try_into().unwrap(),
        })
        .collect();
    // Map even for an empty payload: exec can preserve locks acquired by the old
    // image, and its first operation may be close rather than fcntl.
    let Ok(owner) = Owner::current() else {
        return false;
    };
    release_inodes(owner, &inodes).is_ok()
}

const _: () = assert!(offset_of!(Bank, locks).is_multiple_of(8));

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read, Write};
    use std::os::windows::io::IntoRawHandle;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("kinakaze-lock-{}-{nonce}", std::process::id()));
            std::fs::write(&path, b"test").unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn open_test_file(path: &Path) -> i32 {
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap()
            .into_raw_handle();
        crate::install(
            handle as usize,
            FdKind::File,
            FdFlags::READ_ACCESS.union(FdFlags::WRITE_ACCESS),
        )
        .unwrap()
    }

    struct Helper(Child);
    impl Helper {
        fn new(path: &Path, operation: &str) -> Self {
            let child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "record_lock::tests::process_helper",
                    "--nocapture",
                ])
                .env("KINAKAZE_RECORD_LOCK_TEST", path)
                .env("KINAKAZE_RECORD_LOCK_OPERATION", operation)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            Self(child)
        }
        fn ready(&mut self) {
            let mut output = std::io::BufReader::new(self.0.stdout.take().unwrap());
            loop {
                let mut line = String::new();
                assert_ne!(
                    output.read_line(&mut line).unwrap(),
                    0,
                    "helper exited before ready"
                );
                if line.contains("record-lock-helper-ready") {
                    // Keep the pipe open for the test harness's final status.
                    self.0.stdout = Some(output.into_inner());
                    break;
                }
            }
        }
    }
    impl Drop for Helper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn process_helper() {
        let Some(path) = std::env::var_os("KINAKAZE_RECORD_LOCK_TEST") else {
            return;
        };
        let fd = open_test_file(Path::new(&path));
        let range = Range::new(0, 9).unwrap();
        if std::env::var("KINAKAZE_RECORD_LOCK_OPERATION").unwrap() == "crash" {
            let shared = shared().unwrap();
            let _guard = shared.acquire().unwrap();
            let bank = shared.bank(1 - shared.header().active.load(Ordering::Acquire));
            // Die halfway through an inactive-bank write, with the mutex held.
            unsafe {
                ptr::addr_of_mut!((*bank).lock_count).write(u32::MAX);
                windows_sys::Win32::System::Threading::ExitProcess(23);
            }
        }
        set(fd, range, Some(Kind::Write), false).unwrap();
        println!("record-lock-helper-ready");
        std::io::stdout().flush().unwrap();
        let mut command = [0];
        std::io::stdin().read_exact(&mut command).unwrap();
        crate::close(fd).unwrap();
    }

    fn await_waiter(thread: u32) {
        let started = Instant::now();
        loop {
            if transaction(|state| {
                Ok(state
                    .waiters
                    .iter()
                    .any(|w| w.thread == thread && w.host == std::process::id()))
            })
            .unwrap()
            {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "lock did not enter a blocking wait"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn foreign_process_death_wakes_blocking_lock_without_retry_timer() {
        let fixture = Fixture::new();
        let mut helper = Helper::new(&fixture.0, "hold");
        helper.ready();
        let fd = open_test_file(&fixture.0);
        let range = Range::new(0, 9).unwrap();
        let conflict = query(fd, range, Kind::Write).unwrap().unwrap();
        assert_eq!(conflict.kind, Kind::Write);
        assert_ne!(conflict.pid, Owner::current().unwrap().pid);
        assert_eq!(set(fd, range, Some(Kind::Read), false), Err(EAGAIN));
        // Advisory locks must never obstruct ordinary host/guest file access.
        std::fs::write(&fixture.0, b"still writable").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let waiting = std::thread::spawn(move || {
            tx.send(crate::interrupt::current_thread_id()).unwrap();
            set(fd, range, Some(Kind::Write), true)
        });
        await_waiter(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        helper.0.kill().unwrap();
        helper.0.wait().unwrap();
        assert_eq!(waiting.join().unwrap(), Ok(()));
        crate::close(fd).unwrap();
    }

    unsafe extern "sysv64" fn signal_handler(_: i32) {}

    #[test]
    fn interrupt_and_restart_follow_signal_disposition() {
        let fixture = Fixture::new();
        let mut helper = Helper::new(&fixture.0, "hold");
        helper.ready();
        let fd = open_test_file(&fixture.0);
        let range = Range::new(0, 9).unwrap();
        let previous = crate::signal::sigaction(
            crate::signal::SIGUSR2,
            Some(crate::signal::Action {
                disposition: crate::signal::Disposition::Handle(signal_handler, 0),
                flags: 0,
                ..Default::default()
            }),
        )
        .unwrap();
        for restart in [false, true] {
            crate::signal::sigaction(
                crate::signal::SIGUSR2,
                Some(crate::signal::Action {
                    disposition: crate::signal::Disposition::Handle(signal_handler, 0),
                    flags: if restart {
                        crate::signal::SA_RESTART
                    } else {
                        0
                    },
                    ..Default::default()
                }),
            )
            .unwrap();
            let (tid_tx, tid_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let waiting = std::thread::spawn(move || {
                tid_tx.send(crate::interrupt::current_thread_id()).unwrap();
                done_tx
                    .send(set(fd, range, Some(Kind::Write), true))
                    .unwrap();
            });
            let thread = tid_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            await_waiter(thread);
            crate::signal::raise_thread_signal(thread, crate::signal::SIGUSR2).unwrap();
            if restart {
                assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());
                helper.0.stdin.as_mut().unwrap().write_all(b"u").unwrap();
                assert_eq!(
                    done_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                    Ok(())
                );
            } else {
                assert_eq!(
                    done_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                    Err(EINTR)
                );
            }
            waiting.join().unwrap();
        }
        crate::signal::sigaction(crate::signal::SIGUSR2, Some(previous)).unwrap();
        crate::close(fd).unwrap();
    }

    #[test]
    fn abandoned_transaction_keeps_other_process_locks_intact() {
        let fixture = Fixture::new();
        let fd = open_test_file(&fixture.0);
        let range = Range::new(0, 9).unwrap();
        set(fd, range, Some(Kind::Write), false).unwrap();
        let mut helper = Helper::new(&fixture.0, "crash");
        assert_eq!(helper.0.wait().unwrap().code(), Some(23));
        let owner = Owner::current().unwrap();
        let inode = descriptor(fd).unwrap().1;
        transaction(|state| {
            assert!(state.locks.iter().any(|r| r.owner == owner
                && r.inode == inode
                && r.range == range
                && r.kind == Kind::Write.raw()));
            Ok(())
        })
        .unwrap();
        crate::close(fd).unwrap();
    }

    fn record(pid: u32, inode: u64, start: u64, end: u64, kind: Kind) -> Record {
        Record {
            inode: Inode {
                volume: inode,
                id: [0; 16],
            },
            owner: Owner {
                pid,
                namespace: 0,
                born: pid as u64,
            },
            range: Range::new(start, end).unwrap(),
            kind: kind.raw(),
            reserved: 0,
        }
    }

    #[test]
    fn upgrades_split_and_coalesce_without_self_conflict() {
        let mut state = State::default();
        let read = record(1, 10, 0, 99, Kind::Read);
        state.replace(read, false).unwrap();
        let write = record(1, 10, 20, 29, Kind::Write);
        assert_eq!(state.conflict(write), None);
        state.replace(write, false).unwrap();
        assert_eq!(
            state
                .locks
                .iter()
                .map(|r| (r.range, r.kind))
                .collect::<Vec<_>>(),
            vec![
                (Range { start: 0, end: 19 }, 0),
                (Range { start: 20, end: 29 }, 1),
                (Range { start: 30, end: 99 }, 0),
            ]
        );
        state.replace(read, false).unwrap();
        assert_eq!(state.locks, vec![read]);
        state
            .replace(record(1, 10, 30, 49, Kind::Read), true)
            .unwrap();
        assert_eq!(
            state.locks.iter().map(|r| r.range).collect::<Vec<_>>(),
            vec![Range { start: 0, end: 29 }, Range { start: 50, end: 99 },]
        );
    }

    #[test]
    fn query_reports_exact_owner_type_and_range() {
        let mut state = State::default();
        let held = record(1, 10, 20, 39, Kind::Read);
        state.replace(held, false).unwrap();
        assert_eq!(state.conflict(record(2, 10, 25, 25, Kind::Read)), None);
        assert_eq!(
            state.conflict(record(2, 10, 25, 25, Kind::Write)),
            Some(held)
        );
        assert_eq!(state.conflict(record(2, 11, 25, 25, Kind::Write)), None);
        assert_eq!(state.locks, vec![held]);
    }

    #[test]
    fn unbounded_ranges_include_off_max() {
        let mut state = State::default();
        state
            .replace(record(1, 10, 0, i64::MAX as u64, Kind::Write), false)
            .unwrap();
        let byte = record(2, 10, i64::MAX as u64, i64::MAX as u64, Kind::Read);
        assert!(state.conflict(byte).is_some());
        state
            .replace(record(1, 10, 0, i64::MAX as u64 - 1, Kind::Read), true)
            .unwrap();
        assert_eq!(state.locks[0].range, byte.range);
    }

    #[test]
    fn deadlock_detection_crosses_files_and_processes() {
        let mut state = State::default();
        state
            .replace(record(1, 10, 0, 9, Kind::Write), false)
            .unwrap();
        state
            .replace(record(2, 11, 0, 9, Kind::Write), false)
            .unwrap();
        let first = record(1, 11, 0, 9, Kind::Write);
        assert!(!state.would_deadlock(first));
        state.waiters.push(Waiter {
            wanted: first,
            host: 1,
            thread: 1,
        });
        assert!(state.would_deadlock(record(2, 10, 0, 9, Kind::Write)));
        assert!(!state.would_deadlock(record(3, 10, 0, 9, Kind::Write)));
    }
}
