//! Linux-style queues backed by native Windows IoRing. Each native SQE owns
//! its file object and staging buffer through completion or cancellation.
//! Unsupported semantics are rejected, never replaced by synchronous file I/O.

use crate::fs::object::Object;
use crate::{EAGAIN, EBADF, EINVAL, EIO, ENOSYS, FdEntry, FdFlags, FdKind, interrupt, signal};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_HANDLE_EOF, ERROR_OPERATION_ABORTED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingReadFile, BuildIoRingWriteFile, CloseIoRing, CreateIoRing, FILE_APPEND_DATA,
    FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_WRITE_DATA, HIORING, IORING_BUFFER_REF,
    IORING_BUFFER_REF_0, IORING_CQE, IORING_CREATE_FLAGS, IORING_HANDLE_REF, IORING_HANDLE_REF_0,
    IORING_OP_READ as WINDOWS_OP_READ, IORING_OP_WRITE as WINDOWS_OP_WRITE, IORING_REF_RAW,
    IORING_VERSION_1, IORING_VERSION_2, IORING_VERSION_3, IsIoRingOpSupported, PopIoRingCompletion,
    SetIoRingCompletionEvent, SubmitIoRing,
};
use windows_sys::Win32::System::IO::CancelIoEx;
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects,
};

pub mod aio;
pub(crate) mod flush;
mod memory;
#[cfg(test)]
mod tests;

pub const IORING_OP_NOP: u8 = 0;
pub const IORING_OP_READV: u8 = 1;
pub const IORING_OP_WRITEV: u8 = 2;
pub const IORING_OP_READ_FIXED: u8 = 4;
pub const IORING_OP_WRITE_FIXED: u8 = 5;
pub const IORING_OP_READ: u8 = 22;
pub const IORING_OP_WRITE: u8 = 23;
pub const IORING_SETUP_IOPOLL: u32 = 1 << 0;
pub const IORING_SETUP_SQPOLL: u32 = 1 << 1;
pub const IORING_SETUP_CLAMP: u32 = 1 << 4;
pub const IORING_ENTER_GETEVENTS: u32 = 1;

/// Linux `io_uring_sqe`; this 64-byte layout is guest ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SubmissionEntry {
    pub opcode: u8,
    pub flags: u8,
    pub ioprio: u16,
    pub fd: i32,
    pub offset: u64,
    pub address: u64,
    pub length: u32,
    pub op_flags: u32,
    pub user_data: u64,
    pub buffer_index: u16,
    pub personality: u16,
    pub splice_fd_in: i32,
    pub padding: [u64; 2],
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CompletionEntry {
    pub user_data: u64,
    pub result: i32,
    pub flags: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IoVec {
    pub base: *mut u8,
    pub length: usize,
}

struct Event(usize);
impl Event {
    fn new(manual: bool) -> Result<Self, i32> {
        let raw = unsafe { CreateEventW(std::ptr::null(), manual as i32, 0, std::ptr::null()) };
        if raw.is_null() {
            Err(EIO)
        } else {
            Ok(Self(raw as usize))
        }
    }
    fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
    fn signal(&self) {
        unsafe { SetEvent(self.raw()) };
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.raw()) };
    }
}

/// Retained until the ORIGINAL native CQE, not merely a cancellation CQE.
struct Request {
    file: Object,
    buffer: Vec<u8>,
    targets: Vec<memory::Segment>,
    offset: u64,
    read: bool,
    user_data: u64,
    submitted: bool,
}
struct State {
    /// Zero only after submitted requests retire and CloseIoRing returns.
    handle: usize,
    closing: bool,
    pending: u32,
    capacity: u32,
    requests: HashMap<usize, Request>,
    next_cookie: usize,
    local_pending: VecDeque<CompletionEntry>,
    completions: VecDeque<CompletionEntry>,
    /// Notification for CQEs another enter/reap moved into the software queue.
    waiters: Vec<Weak<Event>>,
}
struct Ring {
    descriptor: FdEntry,
    completion_event: Event,
    closed_event: Event,
    state: Mutex<State>,
}
static RINGS: OnceLock<Mutex<HashMap<i32, Arc<Ring>>>> = OnceLock::new();
fn rings() -> &'static Mutex<HashMap<i32, Arc<Ring>>> {
    RINGS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Classify the current descriptor, not a possibly delayed side-table entry.
/// Callers already holding the fd table must inspect their FdEntry.kind instead.
pub fn is_ring(fd: i32) -> bool {
    crate::get(fd).is_ok_and(|entry| entry.kind == FdKind::IoRing)
}
fn lookup(fd: i32) -> Result<Arc<Ring>, i32> {
    let ring = rings()
        .lock()
        .map_err(|_| EIO)?
        .get(&fd)
        .cloned()
        .ok_or(EBADF)?;
    // Never hold the registry while acquiring the table. An old Arc cannot be
    // redirected to a replacement ring by close/reuse between these snapshots.
    let current = crate::get(fd)?;
    if current.description_id != ring.descriptor.description_id
        || current.raw != ring.descriptor.raw
        || current.kind != FdKind::IoRing
    {
        return Err(EBADF);
    }
    Ok(ring)
}

const PREFERRED_VERSIONS: [i32; 3] = [IORING_VERSION_3, IORING_VERSION_2, IORING_VERSION_1];
fn create_best_ring(submission_size: u32, completion_size: u32) -> Option<HIORING> {
    let flags = IORING_CREATE_FLAGS {
        Required: 0,
        Advisory: 0,
    };
    for version in PREFERRED_VERSIONS {
        let mut handle = std::ptr::null_mut();
        if unsafe {
            CreateIoRing(
                version,
                flags,
                submission_size,
                completion_size,
                &mut handle,
            )
        } >= 0
        {
            return Some(handle);
        }
    }
    None
}
pub fn available() -> bool {
    let Some(handle) = create_best_ring(4, 4) else {
        return false;
    };
    unsafe { CloseIoRing(handle) };
    true
}
pub fn setup(entries: u32, flags: u32) -> Result<i32, i32> {
    if entries == 0
        || flags & !IORING_SETUP_CLAMP != 0
        || (entries > 32768 && flags & IORING_SETUP_CLAMP == 0)
    {
        return Err(EINVAL);
    }
    let capacity = entries.min(32768).next_power_of_two();
    let completion_event = Event::new(true)?;
    let closed_event = Event::new(true)?;
    let placeholder = Event::new(true)?;
    let handle = create_best_ring(capacity, capacity * 2).ok_or(ENOSYS)?;
    if let Err(error) = hresult(unsafe { SetIoRingCompletionEvent(handle, completion_event.raw()) })
    {
        unsafe { CloseIoRing(handle) };
        return Err(error);
    }
    let state = State {
        handle: handle as usize,
        closing: false,
        pending: 0,
        capacity,
        requests: HashMap::new(),
        next_cookie: 1,
        local_pending: VecDeque::new(),
        completions: VecDeque::new(),
        waiters: Vec::new(),
    };
    let mut owned = Some((completion_event, closed_event, state));
    let mut replaced = None;
    // Fd allocation and side-table publication are one table-locked transaction.
    let flags = FdFlags::CLOSE_ON_EXEC
        .union(FdFlags::READ_ACCESS)
        .union(FdFlags::WRITE_ACCESS);
    let result = crate::install_with(placeholder.0, FdKind::IoRing, flags, |fd, descriptor| {
        let mut registry = rings().lock().map_err(|_| EIO)?;
        registry.try_reserve(1).map_err(|_| crate::ENOMEM)?;
        let (completion_event, closed_event, state) = owned.take().ok_or(EIO)?;
        // An old close may already have removed its fd but not its side-table
        // entry. Move that Arc out; dropping it under the table/registry locks
        // could block in cancellation retirement and violate lock ordering.
        replaced = registry.insert(
            fd,
            Arc::new(Ring {
                descriptor,
                completion_event,
                closed_event,
                state: Mutex::new(state),
            }),
        );
        Ok(())
    });
    if let Some(old) = replaced {
        old.shutdown();
    }
    match result {
        Ok(fd) => {
            std::mem::forget(placeholder);
            Ok(fd)
        }
        Err(error) => {
            unsafe { CloseIoRing(handle) };
            Err(error)
        }
    }
}
/// The removed descriptor's old placeholder stays live until this returns, so
/// its raw value cannot yet be reused by a replacement ring.
pub fn forget_ring(fd: i32, raw: usize) {
    let ring = {
        let mut registry = rings().lock().unwrap_or_else(|error| error.into_inner());
        if !registry
            .get(&fd)
            .is_some_and(|ring| ring.descriptor.raw == raw)
        {
            return;
        }
        registry.remove(&fd)
    };
    if let Some(ring) = ring {
        ring.shutdown();
    }
}
fn native_opcode(opcode: u8) -> Option<i32> {
    match opcode {
        IORING_OP_READ | IORING_OP_READV => Some(WINDOWS_OP_READ),
        IORING_OP_WRITE | IORING_OP_WRITEV => Some(WINDOWS_OP_WRITE),
        _ => None,
    }
}
pub fn opcode_supported(fd: i32, opcode: u8) -> Result<bool, i32> {
    let ring = lookup(fd)?;
    let state = ring.state.lock().map_err(|_| EIO)?;
    if state.closing {
        return Err(EBADF);
    }
    if opcode == IORING_OP_NOP {
        return Ok(true);
    }
    Ok(native_opcode(opcode)
        .is_some_and(|opcode| unsafe { IsIoRingOpSupported(state.handle as HIORING, opcode) } != 0))
}
fn hresult(result: i32) -> Result<(), i32> {
    if result >= 0 {
        return Ok(());
    }
    let code = result as u32;
    if code & 0xffff_0000 == 0x8007_0000 {
        Err(crate::errno_from_win32(code & 0xffff))
    } else {
        Err(match code {
            0x8046_0002 | 0x8046_0008 => crate::EBUSY,
            0x8000_4001 => crate::EOPNOTSUPP,
            _ => EIO,
        })
    }
}
fn prepare(entry: &SubmissionEntry) -> Result<Request, i32> {
    let (pinned, descriptor) = Object::from_fd_with_entry(entry.fd)?;
    if descriptor.kind == FdKind::Directory {
        return Err(crate::EISDIR);
    }
    if descriptor.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    // Linux -1 means shared OFD position, not Windows' append sentinel.
    if entry.offset == u64::MAX {
        return Err(crate::EOPNOTSUPP);
    }
    if entry.offset > i64::MAX as u64 {
        return Err(EINVAL);
    }
    let read = matches!(entry.opcode, IORING_OP_READ | IORING_OP_READV);
    let original_access = Object::granted_access(pinned.raw())?;
    let access = if read {
        FILE_READ_DATA
    } else {
        FILE_WRITE_DATA | FILE_APPEND_DATA
    };
    if original_access & access == 0 {
        return Err(EBADF);
    }
    let _inode = crate::fs::verity::lock(pinned.raw())?;
    if crate::fs::verity::logical_size(pinned.raw())?.is_some() {
        return Err(
            if !read && crate::fs::verity::descriptor(pinned.raw())?.is_some() {
                crate::EPERM
            } else {
                crate::EOPNOTSUPP
            },
        );
    }
    let append = !read
        && (descriptor.flags.contains(FdFlags::APPEND) || original_access & FILE_WRITE_DATA == 0);
    let data_access = if append {
        FILE_APPEND_DATA
    } else {
        access & original_access
    };
    // Independent FileObject: cancellation cannot affect unrelated operations.
    // Its write access excludes ENABLE for the entire native request lifetime.
    let file = Object::reopen(
        pinned.raw(),
        data_access | (original_access & FILE_READ_ATTRIBUTES),
    )?;
    let targets = memory::segments(entry)?;
    let length = targets.iter().map(|segment| segment.length).sum();
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(length)
        .map_err(|_| crate::ENOMEM)?;
    buffer.resize(length, 0);
    if !read {
        let copied = memory::gather(&targets, &mut buffer)?;
        buffer.truncate(copied);
    }
    Ok(Request {
        file,
        buffer,
        targets,
        offset: if append { u64::MAX } else { entry.offset },
        read,
        user_data: entry.user_data,
        submitted: false,
    })
}

/// Queue one SQE. Bad guest addresses produce an EFAULT CQE, not a host fault.
///
/// # Safety
/// The guest owns the addressed memory. Its contents may change concurrently;
/// callers cannot rely on Rust aliasing guarantees across this syscall boundary.
/// Native requests retain no guest pointers.
pub unsafe fn push(fd: i32, entry: &SubmissionEntry) -> Result<(), i32> {
    let ring = lookup(fd)?;
    let mut state = ring.state.lock().map_err(|_| EIO)?;
    if state.closing {
        return Err(EBADF);
    }
    if state.pending >= state.capacity {
        return Err(crate::EBUSY);
    }
    // Reserve before Build: unwinding an allocation failure after Build must
    // never free a buffer already referenced by the native submission queue.
    state
        .local_pending
        .try_reserve(1)
        .map_err(|_| crate::ENOMEM)?;
    state.requests.try_reserve(1).map_err(|_| crate::ENOMEM)?;
    let prepared = if entry.flags != 0
        || entry.ioprio != 0
        || entry.op_flags != 0
        || entry.buffer_index != 0
        || entry.personality != 0
    {
        Err(crate::EOPNOTSUPP)
    } else if entry.opcode == IORING_OP_NOP {
        Ok(None)
    } else if let Some(opcode) = native_opcode(entry.opcode) {
        if unsafe { IsIoRingOpSupported(state.handle as HIORING, opcode) } == 0 {
            Err(crate::EOPNOTSUPP)
        } else {
            prepare(entry).map(Some)
        }
    } else {
        Err(
            if matches!(entry.opcode, IORING_OP_READ_FIXED | IORING_OP_WRITE_FIXED) {
                crate::EOPNOTSUPP
            } else {
                ENOSYS
            },
        )
    };
    let result = match prepared {
        Ok(Some(request)) => state.queue_native(request).err().map(|error| -error),
        Ok(_) => Some(0),
        Err(error) => Some(-error),
    };
    if let Some(result) = result {
        state.local_pending.push_back(CompletionEntry {
            user_data: entry.user_data,
            result,
            flags: 0,
        });
    }
    state.pending += 1;
    Ok(())
}

/// Import an ABI SQE without dereferencing an untrusted guest pointer.
///
/// # Safety
/// Guest memory may be modified concurrently; callers must not rely on Rust
/// aliasing guarantees across this syscall boundary. Inaccessible memory is an
/// EFAULT error and does not queue even a partially copied submission.
pub unsafe fn push_guest(fd: i32, address: usize) -> Result<(), i32> {
    let mut entry = SubmissionEntry::default();
    let length = std::mem::size_of::<SubmissionEntry>();
    if memory::copy(address, &mut entry as *mut SubmissionEntry as usize, length)? != length {
        return Err(crate::EFAULT);
    }
    unsafe { push(fd, &entry) }
}

impl Request {
    fn complete(&self, completion: &IORING_CQE) -> Result<usize, i32> {
        if completion.ResultCode < 0 {
            let code = completion.ResultCode as u32;
            if code == (0x8007_0000 | ERROR_HANDLE_EOF) {
                return Ok(0);
            }
            if code == (0x8007_0000 | ERROR_OPERATION_ABORTED) {
                return Err(125); /* ECANCELED */
            }
            hresult(completion.ResultCode)?;
        }
        let count = completion.Information as usize;
        if count > self.buffer.len() {
            return Err(EIO);
        }
        if !self.read {
            return Ok(count);
        }
        let _inode = crate::fs::verity::lock(self.file.raw())?;
        if crate::fs::verity::logical_size(self.file.raw())?.is_some() {
            // An ordinary read may have been queued before ENABLE. Never
            // publish bytes read before the integrity policy was committed.
            return Err(crate::EOPNOTSUPP);
        }
        // Failed ENABLE can append then remove its marker. Clip to the current
        // logical EOF, holding the same inode mutex through the ENTIRE scatter.
        let size = crate::fs::verity::authoritative_size(self.file.raw())?;
        let count = count.min(size.saturating_sub(self.offset).min(usize::MAX as u64) as usize);
        memory::scatter(&self.buffer[..count], &self.targets)
    }
}
impl State {
    fn shutdown(&mut self, completion_event: &Event) {
        if self.handle == 0 {
            return;
        }
        self.closing = true;
        for request in self.requests.values().filter(|request| request.submitted) {
            unsafe { CancelIoEx(request.file.raw(), std::ptr::null()) };
        }
        loop {
            // CloseIoRing does not retire submitted buffers. A corrupt queue
            // cannot justify freeing memory still potentially owned by kernel.
            if self.harvest(completion_event, false).is_err() {
                std::process::abort();
            }
            if !self.requests.values().any(|request| request.submitted) {
                break;
            }
            let event = completion_event.raw();
            // Cancellation retirement cannot deliver guest handlers midway.
            if unsafe { WaitForMultipleObjects(1, &event, 0, INFINITE) } != WAIT_OBJECT_0 {
                std::process::abort();
            }
        }
        // Closing discards unsubmitted native entries; release them afterwards.
        if unsafe { CloseIoRing(self.handle as HIORING) } < 0 {
            std::process::abort();
        }
        self.handle = 0;
        self.requests.clear();
        self.local_pending.clear();
        self.completions.clear();
    }
    fn queue_native(&mut self, mut request: Request) -> Result<(), i32> {
        if request.buffer.is_empty() {
            self.local_pending.push_back(CompletionEntry {
                user_data: request.user_data,
                result: 0,
                flags: 0,
            });
            return Ok(());
        }
        let first = self.next_cookie;
        while self.requests.contains_key(&self.next_cookie) {
            self.next_cookie = self.next_cookie.wrapping_add(1);
            if self.next_cookie == first {
                return Err(crate::EBUSY);
            }
        }
        let cookie = self.next_cookie;
        self.next_cookie = cookie.wrapping_add(1);
        let file = IORING_HANDLE_REF {
            Kind: IORING_REF_RAW,
            Handle: IORING_HANDLE_REF_0 {
                Handle: request.file.raw(),
            },
        };
        let buffer = IORING_BUFFER_REF {
            Kind: IORING_REF_RAW,
            Buffer: IORING_BUFFER_REF_0 {
                Address: request.buffer.as_mut_ptr().cast(),
            },
        };
        let native = unsafe {
            if request.read {
                BuildIoRingReadFile(
                    self.handle as HIORING,
                    file,
                    buffer,
                    request.buffer.len() as u32,
                    request.offset,
                    cookie,
                    0,
                )
            } else {
                BuildIoRingWriteFile(
                    self.handle as HIORING,
                    file,
                    buffer,
                    request.buffer.len() as u32,
                    request.offset,
                    0,
                    cookie,
                    0,
                )
            }
        };
        hresult(native)?;
        self.requests.insert(cookie, request);
        Ok(())
    }
    fn notify(&mut self) {
        self.waiters.retain(|weak| {
            if let Some(event) = weak.upgrade() {
                event.signal();
                true
            } else {
                false
            }
        });
    }
    fn harvest(&mut self, event: &Event, publish: bool) -> Result<(), i32> {
        // Reset BEFORE draining, never after the final empty Pop (lost wakeup).
        unsafe { ResetEvent(event.raw()) };
        let mut changed = false;
        loop {
            if publish {
                self.completions.try_reserve(1).map_err(|_| crate::ENOMEM)?;
            }
            let mut completion: IORING_CQE = unsafe { std::mem::zeroed() };
            match unsafe { PopIoRingCompletion(self.handle as HIORING, &mut completion) } {
                1 => break,
                0 => {}
                result => {
                    hresult(result)?;
                    return Err(EIO);
                }
            }
            let Some(request) = self.requests.remove(&completion.UserData) else {
                std::process::abort();
            };
            if !request.submitted {
                std::process::abort();
            }
            if publish {
                let result = request
                    .complete(&completion)
                    .map(|count| count as i32)
                    .unwrap_or_else(|error| -error);
                self.completions.push_back(CompletionEntry {
                    user_data: request.user_data,
                    result,
                    flags: 0,
                });
                changed = true;
            }
            // Only the original CQE releases its buffer and native write lease.
        }
        if changed {
            self.notify();
        }
        Ok(())
    }
    fn submit(&mut self) -> Result<u32, i32> {
        self.completions
            .try_reserve(self.local_pending.len())
            .map_err(|_| crate::ENOMEM)?;
        if self.pending == 0 {
            return Ok(0);
        }
        let expected = self
            .requests
            .values()
            .filter(|request| !request.submitted)
            .count();
        let mut submitted = 0;
        hresult(unsafe { SubmitIoRing(self.handle as HIORING, 0, 0, &mut submitted) })?;
        // Microsoft's S_OK contract submits every built entry. A mismatch
        // leaves kernel ownership unknowable, not safely recoverable as EIO.
        if submitted as usize != expected {
            std::process::abort();
        }
        // On failure ALL native entries remain queued. Preserve their ownership
        // and counters; local CQEs also become visible only after submission.
        for request in self.requests.values_mut() {
            request.submitted = true;
        }
        self.completions.append(&mut self.local_pending);
        let consumed = std::mem::take(&mut self.pending);
        self.notify();
        Ok(consumed)
    }
}
impl Ring {
    fn shutdown(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.handle == 0 {
            return;
        }
        state.closing = true;
        self.closed_event.signal();
        state.shutdown(&self.completion_event);
    }
}
impl Drop for Ring {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Submit guest SQEs, optionally waiting for at least `wait_completions` ready
/// CQEs. Timeout leaves native requests owned by the ring, not abandoned.
pub fn enter(fd: i32, wait_completions: u32, timeout_ms: Option<u32>) -> Result<u32, i32> {
    enter_policy(fd, None, wait_completions, timeout_ms)
}

/// Guest enter supports full native batches and wait-only calls. Windows' built
/// queue has no prefix-submit operation: explicitly reject a nonzero partial
/// request without consuming any SQEs instead of silently submitting too many.
pub fn enter_guest(fd: i32, to_submit: u32, min_complete: u32, flags: u32) -> Result<u32, i32> {
    if flags & !IORING_ENTER_GETEVENTS != 0 {
        return Err(EINVAL);
    }
    let wait = if flags & IORING_ENTER_GETEVENTS != 0 {
        min_complete
    } else {
        0
    };
    enter_policy(fd, Some(to_submit), wait, None)
}

fn enter_policy(
    fd: i32,
    to_submit: Option<u32>,
    wait_completions: u32,
    timeout_ms: Option<u32>,
) -> Result<u32, i32> {
    let mut interrupted = false;
    let result = enter_owned(
        fd,
        to_submit,
        wait_completions,
        timeout_ms,
        &mut interrupted,
    );
    if interrupted || result == Err(crate::EINTR) {
        // The C ABI only translates errno. Deliver here, after enter_owned's
        // transient Arcs, wait events and locks have ALL unwound. Native SQEs
        // remain registry-owned; an interrupted wait does not cancel them.
        signal::deliver_pending();
    }
    result
}

fn enter_owned(
    fd: i32,
    to_submit: Option<u32>,
    wait_completions: u32,
    timeout_ms: Option<u32>,
    interrupted: &mut bool,
) -> Result<u32, i32> {
    let ring = lookup(fd)?;
    let mut state = ring.state.lock().map_err(|_| EIO)?;
    if state.closing {
        return Err(EBADF);
    }
    let submitted = match to_submit {
        Some(0) => 0,
        Some(limit) if limit < state.pending => return Err(crate::EOPNOTSUPP),
        _ => state.submit()?,
    };
    state.harvest(&ring.completion_event, true)?;
    if wait_completions == 0 || state.completions.len() >= wait_completions as usize {
        return Ok(submitted);
    }
    let interrupt_event = interrupt::current();
    if interrupt_event.is_null() {
        return Err(EIO);
    }
    let notification = Arc::new(Event::new(false)?);
    // Idle rings with repeated timed waits otherwise accumulate expired weak
    // registrations until a future completion happens to prune them.
    state.waiters.retain(|waiter| waiter.strong_count() != 0);
    state.waiters.try_reserve(1).map_err(|_| crate::ENOMEM)?;
    state.waiters.push(Arc::downgrade(&notification));
    drop(state);
    let started = Instant::now();
    loop {
        let timeout = timeout_ms
            .map(|limit| {
                limit.saturating_sub(started.elapsed().as_millis().min(u32::MAX as u128) as u32)
            })
            .unwrap_or(INFINITE);
        let handles = [
            ring.closed_event.raw(),
            ring.completion_event.raw(),
            notification.raw(),
            interrupt_event,
        ];
        signal::register_waiter();
        let pending = signal::pending() & !signal::blocked_mask() != 0;
        let waited = if pending {
            WAIT_OBJECT_0 + 3
        } else {
            unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout) }
        };
        signal::unregister_waiter();
        // Deliver/restart at the syscall boundary after transient Arcs unwind.
        if waited == WAIT_OBJECT_0 + 3 {
            *interrupted = true;
            // Linux keeps io_submit_sqes' positive count when the following
            // CQ wait is interrupted; only a wait-only enter returns EINTR.
            return if submitted == 0 {
                Err(crate::EINTR)
            } else {
                Ok(submitted)
            };
        }
        let mut state = ring.state.lock().map_err(|_| EIO)?;
        if state.closing {
            return Err(EBADF);
        }
        state.harvest(&ring.completion_event, true)?;
        if state.completions.len() >= wait_completions as usize {
            return Ok(submitted);
        }
        if waited == WAIT_TIMEOUT {
            return Ok(submitted);
        }
        if waited != WAIT_OBJECT_0 + 1 && waited != WAIT_OBJECT_0 + 2 {
            return Err(EIO);
        }
    }
}
/// Drain the native CQ completely before limiting output: its event only
/// signals native empty-to-nonempty transitions.
pub fn reap(fd: i32, out: &mut [CompletionEntry]) -> Result<usize, i32> {
    if out.is_empty() {
        return Ok(0);
    }
    let ring = lookup(fd)?;
    let mut state = ring.state.lock().map_err(|_| EIO)?;
    if state.closing {
        return Err(EBADF);
    }
    state.harvest(&ring.completion_event, true)?;
    let count = out.len().min(state.completions.len());
    for slot in &mut out[..count] {
        *slot = state.completions.pop_front().ok_or(EIO)?;
    }
    if count == 0 { Err(EAGAIN) } else { Ok(count) }
}

/// Export CQEs through a fault-contained ABI copy. A CQE is consumed only after
/// every byte of that entry reached guest memory. A fault after complete CQEs
/// returns that prefix count; a fault in the first CQE leaves it queued/EFAULT.
///
/// # Safety
/// Guest output must not alias Rust-owned queue state. Inaccessible, readonly
/// or concurrently unmapped guest destinations are handled as EFAULT, without
/// constructing Rust references or slices into that memory.
pub unsafe fn reap_guest(fd: i32, address: usize, capacity: u32) -> Result<usize, i32> {
    // Keep the existing C ABI's zero-count/null-output convention. Its signed
    // return count must also remain representable for every accepted capacity.
    if address == 0 || capacity == 0 || capacity > i32::MAX as u32 {
        return Err(EINVAL);
    }
    let entry_size = std::mem::size_of::<CompletionEntry>();
    let range = (capacity as usize).checked_mul(entry_size).ok_or(EINVAL)?;
    address.checked_add(range).ok_or(crate::EFAULT)?;
    let ring = lookup(fd)?;
    let mut state = ring.state.lock().map_err(|_| EIO)?;
    if state.closing {
        return Err(EBADF);
    }
    state.harvest(&ring.completion_event, true)?;
    let count = (capacity as usize).min(state.completions.len());
    if count == 0 {
        return Err(EAGAIN);
    }
    // This contiguous slice belongs to us, never to the guest. A normal valid
    // batch needs one native copy; fault probing preserves complete prefixes.
    let source = state.completions.make_contiguous().as_ptr() as usize;
    let copied = memory::copy(source, address, count * entry_size)?;
    let published = copied / entry_size;
    if published == 0 {
        return Err(crate::EFAULT);
    }
    state.completions.drain(..published);
    Ok(published)
}
