//! Private robust mutexes: native abandonment plus explicit POSIX repair state.
//! Handles never enter guest objects or fork payloads. Mutex waits are not
//! cancellation points; the native thread owns exactly one kernel acquisition.
use super::*;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::atomic::AtomicPtr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{CreateEventW, CreateMutexW, ReleaseMutex};

pub(super) const ATTR_ROBUST: i32 = 1 << 30;
const OBJECT_ROBUST: i32 = 16;
const EOWNERDEAD: i32 = 130;
const ENOTRECOVERABLE: i32 = 131;
static COUNT: AtomicUsize = AtomicUsize::new(0);
static REGISTRY: AtomicPtr<Registry> = AtomicPtr::new(core::ptr::null_mut());

struct NativeHandle {
    host: u32,
    value: usize,
}
impl NativeHandle {
    fn new(value: HANDLE) -> Result<Self, i32> {
        if value.is_null() {
            return Err(EAGAIN);
        }
        Ok(Self {
            host: std::process::id(),
            value: value as usize,
        })
    }
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

struct State {
    owner: usize,
    depth: usize,
    repair: u64, // 0 healthy, 1 inconsistent, 2 not recoverable
    owner_handle: Option<Arc<OwnedHandle>>,
}
struct Lock {
    handle: NativeHandle,
    kind: i32,
    state: Mutex<State>,
}
struct Registry {
    host: u32,
    rows: Mutex<HashMap<usize, Arc<Lock>>>,
}
pub(super) fn retire_range(base: usize, end: usize) {
    if COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut rows = registry()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    rows.retain(|address, _| !(base..end).contains(address));
    COUNT.store(rows.len(), Ordering::Release);
}

fn registry() -> &'static Registry {
    loop {
        let old = REGISTRY.load(Ordering::Acquire);
        if !old.is_null() && unsafe { (*old).host } == std::process::id() {
            return unsafe { &*old };
        }
        let next = Box::into_raw(Box::new(Registry {
            host: std::process::id(),
            rows: Mutex::new(HashMap::new()),
        }));
        if REGISTRY
            .compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // A parent's native pointers/handle integers are not child owners.
            return unsafe { &*next };
        }
        unsafe { drop(Box::from_raw(next)) };
    }
}

fn current_handle() -> Result<Arc<OwnedHandle>, i32> {
    sched::retain_current(pthread_self())
}

fn lookup(mutex: *mut usize) -> Result<Arc<Lock>, i32> {
    registry()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&(mutex as usize))
        .cloned()
        .ok_or(EINVAL)
}

pub(super) unsafe fn is_mutex(mutex: *mut usize) -> bool {
    COUNT.load(Ordering::Acquire) != 0
        && unsafe {
            mutex
                .cast::<u8>()
                .add(PTHREAD_MUTEX_KIND_OFFSET)
                .cast::<i32>()
                .read()
        } & OBJECT_ROBUST
            != 0
}

unsafe fn mark(mutex: *mut usize, kind: i32) {
    unsafe {
        mutex
            .cast::<u8>()
            .add(PTHREAD_MUTEX_KIND_OFFSET)
            .cast::<i32>()
            .write(kind)
    };
}

pub(super) unsafe fn init(mutex: *mut usize, kind: i32) -> i32 {
    let handle =
        match NativeHandle::new(unsafe { CreateMutexW(core::ptr::null(), 0, core::ptr::null()) }) {
            Ok(handle) => handle,
            Err(error) => return error,
        };
    let mut rows = registry()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let old = rows.insert(
        mutex as usize,
        Arc::new(Lock {
            handle,
            kind,
            state: Mutex::new(State {
                owner: 0,
                depth: 0,
                repair: 0,
                owner_handle: None,
            }),
        }),
    );
    if old.is_none() {
        COUNT.fetch_add(1, Ordering::AcqRel);
    }
    unsafe {
        mutex.write(0);
        mark(mutex, kind | OBJECT_ROBUST);
    }
    0
}

pub(super) unsafe fn forget(mutex: *mut usize) {
    if COUNT.load(Ordering::Acquire) == 0 {
        return;
    }
    if registry()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&(mutex as usize))
        .is_some()
    {
        COUNT.fetch_sub(1, Ordering::AcqRel);
        unsafe { mark(mutex, 0) };
    }
}

pub(super) unsafe fn destroy(mutex: *mut usize) -> i32 {
    let mut rows = registry()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let Some(lock) = rows.get(&(mutex as usize)) else {
        return EINVAL;
    };
    if lock
        .state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .owner
        != 0
    {
        return EBUSY;
    }
    rows.remove(&(mutex as usize));
    COUNT.fetch_sub(1, Ordering::AcqRel);
    unsafe { mark(mutex, 0) };
    0
}

pub(super) unsafe fn acquire(
    mutex: *mut usize,
    deadline: Option<(&Timespec, i32)>,
    trying: bool,
) -> i32 {
    let lock = match lookup(mutex) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    let owner_handle = match current_handle() {
        Ok(handle) => handle,
        Err(error) => return error,
    };
    let self_id = pthread_self();
    let self_owned = {
        let mut state = lock.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.repair == 2 {
            return ENOTRECOVERABLE;
        }
        if state.owner == self_id {
            if lock.kind == PTHREAD_MUTEX_RECURSIVE {
                let Some(depth) = state.depth.checked_add(1) else {
                    return EAGAIN;
                };
                state.depth = depth;
                return 0;
            }
            if trying {
                return EBUSY;
            }
            if lock.kind == PTHREAD_MUTEX_ERRORCHECK {
                return EDEADLK;
            }
            true
        } else {
            false
        }
    };
    // Windows mutexes recurse unconditionally. NORMAL instead waits on its
    // own live thread object, preserving timed self-deadlock semantics.
    let handle = if self_owned {
        owner_handle.as_raw_handle()
    } else {
        lock.handle.raw()
    };
    let waited = if trying {
        match unsafe { WaitForSingleObject(handle, 0) } {
            WAIT_OBJECT_0 => Ok(WAIT_OBJECT_0),
            WAIT_ABANDONED => Ok(WAIT_ABANDONED),
            windows_sys::Win32::Foundation::WAIT_TIMEOUT => Err(EBUSY),
            _ => Err(EINVAL),
        }
    } else {
        parking::wait_native(handle, deadline)
    };
    let outcome = match waited {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut state = lock.state.lock().unwrap_or_else(PoisonError::into_inner);
    if state.repair == 2 {
        unsafe { ReleaseMutex(lock.handle.raw()) };
        return ENOTRECOVERABLE;
    }
    if outcome == WAIT_ABANDONED {
        state.repair = 1;
    }
    state.owner = self_id;
    state.depth = 1;
    state.owner_handle = Some(owner_handle);
    if state.repair == 1 { EOWNERDEAD } else { 0 }
}

pub(super) unsafe fn unlock(mutex: *mut usize) -> i32 {
    let lock = match lookup(mutex) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    let mut state = lock.state.lock().unwrap_or_else(PoisonError::into_inner);
    if state.owner != pthread_self() {
        return EPERM;
    }
    if state.depth > 1 {
        state.depth -= 1;
        return 0;
    }
    // Keep the state gate through release: a new kernel owner must not race
    // the old owner's metadata retirement or repair-state publication.
    if unsafe { ReleaseMutex(lock.handle.raw()) } == 0 {
        return EPERM;
    }
    if state.repair == 1 {
        state.repair = 2;
    }
    state.owner = 0;
    state.depth = 0;
    state.owner_handle = None;
    0
}

pub(super) unsafe fn consistent(mutex: *mut usize) -> i32 {
    let lock = match lookup(mutex) {
        Ok(lock) => lock,
        Err(error) => return error,
    };
    let mut state = lock.state.lock().unwrap_or_else(PoisonError::into_inner);
    if state.owner != pthread_self() || state.repair != 1 {
        return EINVAL;
    }
    state.repair = 0;
    0
}

pub(super) fn snapshot() -> Vec<[u64; 5]> {
    let rows = registry()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    rows.iter()
        .map(|(address, lock)| {
            let state = lock.state.lock().unwrap_or_else(PoisonError::into_inner);
            let dead = state.owner_handle.as_ref().is_some_and(
                |handle| unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } == WAIT_OBJECT_0,
            );
            [
                *address as u64,
                lock.kind as u64,
                if dead { 0 } else { state.owner as u64 },
                if dead { 0 } else { state.depth as u64 },
                if dead { 1 } else { state.repair },
            ]
        })
        .collect()
}

pub(super) fn read_snapshot(reader: &mut ForkReader<'_>) -> Option<Vec<[u64; 5]>> {
    if reader.at == reader.bytes.len() {
        return Some(Vec::new());
    }
    let count = usize::try_from(reader.u64()?).ok()?;
    if count > reader.bytes.len().checked_sub(reader.at)? / 40 {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::with_capacity(count);
    for _ in 0..count {
        let row = [
            reader.u64()?,
            reader.u64()?,
            reader.u64()?,
            reader.u64()?,
            reader.u64()?,
        ];
        if row[0] == 0
            || row[0] % 8 != 0
            || row[1] > 2
            || row[4] > 2
            || (row[2] == 0) != (row[3] == 0)
            || (row[4] == 2 && row[2] != 0)
            || (row[1] != PTHREAD_MUTEX_RECURSIVE as u64 && row[3] > 1)
            || !seen.insert(row[0])
        {
            return None;
        }
        rows.push(row);
    }
    (reader.at == reader.bytes.len()).then_some(rows)
}

pub(super) unsafe fn restore(rows: Vec<[u64; 5]>, self_id: usize) -> Result<(), i32> {
    let mut locks = HashMap::with_capacity(rows.len());
    for [address, kind, owner, depth, repair] in rows {
        let current = owner == self_id as u64 && owner != 0;
        let foreign = owner != 0 && !current;
        let handle = if foreign {
            // As for typed SRW locks, preserve a vanished sibling's lock in
            // this private child copy. No native parent waiter is imported.
            unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) }
        } else {
            unsafe { CreateMutexW(core::ptr::null(), i32::from(current), core::ptr::null()) }
        };
        let handle = NativeHandle::new(handle)?;
        locks.insert(
            address as usize,
            Arc::new(Lock {
                handle,
                kind: kind as i32,
                state: Mutex::new(State {
                    owner: owner as usize,
                    depth: depth as usize,
                    repair,
                    owner_handle: if current {
                        Some(current_handle()?)
                    } else {
                        None
                    },
                }),
            }),
        );
        unsafe { mark(address as *mut usize, kind as i32 | OBJECT_ROBUST) };
    }
    let mut active = registry()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    COUNT.store(locks.len(), Ordering::Release);
    *active = locks;
    Ok(())
}

#[cfg(test)]
mod tests;
