//! Descriptor integration and explicit fork/exec restoration.

use std::collections::HashMap;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
    SetHandleInformation,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use super::lifecycle::{Owned, last_errno};
use super::shared::{Channel, Context, Readiness, WaitRegistration};
use crate::fs::{
    O_ACCMODE, O_APPEND, O_CLOEXEC, O_DIRECTORY, O_NONBLOCK, O_PATH, O_RDONLY, O_RDWR, O_WRONLY,
    Stat,
};
use crate::{EBADF, EINVAL, EIO, ENOTDIR, FdEntry, FdFlags, FdKind};

#[cfg(test)]
mod tests;

struct Record {
    entry: FdEntry,
    marker: Arc<Owned>,
    context: Context,
}
pub(crate) struct RightsPin(Arc<Owned>);
impl RightsPin {
    pub(crate) fn raw(&self) -> u64 {
        self.0.0 as u64
    }
}
pub(crate) fn rights_reference(entry: FdEntry) -> Result<Option<RightsPin>, i32> {
    if entry.kind != FdKind::Fifo {
        return Ok(None);
    }
    let records = registry().records.lock().map_err(|_| EIO)?;
    let record = records
        .values()
        .find(|r| same(r.entry, entry))
        .ok_or(EBADF)?;
    Ok(Some(RightsPin(record.marker.clone())))
}
pub(crate) fn import_rights(fd: i32, marker: crate::fs::object::Object) -> Result<(), i32> {
    let entry = crate::get(fd)?;
    let (channel, id) = Channel::restore(entry.raw as HANDLE)?;
    crate::platform::try_set_inheritable(marker.raw() as usize, true)?;
    let record = Record {
        entry,
        marker: Arc::new(Owned(marker.into_raw())),
        context: Context { channel, id },
    };
    let mut records = registry().records.lock().map_err(|_| EIO)?;
    publish_record(&mut records, fd, record)
}

fn publish_record(records: &mut HashMap<i32, Record>, fd: i32, record: Record) -> Result<(), i32> {
    if let Some(detached) = records.get(&fd) {
        // close() detaches its slot before native cleanup. A racing allocator
        // can reuse the number while an operation still pins the old marker.
        // Once the old record leaves this enumerable registry, its marker must
        // no longer participate in ambient native handle inheritance.
        if unsafe { SetHandleInformation(detached.marker.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(last_errno());
        }
    }
    records.insert(fd, record);
    Ok(())
}
struct Registry {
    pid: u32,
    records: Mutex<HashMap<i32, Record>>,
}
static REGISTRY: AtomicPtr<Registry> = AtomicPtr::new(ptr::null_mut());

fn new_registry() -> *mut Registry {
    Box::into_raw(Box::new(Registry {
        pid: std::process::id(),
        records: Mutex::new(HashMap::new()),
    }))
}

fn registry() -> &'static Registry {
    loop {
        let current = REGISTRY.load(Ordering::Acquire);
        if !current.is_null() && unsafe { (*current).pid } == std::process::id() {
            return unsafe { &*current };
        }
        let candidate = new_registry();
        if REGISTRY
            .compare_exchange(current, candidate, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // A fork-copied registry has parent pointers/locks and does not own
            // child resources. Never run its destructors; tag14 reconstructs it.
            return unsafe { &*candidate };
        }
        unsafe { drop(Box::from_raw(candidate)) };
    }
}

fn same(left: FdEntry, right: FdEntry) -> bool {
    left.kind == FdKind::Fifo
        && right.kind == FdKind::Fifo
        && left.raw == right.raw
        && left.generation == right.generation
        && left.description_id == right.description_id
}

fn duplicate(raw: HANDLE, inherit: bool) -> Result<Owned, i32> {
    let mut result = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut result,
            0,
            i32::from(inherit),
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(last_errno());
    }
    Owned::checked(result)
}

/// A complete operation-local reference. A Context alone must never keep an
/// operation running after its token disappeared from the shared liveness set.
pub(crate) struct Pinned {
    _token: Owned,
    marker: Arc<Owned>,
    context: Context,
}

/// The caller holds the fd-table read/write lock and has verified this entry.
pub(crate) fn pin_entry_locked(entry: FdEntry) -> Result<Pinned, i32> {
    let records = registry().records.lock().map_err(|_| EIO)?;
    let record = records
        .values()
        .find(|record| same(record.entry, entry))
        .ok_or(EBADF)?;
    let token = duplicate(entry.raw as HANDLE, false)?;
    Ok(Pinned {
        _token: token,
        marker: record.marker.clone(),
        context: record.context.clone(),
    })
}

fn pin(fd: i32, expected: FdEntry) -> Result<Pinned, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let current = usize::try_from(fd)
        .ok()
        .and_then(|fd| table.slots.get(fd))
        .and_then(|entry| *entry)
        .ok_or(EBADF)?;
    if !same(current, expected) {
        return Err(EBADF);
    }
    pin_entry_locked(current)
}

fn descriptor_flags(flags: i32) -> FdFlags {
    let mut result = FdFlags::NONE;
    if flags & O_CLOEXEC != 0 {
        result = result.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & O_NONBLOCK != 0 {
        result = result.union(FdFlags::NONBLOCK);
    }
    if flags & O_APPEND != 0 {
        result = result.union(FdFlags::APPEND);
    }
    match flags & O_ACCMODE {
        O_RDONLY => result
            .union(FdFlags::READ_ACCESS)
            .union(FdFlags::PIPE_READ_END),
        O_WRONLY => result
            .union(FdFlags::WRITE_ACCESS)
            .union(FdFlags::PIPE_WRITE_END),
        O_RDWR => result
            .union(FdFlags::READ_ACCESS)
            .union(FdFlags::WRITE_ACCESS)
            .union(FdFlags::PIPE_READ_END)
            .union(FdFlags::PIPE_WRITE_END),
        _ => result,
    }
}

/// Opens the shared queue identified by a pinned physical FIFO marker.
pub(crate) fn open_shared_marker(marker: HANDLE, flags: i32) -> Result<i32, i32> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    if flags & O_PATH != 0 {
        return Err(EINVAL);
    }
    // Pending opens are not descriptors and must not be inherited by a racing
    // fork. Only atomic fd publication makes these two handles inheritable.
    let marker = Arc::new(duplicate(marker, false)?);
    let mut restart = || crate::signal::deliver_pending() != crate::signal::Delivery::Interrupted;
    let channel = super::shared::retry_interrupt(|| Channel::attach(marker.0), &mut restart)?;
    let opened = channel.open_with_restart(flags, &mut restart)?;
    let raw = opened.token.0 as usize;
    let context = opened.context;
    let fd = crate::install_with(raw, FdKind::Fifo, descriptor_flags(flags), |fd, entry| {
        let mut records = registry().records.lock().map_err(|_| EIO)?;
        if unsafe { SetHandleInformation(marker.0, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0
        {
            return Err(last_errno());
        }
        publish_record(
            &mut records,
            fd,
            Record {
                entry,
                marker,
                context,
            },
        )
    })?;
    opened.token.into_raw();
    Ok(fd)
}

pub(crate) fn reopen_pinned(pinned: &Pinned, flags: i32) -> Result<i32, i32> {
    if flags & O_DIRECTORY != 0 {
        return Err(ENOTDIR);
    }
    if flags & O_PATH != 0 {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_READ_ATTRIBUTES, FILE_READ_EA, SYNCHRONIZE,
        };
        let handle = {
            crate::fs::object::Object::reopen(
                pinned.marker.0,
                FILE_READ_ATTRIBUTES | FILE_READ_EA | SYNCHRONIZE,
            )
        }?;
        let mut retained = FdFlags::PATH_ONLY;
        if flags & O_CLOEXEC != 0 {
            retained = retained.union(FdFlags::CLOSE_ON_EXEC);
        }
        let fd = crate::install(handle.raw() as usize, FdKind::File, retained)?;
        handle.into_raw();
        return Ok(fd);
    }
    open_shared_marker(pinned.marker.0, flags)
}

pub fn read(fd: i32, expected: FdEntry, buffer: &mut [u8]) -> Result<usize, i32> {
    let pinned = pin(fd, expected)?;
    pinned.context.read(buffer)
}

pub fn write(fd: i32, expected: FdEntry, buffer: &[u8]) -> Result<usize, i32> {
    let pinned = pin(fd, expected)?;
    let (result, broken_pipe) = pinned.context.write_report(buffer);
    if broken_pipe {
        // Queue only after leaving the inode mutex; dispatch remains at the ABI
        // boundary so a handler can safely fork/exec or reopen the FIFO.
        crate::signal::raise_thread_signal(
            crate::interrupt::current_thread_id(),
            crate::signal::SIGPIPE,
        )?;
    }
    result
}

pub fn queued_bytes(fd: i32, expected: FdEntry) -> Result<usize, i32> {
    pin(fd, expected)?.context.queued()
}

pub fn capacity(fd: i32, expected: FdEntry) -> Result<usize, i32> {
    let pinned = pin(fd, expected)?;
    pinned.context.flags()?;
    Ok(super::shared::CAPACITY)
}

pub fn metadata(fd: i32, expected: FdEntry) -> Result<Stat, i32> {
    let pinned = pin(fd, expected)?;
    let mut stat = crate::fs::stat_handle(pinned.marker.0, false)?;
    stat.st_size = 0;
    stat.st_blocks = 0;
    Ok(stat)
}

pub fn link_target(fd: i32, expected: FdEntry) -> Result<String, i32> {
    let pinned = pin(fd, expected)?;
    let path = crate::fs::path_from_handle(pinned.marker.0).ok_or(EIO)?;
    Ok(crate::to_guest_path(&path))
}

pub fn status_flags(fd: i32, expected: FdEntry) -> Result<i32, i32> {
    Ok(pin(fd, expected)?.context.flags()? & (O_ACCMODE | O_APPEND | O_NONBLOCK))
}

pub fn set_status_flags(
    fd: i32,
    expected: FdEntry,
    append: bool,
    nonblock: bool,
) -> Result<(), i32> {
    pin(fd, expected)?.context.set_flags(nonblock, append)
}

pub fn set_nonblocking(fd: i32, expected: FdEntry, enabled: bool) -> Result<(), i32> {
    pin(fd, expected)?.context.set_nonblocking(enabled)
}

pub fn prepare_wait(fd: i32, expected: FdEntry) -> Result<(Readiness, WaitRegistration), i32> {
    prepare_wait_pinned(pin(fd, expected)?)
}

pub(crate) fn prepare_wait_pinned(pinned: Pinned) -> Result<(Readiness, WaitRegistration), i32> {
    let (ready, mut registration) = pinned.context.prepare_wait()?;
    // Keep the native endpoint alive for the entire poll wait, not just while
    // checking readiness. These are non-inheritable operation-local pins.
    registration.retain(pinned);
    Ok((ready, registration))
}

/// Called by the fd allocator while holding its write lock, before publication.
pub fn duplicate_descriptor(source: FdEntry, newfd: i32, new_entry: FdEntry) -> Result<(), i32> {
    let mut records = registry().records.lock().map_err(|_| EIO)?;
    let record = records
        .values()
        .find(|record| same(record.entry, source))
        .ok_or(EBADF)?;
    let marker = Arc::new(duplicate(record.marker.0, true)?);
    let context = record.context.clone();
    publish_record(
        &mut records,
        newfd,
        Record {
            entry: new_entry,
            marker,
            context,
        },
    )
}

/// Called before descriptor detachment while the caller still owns fd-table
/// write exclusion. A later fork must not inherit a marker absent from tag14.
pub(crate) fn disable_inheritance_locked(entry: FdEntry) -> Result<(), i32> {
    let records = registry().records.lock().map_err(|_| EIO)?;
    let record = records
        .values()
        .find(|record| same(record.entry, entry))
        .ok_or(EBADF)?;
    disable_pair(record.marker.0, entry.raw as HANDLE, set_inheritance_bits)
}

fn set_inheritance_bits(handle: HANDLE, bits: u32) -> Result<(), i32> {
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, bits) } == 0 {
        Err(last_errno())
    } else {
        Ok(())
    }
}

fn disable_pair(
    marker: HANDLE,
    token: HANDLE,
    mut set: impl FnMut(HANDLE, u32) -> Result<(), i32>,
) -> Result<(), i32> {
    let mut previous = [(marker, 0), (token, 0)];
    // Validate both handles and capture their actual baseline before changing
    // either. A failure must leave the still-live descriptor fork-restorable.
    for (handle, bits) in &mut previous {
        if unsafe { GetHandleInformation(*handle, bits) } == 0 {
            return Err(last_errno());
        }
        *bits &= HANDLE_FLAG_INHERIT;
    }
    for index in 0..previous.len() {
        if let Err(error) = set(previous[index].0, 0) {
            for &(handle, bits) in previous[..=index].iter().rev() {
                if let Err(rollback) = set_inheritance_bits(handle, bits) {
                    eprintln!("kinakaze: FIFO inheritance rollback failed: errno {rollback}");
                }
            }
            return Err(error);
        }
    }
    Ok(())
}

/// Owns native close. The fd-table slot was already detached by the caller.
pub fn close_entry(fd: i32, entry: FdEntry) -> Result<(), i32> {
    let mut records = registry().records.lock().map_err(|_| EIO)?;
    let record = if records
        .get(&fd)
        .is_some_and(|record| same(record.entry, entry))
    {
        records.remove(&fd)
    } else {
        None
    };
    if let Some(record) = &record {
        // An in-flight operation may still pin the marker, but it is no longer
        // an inheritable descriptor resource after this descriptor is detached.
        unsafe { SetHandleInformation(record.marker.0, HANDLE_FLAG_INHERIT, 0) };
    }
    drop(records);
    drop(Owned(entry.raw as HANDLE));
    drop(record);
    Ok(())
}

/// Caller holds the fd-table lock; this function does not reenter that lock.
/// Native-child filters exclude every marker; guest exec excludes CLOEXEC ones.
pub struct AuxiliaryHandle {
    pub fd: i32,
    pub entry: FdEntry,
    marker: Arc<Owned>,
}
impl AuxiliaryHandle {
    pub fn raw(&self) -> usize {
        self.marker.0 as usize
    }
}

pub fn auxiliary_handles() -> Result<Vec<AuxiliaryHandle>, i32> {
    registry()
        .records
        .lock()
        .map(|records| {
            records
                .iter()
                .map(|(&fd, record)| AuxiliaryHandle {
                    fd,
                    entry: record.entry,
                    marker: record.marker.clone(),
                })
                .collect()
        })
        .map_err(|_| EIO)
}

/// Only retained markers are serialized. Generic process creation filters these
/// auxiliary handles alongside token FDs; excluded handle values must never be
/// closed in the child, where the same numeric value can name a fresh object.
pub fn serialize_matching(filter: impl Fn(i32) -> bool) -> Result<Vec<u8>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let records = registry().records.lock().map_err(|_| EIO)?;
    for (fd, entry) in table
        .slots
        .enumerated()
        .filter_map(|(fd, entry)| entry.map(|e| (fd, e)))
    {
        if entry.kind == FdKind::Fifo
            && !records
                .get(&(fd as i32))
                .is_some_and(|record| same(record.entry, entry))
        {
            return Err(EIO);
        }
    }
    let retained = records
        .iter()
        .filter(|(fd, record)| {
            filter(**fd)
                && table
                    .slots
                    .get(**fd as usize)
                    .and_then(|slot| *slot)
                    .is_some_and(|entry| same(entry, record.entry))
        })
        .collect::<Vec<_>>();
    let mut payload = Vec::with_capacity(8 + retained.len() * 40);
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&(retained.len() as u32).to_le_bytes());
    for (&fd, record) in retained {
        payload.extend_from_slice(&fd.to_le_bytes());
        payload.extend_from_slice(&record.entry.generation.to_le_bytes());
        payload.extend_from_slice(&(record.entry.raw as u64).to_le_bytes());
        payload.extend_from_slice(&record.entry.description_id.to_le_bytes());
        payload.extend_from_slice(&(record.marker.0 as u64).to_le_bytes());
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
    }
    Ok(payload)
}

pub fn restore_fork_state(payload: &[u8]) -> bool {
    if payload.len() < 8 {
        return false;
    }
    let number = |offset| u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap());
    if number(0) != 1
        || number(4) as usize > crate::MAX_FDS
        || payload.len() != 8 + number(4) as usize * 40
    {
        return false;
    }
    let mut replacement = Box::new(Registry {
        pid: std::process::id(),
        records: Mutex::new(HashMap::new()),
    });
    let mut records = HashMap::new();
    for chunk in payload[8..].chunks_exact(40) {
        let u32_at = |offset| u32::from_le_bytes(chunk[offset..offset + 4].try_into().unwrap());
        let u64_at = |offset| u64::from_le_bytes(chunk[offset..offset + 8].try_into().unwrap());
        let fd = u32_at(0) as i32;
        if u32_at(32) != 1 || u32_at(36) != 0 || u64_at(24) == 0 {
            return false;
        }
        let marker = Owned(u64_at(24) as HANDLE);
        let Ok(entry) = crate::get(fd) else {
            return false;
        };
        if entry.kind != FdKind::Fifo
            || entry.raw as u64 != u64_at(8)
            || entry.description_id != u64_at(16)
        {
            return false;
        }
        let Ok((channel, id)) = Channel::restore(entry.raw as HANDLE) else {
            return false;
        };
        records.insert(
            fd,
            Record {
                entry,
                marker: Arc::new(marker),
                context: Context { channel, id },
            },
        );
    }
    *replacement.records.get_mut().unwrap() = records;
    // Parent allocations and non-inherited native handles must never be dropped
    // as though the child owned them. All actual child resources are above.
    REGISTRY.store(Box::into_raw(replacement), Ordering::Release);
    true
}
