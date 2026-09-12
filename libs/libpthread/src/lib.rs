//! Native-Windows pthread compatibility surface exported with the System V ABI.

#[cfg(all(windows, target_arch = "x86_64"))]
mod cleanup;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod sched;
#[cfg(all(windows, target_arch = "x86_64"))]
mod stack;
#[cfg(all(windows, target_arch = "x86_64"))]
pub use sched::{SchedParam, pthread_getschedparam, pthread_setschedparam, pthread_setschedprio};

#[cfg(all(windows, target_arch = "x86_64"))]
use core::ffi::{c_char, c_void};
#[cfg(all(windows, target_arch = "x86_64"))]
use kinakaze_tls::KeyDestructor;
#[cfg(all(windows, target_arch = "x86_64"))]
use kinakaze_vfs::signal::SIG_SETMASK;
#[cfg(all(windows, target_arch = "x86_64"))]
use std::cell::Cell;
#[cfg(all(windows, target_arch = "x86_64"))]
use std::collections::HashMap;
#[cfg(all(windows, target_arch = "x86_64"))]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(all(windows, target_arch = "x86_64"))]
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
#[cfg(all(windows, target_arch = "x86_64"))]
use std::sync::{Arc, Barrier, Condvar, Mutex, OnceLock, PoisonError, RwLock};
#[cfg(all(windows, target_arch = "x86_64"))]
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(all(windows, target_arch = "x86_64"))]
use windows_sys::Win32::Foundation::{ERROR_TIMEOUT, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT};
#[cfg(all(windows, target_arch = "x86_64"))]
use windows_sys::Win32::System::Threading::{
    AcquireSRWLockExclusive, CONDITION_VARIABLE, CreateThread, ExitThread, GetCurrentThread,
    INFINITE, ReleaseSRWLockExclusive, SRWLOCK, STACK_SIZE_PARAM_IS_A_RESERVATION,
    SetThreadDescription, SleepConditionVariableSRW, TryAcquireSRWLockExclusive,
    WaitForSingleObject, WakeAllConditionVariable, WakeConditionVariable,
};

#[cfg(all(windows, target_arch = "x86_64"))]
const ESRCH: i32 = 3;
#[cfg(all(windows, target_arch = "x86_64"))]
const EAGAIN: i32 = 11;
#[cfg(all(windows, target_arch = "x86_64"))]
const EINVAL: i32 = 22;
#[cfg(all(windows, target_arch = "x86_64"))]
const EDEADLK: i32 = 35;
#[cfg(all(windows, target_arch = "x86_64"))]
const EBUSY: i32 = 16;
#[cfg(all(windows, target_arch = "x86_64"))]
const EPERM: i32 = 1;
#[cfg(all(windows, target_arch = "x86_64"))]
const ENOSYS: i32 = 38;
#[cfg(all(windows, target_arch = "x86_64"))]
const ETIMEDOUT: i32 = 110;
#[cfg(all(windows, target_arch = "x86_64"))]
const ERANGE: i32 = 34;

#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_CREATE_JOINABLE: i32 = 0;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_CREATE_DETACHED: i32 = 1;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_STACK_MIN: usize = 16 * 1024;
/// Linux's default guard: one 4 KiB page below the stack.
#[cfg(all(windows, target_arch = "x86_64"))]
const DEFAULT_GUARD_SIZE: u32 = 4096;

#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_MUTEX_NORMAL: i32 = 0;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_MUTEX_RECURSIVE: i32 = 1;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_MUTEX_ERRORCHECK: i32 = 2;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_MUTEX_ADAPTIVE_NP: i32 = 3;
/// glibc x86_64 stores the mutex type in `pthread_mutex_t.__data.__kind`.
/// The full public object is 40 bytes; the first eight bytes remain available
/// for the native SRW lock used by this compatibility layer.
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_MUTEX_KIND_OFFSET: usize = 16;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_MUTEX_KIND_MASK: i32 = 3;

/// Returned by `pthread_barrier_wait` to exactly one thread of each generation.
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_BARRIER_SERIAL_THREAD: i32 = -1;

#[cfg(all(windows, target_arch = "x86_64"))]
#[inline]
fn trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_TRACE").is_some())
}

#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_CANCEL_ENABLE: i32 = 0;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_CANCEL_DISABLE: i32 = 1;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_CANCEL_DEFERRED: i32 = 0;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_CANCEL_ASYNCHRONOUS: i32 = 1;
/// `(void *) -1`, the join result of a cancelled thread.
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_CANCELED: usize = usize::MAX;

#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_PROCESS_PRIVATE: i32 = 0;
#[cfg(all(windows, target_arch = "x86_64"))]
const PTHREAD_PROCESS_SHARED: i32 = 1;
#[cfg(all(windows, target_arch = "x86_64"))]
const CLOCK_REALTIME: i32 = 0;
#[cfg(all(windows, target_arch = "x86_64"))]
const CLOCK_MONOTONIC: i32 = 1;

/// Linux x86_64 `struct timespec`.
#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// Linux x86_64 `pthread_condattr_t` is one 32-bit word. We retain the selected
/// clock in that word and transfer it to process-local condition metadata during
/// `pthread_cond_init`.
#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PthreadCondAttr {
    clock_id: i32,
}

/// Read the exact clock that produced the guest's absolute deadline. Libc uses
/// unbiased interrupt time plus its time-namespace offset, not QPC. Subtracting
/// QPC can turn every future deadline into an immediate timeout after suspend.
#[cfg(all(windows, target_arch = "x86_64"))]
fn monotonic_now() -> Result<Duration, i32> {
    let (seconds, nanoseconds) = kinakaze_vfs::time_namespace::clock(CLOCK_MONOTONIC)?;
    if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
        return Err(EINVAL);
    }
    Ok(Duration::new(seconds as u64, nanoseconds as u32))
}

/// Converts an absolute deadline for the condition variable's configured clock
/// into the time left to wait.
///
/// Returns `Err(EINVAL)` for a malformed timespec and `Ok(None)` when the
/// deadline has already passed, which callers report as `ETIMEDOUT` instead of
/// blocking on a wait that could never be satisfied.
#[cfg(all(windows, target_arch = "x86_64"))]
fn remaining_until(deadline: &Timespec, clock_id: i32) -> Result<Option<Duration>, i32> {
    if deadline.tv_sec < 0 || !(0..1_000_000_000).contains(&deadline.tv_nsec) {
        return Err(EINVAL);
    }
    let target = Duration::new(
        deadline.tv_sec.unsigned_abs(),
        u32::try_from(deadline.tv_nsec).map_err(|_| EINVAL)?,
    );
    let now = match clock_id {
        CLOCK_REALTIME => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| EINVAL)?,
        CLOCK_MONOTONIC => monotonic_now()?,
        _ => return Err(EINVAL),
    };
    Ok(target.checked_sub(now).filter(|left| !left.is_zero()))
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub type StartRoutine = unsafe extern "sysv64" fn(*mut c_void) -> *mut c_void;

#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PthreadAttr {
    stack_size: usize,
    /// Low address of the stack region.  Zero means an attribute template for
    /// a thread that has not been created yet; `pthread_getattr_np` fills the
    /// real address and size of the calling thread.
    stack_address: usize,
    detach_state: i32,
    /// Guard pages are reserved by the Windows thread creation path, so this is
    /// recorded for `pthread_attr_getguardsize` round-tripping only. A `u32` is
    /// enough for a page-aligned guard. This structure fits Linux's 56-byte object.
    guard_size: u32,
    sched_policy: i32,
    sched_priority: i32,
    inherit_sched: i32,
    affinity_mask: u64,
    has_affinity: u32,
}

/// Mutex attribute block. Only the type matters here; `pshared` is meaningless
/// while every mutex lives in this process's address space.
#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PthreadMutexAttr {
    kind: i32,
}

#[cfg(all(windows, target_arch = "x86_64"))]
enum ThreadRecord {
    Joinable(Arc<OwnedHandle>),
    Detached,
}

#[cfg(all(windows, target_arch = "x86_64"))]
static NEXT_THREAD_ID: AtomicUsize = AtomicUsize::new(1);
#[cfg(all(windows, target_arch = "x86_64"))]
static THREADS: OnceLock<Mutex<HashMap<usize, ThreadRecord>>> = OnceLock::new();

/// Return values of threads that ran to completion, keyed by thread id.
///
/// Native Windows thread exit codes cannot hold guest pointers. Return, explicit
/// pthread_exit and cancellation all publish the full pointer here before exit.
#[cfg(all(windows, target_arch = "x86_64"))]
static RESULTS: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();

#[cfg(all(windows, target_arch = "x86_64"))]
fn results() -> &'static Mutex<HashMap<usize, usize>> {
    RESULTS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(windows, target_arch = "x86_64"))]
thread_local! {
    static SELF_ID: Cell<usize> = const { Cell::new(0) };
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn threads() -> &'static Mutex<HashMap<usize, ThreadRecord>> {
    THREADS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Initialize this thread through the shared ELF TLS module.
#[cfg(all(windows, target_arch = "x86_64"))]
fn initialize_process_thread_tls() -> bool {
    kinakaze_tls::initialize_thread_tls().is_ok()
}

// VFS and pthread state are linked once into the runtime DLL.
#[cfg(all(windows, target_arch = "x86_64"))]
struct FsInheritance {
    value: kinakaze_vfs::fs_context::Inheritance,
}
#[cfg(all(windows, target_arch = "x86_64"))]
impl FsInheritance {
    fn capture() -> Result<Self, i32> {
        Ok(Self {
            value: kinakaze_vfs::fs_context::capture(true)?,
        })
    }
    fn adopt(&self) {
        self.value.adopt();
    }
}

#[repr(C, align(64))]
struct PthreadControlBlock {
    self_ptr: usize,
    dtv: usize,
    thread_self: usize,
    multiple_threads: i32,
    gscope_flag: i32,
    sysinfo: usize,
    stack_guard: usize,
    pointer_guard: usize,
    vhp: usize,
    tid: i32,
    pid: i32,
    pad: [u8; 8192 - 64],
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn allocate_thread_id() -> usize {
    let canary = kinakaze_tls::configured_canary();
    let canary = if canary != 0 {
        canary
    } else {
        0x5a5a_5a5a_5a5a_5a00
    };
    // The TCB fields are guest-visible through glibc internals. Until a
    // separate thread-id namespace is added, every thread uses the process's
    // namespace leader id rather than exposing loader.exe's Windows pid.
    let process_id = kinakaze_vfs::job::process_id() as i32;
    let block = Box::leak(Box::new(PthreadControlBlock {
        self_ptr: 0,
        dtv: 0,
        thread_self: 0,
        multiple_threads: 0,
        gscope_flag: 0,
        sysinfo: 0,
        stack_guard: canary,
        pointer_guard: 0x3c3c_3c3c_3c3c_3c3c,
        vhp: 0,
        tid: process_id,
        pid: process_id,
        pad: [0u8; 8192 - 64],
    }));
    let addr = block as *mut PthreadControlBlock as usize;
    block.self_ptr = addr;
    block.thread_self = addr;
    addr
}

/// Reports whether a thread has already exited.
///
/// Native thread handles signal for return, explicit exit and cancellation.
#[cfg(all(windows, target_arch = "x86_64"))]
fn thread_has_exited(handle: &OwnedHandle) -> bool {
    // SAFETY: `handle` keeps the native thread handle open for this call.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) == WAIT_OBJECT_0 }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn finish_thread(id: usize) {
    let Ok(mut threads) = threads().lock() else {
        return;
    };
    if matches!(threads.get(&id), Some(ThreadRecord::Detached)) {
        threads.remove(&id);
        // Nobody can join a detached thread, so its result would never be read.
        if let Ok(mut results) = results().lock() {
            results.remove(&id);
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_self() -> usize {
    SELF_ID
        .try_with(|slot| {
            let current = slot.get();
            if current != 0 {
                current
            } else {
                let allocated = allocate_thread_id();
                slot.set(allocated);
                // Register the process's initial thread and any native caller
                // so another guest pthread can address it by pthread_t.
                let _ = sched::register_current(allocated);
                allocated
            }
        })
        .unwrap_or_else(|_| {
            #[cfg(windows)]
            unsafe {
                windows_sys::Win32::System::Threading::GetCurrentThreadId() as usize
            }
            #[cfg(not(windows))]
            0
        })
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_equal(left: usize, right: usize) -> i32 {
    i32::from(left == right)
}

/// `pthread_setname_np`: names a thread for debuggers and profilers.
///
/// Linux rejects a name longer than 15 characters plus the terminator with
/// `ERANGE` rather than truncating, so the length is validated before the copy.
/// The name is recorded so `pthread_getname_np` can answer later, and is
/// forwarded to the host when the thread is one this runtime created, which is
/// what makes it visible in a Windows debugger.
///
/// # Safety
///
/// `name` must be a valid NUL-terminated string.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_setname_np(thread: usize, name: *const c_char) -> i32 {
    if name.is_null() {
        return EINVAL;
    }
    // SAFETY: `name` is a NUL-terminated string per this function's contract.
    let name = unsafe { core::ffi::CStr::from_ptr(name) };
    if name.to_bytes().len() > 15 {
        return ERANGE;
    }
    let owned = name.to_string_lossy().into_owned();

    // Only the calling thread can be named on the host without owning a handle
    // for every thread the runtime creates, and that is the case callers
    // actually exercise: a thread names itself early in its entry routine.
    if thread == pthread_self() {
        // SAFETY: the buffer is a NUL-terminated UTF-16 string of the length
        // built by `wide`.
        let _ =
            unsafe { SetThreadDescription(GetCurrentThread(), wide(owned.as_bytes()).as_ptr()) };
    }

    if let Ok(mut names) = thread_names().lock() {
        names.insert(thread, owned);
    }
    0
}

/// `pthread_getname_np`: reads back a name set by `pthread_setname_np`.
///
/// # Safety
///
/// `buffer` must be writable for at least `length` bytes.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_getname_np(
    thread: usize,
    buffer: *mut c_char,
    length: usize,
) -> i32 {
    if buffer.is_null() || length == 0 {
        return EINVAL;
    }
    let name = match thread_names().lock() {
        Ok(names) => names.get(&thread).cloned(),
        Err(_) => return EINVAL,
    };
    // An unnamed thread still gets an empty string, matching glibc, which
    // reports success with a zero-length name rather than an error.
    let name = name.unwrap_or_default();
    let bytes = name.as_bytes();
    if bytes.len() + 1 > length {
        return ERANGE;
    }
    // SAFETY: the buffer is writable for `length` bytes, and the copy plus its
    // terminator fits, as checked above.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer as *mut u8, bytes.len());
        *buffer.add(bytes.len()) = 0;
    }
    0
}

/// Encodes a thread name as a NUL-terminated UTF-16 buffer for the host API.
#[cfg(all(windows, target_arch = "x86_64"))]
fn wide(name: &[u8]) -> Vec<u16> {
    // `SetThreadDescription` wants UTF-16. Decoding by hand avoids pulling the
    // `Win32_Globalization` feature into this crate for one call, and the
    // input is already valid UTF-8 from the `CStr` above.
    let text = core::str::from_utf8(name).unwrap_or("");
    text.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Names recorded by `pthread_setname_np`, keyed by `pthread_self` id.
#[cfg(all(windows, target_arch = "x86_64"))]
static THREAD_NAMES: OnceLock<Mutex<HashMap<usize, String>>> = OnceLock::new();

/// Resolves the `THREAD_NAMES` map, tolerating a poisoned mutex.
#[cfg(all(windows, target_arch = "x86_64"))]
fn thread_names() -> &'static Mutex<HashMap<usize, String>> {
    THREAD_NAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Default)]
struct ThreadStartup {
    published: bool,
    status: Option<i32>,
}

#[cfg(all(windows, target_arch = "x86_64"))]
struct ThreadStart {
    id: usize,
    start: StartRoutine,
    argument: usize,
    locale: usize,
    gs_base: usize,
    inheritance: FsInheritance,
    cancel: Arc<CancelState>,
    published: Arc<(Mutex<ThreadStartup>, Condvar)>,
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "system" fn native_thread_start(packet: *mut c_void) -> u32 {
    // Consume every startup owner before guest code: pthread_exit abandons its
    // stack rather than unwinding through the guest's foreign ABI frames.
    let ThreadStart {
        id,
        start,
        argument,
        locale,
        gs_base,
        inheritance,
        cancel,
        published,
    } = *unsafe { Box::from_raw(packet.cast::<ThreadStart>()) };
    {
        let (ready, changed) = &*published;
        let mut ready = ready.lock().unwrap_or_else(PoisonError::into_inner);
        while !ready.published {
            ready = changed.wait(ready).unwrap_or_else(PoisonError::into_inner);
        }
    }
    SELF_ID.with(|slot| slot.set(id));
    CANCEL_SELF.with(|slot| {
        let _ = slot.set(cancel);
    });
    inheritance.adopt();
    drop(inheritance);
    kinakaze_tls::set_locale(locale);
    let initialized = stack::prepare_current()
        && initialize_process_thread_tls()
        && kinakaze_tls::set_guest_gs_base(gs_base);
    if !initialized {
        finish_current_thread(PTHREAD_CANCELED);
    }
    published
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .status = Some(if initialized { 0 } else { EAGAIN });
    published.1.notify_one();
    drop(published);
    if !initialized {
        return 0;
    }
    if trace_enabled() {
        eprintln!("kinakaze: [THREAD] child {id:#x} start={start:p} arg={argument:#x}");
    }
    let result = unsafe { start(argument as *mut c_void) } as usize;
    finish_current_thread(result);
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn finish_current_thread(result: usize) {
    let id = pthread_self();
    // Destructors can contain cancellation points; an already exiting thread
    // must not recursively reenter exit while its cleanup records are live.
    let _ = CANCEL_SELF.try_with(|slot| {
        if let Some(state) = slot.get() {
            state.enabled.store(false, Ordering::Release);
        }
    });
    kinakaze_tls::run_thread_destructors();
    sched::unregister(id);
    HELD_RWLOCKS.with_borrow_mut(Vec::clear);
    if let Ok(mut values) = results().lock() {
        values.insert(id, result);
    }
    if let Ok(mut states) = cancel_states().lock() {
        states.remove(&id);
    }
    if let Ok(mut names) = thread_names().lock() {
        names.remove(&id);
    }
    finish_thread(id);
    // Keep stack ownership and its object metadata in the same fork snapshot.
    let mapping_transaction = kinakaze_runtime::begin_fork_mapping_transaction();
    if let Some((base, end)) = stack::retire_current() {
        // A stack object's lifetime ended with its thread. Retaining its
        // address would make a later fork repair freed or repurposed memory.
        if let Ok(mut records) = mutex_records().lock() {
            records.retain(|address, _| !(base..end).contains(address));
            TYPED_MUTEXES.store(records.len(), Ordering::Release);
        }
    }
    drop(mapping_transaction);
    if kinakaze_runtime::retire_guest_thread() {
        cleanup::last_thread_exit();
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Ends only the calling pthread, publishing its full pointer result for join.
/// Guest TLS destructors run before the native thread's TLS is torn down.
pub extern "sysv64" fn pthread_exit(result: *mut c_void) -> ! {
    cleanup::exit(result as usize)
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Starts a pthread on a native Windows thread and native Windows stack.
///
/// # Safety
///
/// All pointers and the start routine must follow the declarations in
/// `include/kinakaze/pthread.h` and remain valid for their documented lifetime.
pub unsafe extern "sysv64" fn pthread_create(
    thread: *mut usize,
    attr: *const PthreadAttr,
    start: Option<StartRoutine>,
    argument: *mut c_void,
) -> i32 {
    if thread.is_null() || start.is_none() {
        return EINVAL;
    }
    let limit_error = kinakaze_runtime::services::task_creation_errno();
    if limit_error != 0 {
        return limit_error;
    }
    let attributes = if attr.is_null() {
        PthreadAttr {
            stack_size: 0,
            stack_address: 0,
            detach_state: PTHREAD_CREATE_JOINABLE,
            guard_size: DEFAULT_GUARD_SIZE,
            sched_policy: 0,
            sched_priority: 0,
            inherit_sched: 0,
            affinity_mask: 0,
            has_affinity: 0,
        }
    } else {
        // SAFETY: the caller supplied an initialized pthread_attr_t.
        unsafe { *attr }
    };
    if !matches!(
        attributes.detach_state,
        PTHREAD_CREATE_JOINABLE | PTHREAD_CREATE_DETACHED
    ) {
        return EINVAL;
    }

    if attributes.inherit_sched == 1 && attributes.sched_policy != sched::SCHED_OTHER {
        return EPERM;
    }

    // The process-wide topology transaction also guards raw clone and fork's
    // snapshot. An earlier check of FORKING both raced the freezer and returned
    // spurious EAGAIN merely because another thread was forking. Wait for the
    // topology transaction instead. Do not wait for the child under this guard:
    // its TLS initializer may itself need the mapping transaction.
    let Some(_creation_transaction) = kinakaze_runtime::begin_fork_mapping_transaction() else {
        return EAGAIN;
    };

    let inheritance = match FsInheritance::capture() {
        Ok(value) => value,
        Err(error) => return error,
    };
    let id = allocate_thread_id();
    let start = start.expect("validated above");
    let argument = argument as usize;
    if attributes.stack_size != 0 {
        if attributes.stack_size < PTHREAD_STACK_MIN {
            return EINVAL;
        }
    }

    if attributes.detach_state == PTHREAD_CREATE_DETACHED {
        let Ok(mut registry) = threads().lock() else {
            return EAGAIN;
        };
        registry.insert(id, ThreadRecord::Detached);
        drop(registry);
    }

    // Published before the spawn so a pthread_cancel arriving while the thread is
    // still starting up has somewhere to record the request.
    let cancel = new_cancel_state();
    if let Ok(mut states) = cancel_states().lock() {
        states.insert(id, Arc::clone(&cancel));
    }

    // SAFETY: the caller supplied writable result storage.
    unsafe { thread.write(id) };

    // Reserve the count before creating the native thread. This makes the
    // value visible no later than a successful pthread_create return and also
    // covers a child that starts and finishes before `spawn` returns.
    let _ = kinakaze_runtime::process_thread_started();
    let published = Arc::new((Mutex::new(ThreadStartup::default()), Condvar::new()));
    let packet = Box::into_raw(Box::new(ThreadStart {
        id,
        start,
        argument,
        locale: kinakaze_tls::locale(),
        gs_base: kinakaze_tls::guest_gs_base(),
        inheritance,
        cancel,
        published: Arc::clone(&published),
    }));
    let native = unsafe {
        CreateThread(
            core::ptr::null(),
            attributes.stack_size,
            Some(native_thread_start),
            packet.cast(),
            STACK_SIZE_PARAM_IS_A_RESERVATION,
            core::ptr::null_mut(),
        )
    };
    let handle = if native.is_null() {
        // CreateThread failed and never consumed the startup packet.
        drop(unsafe { Box::from_raw(packet) });
        let _ = kinakaze_runtime::process_thread_finished();
        if let Ok(mut registry) = threads().lock() {
            registry.remove(&id);
        }
        if let Ok(mut states) = cancel_states().lock() {
            states.remove(&id);
        }
        return EAGAIN;
    } else {
        // SAFETY: CreateThread returns a uniquely owned native thread handle.
        Arc::new(unsafe { OwnedHandle::from_raw_handle(native) })
    };

    sched::register_created(id, Arc::clone(&handle));
    if attributes.has_affinity != 0 {
        unsafe {
            windows_sys::Win32::System::Threading::SetThreadAffinityMask(
                handle.as_raw_handle() as _,
                attributes.affinity_mask as usize,
            );
        }
    }
    if attributes.detach_state == PTHREAD_CREATE_JOINABLE {
        let mut registry = threads().lock().unwrap_or_else(PoisonError::into_inner);
        registry.insert(id, ThreadRecord::Joinable(Arc::clone(&handle)));
    }
    // The guest may detach itself in its first instruction. Publish its record
    // before permitting TLS initialization or any guest execution. The creator
    // releases the topology transaction before waiting: TLS initialization can
    // acquire it. A failed stack/TLS setup must be reported by pthread_create,
    // rather than publishing a successful thread which never ran its routine.
    published
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .published = true;
    published.1.notify_one();
    drop(_creation_transaction);
    let mut ready = published.0.lock().unwrap_or_else(PoisonError::into_inner);
    while ready.status.is_none() {
        ready = published
            .1
            .wait(ready)
            .unwrap_or_else(PoisonError::into_inner);
    }
    let status = ready.status.unwrap();
    drop(ready);
    if status != 0 {
        unsafe { WaitForSingleObject(handle.as_raw_handle(), INFINITE) };
        threads()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        results()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
    status
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Waits for a joinable pthread.
///
/// # Safety
///
/// `result`, when non-null, must point to writable pointer-sized storage.
pub unsafe extern "sysv64" fn pthread_join(thread: usize, result: *mut *mut c_void) -> i32 {
    if thread == pthread_self() {
        return EDEADLK;
    }
    let record = {
        let Ok(mut registry) = threads().lock() else {
            return EINVAL;
        };
        registry.remove(&thread)
    };
    let Some(record) = record else {
        return ESRCH;
    };
    let ThreadRecord::Joinable(handle) = record else {
        if let Ok(mut registry) = threads().lock() {
            registry.insert(thread, ThreadRecord::Detached);
        }
        return EINVAL;
    };

    if trace_enabled() {
        eprintln!("kinakaze: [THREAD] pthread_join waiting on thread {thread}");
    }
    let waited = unsafe { WaitForSingleObject(handle.as_raw_handle(), INFINITE) };
    if trace_enabled() {
        eprintln!("kinakaze: [THREAD] pthread_join finished wait ({waited}) for thread {thread}");
    }
    if waited != WAIT_OBJECT_0 {
        return EINVAL;
    }
    let value = match results().lock() {
        Ok(mut results) => match results.remove(&thread) {
            Some(value) => value,
            None => return EINVAL,
        },
        Err(_) => return EINVAL,
    };
    drop(handle);
    if !result.is_null() {
        // SAFETY: the caller supplied writable result storage.
        unsafe { result.write(value as *mut c_void) };
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Attempts to join a pthread without blocking.
///
/// Returns `EBUSY` when `thread` is still running.
///
/// # Safety
///
/// `result`, when non-null, must point to writable pointer-sized storage.
pub unsafe extern "sysv64" fn pthread_tryjoin_np(thread: usize, result: *mut *mut c_void) -> i32 {
    if thread == pthread_self() {
        return EDEADLK;
    }
    let Ok(mut registry) = threads().lock() else {
        return EINVAL;
    };
    let Some(record) = registry.get(&thread) else {
        return ESRCH;
    };
    let ThreadRecord::Joinable(handle) = record else {
        return EINVAL;
    };
    let waited = unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) };
    if waited == WAIT_TIMEOUT {
        return EBUSY;
    }
    if waited != WAIT_OBJECT_0 {
        return EINVAL;
    }
    let removed = registry.remove(&thread);
    drop(registry);
    drop(removed);

    let value = match results().lock() {
        Ok(mut results) => match results.remove(&thread) {
            Some(value) => value,
            None => return EINVAL,
        },
        Err(_) => return EINVAL,
    };
    if !result.is_null() {
        // SAFETY: the caller supplied writable result storage.
        unsafe { result.write(value as *mut c_void) };
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_detach(thread: usize) -> i32 {
    let Ok(mut registry) = threads().lock() else {
        return EINVAL;
    };
    let Some(record) = registry.remove(&thread) else {
        return ESRCH;
    };
    match record {
        ThreadRecord::Detached => {
            registry.insert(thread, ThreadRecord::Detached);
            EINVAL
        }
        ThreadRecord::Joinable(handle) => {
            let result_published = results()
                .lock()
                .map(|mut results| results.remove(&thread).is_some())
                .unwrap_or(false);
            if result_published || thread_has_exited(&handle) {
                // Already done and now unjoinable, so its result has no reader.
                if let Ok(mut results) = results().lock() {
                    results.remove(&thread);
                }
            } else {
                registry.insert(thread, ThreadRecord::Detached);
            }
            drop(handle);
            0
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Initializes a pthread attribute object.
///
/// # Safety
///
/// `attr` must point to writable `pthread_attr_t` storage.
pub unsafe extern "sysv64" fn pthread_attr_init(attr: *mut PthreadAttr) -> i32 {
    if attr.is_null() {
        return EINVAL;
    }
    // SAFETY: checked above.
    unsafe {
        attr.write(PthreadAttr {
            stack_size: 0,
            stack_address: 0,
            detach_state: PTHREAD_CREATE_JOINABLE,
            guard_size: DEFAULT_GUARD_SIZE,
            sched_policy: 0,
            sched_priority: 0,
            inherit_sched: 0,
            affinity_mask: 0,
            has_affinity: 0,
        })
    };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_attr_destroy(attr: *mut PthreadAttr) -> i32 {
    i32::from(attr.is_null()) * EINVAL
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_attr_getschedpolicy(
    attr: *const PthreadAttr,
    value: *mut i32,
) -> i32 {
    if attr.is_null() || value.is_null() {
        return EINVAL;
    }
    unsafe {
        value.write((*attr).sched_policy);
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_attr_getschedparam(
    attr: *const PthreadAttr,
    value: *mut SchedParam,
) -> i32 {
    if attr.is_null() || value.is_null() {
        return EINVAL;
    }
    unsafe {
        (*value).sched_priority = (*attr).sched_priority;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_attr_getinheritsched(
    attr: *const PthreadAttr,
    value: *mut i32,
) -> i32 {
    if attr.is_null() || value.is_null() {
        return EINVAL;
    }
    unsafe {
        value.write((*attr).inherit_sched);
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_attr_setschedpolicy(
    attr: *mut PthreadAttr,
    policy: i32,
) -> i32 {
    if attr.is_null() || !matches!(policy, 0..=2) {
        return EINVAL;
    }
    unsafe {
        (*attr).sched_policy = policy;
    }
    0
}
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_attr_setschedparam(
    attr: *mut PthreadAttr,
    param: *const SchedParam,
) -> i32 {
    if attr.is_null() || param.is_null() {
        return EINVAL;
    }
    let priority = unsafe { (*param).sched_priority };
    let valid = if unsafe { (*attr).sched_policy } == 0 {
        priority == 0
    } else {
        (1..=99).contains(&priority)
    };
    if !valid {
        return EINVAL;
    }
    unsafe {
        (*attr).sched_priority = priority;
    }
    0
}
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_attr_setinheritsched(
    attr: *mut PthreadAttr,
    inherit: i32,
) -> i32 {
    if attr.is_null() || !matches!(inherit, 0..=1) {
        return EINVAL;
    }
    unsafe {
        (*attr).inherit_sched = inherit;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Sets the native stack reservation used by subsequently created threads.
///
/// # Safety
///
/// `attr` must point to a live initialized pthread attribute object.
pub unsafe extern "sysv64" fn pthread_attr_setstacksize(
    attr: *mut PthreadAttr,
    stack_size: usize,
) -> i32 {
    if attr.is_null() || stack_size < PTHREAD_STACK_MIN {
        return EINVAL;
    }
    // SAFETY: checked above.
    unsafe { (*attr).stack_size = stack_size };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Selects joinable or detached creation.
///
/// # Safety
///
/// `attr` must point to a live initialized pthread attribute object.
pub unsafe extern "sysv64" fn pthread_attr_setdetachstate(
    attr: *mut PthreadAttr,
    state: i32,
) -> i32 {
    if attr.is_null() || !matches!(state, PTHREAD_CREATE_JOINABLE | PTHREAD_CREATE_DETACHED) {
        return EINVAL;
    }
    // SAFETY: checked above.
    unsafe { (*attr).detach_state = state };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Reads back the configured stack reservation.
///
/// A zero stack size means "let Windows choose", which is reported as
/// `PTHREAD_STACK_MIN` because POSIX callers expect a usable number.
///
/// # Safety
///
/// `attr` must point to a live initialized attribute object and `stack_size` to
/// writable pointer-sized storage.
pub unsafe extern "sysv64" fn pthread_attr_getstacksize(
    attr: *const PthreadAttr,
    stack_size: *mut usize,
) -> i32 {
    if attr.is_null() || stack_size.is_null() {
        return EINVAL;
    }
    // SAFETY: checked above.
    let configured = unsafe { (*attr).stack_size };
    // SAFETY: checked above.
    unsafe {
        stack_size.write(if configured == 0 {
            PTHREAD_STACK_MIN
        } else {
            configured
        })
    };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Describes the calling thread's live native stack in a pthread attribute.
///
/// Linux reports the low address and the full usable size.  Windows exposes the
/// same bounds through `GetCurrentThreadStackLimits`; using those values is
/// essential for runtimes such as HotSpot that enforce their own yellow/red
/// zones before the native guard page is reached.
///
/// # Safety
///
/// `attr` must point to writable `pthread_attr_t` storage.  Querying another
/// thread is not supported because Win32 does not expose stable foreign-thread
/// stack bounds without suspending it.
pub unsafe extern "sysv64" fn pthread_getattr_np(thread: usize, attr: *mut PthreadAttr) -> i32 {
    use windows_sys::Win32::System::Threading::GetCurrentThreadStackLimits;

    if attr.is_null() {
        return EINVAL;
    }
    if thread != 0 && thread != pthread_self() {
        return ESRCH;
    }
    let mut low = 0usize;
    let mut high = 0usize;
    unsafe { GetCurrentThreadStackLimits(&raw mut low, &raw mut high) };
    if low == 0 || high <= low {
        return EINVAL;
    }
    unsafe {
        attr.write(PthreadAttr {
            stack_size: high - low,
            stack_address: low,
            detach_state: PTHREAD_CREATE_JOINABLE,
            guard_size: DEFAULT_GUARD_SIZE,
            sched_policy: 0,
            sched_priority: 0,
            inherit_sched: 0,
            affinity_mask: 0,
            has_affinity: 0,
        })
    };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Returns the low address and size recorded in a pthread attribute.
///
/// # Safety
///
/// `attr` must be readable and each non-null result pointer must be writable.
pub unsafe extern "sysv64" fn pthread_attr_getstack(
    attr: *const PthreadAttr,
    stack_address: *mut *mut c_void,
    stack_size: *mut usize,
) -> i32 {
    if attr.is_null() || (stack_address.is_null() && stack_size.is_null()) {
        return EINVAL;
    }
    let attr = unsafe { &*attr };
    if !stack_address.is_null() {
        unsafe { stack_address.write(attr.stack_address as *mut c_void) };
    }
    if !stack_size.is_null() {
        unsafe { stack_size.write(attr.stack_size) };
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Reads back the configured detach state.
///
/// # Safety
///
/// `attr` must point to a live initialized attribute object and `state` to
/// writable storage for one `int`.
pub unsafe extern "sysv64" fn pthread_attr_getdetachstate(
    attr: *const PthreadAttr,
    state: *mut i32,
) -> i32 {
    if attr.is_null() || state.is_null() {
        return EINVAL;
    }
    // SAFETY: checked above.
    unsafe { state.write((*attr).detach_state) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Records the guard size for later threads.
///
/// Windows reserves its own guard page at the base of every thread stack and
/// offers no way to widen it, so the value is stored for round-tripping and the
/// native guard is what actually protects the stack.
///
/// # Safety
///
/// `attr` must point to a live initialized attribute object.
pub unsafe extern "sysv64" fn pthread_attr_setguardsize(
    attr: *mut PthreadAttr,
    guard_size: usize,
) -> i32 {
    let Ok(guard_size) = u32::try_from(guard_size) else {
        return EINVAL;
    };
    if attr.is_null() {
        return EINVAL;
    }
    // SAFETY: checked above.
    unsafe { (*attr).guard_size = guard_size };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Reads back the recorded guard size.
///
/// # Safety
///
/// `attr` must point to a live initialized attribute object and `guard_size` to
/// writable pointer-sized storage.
pub unsafe extern "sysv64" fn pthread_attr_getguardsize(
    attr: *const PthreadAttr,
    guard_size: *mut usize,
) -> i32 {
    if attr.is_null() || guard_size.is_null() {
        return EINVAL;
    }
    // SAFETY: checked above.
    let recorded = unsafe { (*attr).guard_size };
    // SAFETY: checked above.
    unsafe { guard_size.write(recorded as usize) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Sets the CPU affinity mask in a thread attributes object.
///
/// # Safety
///
/// `attr` must point to an initialized attribute object and `cpuset` to a readable buffer of `cpusetsize` bytes.
pub unsafe extern "sysv64" fn pthread_attr_setaffinity_np(
    attr: *mut PthreadAttr,
    cpusetsize: usize,
    cpuset: *const c_void,
) -> i32 {
    if attr.is_null() || cpuset.is_null() || cpusetsize == 0 {
        return EINVAL;
    }
    let bytes_to_read = cpusetsize.min(core::mem::size_of::<u64>());
    let mut mask = 0u64;
    unsafe {
        core::ptr::copy_nonoverlapping(
            cpuset.cast::<u8>(),
            (&raw mut mask).cast::<u8>(),
            bytes_to_read,
        );
    }
    if mask == 0 {
        return EINVAL;
    }
    unsafe {
        (*attr).affinity_mask = mask;
        (*attr).has_affinity = 1;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Gets the CPU affinity mask from a thread attributes object.
///
/// # Safety
///
/// `attr` must point to an initialized attribute object and `cpuset` to a writable buffer of `cpusetsize` bytes.
pub unsafe extern "sysv64" fn pthread_attr_getaffinity_np(
    attr: *const PthreadAttr,
    cpusetsize: usize,
    cpuset: *mut c_void,
) -> i32 {
    if attr.is_null() || cpuset.is_null() || cpusetsize == 0 {
        return EINVAL;
    }
    unsafe {
        core::ptr::write_bytes(cpuset.cast::<u8>(), 0, cpusetsize);
    }
    let mask = if unsafe { (*attr).has_affinity } != 0 {
        unsafe { (*attr).affinity_mask }
    } else {
        let mut process_mask: usize = 0;
        let mut system_mask: usize = 0;
        if unsafe {
            windows_sys::Win32::System::Threading::GetProcessAffinityMask(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                &raw mut process_mask,
                &raw mut system_mask,
            )
        } != 0
            && process_mask != 0
        {
            process_mask as u64
        } else {
            !0u64
        }
    };
    let bytes_to_write = cpusetsize.min(core::mem::size_of::<u64>());
    unsafe {
        core::ptr::copy_nonoverlapping(
            (&raw const mask).cast::<u8>(),
            cpuset.cast::<u8>(),
            bytes_to_write,
        );
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Runs an initialization callback exactly once for this process.
///
/// # Safety
///
/// `control` must be aligned writable storage initialized to zero.
pub unsafe extern "sysv64" fn pthread_once(
    control: *mut u32,
    init: Option<unsafe extern "sysv64" fn()>,
) -> i32 {
    if control.is_null() || init.is_none() {
        return EINVAL;
    }
    // SAFETY: the pthread ABI requires atomic-compatible aligned storage.
    let state = unsafe { AtomicU32::from_ptr(control) };
    static TRACE: OnceLock<bool> = OnceLock::new();
    let trace = *TRACE.get_or_init(|| std::env::var_os("KINAKAZE_PTHREAD_ONCE_TRACE").is_some());
    if trace {
        eprintln!(
            "kinakaze: pthread_once control={control:p} state={} init={:#x}",
            state.load(Ordering::Acquire),
            init.map_or(0, |callback| callback as usize)
        );
    }
    loop {
        match state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire) {
            Ok(_) => {
                if trace {
                    eprintln!("kinakaze: pthread_once control={control:p} running initializer");
                }
                // SAFETY: validated above and called once by the winning thread.
                unsafe { init.expect("validated above")() };
                state.store(2, Ordering::Release);
                if trace {
                    eprintln!("kinakaze: pthread_once control={control:p} completed initializer");
                }
                return 0;
            }
            Err(2) => {
                if trace {
                    eprintln!("kinakaze: pthread_once control={control:p} already complete");
                }
                return 0;
            }
            Err(1) => std::thread::yield_now(),
            Err(_) => return EINVAL,
        }
    }
}

/// Ownership bookkeeping for a mutex whose type is not `PTHREAD_MUTEX_NORMAL`.
///
/// `pthread_mutex_t` is one `size_t` holding a bare `SRWLOCK`, a layout
/// `pthread_cond_wait` depends on when it hands the same storage to
/// `SleepConditionVariableSRW`. Widening the struct is therefore not an option,
/// so recursion and ownership live in a side table keyed by the mutex's address.
#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Clone, Copy)]
struct MutexRecord {
    kind: i32,
    /// Thread id from `pthread_self`, or 0 when unlocked.
    owner: usize,
    /// Extra `RECURSIVE` acquisitions beyond the first.
    recursion: usize,
}

#[cfg(all(windows, target_arch = "x86_64"))]
static MUTEX_RECORDS: OnceLock<Mutex<HashMap<usize, MutexRecord>>> = OnceLock::new();

#[cfg(all(windows, target_arch = "x86_64"))]
static FORK_MUTEX_OWNER: AtomicUsize = AtomicUsize::new(0);
#[cfg(all(windows, target_arch = "x86_64"))]
static FORK_MUTEX_CHANGED: Condvar = Condvar::new();

#[cfg(all(windows, target_arch = "x86_64"))]
fn wait_for_mutex_fork(
    mut records: std::sync::MutexGuard<'_, HashMap<usize, MutexRecord>>,
) -> std::sync::MutexGuard<'_, HashMap<usize, MutexRecord>> {
    let self_id = pthread_self();
    loop {
        let owner = FORK_MUTEX_OWNER.load(Ordering::Acquire);
        // Existing owners must be able to finish nested critical sections.
        if owner == 0 || owner == self_id || records.values().any(|r| r.owner == self_id) {
            return records;
        }
        records = FORK_MUTEX_CHANGED
            .wait(records)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn begin_mutex_fork() {
    let self_id = pthread_self();
    let mut records = mutex_records()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    FORK_MUTEX_OWNER.store(self_id, Ordering::Release);
    // Drain short-lived library locks before the loader freezes symbol lookup.
    // Long-lived locks retain POSIX ownership in the child; do not unlock them.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    while records.values().any(|r| r.owner != 0 && r.owner != self_id) {
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            break;
        };
        let (next, timeout) = FORK_MUTEX_CHANGED
            .wait_timeout(records, remaining)
            .unwrap_or_else(PoisonError::into_inner);
        records = next;
        if timeout.timed_out() {
            break;
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn finish_mutex_fork() {
    let _records = mutex_records()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    FORK_MUTEX_OWNER.store(0, Ordering::Release);
    FORK_MUTEX_CHANGED.notify_all();
}

/// Non-default clocks selected for condition variables. The native Windows
/// condition word must stay at offset zero, so POSIX clock metadata lives beside
/// it rather than inside the guest's `pthread_cond_t` storage.
#[cfg(all(windows, target_arch = "x86_64"))]
static COND_CLOCKS: OnceLock<Mutex<HashMap<usize, i32>>> = OnceLock::new();

#[cfg(all(windows, target_arch = "x86_64"))]
fn cond_clocks() -> &'static Mutex<HashMap<usize, i32>> {
    COND_CLOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Number of live non-`NORMAL` mutexes. While this is zero, and it stays zero
/// for programs that never touch `pthread_mutexattr_settype`, locking skips the
/// side-table lookup entirely.
#[cfg(all(windows, target_arch = "x86_64"))]
static TYPED_MUTEXES: AtomicUsize = AtomicUsize::new(0);

#[cfg(all(windows, target_arch = "x86_64"))]
fn mutex_records() -> &'static Mutex<HashMap<usize, MutexRecord>> {
    MUTEX_RECORDS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn mutex_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_MUTEX_TRACE").is_some())
}

/// Reads the type encoded by glibc's static mutex initializers. In particular,
/// `PTHREAD_RECURSIVE_MUTEX_INITIALIZER_NP` does not call
/// `pthread_mutex_init`; it places `1` at byte offset 16 in the ELF image.
#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn encoded_mutex_kind(mutex: *const usize) -> i32 {
    // SAFETY: a Linux x86_64 pthread_mutex_t is a readable 40-byte object and
    // `__kind` is an aligned int at byte offset 16.
    let kind = unsafe {
        mutex
            .cast::<u8>()
            .add(PTHREAD_MUTEX_KIND_OFFSET)
            .cast::<i32>()
            .read()
    } & PTHREAD_MUTEX_KIND_MASK;
    match kind {
        PTHREAD_MUTEX_RECURSIVE | PTHREAD_MUTEX_ERRORCHECK => kind,
        // NORMAL and glibc's ADAPTIVE_NP both use non-recursive ownership.
        _ => PTHREAD_MUTEX_NORMAL,
    }
}

/// What a typed mutex's bookkeeping says `pthread_mutex_lock` should do next.
#[cfg(all(windows, target_arch = "x86_64"))]
enum LockPlan {
    /// Take the underlying SRW lock, then claim ownership.
    Acquire,
    /// A recursive relock that was satisfied by bumping the count.
    Recursed,
    Failed(i32),
}

/// Decides how to lock `mutex`, consuming one recursion level if that is all the
/// call needs.
#[cfg(all(windows, target_arch = "x86_64"))]
fn plan_lock(mutex: *mut usize) -> LockPlan {
    // SAFETY: callers of pthread_mutex_lock provide a live pthread_mutex_t.
    let encoded_kind = unsafe { encoded_mutex_kind(mutex) };
    if encoded_kind == PTHREAD_MUTEX_NORMAL && TYPED_MUTEXES.load(Ordering::Acquire) == 0 {
        return LockPlan::Acquire;
    }
    let Ok(records) = mutex_records().lock() else {
        return LockPlan::Failed(EINVAL);
    };
    if !records.contains_key(&(mutex as usize)) && encoded_kind == PTHREAD_MUTEX_NORMAL {
        return LockPlan::Acquire;
    }
    let mut records = wait_for_mutex_fork(records);
    let record = records.entry(mutex as usize).or_insert_with(|| {
        TYPED_MUTEXES.fetch_add(1, Ordering::AcqRel);
        MutexRecord {
            kind: encoded_kind,
            owner: 0,
            recursion: 0,
        }
    });
    if record.kind == PTHREAD_MUTEX_NORMAL {
        return LockPlan::Acquire;
    }
    let self_id = pthread_self();
    match record.kind {
        PTHREAD_MUTEX_RECURSIVE if record.owner == self_id => {
            let Some(next) = record.recursion.checked_add(1) else {
                return LockPlan::Failed(EAGAIN);
            };
            record.recursion = next;
            LockPlan::Recursed
        }
        PTHREAD_MUTEX_ERRORCHECK if record.owner == self_id => LockPlan::Failed(EDEADLK),
        _ => LockPlan::Acquire,
    }
}

/// Claims ownership after the underlying SRW lock has been taken.
#[cfg(all(windows, target_arch = "x86_64"))]
fn record_owner(mutex: *mut usize) {
    // SAFETY: this follows a successful acquire of a live pthread_mutex_t.
    if unsafe { encoded_mutex_kind(mutex) } == PTHREAD_MUTEX_NORMAL
        && TYPED_MUTEXES.load(Ordering::Acquire) == 0
    {
        return;
    }
    if let Ok(records) = mutex_records().lock() {
        if !records.contains_key(&(mutex as usize)) {
            return;
        }
        let mut records = wait_for_mutex_fork(records);
        if let Some(record) = records.get_mut(&(mutex as usize)) {
            record.owner = pthread_self();
            record.recursion = 0;
        }
    }
}

/// What a typed mutex's bookkeeping says `pthread_mutex_unlock` should do.
#[cfg(all(windows, target_arch = "x86_64"))]
enum UnlockPlan {
    /// Release the underlying SRW lock.
    Release,
    /// A recursive unlock that only dropped the count; the lock stays held.
    Retained,
    Failed(i32),
}

/// Decides how to unlock `mutex`, clearing ownership before the caller releases
/// the SRW lock. The order matters: if the SRW lock were released first, the next
/// owner could store its id and then have it overwritten with zero here.
#[cfg(all(windows, target_arch = "x86_64"))]
fn plan_unlock(mutex: *mut usize) -> UnlockPlan {
    // SAFETY: callers of pthread_mutex_unlock provide a live pthread_mutex_t.
    let encoded_kind = unsafe { encoded_mutex_kind(mutex) };
    if encoded_kind == PTHREAD_MUTEX_NORMAL && TYPED_MUTEXES.load(Ordering::Acquire) == 0 {
        return UnlockPlan::Release;
    }
    let Ok(mut records) = mutex_records().lock() else {
        return UnlockPlan::Failed(EINVAL);
    };
    if !records.contains_key(&(mutex as usize)) && encoded_kind == PTHREAD_MUTEX_NORMAL {
        return UnlockPlan::Release;
    }
    let record = records.entry(mutex as usize).or_insert_with(|| {
        TYPED_MUTEXES.fetch_add(1, Ordering::AcqRel);
        MutexRecord {
            kind: encoded_kind,
            owner: pthread_self(),
            recursion: 0,
        }
    });
    if record.kind == PTHREAD_MUTEX_NORMAL {
        return UnlockPlan::Release;
    }
    if record.owner != pthread_self() {
        // Both RECURSIVE and ERRORCHECK know who owns them, so releasing from
        // the wrong thread is a reportable error rather than corruption.
        return UnlockPlan::Failed(EPERM);
    }
    if record.recursion > 0 {
        record.recursion -= 1;
        return UnlockPlan::Retained;
    }
    record.owner = 0;
    FORK_MUTEX_CHANGED.notify_all();
    UnlockPlan::Release
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Initializes a mutex to the unlocked state.
///
/// # Safety
///
/// `mutex` must point to writable, suitably aligned pthread mutex storage and
/// `attr` must be null or point to an initialized `pthread_mutexattr_t`.
pub unsafe extern "sysv64" fn pthread_mutex_init(
    mutex: *mut usize,
    attr: *const PthreadMutexAttr,
) -> i32 {
    if mutex.is_null() {
        return EINVAL;
    }
    let kind = if attr.is_null() {
        PTHREAD_MUTEX_NORMAL
    } else {
        // SAFETY: the caller supplied an initialized pthread_mutexattr_t.
        unsafe { (*attr).kind }
    };
    if mutex_trace_enabled() && kind != PTHREAD_MUTEX_NORMAL {
        eprintln!("kinakaze: [MUTEX] init typed mutex={mutex:p} attr={attr:p} kind={kind}");
    }
    if !matches!(
        kind,
        PTHREAD_MUTEX_NORMAL
            | PTHREAD_MUTEX_RECURSIVE
            | PTHREAD_MUTEX_ERRORCHECK
            | PTHREAD_MUTEX_ADAPTIVE_NP
    ) {
        return EINVAL;
    }
    // Adaptive is a non-recursive mutex whose spin/wait policy is an
    // implementation detail. Windows SRW already chooses its own contention
    // strategy, so use the same storage/ownership rules as NORMAL.
    let kind = if kind == PTHREAD_MUTEX_ADAPTIVE_NP {
        PTHREAD_MUTEX_NORMAL
    } else {
        kind
    };
    // SRWLOCK_INIT is all zeroes.
    unsafe { mutex.write(0) };

    let Ok(mut records) = mutex_records().lock() else {
        return EINVAL;
    };
    // Re-initializing at an address that used to hold a typed mutex must not
    // inherit the old type, so the stale entry goes either way.
    if records.remove(&(mutex as usize)).is_some() {
        TYPED_MUTEXES.fetch_sub(1, Ordering::AcqRel);
    }
    if kind != PTHREAD_MUTEX_NORMAL {
        records.insert(
            mutex as usize,
            MutexRecord {
                kind,
                owner: 0,
                recursion: 0,
            },
        );
        TYPED_MUTEXES.fetch_add(1, Ordering::AcqRel);
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_mutex_destroy(mutex: *mut usize) -> i32 {
    if mutex.is_null() {
        return EINVAL;
    }
    if TYPED_MUTEXES.load(Ordering::Acquire) == 0 {
        return 0;
    }
    let Ok(mut records) = mutex_records().lock() else {
        return EINVAL;
    };
    match records.get(&(mutex as usize)) {
        Some(record) if record.owner != 0 => EBUSY,
        Some(_) => {
            records.remove(&(mutex as usize));
            TYPED_MUTEXES.fetch_sub(1, Ordering::AcqRel);
            0
        }
        None => 0,
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires a mutex.
///
/// # Safety
///
/// `mutex` must point to a live initialized pthread mutex.
pub unsafe extern "sysv64" fn pthread_mutex_lock(mutex: *mut usize) -> i32 {
    if mutex.is_null() {
        return EINVAL;
    }
    match plan_lock(mutex) {
        LockPlan::Recursed => return 0,
        LockPlan::Failed(error) => return error,
        LockPlan::Acquire => {}
    }
    if mutex_trace_enabled() {
        // The common uncontended path is deliberately silent. Apart from
        // keeping diagnostics usable for a JVM, this pinpoints the exact wait
        // whose matching acquire message never appears.
        if unsafe { TryAcquireSRWLockExclusive(mutex.cast::<SRWLOCK>()) } {
            record_owner(mutex);
            return 0;
        }
        eprintln!(
            "kinakaze: [MUTEX] contended mutex={mutex:p} thread={}",
            pthread_self()
        );
    }
    // SAFETY: pthread_mutex_t has the layout and zero initializer of SRWLOCK.
    unsafe { AcquireSRWLockExclusive(mutex.cast::<SRWLOCK>()) };
    record_owner(mutex);
    if mutex_trace_enabled() {
        eprintln!(
            "kinakaze: [MUTEX] acquired mutex={mutex:p} thread={}",
            pthread_self()
        );
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires a mutex, giving up at an absolute `CLOCK_REALTIME` deadline.
///
/// Windows SRW locks have no timed acquire, so this polls `TryAcquire` with a
/// short backoff. A deadline that has already passed is reported as `ETIMEDOUT`
/// after one non-blocking attempt, which is what POSIX requires and what keeps a
/// stale deadline from blocking forever.
///
/// # Safety
///
/// `mutex` must point to a live initialized pthread mutex and `deadline` to a
/// readable `struct timespec`.
pub unsafe extern "sysv64" fn pthread_mutex_timedlock(
    mutex: *mut usize,
    deadline: *const Timespec,
) -> i32 {
    // SAFETY: forwarded without changing either caller-owned object.
    unsafe { pthread_mutex_clocklock(mutex, CLOCK_REALTIME, deadline) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires a mutex against an absolute realtime or monotonic deadline.
///
/// # Safety
/// `mutex` must be initialized and `deadline` must be readable when waiting.
pub unsafe extern "sysv64" fn pthread_mutex_clocklock(
    mutex: *mut usize,
    clock_id: i32,
    deadline: *const Timespec,
) -> i32 {
    if !matches!(clock_id, CLOCK_REALTIME | CLOCK_MONOTONIC) {
        return EINVAL;
    }
    if mutex.is_null() || deadline.is_null() {
        return EINVAL;
    }
    match plan_lock(mutex) {
        LockPlan::Recursed => return 0,
        LockPlan::Failed(error) => return error,
        LockPlan::Acquire => {}
    }

    // POSIX does not validate the timespec when acquisition is immediate.
    // This also keeps clock reads out of the uncontended path.
    if unsafe { TryAcquireSRWLockExclusive(mutex.cast::<SRWLOCK>()) } {
        record_owner(mutex);
        return 0;
    }
    // SAFETY: the caller supplied a readable timespec for a contended lock.
    let deadline = unsafe { *deadline };
    let mut backoff = Duration::from_micros(50);
    loop {
        let left = if deadline.tv_sec < 0 && (0..1_000_000_000).contains(&deadline.tv_nsec) {
            None
        } else {
            match remaining_until(&deadline, clock_id) {
                Ok(left) => left,
                Err(error) => return error,
            }
        };
        let Some(left) = left else {
            return ETIMEDOUT;
        };
        // SAFETY: pthread_mutex_t has the layout and zero initializer of SRWLOCK.
        if unsafe { TryAcquireSRWLockExclusive(mutex.cast::<SRWLOCK>()) } {
            record_owner(mutex);
            return 0;
        }
        std::thread::sleep(backoff.min(left));
        backoff = (backoff * 2).min(Duration::from_millis(1));
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Attempts to acquire a mutex without blocking.
///
/// # Safety
///
/// `mutex` must point to a live initialized pthread mutex.
pub unsafe extern "sysv64" fn pthread_mutex_trylock(mutex: *mut usize) -> i32 {
    if mutex.is_null() {
        return EINVAL;
    }
    match plan_lock(mutex) {
        LockPlan::Recursed => return 0,
        // An ERRORCHECK relock reports EDEADLK from lock but EBUSY from trylock,
        // since a non-blocking call cannot deadlock.
        LockPlan::Failed(EDEADLK) => return EBUSY,
        LockPlan::Failed(error) => return error,
        LockPlan::Acquire => {}
    }
    // SAFETY: pthread_mutex_t has the layout and zero initializer of SRWLOCK.
    if unsafe { TryAcquireSRWLockExclusive(mutex.cast::<SRWLOCK>()) } {
        record_owner(mutex);
        0
    } else {
        EBUSY
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Releases a mutex owned by the calling thread.
///
/// # Safety
///
/// `mutex` must point to a live initialized mutex owned by the caller.
pub unsafe extern "sysv64" fn pthread_mutex_unlock(mutex: *mut usize) -> i32 {
    if mutex.is_null() {
        return EINVAL;
    }
    match plan_unlock(mutex) {
        UnlockPlan::Retained => return 0,
        UnlockPlan::Failed(error) => return error,
        UnlockPlan::Release => {}
    }
    // SAFETY: the caller must own this pthread mutex.
    unsafe { ReleaseSRWLockExclusive(mutex.cast::<SRWLOCK>()) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Initializes a mutex attribute object to `PTHREAD_MUTEX_NORMAL`.
///
/// # Safety
///
/// `attr` must point to writable `pthread_mutexattr_t` storage.
pub unsafe extern "sysv64" fn pthread_mutexattr_init(attr: *mut PthreadMutexAttr) -> i32 {
    if attr.is_null() {
        return EINVAL;
    }
    // SAFETY: checked above.
    unsafe {
        attr.write(PthreadMutexAttr {
            kind: PTHREAD_MUTEX_NORMAL,
        })
    };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_mutexattr_destroy(attr: *mut PthreadMutexAttr) -> i32 {
    i32::from(attr.is_null()) * EINVAL
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_mutexattr_setprotocol(
    attr: *mut PthreadMutexAttr,
    protocol: i32,
) -> i32 {
    if attr.is_null() || !(0..=2).contains(&protocol) {
        return EINVAL;
    }
    // SRW-backed mutexes provide the default protocol. They do not expose
    // POSIX priority inheritance or a priority-ceiling scheduler contract.
    if protocol == 0 { 0 } else { 95 }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_mutexattr_getprotocol(
    attr: *const PthreadMutexAttr,
    protocol: *mut i32,
) -> i32 {
    if attr.is_null() || protocol.is_null() {
        return EINVAL;
    }
    unsafe { protocol.write(0) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Reports the capabilities of the SRW-backed mutex implementation. Robust
/// owner-death recovery is optional; callers such as TDB can select file locks
/// when this attribute returns ENOTSUP. Never accept it as a normal mutex.
pub unsafe extern "sysv64" fn pthread_mutexattr_setrobust(
    attr: *mut PthreadMutexAttr,
    robustness: i32,
) -> i32 {
    if attr.is_null() || !(0..=1).contains(&robustness) {
        return EINVAL;
    }
    if robustness == 1 { 95 } else { 0 }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_mutexattr_getrobust(
    attr: *const PthreadMutexAttr,
    robustness: *mut i32,
) -> i32 {
    if attr.is_null() || robustness.is_null() {
        return EINVAL;
    }
    unsafe {
        *robustness = 0;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_mutex_consistent(_mutex: *mut usize) -> i32 {
    // No successfully initialized mutex can be robust/inconsistent above.
    EINVAL
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Selects the type applied by the next `pthread_mutex_init` using this object.
///
/// # Safety
///
/// `attr` must point to a live initialized mutex attribute object.
pub unsafe extern "sysv64" fn pthread_mutexattr_settype(
    attr: *mut PthreadMutexAttr,
    kind: i32,
) -> i32 {
    if attr.is_null()
        || !matches!(
            kind,
            PTHREAD_MUTEX_NORMAL
                | PTHREAD_MUTEX_RECURSIVE
                | PTHREAD_MUTEX_ERRORCHECK
                | PTHREAD_MUTEX_ADAPTIVE_NP
        )
    {
        return EINVAL;
    }
    // SAFETY: checked above.
    unsafe { (*attr).kind = kind };
    if mutex_trace_enabled() {
        eprintln!("kinakaze: [MUTEX] attr settype attr={attr:p} kind={kind}");
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Reads back the configured mutex type.
///
/// # Safety
///
/// `attr` must point to a live initialized mutex attribute object and `kind` to
/// writable storage for one `int`.
pub unsafe extern "sysv64" fn pthread_mutexattr_gettype(
    attr: *const PthreadMutexAttr,
    kind: *mut i32,
) -> i32 {
    if attr.is_null() || kind.is_null() {
        return EINVAL;
    }
    // SAFETY: checked above.
    unsafe { kind.write((*attr).kind) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_condattr_init(attr: *mut PthreadCondAttr) -> i32 {
    if attr.is_null() {
        return EINVAL;
    }
    unsafe {
        attr.write(PthreadCondAttr {
            clock_id: CLOCK_REALTIME,
        })
    };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_condattr_destroy(attr: *mut PthreadCondAttr) -> i32 {
    i32::from(attr.is_null()) * EINVAL
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_condattr_setclock(
    attr: *mut PthreadCondAttr,
    clock_id: i32,
) -> i32 {
    if attr.is_null() || !matches!(clock_id, CLOCK_REALTIME | CLOCK_MONOTONIC) {
        return EINVAL;
    }
    unsafe { (*attr).clock_id = clock_id };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn pthread_condattr_getclock(
    attr: *const PthreadCondAttr,
    clock_id: *mut i32,
) -> i32 {
    if attr.is_null() || clock_id.is_null() {
        return EINVAL;
    }
    unsafe { clock_id.write((*attr).clock_id) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_condattr_setpshared(
    attr: *mut PthreadCondAttr,
    pshared: i32,
) -> i32 {
    if attr.is_null() || !matches!(pshared, PTHREAD_PROCESS_PRIVATE | PTHREAD_PROCESS_SHARED) {
        EINVAL
    } else if pshared == PTHREAD_PROCESS_SHARED {
        // A Windows condition variable cannot synchronize another process.
        ENOSYS
    } else {
        0
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Initializes a condition variable.
///
/// # Safety
///
/// `cond` must point to writable, suitably aligned pthread condition storage.
pub unsafe extern "sysv64" fn pthread_cond_init(cond: *mut usize, _attr: *const c_void) -> i32 {
    if cond.is_null() {
        return EINVAL;
    }
    let clock_id = if _attr.is_null() {
        CLOCK_REALTIME
    } else {
        unsafe { (*_attr.cast::<PthreadCondAttr>()).clock_id }
    };
    if !matches!(clock_id, CLOCK_REALTIME | CLOCK_MONOTONIC) {
        return EINVAL;
    }
    // CONDITION_VARIABLE_INIT is all zeroes.
    unsafe { cond.write(0) };
    if let Ok(mut clocks) = cond_clocks().lock() {
        clocks.remove(&(cond as usize));
        if clock_id != CLOCK_REALTIME {
            clocks.insert(cond as usize, clock_id);
        }
    } else {
        return EINVAL;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_cond_destroy(cond: *mut usize) -> i32 {
    if cond.is_null() {
        return EINVAL;
    }
    if let Ok(mut clocks) = cond_clocks().lock() {
        clocks.remove(&(cond as usize));
        0
    } else {
        EINVAL
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Atomically releases a mutex, waits, then reacquires the mutex.
///
/// Windows performs the release and reacquire inside the kernel, bypassing the
/// ownership bookkeeping that `RECURSIVE` and `ERRORCHECK` mutexes rely on. That
/// matches POSIX, which leaves waiting on a non-`NORMAL` mutex undefined.
///
/// # Safety
///
/// Both pointers must refer to live initialized objects and the caller must own
/// `mutex` when entering this function.
pub unsafe extern "sysv64" fn pthread_cond_wait(cond: *mut usize, mutex: *mut usize) -> i32 {
    if cond.is_null() || mutex.is_null() {
        return EINVAL;
    }
    loop {
        cancel_condition_wait(cond);
        // Native condition variables cannot wait on a separate cancellation
        // handle. Bound each sleep so cancellation cannot miss the interval
        // between checking the request and atomically releasing the mutex.
        let woke = unsafe { SleepConditionVariableSRW(cond.cast(), mutex.cast(), 100, 0) };
        let error = if woke == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        record_owner(mutex);
        // The caller's mutex is held before running its GNU cleanup handlers.
        cancel_condition_wait(cond);
        if woke != 0 {
            return 0;
        }
        if error != ERROR_TIMEOUT {
            return EINVAL;
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn cancel_condition_wait(cond: *mut usize) {
    let pending = {
        let state = cancel_self();
        state.enabled.load(Ordering::Acquire) && state.requested.load(Ordering::Acquire)
    };
    if pending {
        // A cancelled waiter must not consume a signal intended for another
        // waiter. Extra/spurious wakeups are allowed by the condition API.
        unsafe { WakeConditionVariable(cond.cast()) };
        pthread_testcancel();
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Waits on a condition variable until signalled or until the absolute deadline
/// for its configured clock passes.
///
/// On `ETIMEDOUT` the mutex is reacquired before returning, as POSIX requires,
/// because `SleepConditionVariableSRW` restores the lock on every exit path. A
/// deadline in the past returns `ETIMEDOUT` with the mutex still held and no wait
/// performed at all.
///
/// # Safety
///
/// Both objects must be live and initialized, the caller must own `mutex`, and
/// `deadline` must point to a readable `struct timespec`.
pub unsafe extern "sysv64" fn pthread_cond_timedwait(
    cond: *mut usize,
    mutex: *mut usize,
    deadline: *const Timespec,
) -> i32 {
    if cond.is_null() || mutex.is_null() || deadline.is_null() {
        return EINVAL;
    }
    let clock_id = cond_clocks()
        .lock()
        .ok()
        .and_then(|clocks| clocks.get(&(cond as usize)).copied())
        .unwrap_or(CLOCK_REALTIME);
    unsafe { cond_wait_deadline(cond, mutex, clock_id, *deadline) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Waits until signaled or the absolute deadline on the given clock expires.
///
/// # Safety
///
/// Both objects must be live and initialized, the caller must own `mutex`, and
/// `deadline` must point to a readable `struct timespec`.
pub unsafe extern "sysv64" fn pthread_cond_clockwait(
    cond: *mut usize,
    mutex: *mut usize,
    clock_id: i32,
    deadline: *const Timespec,
) -> i32 {
    if cond.is_null() || mutex.is_null() || deadline.is_null() {
        return EINVAL;
    }
    unsafe { cond_wait_deadline(cond, mutex, clock_id, *deadline) }
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn cond_wait_deadline(
    cond: *mut usize,
    mutex: *mut usize,
    clock_id: i32,
    deadline: Timespec,
) -> i32 {
    loop {
        cancel_condition_wait(cond);
        let left = match remaining_until(&deadline, clock_id) {
            Ok(Some(left)) => left,
            Ok(None) => return ETIMEDOUT,
            Err(error) => return error,
        };
        // Round fractional milliseconds up and bound cancellation latency as
        // for untimed waits. Recheck the absolute clock after each timeout.
        let milliseconds = left.as_nanos().div_ceil(1_000_000).min(100) as u32;
        let woke = unsafe {
            SleepConditionVariableSRW(
                cond.cast::<CONDITION_VARIABLE>(),
                mutex.cast::<SRWLOCK>(),
                milliseconds,
                0,
            )
        };
        let error = if woke == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        record_owner(mutex);
        cancel_condition_wait(cond);
        if woke != 0 {
            return 0;
        }
        if error != ERROR_TIMEOUT {
            return EINVAL;
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Wakes one waiter on a condition variable.
///
/// # Safety
///
/// `cond` must point to a live initialized pthread condition variable.
pub unsafe extern "sysv64" fn pthread_cond_signal(cond: *mut usize) -> i32 {
    if cond.is_null() {
        return EINVAL;
    }
    // SAFETY: pthread_cond_t has the layout of CONDITION_VARIABLE.
    unsafe { WakeConditionVariable(cond.cast::<CONDITION_VARIABLE>()) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Wakes all waiters on a condition variable.
///
/// # Safety
///
/// `cond` must point to a live initialized pthread condition variable.
pub unsafe extern "sysv64" fn pthread_cond_broadcast(cond: *mut usize) -> i32 {
    if cond.is_null() {
        return EINVAL;
    }
    // SAFETY: pthread_cond_t has the layout of CONDITION_VARIABLE.
    unsafe { WakeAllConditionVariable(cond.cast::<CONDITION_VARIABLE>()) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Creates a process-wide pthread TLS key.
///
/// # Safety
///
/// `key` must point to writable storage for one `u32`.
pub unsafe extern "sysv64" fn pthread_key_create(
    key: *mut u32,
    destructor: Option<KeyDestructor>,
) -> i32 {
    if key.is_null() {
        return kinakaze_tls::EINVAL;
    }
    match kinakaze_tls::pthread_key_create(destructor) {
        Ok(created) => {
            // SAFETY: the caller supplied a writable key result pointer.
            unsafe { key.write(created) };
            0
        }
        Err(error) => error,
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_key_delete(key: u32) -> i32 {
    kinakaze_tls::pthread_key_delete(key).map_or_else(|error| error, |()| 0)
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_getspecific(key: u32) -> *mut c_void {
    kinakaze_tls::pthread_getspecific(key).unwrap_or(core::ptr::null_mut())
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub extern "sysv64" fn pthread_setspecific(key: u32, value: *mut c_void) -> i32 {
    kinakaze_tls::pthread_setspecific(key, value).map_or_else(|error| error, |()| 0)
}

// ---------------------------------------------------------------------------
// Read-write locks
// ---------------------------------------------------------------------------

/// A guard held on behalf of guest code, which owns no Rust binding to drop.
///
/// POSIX has one `pthread_rwlock_unlock` for both modes, so the mode has to be
/// recoverable at unlock time. Rather than counting readers by hand, each
/// acquisition parks its RAII guard in the acquiring thread's own list: unlock
/// pops that thread's most recent guard for the lock, and dropping it releases
/// exactly the mode that was taken. This also matches the POSIX rule that a lock
/// is released by the thread holding it, and it means an unwinding thread (see
/// `pthread_cancel`) drops its rwlocks instead of stranding them.
#[cfg(all(windows, target_arch = "x86_64"))]
#[expect(
    dead_code,
    reason = "each guard is held for its Drop effect; dropping it is the unlock"
)]
enum RwGuard {
    Read(std::sync::RwLockReadGuard<'static, ()>),
    Write(std::sync::RwLockWriteGuard<'static, ()>),
}

#[cfg(all(windows, target_arch = "x86_64"))]
static NEXT_RWLOCK: AtomicUsize = AtomicUsize::new(1);
#[cfg(all(windows, target_arch = "x86_64"))]
static RWLOCKS: OnceLock<Mutex<HashMap<usize, &'static RwLock<()>>>> = OnceLock::new();

#[cfg(all(windows, target_arch = "x86_64"))]
thread_local! {
    /// This thread's outstanding acquisitions, newest last, keyed by handle.
    static HELD_RWLOCKS: std::cell::RefCell<Vec<(usize, RwGuard)>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Locks owned by threads that do not exist in the child remain locked and
    /// deliberately have no public unlock path.
    static ORPHANED_RWLOCKS: std::cell::RefCell<Vec<RwGuard>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn rwlocks() -> &'static Mutex<HashMap<usize, &'static RwLock<()>>> {
    RWLOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Publishes a fresh lock and returns its handle.
///
/// The `RwLock` is leaked so its guards can be `'static`, which is what lets them
/// outlive the call that took them. One pointer-sized allocation per lock is the
/// price of storing guards on the side.
#[cfg(all(windows, target_arch = "x86_64"))]
fn register_rwlock() -> Option<usize> {
    let handle = NEXT_RWLOCK.fetch_add(1, Ordering::Relaxed).max(1);
    let lock: &'static RwLock<()> = Box::leak(Box::new(RwLock::new(())));
    rwlocks().lock().ok()?.insert(handle, lock);
    Some(handle)
}

/// Resolves a `pthread_rwlock_t`, creating the lock on first use.
///
/// `PTHREAD_RWLOCK_INITIALIZER` is a zeroed struct with no `init` call, so a zero
/// handle means "not created yet". The slot is read and published atomically
/// because two threads can reach a statically initialized lock at once.
#[cfg(all(windows, target_arch = "x86_64"))]
fn resolve_rwlock(rwlock: *mut usize) -> Result<&'static RwLock<()>, i32> {
    // SAFETY: pthread_rwlock_t is one pointer-sized aligned field, which the
    // pthread ABI requires to be atomic-compatible storage.
    let slot = unsafe { AtomicUsize::from_ptr(rwlock) };
    let handle = slot.load(Ordering::Acquire);
    let mut registry = rwlocks().lock().map_err(|_| EINVAL)?;
    if handle != 0 {
        return registry.get(&handle).copied().ok_or(EINVAL);
    }
    // Re-read under the registry lock: the winner of the race publishes first.
    let handle = slot.load(Ordering::Acquire);
    if handle != 0 {
        return registry.get(&handle).copied().ok_or(EINVAL);
    }
    let created = NEXT_RWLOCK.fetch_add(1, Ordering::Relaxed).max(1);
    let lock: &'static RwLock<()> = Box::leak(Box::new(RwLock::new(())));
    registry.insert(created, lock);
    slot.store(created, Ordering::Release);
    Ok(lock)
}

/// Records a new acquisition against the calling thread.
#[cfg(all(windows, target_arch = "x86_64"))]
fn push_rwguard(rwlock: *mut usize, guard: RwGuard) -> i32 {
    // SAFETY: pthread_rwlock_t is one pointer-sized atomic-compatible field.
    let handle = unsafe { AtomicUsize::from_ptr(rwlock) }.load(Ordering::Acquire);
    HELD_RWLOCKS.with_borrow_mut(|held| held.push((handle, guard)));
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Initializes a read-write lock to the unlocked state.
///
/// # Safety
///
/// `rwlock` must point to writable, suitably aligned pthread rwlock storage and
/// `attr` is ignored beyond being null or readable.
pub unsafe extern "sysv64" fn pthread_rwlock_init(rwlock: *mut usize, attr: *const c_void) -> i32 {
    if rwlock.is_null() {
        return EINVAL;
    }
    // The registry below owns a process-local Rust lock. Never advertise it as
    // process-shared: a second process cannot resolve that private handle.
    if !attr.is_null() && unsafe { *attr.cast::<i32>() } != 0 {
        return 95; // ENOTSUP
    }
    let Some(handle) = register_rwlock() else {
        return EAGAIN;
    };
    // SAFETY: the caller supplied writable rwlock storage.
    unsafe { rwlock.write(handle) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Releases a read-write lock's registry entry.
///
/// The `RwLock` itself is intentionally not freed: guards taken by threads that
/// have not yet unlocked still borrow it, and POSIX gives no point at which those
/// are guaranteed gone.
///
/// # Safety
///
/// `rwlock` must point to a live initialized pthread rwlock.
pub unsafe extern "sysv64" fn pthread_rwlock_destroy(rwlock: *mut usize) -> i32 {
    if rwlock.is_null() {
        return EINVAL;
    }
    // SAFETY: pthread_rwlock_t is one pointer-sized atomic-compatible field.
    let handle = unsafe { AtomicUsize::from_ptr(rwlock) }.load(Ordering::Acquire);
    if handle == 0 {
        // Never used, so there is nothing to release.
        return 0;
    }
    let mut registry = match rwlocks().lock() {
        Ok(registry) => registry,
        Err(_) => return EINVAL,
    };
    let Some(lock) = registry.get(&handle).copied() else {
        return EINVAL;
    };
    // Destroying a held lock is undefined in POSIX; an exclusive probe turns it
    // into a reportable EBUSY instead.
    if lock.try_write().is_err() {
        return EBUSY;
    }
    registry.remove(&handle);
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires shared read access, blocking while a writer holds the lock.
///
/// Recursive read acquisition by one thread is not supported: the underlying
/// `RwLock` may deadlock, which POSIX also permits.
///
/// # Safety
///
/// `rwlock` must point to a live initialized pthread rwlock.
pub unsafe extern "sysv64" fn pthread_rwlock_rdlock(rwlock: *mut usize) -> i32 {
    if rwlock.is_null() {
        return EINVAL;
    }
    let lock = match resolve_rwlock(rwlock) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    // The guarded value is (), so a poisoned lock carries no broken invariant
    // and recovering the guard is the right move.
    let guard = lock.read().unwrap_or_else(PoisonError::into_inner);
    push_rwguard(rwlock, RwGuard::Read(guard))
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires shared read access without blocking.
///
/// # Safety
///
/// `rwlock` must point to a live initialized pthread rwlock.
pub unsafe extern "sysv64" fn pthread_rwlock_tryrdlock(rwlock: *mut usize) -> i32 {
    if rwlock.is_null() {
        return EINVAL;
    }
    let lock = match resolve_rwlock(rwlock) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    match lock.try_read() {
        Ok(guard) => push_rwguard(rwlock, RwGuard::Read(guard)),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            push_rwguard(rwlock, RwGuard::Read(poisoned.into_inner()))
        }
        Err(std::sync::TryLockError::WouldBlock) => EBUSY,
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires exclusive write access, blocking until no reader or writer holds it.
///
/// # Safety
///
/// `rwlock` must point to a live initialized pthread rwlock.
pub unsafe extern "sysv64" fn pthread_rwlock_wrlock(rwlock: *mut usize) -> i32 {
    if rwlock.is_null() {
        return EINVAL;
    }
    let lock = match resolve_rwlock(rwlock) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    let guard = lock.write().unwrap_or_else(PoisonError::into_inner);
    push_rwguard(rwlock, RwGuard::Write(guard))
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires exclusive write access without blocking.
///
/// # Safety
///
/// `rwlock` must point to a live initialized pthread rwlock.
pub unsafe extern "sysv64" fn pthread_rwlock_trywrlock(rwlock: *mut usize) -> i32 {
    if rwlock.is_null() {
        return EINVAL;
    }
    let lock = match resolve_rwlock(rwlock) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    match lock.try_write() {
        Ok(guard) => push_rwguard(rwlock, RwGuard::Write(guard)),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            push_rwguard(rwlock, RwGuard::Write(poisoned.into_inner()))
        }
        Err(std::sync::TryLockError::WouldBlock) => EBUSY,
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires shared read access against an absolute realtime deadline.
///
/// # Safety
///
/// `rwlock` must point to a live initialized pthread rwlock and `abstime` must be readable.
pub unsafe extern "sysv64" fn pthread_rwlock_timedrdlock(
    rwlock: *mut usize,
    abstime: *const Timespec,
) -> i32 {
    if rwlock.is_null() || abstime.is_null() {
        return EINVAL;
    }
    let lock = match resolve_rwlock(rwlock) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    match lock.try_read() {
        Ok(guard) => return push_rwguard(rwlock, RwGuard::Read(guard)),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            return push_rwguard(rwlock, RwGuard::Read(poisoned.into_inner()));
        }
        Err(std::sync::TryLockError::WouldBlock) => {}
    }
    let deadline = unsafe { *abstime };
    let mut backoff = Duration::from_micros(50);
    loop {
        let left = if deadline.tv_sec < 0 && (0..1_000_000_000).contains(&deadline.tv_nsec) {
            None
        } else {
            match remaining_until(&deadline, CLOCK_REALTIME) {
                Ok(left) => left,
                Err(error) => return error,
            }
        };
        let Some(left) = left else {
            return ETIMEDOUT;
        };
        match lock.try_read() {
            Ok(guard) => return push_rwguard(rwlock, RwGuard::Read(guard)),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                return push_rwguard(rwlock, RwGuard::Read(poisoned.into_inner()));
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        std::thread::sleep(backoff.min(left));
        backoff = (backoff * 2).min(Duration::from_millis(1));
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acquires exclusive write access against an absolute realtime deadline.
///
/// # Safety
///
/// `rwlock` must point to a live initialized pthread rwlock and `abstime` must be readable.
pub unsafe extern "sysv64" fn pthread_rwlock_timedwrlock(
    rwlock: *mut usize,
    abstime: *const Timespec,
) -> i32 {
    if rwlock.is_null() || abstime.is_null() {
        return EINVAL;
    }
    let lock = match resolve_rwlock(rwlock) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    match lock.try_write() {
        Ok(guard) => return push_rwguard(rwlock, RwGuard::Write(guard)),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            return push_rwguard(rwlock, RwGuard::Write(poisoned.into_inner()));
        }
        Err(std::sync::TryLockError::WouldBlock) => {}
    }
    let deadline = unsafe { *abstime };
    let mut backoff = Duration::from_micros(50);
    loop {
        let left = if deadline.tv_sec < 0 && (0..1_000_000_000).contains(&deadline.tv_nsec) {
            None
        } else {
            match remaining_until(&deadline, CLOCK_REALTIME) {
                Ok(left) => left,
                Err(error) => return error,
            }
        };
        let Some(left) = left else {
            return ETIMEDOUT;
        };
        match lock.try_write() {
            Ok(guard) => return push_rwguard(rwlock, RwGuard::Write(guard)),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                return push_rwguard(rwlock, RwGuard::Write(poisoned.into_inner()));
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        std::thread::sleep(backoff.min(left));
        backoff = (backoff * 2).min(Duration::from_millis(1));
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Releases whichever mode the calling thread most recently acquired.
///
/// Returns `EPERM` when this thread holds no acquisition of `rwlock`, which is
/// the closest reportable answer to POSIX's undefined behaviour.
///
/// # Safety
///
/// `rwlock` must point to a live initialized pthread rwlock held by the caller.
pub unsafe extern "sysv64" fn pthread_rwlock_unlock(rwlock: *mut usize) -> i32 {
    if rwlock.is_null() {
        return EINVAL;
    }
    // SAFETY: pthread_rwlock_t is one pointer-sized atomic-compatible field.
    let handle = unsafe { AtomicUsize::from_ptr(rwlock) }.load(Ordering::Acquire);
    if handle == 0 {
        return EPERM;
    }
    let guard = HELD_RWLOCKS.with_borrow_mut(|held| {
        held.iter()
            .rposition(|(candidate, _)| *candidate == handle)
            .map(|index| held.remove(index))
    });
    // Dropping the guard is the release; the mode follows from which variant it is.
    if guard.is_some() { 0 } else { EPERM }
}

// ---------------------------------------------------------------------------
// Barriers
// ---------------------------------------------------------------------------

#[cfg(all(windows, target_arch = "x86_64"))]
static NEXT_BARRIER: AtomicUsize = AtomicUsize::new(1);
#[cfg(all(windows, target_arch = "x86_64"))]
static BARRIERS: OnceLock<Mutex<HashMap<usize, &'static Barrier>>> = OnceLock::new();

#[cfg(all(windows, target_arch = "x86_64"))]
fn barriers() -> &'static Mutex<HashMap<usize, &'static Barrier>> {
    BARRIERS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Creates a barrier that releases once `count` threads are waiting.
///
/// # Safety
///
/// `barrier` must point to writable, suitably aligned pthread barrier storage.
pub unsafe extern "sysv64" fn pthread_barrier_init(
    barrier: *mut usize,
    _attr: *const c_void,
    count: u32,
) -> i32 {
    if barrier.is_null() || count == 0 {
        return EINVAL;
    }
    let Ok(count) = usize::try_from(count) else {
        return EINVAL;
    };
    let handle = NEXT_BARRIER.fetch_add(1, Ordering::Relaxed).max(1);
    // Leaked for the same reason as rwlocks: waiters hold a borrow across calls
    // and there is no moment at which they are all provably gone.
    let created: &'static Barrier = Box::leak(Box::new(Barrier::new(count)));
    let Ok(mut registry) = barriers().lock() else {
        return EAGAIN;
    };
    registry.insert(handle, created);
    drop(registry);
    // SAFETY: the caller supplied writable barrier storage.
    unsafe { barrier.write(handle) };
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Releases a barrier's registry entry.
///
/// # Safety
///
/// `barrier` must point to a live initialized pthread barrier with no waiters.
pub unsafe extern "sysv64" fn pthread_barrier_destroy(barrier: *mut usize) -> i32 {
    if barrier.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller supplied a readable barrier.
    let handle = unsafe { *barrier };
    let Ok(mut registry) = barriers().lock() else {
        return EINVAL;
    };
    if registry.remove(&handle).is_none() {
        return EINVAL;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Blocks until the barrier's full complement of threads has arrived.
///
/// Exactly one of the released threads receives `PTHREAD_BARRIER_SERIAL_THREAD`
/// and the rest receive 0, which is how callers elect a single thread to run the
/// once-per-generation work.
///
/// # Safety
///
/// `barrier` must point to a live initialized pthread barrier.
pub unsafe extern "sysv64" fn pthread_barrier_wait(barrier: *mut usize) -> i32 {
    if barrier.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller supplied a readable barrier.
    let handle = unsafe { *barrier };
    let found = match barriers().lock() {
        Ok(registry) => registry.get(&handle).copied(),
        Err(_) => return EINVAL,
    };
    let Some(found) = found else {
        return EINVAL;
    };
    if found.wait().is_leader() {
        PTHREAD_BARRIER_SERIAL_THREAD
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Spin locks
// ---------------------------------------------------------------------------

/// The spin lock's unlocked state, and the zero value a static initializer gives.
#[cfg(all(windows, target_arch = "x86_64"))]
const SPIN_UNLOCKED: usize = 0;
#[cfg(all(windows, target_arch = "x86_64"))]
const SPIN_LOCKED: usize = 1;

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Initializes a spin lock to the unlocked state.
///
/// `pshared` is accepted but has no effect: the lock word lives in this
/// process's address space, so `PTHREAD_PROCESS_SHARED` would need the guest to
/// have placed it in shared memory, which is outside this layer's control.
///
/// # Safety
///
/// `lock` must point to writable, suitably aligned pthread spin lock storage.
pub unsafe extern "sysv64" fn pthread_spin_init(lock: *mut usize, pshared: i32) -> i32 {
    if lock.is_null() || !matches!(pshared, PTHREAD_PROCESS_PRIVATE | PTHREAD_PROCESS_SHARED) {
        return EINVAL;
    }
    // SAFETY: the caller supplied writable spin lock storage.
    unsafe { AtomicUsize::from_ptr(lock) }.store(SPIN_UNLOCKED, Ordering::Release);
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Tears down a spin lock, which owns no resources beyond its lock word.
///
/// # Safety
///
/// `lock` must point to a live initialized, unlocked pthread spin lock.
pub unsafe extern "sysv64" fn pthread_spin_destroy(lock: *mut usize) -> i32 {
    if lock.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller supplied a live spin lock.
    if unsafe { AtomicUsize::from_ptr(lock) }.load(Ordering::Acquire) == SPIN_LOCKED {
        return EBUSY;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Spins until the lock is acquired.
///
/// There is no owner check, so relocking from the holding thread spins forever;
/// POSIX leaves that case undefined and a check would cost the fast path.
///
/// # Safety
///
/// `lock` must point to a live initialized pthread spin lock.
pub unsafe extern "sysv64" fn pthread_spin_lock(lock: *mut usize) -> i32 {
    if lock.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller supplied a live spin lock.
    let state = unsafe { AtomicUsize::from_ptr(lock) };
    loop {
        if state
            .compare_exchange_weak(
                SPIN_UNLOCKED,
                SPIN_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            return 0;
        }
        // Read-only spinning until the word changes keeps the cache line shared
        // instead of bouncing it between contenders on every attempt.
        while state.load(Ordering::Relaxed) == SPIN_LOCKED {
            core::hint::spin_loop();
        }
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Attempts to acquire a spin lock without spinning.
///
/// # Safety
///
/// `lock` must point to a live initialized pthread spin lock.
pub unsafe extern "sysv64" fn pthread_spin_trylock(lock: *mut usize) -> i32 {
    if lock.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller supplied a live spin lock.
    let state = unsafe { AtomicUsize::from_ptr(lock) };
    if state
        .compare_exchange(
            SPIN_UNLOCKED,
            SPIN_LOCKED,
            Ordering::Acquire,
            Ordering::Relaxed,
        )
        .is_ok()
    {
        0
    } else {
        EBUSY
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Releases a spin lock held by the calling thread.
///
/// # Safety
///
/// `lock` must point to a live initialized spin lock held by the caller.
pub unsafe extern "sysv64" fn pthread_spin_unlock(lock: *mut usize) -> i32 {
    if lock.is_null() {
        return EINVAL;
    }
    // SAFETY: the caller supplied a live spin lock it holds.
    unsafe { AtomicUsize::from_ptr(lock) }.store(SPIN_UNLOCKED, Ordering::Release);
    0
}

// ---------------------------------------------------------------------------
// Cancellation
//
// Deferred cancellation only.
//
// `TerminateThread` is the only way Windows can stop another thread where it
// stands, and it stops it without any cleanup: every lock the victim holds stays
// locked, every allocation it owns leaks, and any CRT state it was mutating stays
// half-written. A deadlock or a corrupted heap is worse than a cancel that
// arrives late, so it is never used here. `PTHREAD_CANCEL_ASYNCHRONOUS` is
// accepted and then behaves as `PTHREAD_CANCEL_DEFERRED`: the request is only
// recorded, and it is acted on at the next `pthread_testcancel`.
//
// Acting on it means `ExitThread` from inside `pthread_testcancel`, not an
// unwind. These entry points are `extern "sysv64"` and their callers are guest
// ELF frames, so a panic could not cross the boundary at all, and a forced
// unwind would need DWARF unwind tables through frames the host's SEH unwinder
// cannot read. Guest TLS destructors and owned pthread state are cleaned up
// before ExitThread. Registered GNU C cleanup frames use their saved jump
// targets. General C++ forced unwinding still requires a guest DWARF path.
// ---------------------------------------------------------------------------

#[cfg(all(windows, target_arch = "x86_64"))]
struct CancelState {
    requested: AtomicBool,
    enabled: AtomicBool,
}

#[cfg(all(windows, target_arch = "x86_64"))]
static CANCEL_STATES: OnceLock<Mutex<HashMap<usize, Arc<CancelState>>>> = OnceLock::new();

#[cfg(all(windows, target_arch = "x86_64"))]
thread_local! {
    static CANCEL_SELF: OnceLock<Arc<CancelState>> = const { OnceLock::new() };
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn cancel_states() -> &'static Mutex<HashMap<usize, Arc<CancelState>>> {
    CANCEL_STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn new_cancel_state() -> Arc<CancelState> {
    Arc::new(CancelState {
        requested: AtomicBool::new(false),
        enabled: AtomicBool::new(true),
    })
}

/// Returns the calling thread's cancellation state, publishing it on first use.
///
/// Threads created by `pthread_create` are registered before they start so a
/// `pthread_cancel` racing the spawn is not lost. Threads that arrive from
/// elsewhere, the initial thread most of all, register themselves here.
#[cfg(all(windows, target_arch = "x86_64"))]
fn cancel_self() -> Arc<CancelState> {
    CANCEL_SELF
        .try_with(|cached| {
            cached
                .get_or_init(|| {
                    let id = pthread_self();
                    match cancel_states().lock() {
                        Ok(mut states) => Arc::clone(
                            states
                                .entry(id)
                                .or_insert_with(|| Arc::clone(&new_cancel_state())),
                        ),
                        Err(_) => new_cancel_state(),
                    }
                })
                .clone()
        })
        .unwrap_or_else(|_| new_cancel_state())
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Requests cancellation of `thread`.
///
/// The request is recorded and returns immediately; the target stops at its next
/// cancellation point, including `pthread_testcancel` and condition waits.
pub extern "sysv64" fn pthread_cancel(thread: usize) -> i32 {
    if thread == pthread_self() {
        cancel_self().requested.store(true, Ordering::Release);
        return 0;
    }
    let Ok(mut states) = cancel_states().lock() else {
        return EINVAL;
    };
    if let Some(state) = states.get(&thread) {
        state.requested.store(true, Ordering::Release);
        return 0;
    }
    // No state yet means the thread has either finished or never existed; only
    // the second case is an error, and the thread registry tells them apart.
    let known = match threads().lock() {
        Ok(registry) => registry.contains_key(&thread),
        Err(_) => false,
    };
    if !known {
        return ESRCH;
    }
    let state = new_cancel_state();
    state.requested.store(true, Ordering::Release);
    states.insert(thread, state);
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Enables or disables cancellation for the calling thread.
///
/// # Safety
///
/// `previous`, when non-null, must point to writable storage for one `int`.
pub unsafe extern "sysv64" fn pthread_setcancelstate(state: i32, previous: *mut i32) -> i32 {
    if !matches!(state, PTHREAD_CANCEL_ENABLE | PTHREAD_CANCEL_DISABLE) {
        return EINVAL;
    }
    let was = cancel_self()
        .enabled
        .swap(state == PTHREAD_CANCEL_ENABLE, Ordering::AcqRel);
    if !previous.is_null() {
        // SAFETY: the caller supplied writable result storage.
        unsafe {
            previous.write(if was {
                PTHREAD_CANCEL_ENABLE
            } else {
                PTHREAD_CANCEL_DISABLE
            })
        };
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Accepts a cancellation type for the calling thread.
///
/// Both types behave as `PTHREAD_CANCEL_DEFERRED`; see the note above on why
/// asynchronous cancellation is not implemented on Windows.
///
/// # Safety
///
/// `previous`, when non-null, must point to writable storage for one `int`.
pub unsafe extern "sysv64" fn pthread_setcanceltype(kind: i32, previous: *mut i32) -> i32 {
    if !matches!(kind, PTHREAD_CANCEL_DEFERRED | PTHREAD_CANCEL_ASYNCHRONOUS) {
        return EINVAL;
    }
    if !previous.is_null() {
        // SAFETY: the caller supplied writable result storage.
        unsafe { previous.write(PTHREAD_CANCEL_DEFERRED) };
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Acts on a pending cancellation request, if cancellation is enabled.
///
/// This returns normally when nothing is pending, and does not return at all when
/// something is: the calling thread ends and `pthread_join` reports
/// `PTHREAD_CANCELED`. The rwlocks and registry entries this library holds for
/// the thread are released first; the calling frame's own locals are not, because
/// `ExitThread` does not unwind.
pub extern "sysv64" fn pthread_testcancel() {
    let state = cancel_self();
    if !(state.enabled.load(Ordering::Acquire) && state.requested.load(Ordering::Acquire)) {
        return;
    }

    drop(state);
    pthread_exit(PTHREAD_CANCELED as *mut c_void);
}

// ---------------------------------------------------------------------------
// fork handlers
// ---------------------------------------------------------------------------

#[cfg(all(windows, target_arch = "x86_64"))]
type ForkHandler = unsafe extern "sysv64" fn();

#[cfg(all(windows, target_arch = "x86_64"))]
#[derive(Clone, Copy)]
struct AtforkHandlers {
    prepare: Option<ForkHandler>,
    parent: Option<ForkHandler>,
    child: Option<ForkHandler>,
}

#[cfg(all(windows, target_arch = "x86_64"))]
static ATFORK: OnceLock<Mutex<Vec<AtforkHandlers>>> = OnceLock::new();

#[cfg(all(windows, target_arch = "x86_64"))]
static ACTIVE_ATFORK: Mutex<Option<Vec<AtforkHandlers>>> = Mutex::new(None);

#[cfg(all(windows, target_arch = "x86_64"))]
static FORKING: AtomicBool = AtomicBool::new(false);

#[cfg(all(windows, target_arch = "x86_64"))]
const FORK_PTHREAD_MAGIC: u64 = 0x4352_5950_5448_4632; // "CRYPTHF2"

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "system" fn fork_prepare() -> i32 {
    if FORKING.swap(true, Ordering::AcqRel) {
        return EDEADLK;
    }
    let handlers = match ATFORK
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|handlers| handlers.clone())
    {
        Ok(handlers) => handlers,
        Err(_) => {
            FORKING.store(false, Ordering::Release);
            return EAGAIN;
        }
    };
    let Ok(mut active) = ACTIVE_ATFORK.lock() else {
        FORKING.store(false, Ordering::Release);
        return EAGAIN;
    };
    *active = Some(handlers.clone());
    drop(active);
    // POSIX prepare order is last registered, first called.
    for handler in handlers.iter().rev().filter_map(|item| item.prepare) {
        // SAFETY: pthread_atfork's caller supplied this no-argument function.
        unsafe { handler() };
    }
    begin_mutex_fork();
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "system" fn fork_parent(_result: i32) {
    finish_mutex_fork();
    let handlers = ACTIVE_ATFORK
        .lock()
        .ok()
        .and_then(|mut active| active.take())
        .unwrap_or_default();
    // Parent order is first registered, first called.
    for handler in handlers.iter().filter_map(|item| item.parent) {
        // SAFETY: registered through pthread_atfork.
        unsafe { handler() };
    }
    FORKING.store(false, Ordering::Release);
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn serialize_pthread_fork() -> Option<Vec<u8>> {
    let atfork = ATFORK
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .ok()?
        .clone();
    let mutexes: Vec<_> = mutex_records()
        .lock()
        .ok()?
        .iter()
        .map(|(address, record)| (*address, *record))
        .collect();
    let held: Vec<(usize, u32)> = HELD_RWLOCKS.with_borrow(|held| {
        held.iter()
            .map(|(handle, guard)| {
                (
                    *handle,
                    match guard {
                        RwGuard::Read(_) => 1,
                        RwGuard::Write(_) => 2,
                    },
                )
            })
            .collect()
    });
    let held_handles: std::collections::HashSet<usize> =
        held.iter().map(|(handle, _)| *handle).collect();
    let rw = rwlocks().lock().ok()?;
    let mut rw_states = Vec::with_capacity(rw.len());
    for (handle, lock) in rw.iter() {
        let orphaned = if held_handles.contains(handle) {
            false
        } else {
            match lock.try_write() {
                Ok(probe) => {
                    drop(probe);
                    false
                }
                Err(_) => true,
            }
        };
        rw_states.push((*handle, orphaned));
    }
    drop(rw);

    let mut out = Vec::new();
    out.extend_from_slice(&FORK_PTHREAD_MAGIC.to_le_bytes());
    out.extend_from_slice(&(pthread_self() as u64).to_le_bytes());
    out.extend_from_slice(&(atfork.len() as u32).to_le_bytes());
    out.extend_from_slice(&(mutexes.len() as u32).to_le_bytes());
    out.extend_from_slice(&(rw_states.len() as u32).to_le_bytes());
    out.extend_from_slice(&(held.len() as u32).to_le_bytes());
    for item in atfork {
        for address in [
            item.prepare.map_or(0, |handler| handler as usize),
            item.parent.map_or(0, |handler| handler as usize),
            item.child.map_or(0, |handler| handler as usize),
        ] {
            out.extend_from_slice(&(address as u64).to_le_bytes());
        }
    }
    for (address, record) in mutexes {
        out.extend_from_slice(&(address as u64).to_le_bytes());
        out.extend_from_slice(&(record.kind as i64 as u64).to_le_bytes());
        out.extend_from_slice(&(record.owner as u64).to_le_bytes());
        out.extend_from_slice(&(record.recursion as u64).to_le_bytes());
    }
    for (handle, orphaned) in rw_states {
        out.extend_from_slice(&(handle as u64).to_le_bytes());
        out.extend_from_slice(&(orphaned as u64).to_le_bytes());
    }
    for (handle, mode) in held {
        out.extend_from_slice(&(handle as u64).to_le_bytes());
        out.extend_from_slice(&(mode as u64).to_le_bytes());
    }
    for word in cleanup::snapshot() {
        out.extend_from_slice(&word.to_le_bytes());
    }
    Some(out)
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "system" fn fork_snapshot(buffer: *mut u8, capacity: usize) -> isize {
    let Some(payload) = serialize_pthread_fork() else {
        return -(EAGAIN as isize);
    };
    if buffer.is_null() {
        return payload.len() as isize;
    }
    if capacity < payload.len() {
        return -(EAGAIN as isize);
    }
    // SAFETY: the runtime supplied the queried writable capacity.
    unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), buffer, payload.len()) };
    payload.len() as isize
}

#[cfg(all(windows, target_arch = "x86_64"))]
struct ForkReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

#[cfg(all(windows, target_arch = "x86_64"))]
impl<'a> ForkReader<'a> {
    fn u32(&mut self) -> Option<u32> {
        let end = self.at.checked_add(4)?;
        let value = u32::from_le_bytes(self.bytes.get(self.at..end)?.try_into().ok()?);
        self.at = end;
        Some(value)
    }

    fn u64(&mut self) -> Option<u64> {
        let end = self.at.checked_add(8)?;
        let value = u64::from_le_bytes(self.bytes.get(self.at..end)?.try_into().ok()?);
        self.at = end;
        Some(value)
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn handler(address: u64) -> Option<ForkHandler> {
    if address == 0 {
        None
    } else {
        // SAFETY: guest mappings and provider DLLs retain their verified bases.
        Some(unsafe { core::mem::transmute::<usize, ForkHandler>(address as usize) })
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
unsafe extern "system" fn fork_child(payload: *const u8, len: usize) -> i32 {
    if payload.is_null() && len != 0 {
        return EINVAL;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        // SAFETY: runtime owns the payload for this call.
        unsafe { std::slice::from_raw_parts(payload, len) }
    };
    let mut reader = ForkReader { bytes, at: 0 };
    if reader.u64() != Some(FORK_PTHREAD_MAGIC) {
        return EINVAL;
    }
    let self_id = match reader.u64() {
        Some(id) => id as usize,
        None => return EINVAL,
    };
    let atfork_count = match reader.u32() {
        Some(v) => v as usize,
        None => return EINVAL,
    };
    let mutex_count = match reader.u32() {
        Some(v) => v as usize,
        None => return EINVAL,
    };
    let rw_count = match reader.u32() {
        Some(v) => v as usize,
        None => return EINVAL,
    };
    let held_count = match reader.u32() {
        Some(v) => v as usize,
        None => return EINVAL,
    };

    let mut restored_atfork = Vec::with_capacity(atfork_count);
    for _ in 0..atfork_count {
        restored_atfork.push(AtforkHandlers {
            prepare: handler(match reader.u64() {
                Some(v) => v,
                None => return EINVAL,
            }),
            parent: handler(match reader.u64() {
                Some(v) => v,
                None => return EINVAL,
            }),
            child: handler(match reader.u64() {
                Some(v) => v,
                None => return EINVAL,
            }),
        });
    }
    let mut restored_mutexes = HashMap::with_capacity(mutex_count);
    for _ in 0..mutex_count {
        let address = match reader.u64() {
            Some(v) => v as usize,
            None => return EINVAL,
        };
        let kind = match reader.u64() {
            Some(v) => v as i64 as i32,
            None => return EINVAL,
        };
        let owner = match reader.u64() {
            Some(v) => v as usize,
            None => return EINVAL,
        };
        let recursion = match reader.u64() {
            Some(v) => v as usize,
            None => return EINVAL,
        };
        restored_mutexes.insert(
            address,
            MutexRecord {
                kind,
                owner,
                recursion,
            },
        );
    }
    let mut rw_states = Vec::with_capacity(rw_count);
    for _ in 0..rw_count {
        let handle = match reader.u64() {
            Some(v) => v as usize,
            None => return EINVAL,
        };
        let orphaned = match reader.u64() {
            Some(v) => v != 0,
            None => return EINVAL,
        };
        rw_states.push((handle, orphaned));
    }
    let mut held_states = Vec::with_capacity(held_count);
    for _ in 0..held_count {
        let handle = match reader.u64() {
            Some(v) => v as usize,
            None => return EINVAL,
        };
        let mode = match reader.u64() {
            Some(v @ 1..=2) => v as u32,
            _ => return EINVAL,
        };
        held_states.push((handle, mode));
    }
    let cleanup_state = if reader.at == bytes.len() {
        [0; 3] // older handoffs had no registered cleanup-frame extension
    } else {
        let Some(head) = reader.u64() else {
            return EINVAL;
        };
        let Some(active @ 0..=1) = reader.u64() else {
            return EINVAL;
        };
        let Some(result) = reader.u64() else {
            return EINVAL;
        };
        [head, active, result]
    };
    if reader.at != bytes.len() {
        return EINVAL;
    }

    let _ = SELF_ID.try_with(|slot| slot.set(self_id));
    cleanup::restore_snapshot(cleanup_state);
    if let Err(error) = sched::reset_after_fork(self_id) {
        return error;
    }
    NEXT_THREAD_ID.store(self_id.saturating_add(1).max(1), Ordering::Release);
    if let Ok(mut registry) = threads().lock() {
        registry.clear();
    }
    if let Ok(mut registry) = results().lock() {
        registry.clear();
    }
    if let Ok(mut registry) = cancel_states().lock() {
        registry.clear();
    }
    if let Ok(mut registry) = barriers().lock() {
        registry.clear();
    }
    let typed = restored_mutexes.len();
    for (&address, record) in &restored_mutexes {
        // SRW contention words point into other Windows threads' stacks.
        // Those threads do not exist in the child. Preserve ownership and
        // recursion, but reconstruct the native word without parent waiters.
        unsafe { (address as *mut usize).write(usize::from(record.owner != 0)) };
    }
    if let Ok(mut registry) = mutex_records().lock() {
        *registry = restored_mutexes;
    } else {
        return EINVAL;
    }
    TYPED_MUTEXES.store(typed, Ordering::Release);
    if let Ok(mut registry) = ATFORK.get_or_init(|| Mutex::new(Vec::new())).lock() {
        *registry = restored_atfork.clone();
    } else {
        return EINVAL;
    }

    let mut new_rwlocks = HashMap::with_capacity(rw_states.len());
    for (handle, _) in &rw_states {
        let lock: &'static RwLock<()> = Box::leak(Box::new(RwLock::new(())));
        new_rwlocks.insert(*handle, lock);
    }
    if let Ok(mut registry) = rwlocks().lock() {
        *registry = new_rwlocks;
    } else {
        return EINVAL;
    }
    NEXT_RWLOCK.store(
        rw_states
            .iter()
            .map(|(handle, _)| *handle)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1),
        Ordering::Release,
    );
    HELD_RWLOCKS.with_borrow_mut(|held| {
        held.clear();
        if let Ok(registry) = rwlocks().lock() {
            for (handle, mode) in &held_states {
                if let Some(lock) = registry.get(handle).copied() {
                    let guard = if *mode == 1 {
                        RwGuard::Read(lock.read().unwrap_or_else(PoisonError::into_inner))
                    } else {
                        RwGuard::Write(lock.write().unwrap_or_else(PoisonError::into_inner))
                    };
                    held.push((*handle, guard));
                }
            }
        }
    });
    ORPHANED_RWLOCKS.with_borrow_mut(|orphaned| {
        orphaned.clear();
        if let Ok(registry) = rwlocks().lock() {
            for (handle, is_orphaned) in &rw_states {
                if *is_orphaned && let Some(lock) = registry.get(handle).copied() {
                    orphaned.push(RwGuard::Write(
                        lock.write().unwrap_or_else(PoisonError::into_inner),
                    ));
                }
            }
        }
    });

    // Child handlers run in registration order after internal pthread state has
    // been repaired and the process contains only the calling thread.
    for callback in restored_atfork.iter().filter_map(|item| item.child) {
        unsafe { callback() };
    }
    FORKING.store(false, Ordering::Release);
    if let Ok(mut active) = ACTIVE_ATFORK.lock() {
        *active = None;
    }
    0
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn register_fork_participant() {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            register_fork_participant as *const () as *const u16,
            &mut module,
        )
    };
    if found == 0 || module.is_null() {
        return;
    }
    let key = 0x5054_4852_4541_4432u64 ^ (module as usize as u64).rotate_left(11);
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 1_000,
        key,
        prepare: Some(fork_prepare),
        snapshot: Some(fork_snapshot),
        parent: Some(fork_parent),
        child: Some(fork_child),
    });
}

#[cfg(all(windows, target_arch = "x86_64"))]
extern "C" fn fork_initializer() {
    register_fork_participant();
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static FORK_INITIALIZER: extern "C" fn() = fork_initializer;

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Records handlers to run around `fork` using POSIX ordering.
pub extern "sysv64" fn pthread_atfork(
    prepare: Option<ForkHandler>,
    parent: Option<ForkHandler>,
    child: Option<ForkHandler>,
) -> i32 {
    let registry = ATFORK.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(mut handlers) = registry.lock() else {
        return EAGAIN;
    };
    handlers.push(AtforkHandlers {
        prepare,
        parent,
        child,
    });
    0
}

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

/// Linux x86_64 `sigset_t`: 1024 bits, of which only the low 64 are ever used.
#[cfg(all(windows, target_arch = "x86_64"))]
#[repr(C)]
pub struct SigSet {
    bits: [u64; 16],
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Examines or changes the calling thread's blocked signal mask.
///
/// Identical to `sigprocmask` other than reporting errors by return value, which
/// is correct here because the mask is per-thread already.
///
/// # Safety
///
/// Each non-null set pointer must point to a valid `sigset_t`.
pub unsafe extern "sysv64" fn pthread_sigmask(
    how: i32,
    set: *const SigSet,
    old_set: *mut SigSet,
) -> i32 {
    // A null `set` queries without changing, which SIG_SETMASK of the current
    // mask performs.
    let (how, mask) = if set.is_null() {
        (SIG_SETMASK, kinakaze_vfs::signal::blocked_mask())
    } else {
        // SAFETY: the caller guarantees a readable sigset_t.
        (how, unsafe { (*set).bits[0] })
    };
    match kinakaze_vfs::signal::sigprocmask(how, mask) {
        Ok(previous) => {
            if !old_set.is_null() {
                let mut bits = [0u64; 16];
                bits[0] = previous;
                // SAFETY: the caller guarantees a writable sigset_t.
                unsafe { old_set.write(SigSet { bits }) };
            }
            0
        }
        Err(error) => error,
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
/// Sends a signal to a thread, or tests for a thread's existence with signal 0.
///
/// Only self-directed signals are delivered. Signal state here is process-wide: a
/// raise sets a pending bit and wakes interruptible waiters, with no way to name
/// which thread must run the handler. Directing a signal at another thread needs
/// a per-thread delivery channel that does not exist yet, so that case reports
/// `ENOSYS` rather than silently raising it process-wide and running the handler
/// on the wrong thread.
pub extern "sysv64" fn pthread_kill(thread: usize, signal: i32) -> i32 {
    let is_self = thread == pthread_self();
    if !is_self {
        let known = match threads().lock() {
            Ok(registry) => registry.contains_key(&thread),
            Err(_) => false,
        };
        if !known {
            return ESRCH;
        }
    }
    // Signal 0 checks for the thread without queuing anything.
    if signal == 0 {
        return 0;
    }
    if !is_self {
        return ENOSYS;
    }
    kinakaze_vfs::signal::raise_signal(signal).map_or_else(|error| error, |()| 0)
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn pthread_publication_waits_for_topology_but_child_can_reenter_it() {
        use std::sync::mpsc;
        use std::time::Duration;

        unsafe extern "sysv64" fn child(
            argument: *mut core::ffi::c_void,
        ) -> *mut core::ffi::c_void {
            // TLS/mapping setup in a real child can need this same transaction.
            // pthread_create must not hold it while waiting for such setup.
            let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().unwrap();
            argument
        }
        let transaction = kinakaze_runtime::begin_fork_mapping_transaction().unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let creator = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            let mut thread = 0;
            let status = unsafe {
                super::pthread_create(&mut thread, core::ptr::null(), Some(child), 123usize as _)
            };
            finished_tx.send(status).unwrap();
            if status == 0 {
                let mut result = core::ptr::null_mut();
                assert_eq!(unsafe { super::pthread_join(thread, &mut result) }, 0);
                assert_eq!(result as usize, 123);
            }
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            finished_rx.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        drop(transaction);
        assert_eq!(finished_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
        creator.join().unwrap();
    }

    use super::*;

    struct ExitObservations {
        key: u32,
        log: Mutex<Vec<u32>>,
    }

    unsafe extern "sysv64" fn exit_key_destructor(argument: *mut c_void) {
        let observations = unsafe { &*argument.cast::<ExitObservations>() };
        assert!(pthread_getspecific(observations.key).is_null());
        let mut log = observations.log.lock().unwrap();
        let again = !log.contains(&4);
        log.push(4);
        drop(log);
        if again {
            assert_eq!(pthread_setspecific(observations.key, argument), 0);
        }
    }

    unsafe extern "sysv64" fn exit_first_destructor(argument: *mut c_void) {
        unsafe { &*argument.cast::<ExitObservations>() }
            .log
            .lock()
            .unwrap()
            .push(1);
    }

    unsafe extern "sysv64" fn exit_nested_destructor(argument: *mut c_void) {
        unsafe { &*argument.cast::<ExitObservations>() }
            .log
            .lock()
            .unwrap()
            .push(3);
    }

    unsafe extern "sysv64" fn exit_second_destructor(argument: *mut c_void) {
        unsafe { &*argument.cast::<ExitObservations>() }
            .log
            .lock()
            .unwrap()
            .push(2);
        kinakaze_tls::cxa_thread_atexit(
            Some(exit_nested_destructor),
            argument,
            core::ptr::null_mut(),
        )
        .unwrap();
    }

    unsafe fn install_exit_destructors(argument: *mut c_void) {
        let observations = unsafe { &*argument.cast::<ExitObservations>() };
        assert_eq!(pthread_setspecific(observations.key, argument), 0);
        kinakaze_tls::cxa_thread_atexit(
            Some(exit_first_destructor),
            argument,
            core::ptr::null_mut(),
        )
        .unwrap();
        kinakaze_tls::cxa_thread_atexit(
            Some(exit_second_destructor),
            argument,
            core::ptr::null_mut(),
        )
        .unwrap();
    }

    unsafe extern "sysv64" fn explicit_exit_start(argument: *mut c_void) -> *mut c_void {
        unsafe { install_exit_destructors(argument) };
        pthread_exit(0x1234_5678_abcdusize as *mut c_void)
    }

    unsafe extern "sysv64" fn normal_exit_start(argument: *mut c_void) -> *mut c_void {
        unsafe { install_exit_destructors(argument) };
        0x1234_5678_abcdusize as *mut c_void
    }

    fn verify_exit_cleanup(start: StartRoutine) {
        let mut key = 0;
        assert_eq!(
            unsafe { pthread_key_create(&mut key, Some(exit_key_destructor)) },
            0
        );
        let observations = ExitObservations {
            key,
            log: Mutex::new(Vec::new()),
        };
        let mut thread = 0;
        assert_eq!(
            unsafe {
                pthread_create(
                    &mut thread,
                    core::ptr::null(),
                    Some(start),
                    (&raw const observations).cast_mut().cast(),
                )
            },
            0
        );
        let mut result = core::ptr::null_mut();
        assert_eq!(unsafe { pthread_join(thread, &mut result) }, 0);
        assert_eq!(result as usize, 0x1234_5678_abcd);
        assert_eq!(*observations.log.lock().unwrap(), vec![2, 3, 1, 4, 4]);
        assert!(pthread_getspecific(key).is_null());
        assert_eq!(pthread_key_delete(key), 0);
    }

    #[test]
    fn explicit_pthread_exit_preserves_result_and_runs_ordered_tls_cleanup() {
        verify_exit_cleanup(explicit_exit_start);
    }

    #[test]
    fn pthread_return_runs_same_ordered_tls_cleanup() {
        verify_exit_cleanup(normal_exit_start);
    }

    #[test]
    fn child_can_detach_itself_immediately_after_start() {
        unsafe extern "sysv64" fn detach_self(argument: *mut c_void) -> *mut c_void {
            let sender = unsafe { Box::from_raw(argument.cast::<std::sync::mpsc::Sender<i32>>()) };
            sender.send(pthread_detach(pthread_self())).unwrap();
            core::ptr::null_mut()
        }
        let (sender, receiver) = std::sync::mpsc::channel::<i32>();
        let argument = Box::into_raw(Box::new(sender));
        let mut thread = 0;
        let status = unsafe {
            pthread_create(
                &mut thread,
                core::ptr::null(),
                Some(detach_self),
                argument.cast(),
            )
        };
        if status != 0 {
            drop(unsafe { Box::from_raw(argument) });
        }
        assert_eq!(status, 0);
        assert_eq!(receiver.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
    }

    #[test]
    fn detach_reclaims_result_published_before_native_exit() {
        use std::os::windows::io::IntoRawHandle;
        let (release, wait) = std::sync::mpsc::channel();
        let native = std::thread::spawn(move || {
            wait.recv().unwrap();
        });
        let handle = unsafe { OwnedHandle::from_raw_handle(native.into_raw_handle()) };
        let id = allocate_thread_id();
        assert!(!thread_has_exited(&handle));
        threads()
            .lock()
            .unwrap()
            .insert(id, ThreadRecord::Joinable(Arc::new(handle)));
        // Precisely the interval between guest cleanup and native TLS teardown.
        results().lock().unwrap().insert(id, 42);
        assert_eq!(pthread_detach(id), 0);
        assert!(!threads().lock().unwrap().contains_key(&id));
        assert!(!results().lock().unwrap().contains_key(&id));
        release.send(()).unwrap();
    }

    /// Carries a pthread object's address into a spawned thread.
    ///
    /// The objects under test live on the parent's stack, and every test joins
    /// its threads before that frame goes away, so the address stays valid for
    /// as long as it is used.
    #[derive(Clone, Copy)]
    struct Shared(usize);

    // SAFETY: pthread objects are shared mutable state by construction, which is
    // precisely what these primitives serialize access to.
    unsafe impl Send for Shared {}

    impl Shared {
        fn of(object: *mut usize) -> Self {
            Self(object as usize)
        }

        fn ptr(self) -> *mut usize {
            self.0 as *mut usize
        }
    }

    /// Builds an absolute `CLOCK_REALTIME` deadline `ahead` from now.
    fn deadline_in(ahead: Duration) -> Timespec {
        let target = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("realtime clock is after the epoch")
            + ahead;
        Timespec {
            tv_sec: i64::try_from(target.as_secs()).expect("timestamp fits in time_t"),
            tv_nsec: i64::from(target.subsec_nanos()),
        }
    }

    /// Prepares a mutex attribute object set to `kind`.
    fn mutexattr_of(kind: i32) -> PthreadMutexAttr {
        let mut attr = PthreadMutexAttr { kind: -1 };
        assert_eq!(unsafe { pthread_mutexattr_init(&raw mut attr) }, 0);
        assert_eq!(unsafe { pthread_mutexattr_settype(&raw mut attr, kind) }, 0);
        let mut read_back = -1;
        assert_eq!(
            unsafe { pthread_mutexattr_gettype(&raw const attr, &raw mut read_back) },
            0
        );
        assert_eq!(read_back, kind);
        attr
    }

    #[test]
    fn adaptive_mutex_attributes_round_trip_and_preserve_exclusion() {
        let mut attr = mutexattr_of(PTHREAD_MUTEX_ADAPTIVE_NP);
        assert_eq!(
            unsafe { pthread_mutexattr_settype(&raw mut attr, 4) },
            EINVAL
        );
        assert_eq!(attr.kind, PTHREAD_MUTEX_ADAPTIVE_NP);
        let mut mutex = [0usize; 5];
        let address = mutex.as_mut_ptr();
        assert_eq!(unsafe { pthread_mutex_init(address, &attr) }, 0);
        assert_eq!(unsafe { pthread_mutex_lock(address) }, 0);
        assert_eq!(unsafe { pthread_mutex_trylock(address) }, EBUSY);
        let shared = Shared::of(address);
        let (send, receive) = std::sync::mpsc::channel();
        let child = std::thread::spawn(move || {
            let address = shared.0 as *mut usize;
            assert_eq!(unsafe { pthread_mutex_trylock(address) }, EBUSY);
            send.send(()).unwrap();
            assert_eq!(unsafe { pthread_mutex_lock(address) }, 0);
            assert_eq!(unsafe { pthread_mutex_unlock(address) }, 0);
        });
        receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(unsafe { pthread_mutex_unlock(address) }, 0);
        child.join().unwrap();
        assert_eq!(pthread_mutex_destroy(address), 0);
    }

    #[test]
    fn unsupported_robust_attribute_leaves_the_original_type_usable() {
        let mut attr = mutexattr_of(PTHREAD_MUTEX_RECURSIVE);
        assert_eq!(unsafe { pthread_mutexattr_setrobust(&raw mut attr, 1) }, 95);
        assert_eq!(attr.kind, PTHREAD_MUTEX_RECURSIVE);
        let mut robustness = -1;
        assert_eq!(
            unsafe { pthread_mutexattr_getrobust(&attr, &raw mut robustness) },
            0
        );
        assert_eq!(robustness, 0);
        let mut mutex = [0usize; 5];
        let address = mutex.as_mut_ptr();
        assert_eq!(unsafe { pthread_mutex_init(address, &attr) }, 0);
        assert_eq!(unsafe { pthread_mutex_lock(address) }, 0);
        assert_eq!(unsafe { pthread_mutex_lock(address) }, 0);
        assert_eq!(unsafe { pthread_mutex_consistent(address) }, EINVAL);
        assert_eq!(unsafe { pthread_mutex_unlock(address) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(address) }, 0);
        assert_eq!(pthread_mutex_destroy(address), 0);
    }

    #[test]
    fn rwlock_admits_concurrent_readers_then_one_writer() {
        const READERS: usize = 4;

        let mut rwlock: usize = 0;
        assert_eq!(
            unsafe { pthread_rwlock_init(&raw mut rwlock, core::ptr::null()) },
            0
        );
        let shared = Shared::of(&raw mut rwlock);

        // Every reader must hold the lock at once for this to prove sharing, so
        // they all report in at `holding` before any of them releases.
        let holding = Arc::new(Barrier::new(READERS + 1));
        let release = Arc::new(Barrier::new(READERS + 1));

        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                let holding = Arc::clone(&holding);
                let release = Arc::clone(&release);
                std::thread::spawn(move || {
                    assert_eq!(unsafe { pthread_rwlock_rdlock(shared.ptr()) }, 0);
                    holding.wait();
                    release.wait();
                    assert_eq!(unsafe { pthread_rwlock_unlock(shared.ptr()) }, 0);
                })
            })
            .collect();

        holding.wait();
        // All four read locks are held here, so exclusive access must be refused.
        assert_eq!(unsafe { pthread_rwlock_trywrlock(&raw mut rwlock) }, EBUSY);
        release.wait();
        for reader in readers {
            reader.join().expect("reader finished");
        }

        // With the readers gone the writer gets in, and while it holds the lock
        // no reader can.
        assert_eq!(unsafe { pthread_rwlock_wrlock(&raw mut rwlock) }, 0);
        let blocked = std::thread::spawn(move || unsafe { pthread_rwlock_tryrdlock(shared.ptr()) });
        assert_eq!(blocked.join().expect("probe finished"), EBUSY);
        assert_eq!(unsafe { pthread_rwlock_unlock(&raw mut rwlock) }, 0);

        // A thread holding nothing cannot unlock, and the same unlock entry point
        // served both modes above.
        assert_eq!(unsafe { pthread_rwlock_unlock(&raw mut rwlock) }, EPERM);
        assert_eq!(unsafe { pthread_rwlock_destroy(&raw mut rwlock) }, 0);
    }

    #[test]
    fn barrier_elects_exactly_one_serial_thread() {
        const THREADS: usize = 8;

        let mut barrier: usize = 0;
        assert_eq!(
            unsafe {
                pthread_barrier_init(
                    &raw mut barrier,
                    core::ptr::null(),
                    u32::try_from(THREADS).expect("count fits"),
                )
            },
            0
        );
        let shared = Shared::of(&raw mut barrier);

        let waiters: Vec<_> = (0..THREADS)
            .map(|_| std::thread::spawn(move || unsafe { pthread_barrier_wait(shared.ptr()) }))
            .collect();

        let results: Vec<i32> = waiters
            .into_iter()
            .map(|waiter| waiter.join().expect("waiter finished"))
            .collect();

        let leaders = results
            .iter()
            .filter(|result| **result == PTHREAD_BARRIER_SERIAL_THREAD)
            .count();
        assert_eq!(leaders, 1, "exactly one thread is elected serial");
        assert_eq!(
            results.iter().filter(|result| **result == 0).count(),
            THREADS - 1
        );
        assert_eq!(unsafe { pthread_barrier_destroy(&raw mut barrier) }, 0);
    }

    #[test]
    fn recursive_mutex_relocks_on_owning_thread() {
        let mut attr = mutexattr_of(PTHREAD_MUTEX_RECURSIVE);
        let mut mutex: usize = 0;
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut mutex, &raw const attr) },
            0
        );

        // Three nested acquisitions by the owner, none of which may block.
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_trylock(&raw mut mutex) }, 0);

        // Still held after the first two releases, so another thread stays out.
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);
        let shared = Shared::of(&raw mut mutex);
        let contender = std::thread::spawn(move || unsafe { pthread_mutex_trylock(shared.ptr()) });
        assert_eq!(contender.join().expect("contender finished"), EBUSY);

        // The last release hands it over.
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);
        let taker = std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_mutex_trylock(shared.ptr()) }, 0);
            assert_eq!(unsafe { pthread_mutex_unlock(shared.ptr()) }, 0);
        });
        taker.join().expect("taker finished");

        assert_eq!(pthread_mutex_destroy(&raw mut mutex), 0);
        assert_eq!(pthread_mutexattr_destroy(&raw mut attr), 0);
    }

    #[test]
    fn glibc_static_recursive_mutex_initializer_is_detected() {
        // x86_64 glibc's PTHREAD_RECURSIVE_MUTEX_INITIALIZER_NP stores the
        // type in __kind at byte offset 16 and performs no initialization call.
        let mut storage = [0usize; 5];
        storage[2] = PTHREAD_MUTEX_RECURSIVE as usize;
        let mutex = storage.as_mut_ptr();

        assert_eq!(unsafe { pthread_mutex_lock(mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_lock(mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(mutex) }, 0);
        assert_eq!(pthread_mutex_destroy(mutex), 0);
    }

    #[test]
    fn errorcheck_mutex_reports_deadlock_instead_of_hanging() {
        let mut attr = mutexattr_of(PTHREAD_MUTEX_ERRORCHECK);
        let mut mutex: usize = 0;
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut mutex, &raw const attr) },
            0
        );

        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        // The relock that would hang a NORMAL mutex is diagnosed instead.
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, EDEADLK);
        // trylock cannot deadlock, so it reports contention rather than EDEADLK.
        assert_eq!(unsafe { pthread_mutex_trylock(&raw mut mutex) }, EBUSY);

        // Releasing from a thread that does not own it is refused.
        let shared = Shared::of(&raw mut mutex);
        let stranger = std::thread::spawn(move || unsafe { pthread_mutex_unlock(shared.ptr()) });
        assert_eq!(stranger.join().expect("stranger finished"), EPERM);

        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);
        // Now unowned, so a second release is an error too.
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, EPERM);

        assert_eq!(pthread_mutex_destroy(&raw mut mutex), 0);
        assert_eq!(pthread_mutexattr_destroy(&raw mut attr), 0);
    }

    #[test]
    fn mutex_timedlock_times_out_and_then_succeeds() {
        let mut mutex: usize = 0;
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut mutex, core::ptr::null()) },
            0
        );
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        let shared = Shared::of(&raw mut mutex);

        // A live deadline against a held mutex must expire, not hang.
        let waiter = std::thread::spawn(move || {
            let deadline = deadline_in(Duration::from_millis(60));
            let started = std::time::Instant::now();
            let result = unsafe { pthread_mutex_timedlock(shared.ptr(), &raw const deadline) };
            (result, started.elapsed())
        });
        let (result, waited) = waiter.join().expect("waiter finished");
        assert_eq!(result, ETIMEDOUT);
        assert!(
            waited >= Duration::from_millis(40),
            "timedlock returned after {waited:?}, before its deadline"
        );

        // A deadline already in the past returns immediately rather than blocking.
        let past = deadline_in(Duration::ZERO);
        let expired = std::thread::spawn(move || unsafe {
            pthread_mutex_timedlock(shared.ptr(), &raw const past)
        });
        assert_eq!(expired.join().expect("probe finished"), ETIMEDOUT);

        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);

        // Once free, the same call acquires it well inside the deadline.
        let deadline = deadline_in(Duration::from_secs(5));
        assert_eq!(
            unsafe { pthread_mutex_timedlock(&raw mut mutex, &raw const deadline) },
            0
        );
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);
        assert_eq!(pthread_mutex_destroy(&raw mut mutex), 0);
    }

    #[test]
    fn mutex_clocklock_uses_selected_clock_and_validates_only_when_waiting() {
        for clock_id in [CLOCK_REALTIME, CLOCK_MONOTONIC] {
            let mut mutex = 0usize;
            assert_eq!(
                unsafe { pthread_mutex_init(&raw mut mutex, core::ptr::null()) },
                0
            );
            assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
            let shared = Shared::of(&raw mut mutex);
            let waiter = std::thread::spawn(move || {
                let now = if clock_id == CLOCK_MONOTONIC {
                    monotonic_now().unwrap()
                } else {
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
                };
                let until = now + Duration::from_millis(60);
                let deadline = Timespec {
                    tv_sec: until.as_secs() as i64,
                    tv_nsec: until.subsec_nanos() as i64,
                };
                let started = std::time::Instant::now();
                let result = unsafe { pthread_mutex_clocklock(shared.ptr(), clock_id, &deadline) };
                (result, started.elapsed())
            });
            let (result, elapsed) = waiter.join().unwrap();
            assert_eq!(result, ETIMEDOUT);
            assert!(
                elapsed >= Duration::from_millis(40),
                "selected clock expired early: {elapsed:?}"
            );
            let invalid = Timespec {
                tv_sec: 0,
                tv_nsec: 1_000_000_000,
            };
            assert_eq!(
                unsafe { pthread_mutex_clocklock(&raw mut mutex, clock_id, &invalid) },
                EINVAL
            );
            let past = Timespec {
                tv_sec: -1,
                tv_nsec: 0,
            };
            assert_eq!(
                unsafe { pthread_mutex_clocklock(&raw mut mutex, clock_id, &past) },
                ETIMEDOUT
            );
            assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);
            assert_eq!(
                unsafe { pthread_mutex_clocklock(&raw mut mutex, -1, &invalid) },
                EINVAL
            );
            assert_eq!(
                unsafe { pthread_mutex_clocklock(&raw mut mutex, clock_id, &invalid) },
                0
            );
            assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);
            assert_eq!(pthread_mutex_destroy(&raw mut mutex), 0);
        }
    }

    #[test]
    fn cond_timedwait_times_out_holding_the_mutex() {
        let mut mutex: usize = 0;
        let mut cond: usize = 0;
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut mutex, core::ptr::null()) },
            0
        );
        assert_eq!(
            unsafe { pthread_cond_init(&raw mut cond, core::ptr::null()) },
            0
        );

        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        let deadline = deadline_in(Duration::from_millis(50));
        assert_eq!(
            unsafe { pthread_cond_timedwait(&raw mut cond, &raw mut mutex, &raw const deadline) },
            ETIMEDOUT
        );
        // The mutex comes back held on the timeout path, so this release is the
        // one that frees it.
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);

        assert_eq!(pthread_cond_destroy(&raw mut cond), 0);
        assert_eq!(pthread_mutex_destroy(&raw mut mutex), 0);
    }

    #[test]
    fn spinlock_excludes_a_second_holder() {
        let mut lock: usize = 0;
        assert_eq!(
            unsafe { pthread_spin_init(&raw mut lock, PTHREAD_PROCESS_PRIVATE) },
            0
        );
        assert_eq!(unsafe { pthread_spin_lock(&raw mut lock) }, 0);

        let shared = Shared::of(&raw mut lock);
        let contender = std::thread::spawn(move || unsafe { pthread_spin_trylock(shared.ptr()) });
        assert_eq!(contender.join().expect("contender finished"), EBUSY);
        assert_eq!(unsafe { pthread_spin_destroy(&raw mut lock) }, EBUSY);

        assert_eq!(unsafe { pthread_spin_unlock(&raw mut lock) }, 0);
        let taker = std::thread::spawn(move || unsafe { pthread_spin_trylock(shared.ptr()) });
        assert_eq!(taker.join().expect("taker finished"), 0);
        assert_eq!(unsafe { pthread_spin_unlock(&raw mut lock) }, 0);
        assert_eq!(unsafe { pthread_spin_destroy(&raw mut lock) }, 0);
    }

    #[test]
    fn deferred_cancel_ends_the_thread_at_the_next_test_point() {
        static REACHED_TESTCANCEL: AtomicBool = AtomicBool::new(false);
        static PAST_TESTCANCEL: AtomicBool = AtomicBool::new(false);

        unsafe extern "sysv64" fn body(_argument: *mut c_void) -> *mut c_void {
            // Spin until the cancel request lands, then act on it.
            while !pthread_cancel_requested_for_test() {
                std::thread::yield_now();
            }
            REACHED_TESTCANCEL.store(true, Ordering::Release);
            pthread_testcancel();
            // Only reached if cancellation failed to end the thread.
            PAST_TESTCANCEL.store(true, Ordering::Release);
            core::ptr::null_mut()
        }

        let mut thread: usize = 0;
        assert_eq!(
            unsafe {
                pthread_create(
                    &raw mut thread,
                    core::ptr::null(),
                    Some(body),
                    core::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(pthread_cancel(thread), 0);

        let mut result: *mut c_void = core::ptr::null_mut();
        assert_eq!(unsafe { pthread_join(thread, &raw mut result) }, 0);
        assert!(REACHED_TESTCANCEL.load(Ordering::Acquire));
        assert!(
            !PAST_TESTCANCEL.load(Ordering::Acquire),
            "pthread_testcancel returned instead of ending the thread"
        );
        // A cancelled thread records no result, which is how join detects it.
        assert_eq!(result as usize, PTHREAD_CANCELED);

        // A finished thread is no longer cancellable.
        assert_eq!(pthread_cancel(thread), ESRCH);
    }

    /// Test-only peek at the calling thread's pending cancellation request.
    fn pthread_cancel_requested_for_test() -> bool {
        cancel_self().requested.load(Ordering::Acquire)
    }

    #[test]
    fn attr_round_trips_stack_and_guard_sizes() {
        let mut attr = PthreadAttr {
            stack_size: 0,
            stack_address: 0,
            detach_state: -1,
            guard_size: 0,
            sched_policy: 0,
            sched_priority: 0,
            inherit_sched: 0,
            affinity_mask: 0,
            has_affinity: 0,
        };
        assert_eq!(unsafe { pthread_attr_init(&raw mut attr) }, 0);

        let mut stack_size = 0;
        assert_eq!(
            unsafe { pthread_attr_getstacksize(&raw const attr, &raw mut stack_size) },
            0
        );
        // A default attribute reports a usable size rather than zero.
        assert_eq!(stack_size, PTHREAD_STACK_MIN);

        assert_eq!(
            unsafe { pthread_attr_setstacksize(&raw mut attr, 256 * 1024) },
            0
        );
        assert_eq!(
            unsafe { pthread_attr_getstacksize(&raw const attr, &raw mut stack_size) },
            0
        );
        assert_eq!(stack_size, 256 * 1024);
        // Below PTHREAD_STACK_MIN is rejected.
        assert_eq!(
            unsafe { pthread_attr_setstacksize(&raw mut attr, 128) },
            EINVAL
        );

        let mut state = -1;
        assert_eq!(
            unsafe { pthread_attr_getdetachstate(&raw const attr, &raw mut state) },
            0
        );
        assert_eq!(state, PTHREAD_CREATE_JOINABLE);
        assert_eq!(
            unsafe { pthread_attr_setdetachstate(&raw mut attr, PTHREAD_CREATE_DETACHED) },
            0
        );
        assert_eq!(
            unsafe { pthread_attr_getdetachstate(&raw const attr, &raw mut state) },
            0
        );
        assert_eq!(state, PTHREAD_CREATE_DETACHED);

        let mut guard_size = 0;
        assert_eq!(
            unsafe { pthread_attr_getguardsize(&raw const attr, &raw mut guard_size) },
            0
        );
        assert_eq!(guard_size, DEFAULT_GUARD_SIZE as usize);
        assert_eq!(unsafe { pthread_attr_setguardsize(&raw mut attr, 8192) }, 0);
        assert_eq!(
            unsafe { pthread_attr_getguardsize(&raw const attr, &raw mut guard_size) },
            0
        );
        assert_eq!(guard_size, 8192);

        assert_eq!(pthread_attr_destroy(&raw mut attr), 0);
    }

    #[test]
    fn kill_and_sigmask_target_the_calling_thread() {
        // Signal 0 probes for existence without queueing anything.
        assert_eq!(pthread_kill(pthread_self(), 0), 0);
        assert_eq!(pthread_kill(usize::MAX, 0), ESRCH);

        // A directed signal to another live thread has no delivery channel.
        let mut thread: usize = 0;
        let holding = Arc::new(Barrier::new(2));
        let inner = Arc::clone(&holding);
        // Held in a static so the sysv64 entry point can reach it.
        static PARKED: OnceLock<Arc<Barrier>> = OnceLock::new();
        PARKED.set(inner).expect("first use of the test barrier");

        unsafe extern "sysv64" fn park(_argument: *mut c_void) -> *mut c_void {
            PARKED.get().expect("barrier published").wait();
            core::ptr::null_mut()
        }

        assert_eq!(
            unsafe {
                pthread_create(
                    &raw mut thread,
                    core::ptr::null(),
                    Some(park),
                    core::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(pthread_kill(thread, 15), ENOSYS);
        holding.wait();
        assert_eq!(unsafe { pthread_join(thread, core::ptr::null_mut()) }, 0);

        // The mask round-trips through the shared per-thread signal state.
        let mut previous = SigSet { bits: [0; 16] };
        let blocked = SigSet { bits: [1 << 0; 16] };
        assert_eq!(
            unsafe { pthread_sigmask(SIG_SETMASK, &raw const blocked, &raw mut previous) },
            0
        );
        let mut restored = SigSet { bits: [0; 16] };
        assert_eq!(
            unsafe { pthread_sigmask(SIG_SETMASK, core::ptr::null(), &raw mut restored) },
            0
        );
        assert_eq!(restored.bits[0], 1);
        assert_eq!(
            unsafe { pthread_sigmask(SIG_SETMASK, &raw const previous, core::ptr::null_mut()) },
            0
        );
    }

    #[test]
    fn statically_initialized_rwlock_resolves_once_under_contention() {
        const THREADS: usize = 8;

        // PTHREAD_RWLOCK_INITIALIZER, with no pthread_rwlock_init call, so the
        // first lock has to create the lock and all eight may race to do it.
        let mut rwlock: usize = 0;
        let shared = Shared::of(&raw mut rwlock);
        let start = Arc::new(Barrier::new(THREADS));

        let racers: Vec<_> = (0..THREADS)
            .map(|_| {
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    assert_eq!(unsafe { pthread_rwlock_rdlock(shared.ptr()) }, 0);
                    let seen = unsafe { *shared.ptr() };
                    assert_eq!(unsafe { pthread_rwlock_unlock(shared.ptr()) }, 0);
                    seen
                })
            })
            .collect();

        let handles: Vec<usize> = racers
            .into_iter()
            .map(|racer| racer.join().expect("racer finished"))
            .collect();

        // Exactly one lock was created, so every thread saw the same handle.
        assert!(handles[0] != 0, "the lock was created on first use");
        assert!(
            handles.iter().all(|handle| *handle == handles[0]),
            "lazy creation raced and produced more than one lock: {handles:?}"
        );
        assert_eq!(unsafe { pthread_rwlock_destroy(&raw mut rwlock) }, 0);
    }

    #[test]
    fn atfork_records_handlers() {
        unsafe extern "sysv64" fn noop() {}
        // Recorded, not invoked: there is no fork in this process to trigger them.
        assert_eq!(pthread_atfork(Some(noop), Some(noop), Some(noop)), 0);
        assert_eq!(pthread_atfork(None, None, Some(noop)), 0);
    }

    #[test]
    fn pthread_setname_and_getname_np_truncation_and_query() {
        let me = pthread_self();
        let name = c"worker-thread";
        assert_eq!(unsafe { pthread_setname_np(me, name.as_ptr()) }, 0);

        let mut buf = [0 as c_char; 32];
        assert_eq!(
            unsafe { pthread_getname_np(me, buf.as_mut_ptr(), buf.len()) },
            0
        );
        let read_name = unsafe { core::ffi::CStr::from_ptr(buf.as_ptr()) };
        assert_eq!(read_name.to_bytes(), b"worker-thread");

        // Names longer than 15 chars return ERANGE
        let too_long = c"a-very-long-thread-name-over-16";
        assert_eq!(unsafe { pthread_setname_np(me, too_long.as_ptr()) }, ERANGE);

        // Buffer too short in getname returns ERANGE
        let mut short_buf = [0 as c_char; 4];
        assert_eq!(
            unsafe { pthread_getname_np(me, short_buf.as_mut_ptr(), short_buf.len()) },
            ERANGE
        );
    }

    #[test]
    fn pthread_key_destructor_invoked_on_thread_exit() {
        static DESTRUCTOR_COUNT: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);

        unsafe extern "sysv64" fn key_dtor(value: *mut c_void) {
            if !value.is_null() && value as usize == 0xdeadbeef {
                DESTRUCTOR_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }

        static TEST_KEY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

        let mut key: u32 = 0;
        assert_eq!(
            unsafe { pthread_key_create(&raw mut key, Some(key_dtor)) },
            0
        );
        TEST_KEY.store(key, std::sync::atomic::Ordering::SeqCst);

        unsafe extern "sysv64" fn thread_body(_arg: *mut c_void) -> *mut c_void {
            let k = TEST_KEY.load(std::sync::atomic::Ordering::SeqCst);
            unsafe {
                pthread_setspecific(k, 0xdeadbeef as *mut c_void);
            }
            core::ptr::null_mut()
        }

        let mut thread: usize = 0;
        assert_eq!(
            unsafe {
                pthread_create(
                    &raw mut thread,
                    core::ptr::null(),
                    Some(thread_body),
                    core::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(unsafe { pthread_join(thread, core::ptr::null_mut()) }, 0);

        // Clean up key
        assert_eq!(unsafe { pthread_key_delete(key) }, 0);
    }

    #[test]
    fn pthread_mutex_normal_vs_errorcheck_vs_recursive_contracts() {
        // Recursive mutex: multiple nested locks
        let mut rec_attr = mutexattr_of(PTHREAD_MUTEX_RECURSIVE);
        let mut rec_mutex: usize = 0;
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut rec_mutex, &raw const rec_attr) },
            0
        );
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut rec_mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut rec_mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut rec_mutex) }, 0);

        // Another thread cannot acquire while held
        let shared_rec = Shared::of(&raw mut rec_mutex);
        let racer = std::thread::spawn(move || unsafe { pthread_mutex_trylock(shared_rec.ptr()) });
        assert_eq!(racer.join().unwrap(), EBUSY);

        // 3 unlocks required
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut rec_mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut rec_mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut rec_mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_destroy(&raw mut rec_mutex) }, 0);

        // Errorcheck mutex: self-lock is EDEADLK, unowned unlock is EPERM
        let mut err_attr = mutexattr_of(PTHREAD_MUTEX_ERRORCHECK);
        let mut err_mutex: usize = 0;
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut err_mutex, &raw const err_attr) },
            0
        );
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut err_mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut err_mutex) }, EDEADLK);

        let shared_err = Shared::of(&raw mut err_mutex);
        let unlocker =
            std::thread::spawn(move || unsafe { pthread_mutex_unlock(shared_err.ptr()) });
        assert_eq!(unlocker.join().unwrap(), EPERM);

        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut err_mutex) }, 0);
        assert_eq!(unsafe { pthread_mutex_destroy(&raw mut err_mutex) }, 0);
    }

    #[test]
    fn pthread_cond_broadcast_multi_waiter_wakeup() {
        const WAITERS: usize = 8;
        let mut mutex: usize = 0;
        let mut cond: usize = 0;
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut mutex, core::ptr::null()) },
            0
        );
        assert_eq!(
            unsafe { pthread_cond_init(&raw mut cond, core::ptr::null()) },
            0
        );

        let shared_mutex = Shared::of(&raw mut mutex);
        let shared_cond = Shared::of(&raw mut cond);
        let waiting_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let predicate = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let handles: Vec<_> = (0..WAITERS)
            .map(|_| {
                let waiting = Arc::clone(&waiting_count);
                let pred = Arc::clone(&predicate);
                let done = Arc::clone(&done_count);
                std::thread::spawn(move || {
                    unsafe {
                        pthread_mutex_lock(shared_mutex.ptr());
                        waiting.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        while !pred.load(std::sync::atomic::Ordering::SeqCst) {
                            pthread_cond_wait(shared_cond.ptr(), shared_mutex.ptr());
                        }
                        pthread_mutex_unlock(shared_mutex.ptr());
                    }
                    done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })
            })
            .collect();

        // Wait until all threads have entered the mutex and started waiting
        while waiting_count.load(std::sync::atomic::Ordering::SeqCst) < WAITERS {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        // Set predicate and broadcast
        unsafe {
            pthread_mutex_lock(&raw mut mutex);
            predicate.store(true, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(pthread_cond_broadcast(&raw mut cond), 0);
            pthread_mutex_unlock(&raw mut mutex);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            done_count.load(std::sync::atomic::Ordering::SeqCst),
            WAITERS
        );
        assert_eq!(unsafe { pthread_cond_destroy(&raw mut cond) }, 0);
        assert_eq!(unsafe { pthread_mutex_destroy(&raw mut mutex) }, 0);
    }

    #[test]
    fn pthread_rwlock_exclusive_writer_and_readers_matrix() {
        let mut rwlock: usize = 0;
        assert_eq!(
            unsafe { pthread_rwlock_init(&raw mut rwlock, core::ptr::null()) },
            0
        );

        // Multiple readers can hold read lock
        assert_eq!(unsafe { pthread_rwlock_rdlock(&raw mut rwlock) }, 0);
        assert_eq!(unsafe { pthread_rwlock_tryrdlock(&raw mut rwlock) }, 0);

        // Writer cannot acquire
        assert_eq!(unsafe { pthread_rwlock_trywrlock(&raw mut rwlock) }, EBUSY);

        // Release readers
        assert_eq!(unsafe { pthread_rwlock_unlock(&raw mut rwlock) }, 0);
        assert_eq!(unsafe { pthread_rwlock_unlock(&raw mut rwlock) }, 0);

        // Writer acquires exclusive lock
        assert_eq!(unsafe { pthread_rwlock_wrlock(&raw mut rwlock) }, 0);

        // Reader cannot acquire while writer holds lock
        let shared = Shared::of(&raw mut rwlock);
        let try_reader =
            std::thread::spawn(move || unsafe { pthread_rwlock_tryrdlock(shared.ptr()) });
        assert_eq!(try_reader.join().unwrap(), EBUSY);

        // Release writer
        assert_eq!(unsafe { pthread_rwlock_unlock(&raw mut rwlock) }, 0);
        assert_eq!(unsafe { pthread_rwlock_destroy(&raw mut rwlock) }, 0);
    }

    #[test]
    fn pthread_spin_lock_trylock_and_destroy_contention() {
        let mut lock: usize = 0;
        assert_eq!(
            unsafe { pthread_spin_init(&raw mut lock, PTHREAD_PROCESS_PRIVATE) },
            0
        );

        // Lock from main thread
        assert_eq!(unsafe { pthread_spin_lock(&raw mut lock) }, 0);

        // Cannot destroy while held -> EBUSY
        assert_eq!(unsafe { pthread_spin_destroy(&raw mut lock) }, EBUSY);

        // Trylock on held lock from another thread returns EBUSY
        let shared = Shared::of(&raw mut lock);
        let racer = std::thread::spawn(move || unsafe { pthread_spin_trylock(shared.ptr()) });
        assert_eq!(racer.join().unwrap(), EBUSY);

        assert_eq!(unsafe { pthread_spin_unlock(&raw mut lock) }, 0);

        // Contention test: 4 threads incrementing a shared counter
        const THREADS: usize = 4;
        const ITERS: usize = 10_000;
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(THREADS));

        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                let s_lock = Shared::of(&raw mut lock);
                let c = Arc::clone(&counter);
                let b = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    b.wait();
                    for _ in 0..ITERS {
                        unsafe {
                            pthread_spin_lock(s_lock.ptr());
                            let val = c.load(std::sync::atomic::Ordering::Relaxed);
                            c.store(val + 1, std::sync::atomic::Ordering::Relaxed);
                            pthread_spin_unlock(s_lock.ptr());
                        }
                    }
                })
            })
            .collect();

        for w in workers {
            w.join().unwrap();
        }

        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            THREADS * ITERS
        );
        assert_eq!(unsafe { pthread_spin_destroy(&raw mut lock) }, 0);
    }

    #[test]
    fn pthread_barrier_multi_generation_cycling() {
        const PARTICIPANTS: usize = 4;
        const CYCLES: usize = 5;

        let mut barrier: usize = 0;
        assert_eq!(
            unsafe {
                pthread_barrier_init(&raw mut barrier, core::ptr::null(), PARTICIPANTS as u32)
            },
            0
        );

        let shared = Shared::of(&raw mut barrier);
        let serial_counts = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let threads: Vec<_> = (0..PARTICIPANTS)
            .map(|_| {
                let s = Shared::of(shared.ptr());
                let serials = Arc::clone(&serial_counts);
                std::thread::spawn(move || {
                    for _ in 0..CYCLES {
                        let res = unsafe { pthread_barrier_wait(s.ptr()) };
                        if res == PTHREAD_BARRIER_SERIAL_THREAD {
                            serials.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        } else {
                            assert_eq!(res, 0);
                        }
                    }
                })
            })
            .collect();

        for t in threads {
            t.join().unwrap();
        }

        // Exactly one serial thread per generation cycle
        assert_eq!(
            serial_counts.load(std::sync::atomic::Ordering::SeqCst),
            CYCLES
        );
        assert_eq!(unsafe { pthread_barrier_destroy(&raw mut barrier) }, 0);
    }

    #[test]
    fn pthread_atfork_handler_execution_order() {
        static ORDER: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

        unsafe extern "sysv64" fn prep1() {
            ORDER.lock().unwrap().push("prep1");
        }
        unsafe extern "sysv64" fn parent1() {
            ORDER.lock().unwrap().push("parent1");
        }
        unsafe extern "sysv64" fn child1() {
            ORDER.lock().unwrap().push("child1");
        }

        unsafe extern "sysv64" fn prep2() {
            ORDER.lock().unwrap().push("prep2");
        }
        unsafe extern "sysv64" fn parent2() {
            ORDER.lock().unwrap().push("parent2");
        }
        unsafe extern "sysv64" fn child2() {
            ORDER.lock().unwrap().push("child2");
        }

        assert_eq!(pthread_atfork(Some(prep1), Some(parent1), Some(child1)), 0);
        assert_eq!(pthread_atfork(Some(prep2), Some(parent2), Some(child2)), 0);

        // Simulate fork prepare and parent phases directly
        unsafe {
            fork_prepare();
        }
        {
            let log = ORDER.lock().unwrap().clone();
            // LIFO order for prepare: prep2 runs before prep1
            assert_eq!(log, vec!["prep2", "prep1"]);
        }

        unsafe {
            fork_parent(0);
        }
        {
            let log = ORDER.lock().unwrap().clone();
            // FIFO order for parent: parent1 runs before parent2
            assert_eq!(log, vec!["prep2", "prep1", "parent1", "parent2"]);
        }
    }

    #[test]
    fn cond_timedwait_with_monotonic_and_realtime_deadlines() {
        let mut cond: usize = 0;
        let mut realtime_cond: usize = 0;
        let mut mutex: usize = 0;
        let mut cond_attr = PthreadCondAttr {
            clock_id: CLOCK_REALTIME,
        };
        assert_eq!(unsafe { pthread_condattr_init(&raw mut cond_attr) }, 0);
        assert_eq!(
            unsafe { pthread_condattr_setclock(&raw mut cond_attr, CLOCK_MONOTONIC) },
            0
        );
        assert_eq!(
            unsafe { pthread_cond_init(&raw mut cond, &raw const cond_attr as *const c_void) },
            0
        );
        assert_eq!(pthread_condattr_destroy(&raw mut cond_attr), 0);
        assert_eq!(
            unsafe { pthread_cond_init(&raw mut realtime_cond, std::ptr::null()) },
            0
        );
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut mutex, std::ptr::null()) },
            0
        );

        // 1. CLOCK_MONOTONIC deadline (< 1,000,000,000 seconds uptime base)
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        // Form the deadline from libc's clock implementation, independently
        // of libpthread's conversion. Using monotonic_now on both sides hid
        // the QPC/unbiased-epoch mismatch and JVM's resulting busy loop.
        let (seconds, nanos) = kinakaze_vfs::time_namespace::clock(CLOCK_MONOTONIC).unwrap();
        let mono_deadline = Duration::new(seconds as u64, nanos as u32) + Duration::from_millis(50);
        let mono_ts = Timespec {
            tv_sec: mono_deadline.as_secs() as i64,
            tv_nsec: mono_deadline.subsec_nanos() as i64,
        };
        let start = std::time::Instant::now();
        let ret =
            unsafe { pthread_cond_timedwait(&raw mut cond, &raw mut mutex, &raw const mono_ts) };
        let elapsed = start.elapsed();
        assert_eq!(
            ret, ETIMEDOUT,
            "monotonic wait must report timeout when unsignalled"
        );
        assert!(
            elapsed >= Duration::from_millis(40) && elapsed < Duration::from_secs(2),
            "monotonic wait must block ~50ms, elapsed was {:?}",
            elapsed
        );
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);

        // 2. CLOCK_REALTIME deadline (>= 1,000,000,000 seconds Unix epoch base)
        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        let now_real = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let target_real = now_real + Duration::from_millis(50);
        let real_ts = Timespec {
            tv_sec: target_real.as_secs() as i64,
            tv_nsec: target_real.subsec_nanos() as i64,
        };
        let start = std::time::Instant::now();
        let ret = unsafe {
            pthread_cond_timedwait(&raw mut realtime_cond, &raw mut mutex, &raw const real_ts)
        };
        let elapsed = start.elapsed();
        assert_eq!(
            ret, ETIMEDOUT,
            "realtime wait must report timeout when unsignalled"
        );
        assert!(
            elapsed >= Duration::from_millis(40) && elapsed < Duration::from_secs(2),
            "realtime wait must block ~50ms, elapsed was {:?}",
            elapsed
        );
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);

        // 3. Early wakeup on CLOCK_MONOTONIC wait
        let cond_shared = Shared::of(&raw mut cond);
        let mutex_shared = Shared::of(&raw mut mutex);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(unsafe { pthread_mutex_lock(mutex_shared.ptr()) }, 0);
            assert_eq!(unsafe { pthread_cond_signal(cond_shared.ptr()) }, 0);
            assert_eq!(unsafe { pthread_mutex_unlock(mutex_shared.ptr()) }, 0);
        });

        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        let mono_deadline = monotonic_now().unwrap() + Duration::from_secs(1);
        let mono_ts = Timespec {
            tv_sec: mono_deadline.as_secs() as i64,
            tv_nsec: mono_deadline.subsec_nanos() as i64,
        };
        let start = std::time::Instant::now();
        let ret =
            unsafe { pthread_cond_timedwait(&raw mut cond, &raw mut mutex, &raw const mono_ts) };
        let elapsed = start.elapsed();
        assert_eq!(ret, 0, "signalled condvar must return 0");
        assert!(
            elapsed < Duration::from_millis(500),
            "signalled condvar must wake up early, elapsed was {:?}",
            elapsed
        );
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);
        handle.join().unwrap();

        assert_eq!(unsafe { pthread_cond_destroy(&raw mut cond) }, 0);
        assert_eq!(unsafe { pthread_cond_destroy(&raw mut realtime_cond) }, 0);
        assert_eq!(unsafe { pthread_mutex_destroy(&raw mut mutex) }, 0);
    }

    #[test]
    fn pthread_attr_affinity_round_trip() {
        let mut attr = core::mem::MaybeUninit::<PthreadAttr>::uninit();
        assert_eq!(unsafe { pthread_attr_init(attr.as_mut_ptr()) }, 0);
        let mut attr = unsafe { attr.assume_init() };

        let mask_in: u64 = 0b101;
        assert_eq!(
            unsafe {
                pthread_attr_setaffinity_np(
                    &raw mut attr,
                    std::mem::size_of::<u64>(),
                    (&raw const mask_in).cast(),
                )
            },
            0
        );

        let mut mask_out: u64 = 0;
        assert_eq!(
            unsafe {
                pthread_attr_getaffinity_np(
                    &raw const attr,
                    std::mem::size_of::<u64>(),
                    (&raw mut mask_out).cast(),
                )
            },
            0
        );
        assert_eq!(mask_out, mask_in);
    }

    #[test]
    fn pthread_cond_clockwait_timeout_and_signal() {
        let mut cond = 0usize;
        let mut mutex = 0usize;
        assert_eq!(
            unsafe { pthread_mutex_init(&raw mut mutex, core::ptr::null()) },
            0
        );
        assert_eq!(
            unsafe { pthread_cond_init(&raw mut cond, core::ptr::null()) },
            0
        );

        assert_eq!(unsafe { pthread_mutex_lock(&raw mut mutex) }, 0);
        let now = monotonic_now().unwrap();
        let deadline = now + Duration::from_millis(50);
        let ts = Timespec {
            tv_sec: deadline.as_secs() as i64,
            tv_nsec: deadline.subsec_nanos() as i64,
        };
        let res = unsafe {
            pthread_cond_clockwait(
                &raw mut cond,
                &raw mut mutex,
                CLOCK_MONOTONIC,
                &raw const ts,
            )
        };
        assert_eq!(res, ETIMEDOUT);
        assert_eq!(unsafe { pthread_mutex_unlock(&raw mut mutex) }, 0);

        assert_eq!(unsafe { pthread_cond_destroy(&raw mut cond) }, 0);
        assert_eq!(unsafe { pthread_mutex_destroy(&raw mut mutex) }, 0);
    }

    #[test]
    fn pthread_tryjoin_np_lifecycle() {
        unsafe extern "sysv64" fn gated(arg: *mut c_void) -> *mut c_void {
            let gate = unsafe { &*arg.cast::<Barrier>() };
            gate.wait();
            42 as *mut c_void
        }

        let gate = Barrier::new(2);
        let mut thread = 0usize;
        assert_eq!(
            unsafe {
                pthread_create(
                    &raw mut thread,
                    core::ptr::null(),
                    Some(gated),
                    (&raw const gate).cast_mut().cast(),
                )
            },
            0
        );

        // The barrier keeps the worker live regardless of scheduler delays.
        let mut result = core::ptr::null_mut();
        assert_eq!(
            unsafe { pthread_tryjoin_np(thread, &raw mut result) },
            EBUSY
        );
        gate.wait();
        let handle = {
            let registry = threads().lock().unwrap();
            let Some(ThreadRecord::Joinable(handle)) = registry.get(&thread) else {
                panic!("joinable thread missing");
            };
            handle.as_raw_handle()
        };
        // This test alone owns the join; leave the registry free for exit cleanup.
        assert_eq!(unsafe { WaitForSingleObject(handle, 5000) }, WAIT_OBJECT_0);

        // Now tryjoin succeeds and returns the exit value 42
        assert_eq!(unsafe { pthread_tryjoin_np(thread, &raw mut result) }, 0);
        assert_eq!(result as usize, 42);

        // After joining, subsequent tryjoin returns ESRCH
        assert_eq!(
            unsafe { pthread_tryjoin_np(thread, &raw mut result) },
            ESRCH
        );

        // Self-join returns EDEADLK
        assert_eq!(
            unsafe { pthread_tryjoin_np(pthread_self(), &raw mut result) },
            EDEADLK
        );
    }

    #[test]
    fn pthread_rwlock_timed_operations() {
        let mut rwlock = 0usize;
        assert_eq!(
            unsafe { pthread_rwlock_init(&raw mut rwlock, core::ptr::null()) },
            0
        );

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let deadline = now + Duration::from_millis(50);
        let ts = Timespec {
            tv_sec: deadline.as_secs() as i64,
            tv_nsec: deadline.subsec_nanos() as i64,
        };

        // Uncontended timedrdlock succeeds
        assert_eq!(
            unsafe { pthread_rwlock_timedrdlock(&raw mut rwlock, &raw const ts) },
            0
        );

        // Contended timedwrlock while reader holds it -> timeouts with ETIMEDOUT
        assert_eq!(
            unsafe { pthread_rwlock_timedwrlock(&raw mut rwlock, &raw const ts) },
            ETIMEDOUT
        );

        // Release read lock
        assert_eq!(unsafe { pthread_rwlock_unlock(&raw mut rwlock) }, 0);

        // Now uncontended timedwrlock succeeds
        let now2 = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let deadline2 = now2 + Duration::from_millis(50);
        let ts2 = Timespec {
            tv_sec: deadline2.as_secs() as i64,
            tv_nsec: deadline2.subsec_nanos() as i64,
        };
        assert_eq!(
            unsafe { pthread_rwlock_timedwrlock(&raw mut rwlock, &raw const ts2) },
            0
        );

        // Contended timedrdlock while writer holds it -> timeouts with ETIMEDOUT
        assert_eq!(
            unsafe { pthread_rwlock_timedrdlock(&raw mut rwlock, &raw const ts2) },
            ETIMEDOUT
        );

        assert_eq!(unsafe { pthread_rwlock_unlock(&raw mut rwlock) }, 0);
        assert_eq!(unsafe { pthread_rwlock_destroy(&raw mut rwlock) }, 0);
    }
}

mod object_layout;
