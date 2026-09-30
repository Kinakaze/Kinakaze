//! Process-shared mutexes identified by values in the public 40-byte object.
//! Native handles and thread ownership stay local; repair state is shared.
use super::*;
pub(super) mod condition;
use std::cell::RefCell;
use std::sync::atomic::{AtomicPtr, AtomicU64};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_ABANDONED};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentThreadId, GetProcessTimes, ReleaseMutex,
};

pub(super) const ATTR_PSHARED: i32 = i32::MIN;
const OBJECT_SHARED: i32 = 128;
const OBJECT_ROBUST: i32 = 16;
const INCONSISTENT: u32 = i32::MAX as u32;
const NOT_RECOVERABLE: u32 = INCONSISTENT - 1;
const EOWNERDEAD: i32 = 130;
const ENOTRECOVERABLE: i32 = 131;
const CACHE_LIMIT: usize = 128;
static NEXT: AtomicU64 = AtomicU64::new(1);
static CACHE: AtomicPtr<Cache> = AtomicPtr::new(core::ptr::null_mut());

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Key {
    domain: u64,
    creator: u32,
    birth: u64,
    generation: u64,
}
struct NativeHandle {
    host: u32,
    value: usize,
}
impl NativeHandle {
    fn raw(&self) -> HANDLE {
        self.value as HANDLE
    }
}
impl Drop for NativeHandle {
    fn drop(&mut self) {
        if self.host == std::process::id() {
            unsafe { CloseHandle(self.raw()) };
        }
    }
}
struct Cache {
    host: u32,
    rows: Mutex<HashMap<Key, Arc<NativeHandle>>>,
}
struct Held {
    host: u32,
    rows: HashMap<Key, Arc<NativeHandle>>,
}
thread_local! {
    static HELD: RefCell<Held> = RefCell::new(Held { host: std::process::id(), rows: HashMap::new() });
    static BIRTH: Cell<Option<(u32, u64)>> = const { Cell::new(None) };
}

fn cached() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var("KINAKAZE_PTHREAD_SHARED_CACHE_OPT").as_deref() != Ok("0"))
}
fn cache() -> &'static Cache {
    loop {
        let old = CACHE.load(Ordering::Acquire);
        if !old.is_null() && unsafe { (*old).host } == std::process::id() {
            return unsafe { &*old };
        }
        let next = Box::into_raw(Box::new(Cache {
            host: std::process::id(),
            rows: Mutex::new(HashMap::new()),
        }));
        if CACHE
            .compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // The fork image may contain a parent's locked native heap gate.
            return unsafe { &*next };
        }
        unsafe { drop(Box::from_raw(next)) };
    }
}
fn with_held<T>(f: impl FnOnce(&mut HashMap<Key, Arc<NativeHandle>>) -> T) -> T {
    HELD.with_borrow_mut(|held| {
        if held.host != std::process::id() {
            held.rows.clear();
            held.host = std::process::id();
        }
        f(&mut held.rows)
    })
}
pub(super) fn retire_thread() {
    // Never release a dead owner's mutex: native thread teardown abandons it.
    let _ = HELD.try_with(|held| held.borrow_mut().rows.clear());
}
pub(super) fn reset_after_fork() {
    retire_thread();
}

fn process_birth() -> Result<u64, i32> {
    BIRTH.with(|cached| {
        let host = std::process::id();
        if let Some((pid, birth)) = cached.get()
            && pid == host
        {
            return Ok(birth);
        }
        let mut creation: FILETIME = unsafe { core::mem::zeroed() };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        if unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(EAGAIN);
        }
        let birth = u64::from(creation.dwLowDateTime) | (u64::from(creation.dwHighDateTime) << 32);
        cached.set(Some((host, birth)));
        Ok(birth)
    })
}
unsafe fn field<'a>(mutex: *mut usize, offset: usize) -> &'a AtomicU32 {
    unsafe { &*mutex.cast::<u8>().add(offset).cast() }
}
unsafe fn kind(mutex: *mut usize) -> i32 {
    unsafe { mutex.cast::<u8>().add(16).cast::<i32>().read() }
}
pub(super) unsafe fn is_mutex(mutex: *mut usize) -> bool {
    (unsafe { kind(mutex) }) & OBJECT_SHARED != 0
}
unsafe fn key(mutex: *mut usize) -> Result<Key, i32> {
    let bytes = mutex.cast::<u8>();
    let key = Key {
        domain: kinakaze_runtime::authority::domain_id(),
        creator: unsafe { bytes.add(20).cast::<u32>().read() },
        birth: unsafe { bytes.add(24).cast::<u64>().read() },
        generation: unsafe { bytes.add(32).cast::<u64>().read() },
    };
    if key.creator == 0 || key.birth == 0 || key.generation == 0 {
        Err(EINVAL)
    } else {
        Ok(key)
    }
}
fn open(key: Key) -> Result<Arc<NativeHandle>, i32> {
    let name: Vec<u16> = format!(
        "Local\\kinakaze.pthread.shared.v1.{:016x}.{:08x}.{:016x}.{:016x}",
        key.domain, key.creator, key.birth, key.generation
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let value = unsafe { CreateMutexW(core::ptr::null(), 0, name.as_ptr()) };
    if value.is_null() {
        return Err(EAGAIN);
    }
    Ok(Arc::new(NativeHandle {
        host: std::process::id(),
        value: value as usize,
    }))
}
fn handle(key: Key) -> Result<Arc<NativeHandle>, i32> {
    if !cached() {
        return open(key);
    }
    let mut rows = cache().rows.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(value) = rows.get(&key) {
        return Ok(value.clone());
    }
    let value = open(key)?;
    if rows.len() >= CACHE_LIMIT {
        let idle = rows
            .iter()
            .find_map(|(key, value)| (Arc::strong_count(value) == 1).then_some(*key));
        if let Some(idle) = idle {
            rows.remove(&idle);
        }
    }
    // Active owners/waiters retain their Arc even when no cache slot is free.
    if rows.len() < CACHE_LIMIT {
        rows.insert(key, value.clone());
    }
    Ok(value)
}
fn new_identity() -> Result<Key, i32> {
    let birth = process_birth()?;
    let generation = NEXT
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| EAGAIN)?;
    Ok(Key {
        domain: kinakaze_runtime::authority::domain_id(),
        creator: std::process::id(),
        birth,
        generation,
    })
}

pub(super) unsafe fn init(mutex: *mut usize, flags: i32) -> i32 {
    let key = match new_identity() {
        Ok(key) => key,
        Err(error) => return error,
    };
    if let Err(error) = handle(key) {
        return error;
    }
    let mut object_kind = flags & PTHREAD_MUTEX_KIND_MASK;
    if object_kind == PTHREAD_MUTEX_ADAPTIVE_NP {
        object_kind = PTHREAD_MUTEX_NORMAL;
    }
    if flags & robust::ATTR_ROBUST != 0 {
        object_kind |= OBJECT_ROBUST;
    }
    unsafe {
        robust::forget(mutex);
        core::ptr::write_bytes(mutex.cast::<u8>(), 0, 40);
        let bytes = mutex.cast::<u8>();
        bytes
            .add(16)
            .cast::<i32>()
            .write(object_kind | OBJECT_SHARED);
        bytes.add(20).cast::<u32>().write(key.creator);
        bytes.add(24).cast::<u64>().write(key.birth);
        bytes.add(32).cast::<u64>().write(key.generation);
    }
    0
}

fn wait_self(deadline: Option<(&Timespec, i32)>) -> i32 {
    match sched::retain_current(pthread_self()) {
        Ok(thread) => match parking::wait_native(thread.as_raw_handle(), deadline) {
            Err(error) => error,
            Ok(_) => EINVAL, // A live thread cannot observe its own exit.
        },
        Err(error) => error,
    }
}
pub(super) unsafe fn acquire(
    mutex: *mut usize,
    deadline: Option<(&Timespec, i32)>,
    trying: bool,
) -> i32 {
    let key = match unsafe { key(mutex) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let object_kind = unsafe { kind(mutex) };
    let owner = unsafe { field(mutex, 8) };
    if owner.load(Ordering::Acquire) == NOT_RECOVERABLE {
        return ENOTRECOVERABLE;
    }
    if with_held(|rows| rows.contains_key(&key)) {
        if object_kind & 3 == PTHREAD_MUTEX_RECURSIVE {
            let count = unsafe { field(mutex, 4) };
            let depth = count.load(Ordering::Relaxed);
            let Some(next) = depth.checked_add(1) else {
                return EAGAIN;
            };
            count.store(next, Ordering::Relaxed);
            return 0;
        }
        if trying {
            return EBUSY;
        }
        if object_kind & 3 == PTHREAD_MUTEX_ERRORCHECK {
            return EDEADLK;
        }
        return wait_self(deadline);
    }
    let handle = match handle(key) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let waited = if trying {
        match unsafe { WaitForSingleObject(handle.raw(), 0) } {
            WAIT_OBJECT_0 => Ok(WAIT_OBJECT_0),
            WAIT_ABANDONED => Ok(WAIT_ABANDONED),
            WAIT_TIMEOUT => Err(EBUSY),
            _ => Err(EINVAL),
        }
    } else {
        parking::wait_native(handle.raw(), deadline)
    };
    let outcome = match waited {
        Ok(value) => value,
        Err(error) => return error,
    };
    if owner.load(Ordering::Acquire) == NOT_RECOVERABLE {
        unsafe { ReleaseMutex(handle.raw()) };
        return ENOTRECOVERABLE;
    }
    let abandoned =
        outcome == WAIT_ABANDONED || unsafe { field(mutex, 0) }.load(Ordering::Acquire) != 0;
    if abandoned && object_kind & OBJECT_ROBUST == 0 {
        unsafe { ReleaseMutex(handle.raw()) };
        return if trying { EBUSY } else { wait_self(deadline) };
    }
    let tid = unsafe { GetCurrentThreadId() };
    owner.store(
        if abandoned { INCONSISTENT } else { tid },
        Ordering::Relaxed,
    );
    unsafe {
        field(mutex, 4).store(1, Ordering::Relaxed);
        field(mutex, 12).store(1, Ordering::Relaxed);
        field(mutex, 0).store(tid, Ordering::Release);
    }
    with_held(|rows| {
        rows.insert(key, handle);
    });
    if abandoned { EOWNERDEAD } else { 0 }
}
pub(super) unsafe fn unlock(mutex: *mut usize) -> i32 {
    let key = match unsafe { key(mutex) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let Some(handle) = with_held(|rows| rows.get(&key).cloned()) else {
        return EPERM;
    };
    let count = unsafe { field(mutex, 4) };
    let depth = count.load(Ordering::Relaxed);
    if depth > 1 {
        count.store(depth - 1, Ordering::Relaxed);
        return 0;
    }
    let owner = unsafe { field(mutex, 8) };
    owner.store(
        if owner.load(Ordering::Relaxed) == INCONSISTENT {
            NOT_RECOVERABLE
        } else {
            0
        },
        Ordering::Release,
    );
    unsafe {
        count.store(0, Ordering::Relaxed);
        field(mutex, 12).store(0, Ordering::Relaxed);
        field(mutex, 0).store(0, Ordering::Release);
    }
    if unsafe { ReleaseMutex(handle.raw()) } == 0 {
        return EPERM;
    }
    with_held(|rows| {
        rows.remove(&key);
    });
    0
}
pub(super) unsafe fn consistent(mutex: *mut usize) -> i32 {
    if unsafe { kind(mutex) } & OBJECT_ROBUST == 0 {
        return EINVAL;
    }
    let key = match unsafe { key(mutex) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let owner = unsafe { field(mutex, 8) };
    if !with_held(|rows| rows.contains_key(&key)) || owner.load(Ordering::Acquire) != INCONSISTENT {
        return EINVAL;
    }
    owner.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    0
}
pub(super) unsafe fn destroy(mutex: *mut usize) -> i32 {
    if unsafe { field(mutex, 0) }.load(Ordering::Acquire) != 0 {
        return EBUSY;
    }
    let key = match unsafe { key(mutex) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    cache()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&key);
    unsafe { mutex.cast::<u8>().add(16).cast::<i32>().write(0) };
    0
}

#[cfg(test)]
mod tests;
