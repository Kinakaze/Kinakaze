//! One inode, one shared byte queue. No process owns or brokers the queue.
//!
//! The private mapping file keeps contents available while a fork child has
//! inherited tokens but has not restored userspace attachments yet. Token names
//! are the kernel-owned liveness authority; an empty set starts a fresh stream.

use std::collections::HashSet;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, WAIT_ABANDONED_0,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_TEMPORARY, FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FileIdInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
    OPEN_ALWAYS,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, ReleaseMutex, SetEvent,
    WaitForMultipleObjects,
};

use super::lifecycle::{DirectoryWatch, Owned, last_errno, storage_root, token};
use crate::fs::{O_ACCMODE, O_APPEND, O_NONBLOCK, O_RDONLY, O_RDWR, O_WRONLY};
use crate::{EAGAIN, EBADF, EINTR, EINVAL, EIO, ENFILE, ENXIO, EPIPE};

pub(super) const CAPACITY: usize = 65_536;
pub(super) const PIPE_BUF: usize = 4096;
// Open descriptions per FIFO inode; unrelated to process descriptor numbers.
const ROWS: usize = 1024;
const MAGIC: u64 = 0x334f_4649_4653_5943;
const READ: u32 = 1;
const WRITE: u32 = 2;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Row {
    id: u64,
    read_generation: u64,
    write_generation: u64,
    flags: u32,
    access: u32,
}

#[repr(C)]
struct State {
    magic: u64,
    // Head/length commit as one aligned word so mutex-owner death cannot leave
    // a torn pair. Bytes are copied before publishing this word.
    position: u64,
    next_id: u64,
    epoch: u64,
    reader_generation: u64,
    writer_generation: u64,
    epoch_waited: u32,
    readers: u32,
    writers: u32,
    // Protected by gate. Cleared only after a complete authoritative scan, so
    // an interrupted observer cannot publish a falsely clean endpoint set.
    needs_reconcile: u32,
    rows: [Row; ROWS],
    bytes: [u8; CAPACITY],
}

impl State {
    fn position(&self) -> (usize, usize) {
        let position = unsafe {
            std::sync::atomic::AtomicU64::from_ptr((&self.position as *const u64).cast_mut())
        }
        .load(std::sync::atomic::Ordering::Acquire);
        (position as u32 as usize, (position >> 32) as usize)
    }
    fn commit_position(&mut self, head: usize, length: usize) {
        unsafe { std::sync::atomic::AtomicU64::from_ptr(&mut self.position) }.store(
            head as u64 | ((length as u64) << 32),
            std::sync::atomic::Ordering::Release,
        );
    }
    fn row(&self, id: u64) -> Result<usize, i32> {
        self.rows.iter().position(|row| row.id == id).ok_or(EBADF)
    }
}

/// Process-local resources. None of these handles or pointers is serialized.
pub(super) struct Channel {
    directory: PathBuf,
    key: String,
    _file: Owned,
    _mapping: Owned,
    view: *mut State,
    gate: Owned,
    watch: Mutex<DirectoryWatch>,
    #[cfg(test)]
    scans: std::sync::atomic::AtomicUsize,
}
unsafe impl Send for Channel {}
unsafe impl Sync for Channel {}

impl Drop for Channel {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            })
        };
    }
}

fn identity(marker: HANDLE) -> Result<String, i32> {
    let mut info = FILE_ID_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            marker,
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(last_errno());
    }
    let file = info
        .FileId
        .Identifier
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{:016x}-{file}", info.VolumeSerialNumber))
}

fn name(key: &str, suffix: &str) -> Vec<u16> {
    // The backing inode can be opened in different Windows sessions (including
    // session-zero services). Its lock and wakeups must not be session-local.
    // Global event/mutex objects do not require the named-section privilege;
    // the actual file mapping below is unnamed.
    format!("Global\\kinakaze.fifo.v3.{key}.{suffix}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

impl Channel {
    pub(super) fn attach(marker: HANDLE) -> Result<Arc<Self>, i32> {
        let key = identity(marker)?;
        let directory = storage_root()?.join(&key);
        std::fs::create_dir_all(&directory)
            .map_err(|error| crate::errno_from_win32(error.raw_os_error().unwrap_or(1) as u32))?;
        Self::attach_directory(directory, key)
    }

    fn attach_directory(directory: PathBuf, key: String) -> Result<Arc<Self>, i32> {
        let gate_name = name(&key, "lock");
        let gate = Owned::checked(unsafe { CreateMutexW(ptr::null(), 0, gate_name.as_ptr()) })?;
        let taken = wait_handles(&[gate.0], true)?;
        if !matches!(taken, WAIT_OBJECT_0 | WAIT_ABANDONED_0) {
            return Err(EIO);
        }
        struct Unlock(HANDLE);
        impl Drop for Unlock {
            fn drop(&mut self) {
                unsafe { ReleaseMutex(self.0) };
            }
        }
        let unlocked = Unlock(gate.0);
        let path = crate::path::wide_path(&directory.join("queue.bin"))?;
        let file = Owned::checked(unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_TEMPORARY,
                ptr::null_mut(),
            )
        })?;
        let mapping = Owned::checked(unsafe {
            CreateFileMappingW(
                file.0,
                ptr::null(),
                PAGE_READWRITE,
                0,
                size_of::<State>() as u32,
                ptr::null(),
            )
        })?;
        let view =
            unsafe { MapViewOfFile(mapping.0, FILE_MAP_ALL_ACCESS, 0, 0, size_of::<State>()) };
        if view.Value.is_null() {
            return Err(last_errno());
        }
        let watch = match DirectoryWatch::new(&directory) {
            Ok(watch) => watch,
            Err(error) => {
                unsafe { UnmapViewOfFile(view) };
                return Err(error);
            }
        };
        let channel = Arc::new(Self {
            directory,
            key,
            _file: file,
            _mapping: mapping,
            view: view.Value.cast(),
            gate,
            watch: Mutex::new(watch),
            #[cfg(test)]
            scans: std::sync::atomic::AtomicUsize::new(0),
        });
        // The named mutex serializes first creation, mapping and initialization.
        let state = unsafe { &mut *channel.view };
        if state.magic == 0 {
            unsafe { ptr::write_bytes(channel.view.cast::<u8>(), 0, size_of::<State>()) };
            state.next_id = 1;
            state.epoch = 1;
            state.magic = MAGIC;
        }
        if state.magic != MAGIC || state.position().0 >= CAPACITY || state.position().1 > CAPACITY {
            drop(unlocked);
            return Err(EIO);
        }
        // This observer has no notification history from before its watch was
        // armed. The first lock must establish a fresh endpoint census.
        state.needs_reconcile = 1;
        drop(unlocked);
        Ok(channel)
    }

    /// Recover only from a retained native token, never a guest pathname.
    pub(super) fn restore(token: HANDLE) -> Result<(Arc<Self>, u64), i32> {
        let mut buffer = vec![0u16; 512];
        loop {
            let length = unsafe {
                GetFinalPathNameByHandleW(token, buffer.as_mut_ptr(), buffer.len() as u32, 0)
            };
            if length == 0 {
                return Err(last_errno());
            }
            if length as usize >= buffer.len() {
                if length > 32_767 {
                    return Err(crate::ENAMETOOLONG);
                }
                buffer.resize(length as usize + 1, 0);
                continue;
            }
            let path =
                PathBuf::from(String::from_utf16(&buffer[..length as usize]).map_err(|_| EIO)?);
            let leaf = path.file_name().and_then(|name| name.to_str()).ok_or(EIO)?;
            let id = leaf
                .strip_prefix("fd-")
                .and_then(|id| u64::from_str_radix(id, 16).ok())
                .ok_or(EIO)?;
            let directory = path.parent().ok_or(EIO)?.to_path_buf();
            let key = directory
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(EIO)?
                .to_owned();
            if key.len() != 49 || !key.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return Err(EIO);
            }
            let channel = Self::attach_directory(directory, key)?;
            {
                let mut guard = channel.lock()?;
                guard.state().row(id)?;
            }
            return Ok((channel, id));
        }
    }

    fn lock(&self) -> Result<Guard<'_>, i32> {
        let taken = wait_handles(&[self.gate.0], true)?;
        if !matches!(taken, WAIT_OBJECT_0 | WAIT_ABANDONED_0) {
            return Err(EIO);
        }
        let mut guard = Guard { channel: self };
        if taken == WAIT_ABANDONED_0 {
            guard.state().needs_reconcile = 1;
            // Retire a potentially signaled epoch left by a dead mutex owner.
            guard.changed()?;
        }
        // Consume notification under the inode mutex, publishing its old epoch
        // before resetting the shared watch event. It is then safe if this owner
        // dies before the authoritative scan. Rearming precedes that scan so a
        // concurrent last-token close cannot disappear between scan and wait.
        guard.refresh_watch()?;
        if guard.state().needs_reconcile != 0 {
            guard.reconcile()?;
        }
        Ok(guard)
    }

    #[cfg(test)]
    pub(super) fn open(self: &Arc<Self>, flags: i32) -> Result<Opened, i32> {
        self.open_with_restart(flags, &mut || false)
    }

    pub(super) fn open_with_restart(
        self: &Arc<Self>,
        flags: i32,
        restart: &mut impl FnMut() -> bool,
    ) -> Result<Opened, i32> {
        let access = match flags & O_ACCMODE {
            O_RDONLY => READ,
            O_WRONLY => WRITE,
            O_RDWR => READ | WRITE,
            _ => return Err(EINVAL),
        };
        let mut guard = retry_interrupt(|| self.lock(), restart)?;
        let state = guard.state();
        if access == WRITE && flags & O_NONBLOCK != 0 && state.readers == 0 {
            return Err(ENXIO);
        }
        if state.readers == 0 && state.writers == 0 {
            state.commit_position(0, 0);
        }
        let slot = state
            .rows
            .iter()
            .position(|row| row.id == 0)
            .ok_or(ENFILE)?;
        let id = state.next_id;
        state.next_id = id.checked_add(1).ok_or(ENFILE)?;
        let handle = token(&self.directory.join(format!("fd-{id:016x}")), false)?;
        let row = Row {
            id,
            // An already-present peer completes this open even if it closes
            // before the caller resumes. For readers it also means the final
            // writer close must report HUP; only a nonblocking read-open that
            // has never observed any writer suppresses initial HUP.
            read_generation: if state.writers != 0 {
                state.writer_generation - 1
            } else {
                state.writer_generation
            },
            write_generation: if state.readers != 0 {
                state.reader_generation - 1
            } else {
                state.reader_generation
            },
            flags: flags as u32,
            access,
        };
        // Publish identity last. An abandoned mutex may expose an interrupted
        // constructor; a zero-id row is never counted as an endpoint.
        state.rows[slot].id = 0;
        state.rows[slot].read_generation = row.read_generation;
        state.rows[slot].write_generation = row.write_generation;
        state.rows[slot].flags = row.flags;
        state.rows[slot].access = row.access;
        unsafe { std::sync::atomic::AtomicU64::from_ptr(&mut state.rows[slot].id) }
            .store(id, std::sync::atomic::Ordering::Release);
        if access & READ != 0 {
            state.readers += 1;
            state.reader_generation = id;
        }
        if access & WRITE != 0 {
            state.writers += 1;
            state.writer_generation = id;
        }
        guard.changed()?;
        drop(guard);
        let opened = Opened {
            token: handle,
            context: Context {
                channel: self.clone(),
                id,
            },
        };
        if flags & O_NONBLOCK == 0 && access != (READ | WRITE) {
            loop {
                let mut guard = retry_interrupt(|| self.lock(), restart)?;
                let state = guard.state();
                let row = state.rows[state.row(id)?];
                let connected = if access == READ {
                    state.writers > 0 || state.writer_generation != row.read_generation
                } else {
                    state.readers > 0 || state.reader_generation != row.write_generation
                };
                if connected {
                    break;
                }
                let registration = guard.register(self.clone())?;
                drop(guard);
                // Keep this endpoint (and the peer generation it observed)
                // across SA_RESTART. Reopening would drop the reader/writer
                // token and can expose a spurious EOF to an arriving peer.
                retry_interrupt(|| registration.wait(), restart)?;
            }
        }
        Ok(opened)
    }
}

/// Call the ABI restart policy only after the interrupted operation has
/// released its locks, so guest handlers may safely open the same FIFO.
pub(super) fn retry_interrupt<T>(
    mut operation: impl FnMut() -> Result<T, i32>,
    restart: &mut impl FnMut() -> bool,
) -> Result<T, i32> {
    loop {
        match operation() {
            Err(EINTR) if restart() => continue,
            result => return result,
        }
    }
}

struct Guard<'a> {
    channel: &'a Channel,
}
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.channel.gate.0) };
    }
}
impl Guard<'_> {
    fn state(&mut self) -> &mut State {
        unsafe { &mut *self.channel.view }
    }

    fn refresh_watch(&mut self) -> Result<(), i32> {
        let channel = self.channel;
        channel
            .watch
            .lock()
            .map_err(|_| EIO)?
            .rearm_notifying(|| {
                // Publish invalidation before resetting the notification. This
                // also survives a failed rearm or an observer exiting midway.
                self.state().needs_reconcile = 1;
                self.changed()
            })
            .map(|_| ())
    }

    fn reconcile(&mut self) -> Result<(), i32> {
        #[cfg(test)]
        self.channel
            .scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut live = HashSet::new();
        for entry in std::fs::read_dir(&self.channel.directory).map_err(|_| EIO)? {
            let entry = entry.map_err(|_| EIO)?;
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("fd-"))
                .and_then(|id| u64::from_str_radix(id, 16).ok())
            {
                live.insert(id);
            }
        }
        let state = self.state();
        let old = (state.readers, state.writers);
        state.readers = 0;
        state.writers = 0;
        for row in &mut state.rows {
            if row.id == 0 {
                continue;
            }
            if !live.contains(&row.id) {
                row.id = 0;
                continue;
            }
            state.readers += u32::from(row.access & READ != 0);
            state.writers += u32::from(row.access & WRITE != 0);
            // Complete generation publication if the creator died immediately
            // after publishing its row while another process retained a token.
            if row.access & READ != 0 {
                state.reader_generation = state.reader_generation.max(row.id);
            }
            if row.access & WRITE != 0 {
                state.writer_generation = state.writer_generation.max(row.id);
            }
        }
        if old != (state.readers, state.writers) {
            self.changed()?;
        }
        self.state().needs_reconcile = 0;
        Ok(())
    }

    fn changed(&mut self) -> Result<(), i32> {
        let epoch = self.notify_waited_epoch()?;
        // Notify before advancing. If the owner dies between these operations,
        // an abandoned-mutex successor still sees the old waited epoch and can
        // notify it again. The reverse order permanently loses old waiters.
        let state = self.state();
        state.epoch = epoch.checked_add(1).ok_or(EIO)?;
        state.epoch_waited = 0;
        Ok(())
    }

    fn notify_waited_epoch(&mut self) -> Result<u64, i32> {
        let state = self.state();
        let epoch = state.epoch;
        let waited = state.epoch_waited != 0;
        if waited {
            let event_name = name(&self.channel.key, &format!("epoch.{epoch:016x}"));
            let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr()) };
            if event.is_null() {
                if unsafe { GetLastError() } != ERROR_FILE_NOT_FOUND {
                    return Err(last_errno());
                }
            } else {
                let event = Owned(event);
                if unsafe { SetEvent(event.0) } == 0 {
                    return Err(last_errno());
                }
            }
        }
        Ok(epoch)
    }

    fn register(&mut self, channel: Arc<Channel>) -> Result<WaitRegistration, i32> {
        let epoch = self.state().epoch;
        let event_name = name(&self.channel.key, &format!("epoch.{epoch:016x}"));
        let event =
            Owned::checked(unsafe { CreateEventW(ptr::null(), 1, 0, event_name.as_ptr()) })?;
        self.state().epoch_waited = 1;
        let directory_event = channel.watch.lock().map_err(|_| EIO)?.event() as usize;
        Ok(WaitRegistration {
            channel,
            event,
            directory_event,
            retention: None,
        })
    }
}

pub(super) struct Opened {
    pub token: Owned,
    pub context: Context,
}
#[derive(Clone)]
pub(super) struct Context {
    pub channel: Arc<Channel>,
    pub id: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Readiness {
    pub readable: bool,
    pub writable: bool,
    pub hangup: bool,
    pub error: bool,
}

pub struct WaitRegistration {
    channel: Arc<Channel>,
    event: Owned,
    directory_event: usize,
    retention: Option<Box<dyn std::any::Any + Send + Sync>>,
}
impl WaitRegistration {
    pub fn handles(&self) -> [usize; 2] {
        [self.event.0 as usize, self.directory_event]
    }
    pub(super) fn retain<T: std::any::Any + Send + Sync>(&mut self, value: T) {
        self.retention = Some(Box::new(value));
    }
    pub(super) fn wait(&self) -> Result<(), i32> {
        let _keep_channel_alive = &self.channel;
        wait_handles(&[self.event.0, self.directory_event as HANDLE], true).map(|_| ())
    }
}

fn wait_handles(handles: &[HANDLE], interruptible: bool) -> Result<u32, i32> {
    let interrupt = if interruptible {
        crate::interrupt::current()
    } else {
        ptr::null_mut()
    };
    if interruptible && interrupt.is_null() {
        return Err(EIO);
    }
    let mut owned = handles.to_vec();
    if !interrupt.is_null() {
        owned.push(interrupt);
        crate::signal::register_waiter();
    }
    let waited = unsafe { WaitForMultipleObjects(owned.len() as u32, owned.as_ptr(), 0, INFINITE) };
    if !interrupt.is_null() {
        crate::signal::unregister_waiter();
    }
    if !interrupt.is_null() && waited == WAIT_OBJECT_0 + handles.len() as u32 {
        return Err(EINTR);
    }
    if waited < WAIT_OBJECT_0 + handles.len() as u32
        || (WAIT_ABANDONED_0..WAIT_ABANDONED_0 + handles.len() as u32).contains(&waited)
    {
        Ok(waited)
    } else {
        Err(EIO)
    }
}

impl Context {
    fn readiness_locked(&self, guard: &mut Guard<'_>) -> Result<Readiness, i32> {
        let state = guard.state();
        let row = state.rows[state.row(self.id)?];
        let (_, length) = state.position();
        Ok(Readiness {
            readable: row.access & READ != 0 && length > 0,
            writable: row.access & WRITE != 0 && length < CAPACITY,
            hangup: row.access & READ != 0
                && state.writers == 0
                && state.writer_generation != row.read_generation,
            error: row.access & WRITE != 0 && state.readers == 0,
        })
    }

    pub(super) fn readiness(&self) -> Result<Readiness, i32> {
        self.readiness_locked(&mut self.channel.lock()?)
    }

    /// Readiness and subscription are captured in one critical section. Poll
    /// must use this sample before sleeping, not a stale earlier readiness call.
    pub(super) fn prepare_wait(&self) -> Result<(Readiness, WaitRegistration), i32> {
        let mut guard = self.channel.lock()?;
        let readiness = self.readiness_locked(&mut guard)?;
        let registered = guard.register(self.channel.clone())?;
        Ok((readiness, registered))
    }

    pub(super) fn flags(&self) -> Result<i32, i32> {
        let mut guard = self.channel.lock()?;
        let state = guard.state();
        Ok(state.rows[state.row(self.id)?].flags as i32)
    }

    pub(super) fn set_flags(&self, nonblock: bool, append: bool) -> Result<(), i32> {
        let mut guard = self.channel.lock()?;
        let state = guard.state();
        let row = state.row(self.id)?;
        let mask = (O_NONBLOCK | O_APPEND) as u32;
        state.rows[row].flags = (state.rows[row].flags & !mask)
            | if nonblock { O_NONBLOCK as u32 } else { 0 }
            | if append { O_APPEND as u32 } else { 0 };
        guard.changed()
    }

    pub(super) fn set_nonblocking(&self, enabled: bool) -> Result<(), i32> {
        let mut guard = self.channel.lock()?;
        let state = guard.state();
        let row = state.row(self.id)?;
        state.rows[row].flags = (state.rows[row].flags & !(O_NONBLOCK as u32))
            | if enabled { O_NONBLOCK as u32 } else { 0 };
        guard.changed()
    }

    pub(super) fn queued(&self) -> Result<usize, i32> {
        let mut guard = self.channel.lock()?;
        guard.state().row(self.id)?;
        Ok(guard.state().position().1)
    }

    pub(super) fn read(&self, buffer: &mut [u8]) -> Result<usize, i32> {
        loop {
            let mut guard = self.channel.lock()?;
            let state = guard.state();
            let row = state.rows[state.row(self.id)?];
            if row.access & READ == 0 {
                return Err(EBADF);
            }
            if buffer.is_empty() {
                return Ok(0);
            }
            let (head, length) = state.position();
            if length != 0 {
                let count = buffer.len().min(length);
                let first = count.min(CAPACITY - head);
                buffer[..first].copy_from_slice(&state.bytes[head..head + first]);
                buffer[first..count].copy_from_slice(&state.bytes[..count - first]);
                state.commit_position((head + count) % CAPACITY, length - count);
                guard.changed()?;
                return Ok(count);
            }
            if state.writers == 0 {
                return Ok(0);
            }
            if row.flags & O_NONBLOCK as u32 != 0 {
                return Err(EAGAIN);
            }
            let registration = guard.register(self.channel.clone())?;
            drop(guard);
            registration.wait()?;
        }
    }

    pub(super) fn write(&self, buffer: &[u8]) -> Result<usize, i32> {
        self.write_report(buffer).0
    }

    pub(super) fn write_report(&self, buffer: &[u8]) -> (Result<usize, i32>, bool) {
        let mut broken_pipe = false;
        let result = self.write_inner(buffer, &mut broken_pipe);
        (result, broken_pipe)
    }

    fn write_inner(&self, buffer: &[u8], broken_pipe: &mut bool) -> Result<usize, i32> {
        let mut written = 0;
        loop {
            let mut guard = match self.channel.lock() {
                Ok(guard) => guard,
                Err(error) => {
                    return if written != 0 {
                        Ok(written)
                    } else {
                        Err(error)
                    };
                }
            };
            let state = guard.state();
            let row = state.rows[state.row(self.id)?];
            if row.access & WRITE == 0 {
                return Err(EBADF);
            }
            if buffer.is_empty() {
                return Ok(0);
            }
            if state.readers == 0 {
                *broken_pipe = true;
                return if written != 0 {
                    Ok(written)
                } else {
                    Err(EPIPE)
                };
            }
            let (head, length) = state.position();
            let free = CAPACITY - length;
            let remaining = buffer.len() - written;
            let atomic = buffer.len() <= PIPE_BUF;
            if free != 0 && (!atomic || free >= remaining) {
                let count = free.min(remaining);
                let tail = (head + length) % CAPACITY;
                let first = count.min(CAPACITY - tail);
                state.bytes[tail..tail + first].copy_from_slice(&buffer[written..written + first]);
                state.bytes[..count - first]
                    .copy_from_slice(&buffer[written + first..written + count]);
                state.commit_position(head, length + count);
                written += count;
                let nonblock = row.flags & O_NONBLOCK as u32 != 0;
                guard.changed()?;
                if written == buffer.len() || nonblock {
                    return Ok(written);
                }
                continue;
            }
            if row.flags & O_NONBLOCK as u32 != 0 {
                return if written != 0 {
                    Ok(written)
                } else {
                    Err(EAGAIN)
                };
            }
            let registration = guard.register(self.channel.clone())?;
            drop(guard);
            if let Err(error) = registration.wait() {
                return if written != 0 {
                    Ok(written)
                } else {
                    Err(error)
                };
            }
        }
    }
}

#[cfg(test)]
mod tests;
