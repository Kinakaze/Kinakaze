//! Process-local event queues for conditions and timed SRW acquisition.
//! Register before releasing/checking the mutex; unlock publishes after release.
//! No Rust address or native handle is inherited by a fork-restored queue.

use super::*;
use core::{cell::RefCell, ptr};
use std::sync::atomic::AtomicPtr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CancelWaitableTimer, CreateEventW, CreateWaitableTimerW, GetCurrentThread, GetCurrentThreadId,
    GetThreadTimes, INFINITE, ResetEvent, SetEvent, SetWaitableTimer, WaitForMultipleObjects,
};

pub(super) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_PTHREAD_PARK_OPT").is_none_or(|v| v != "0"))
}

#[derive(Clone, Copy)]
pub(super) struct Identity {
    pub host: u32,
    pub thread: u32,
    pub birth: u64,
}

pub(super) fn event_name(domain: u64, identity: Identity) -> Vec<u16> {
    format!(
        "Local\\kinakaze.pthread.park.v1.{domain:016x}.{:08x}.{:08x}.{:016x}",
        identity.host, identity.thread, identity.birth
    )
    .encode_utf16()
    .chain(Some(0))
    .collect()
}

pub(super) fn identity() -> Result<Identity, i32> {
    handles(false)?;
    PARK.with_borrow(|slot| slot.as_ref().map(|p| p.identity).ok_or(EAGAIN))
}

struct Park {
    host: u32,
    domain: u64,
    identity: Identity,
    event: usize,
    timer: usize,
}
impl Drop for Park {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.event as HANDLE) };
        if self.timer != 0 {
            unsafe { CloseHandle(self.timer as HANDLE) };
        }
    }
}
thread_local! {
    static PARK: RefCell<Option<Park>> = const { RefCell::new(None) };
}

fn handles(timed: bool) -> Result<(HANDLE, HANDLE), i32> {
    PARK.with_borrow_mut(|slot| {
        let host = std::process::id();
        let domain = kinakaze_runtime::authority::domain_id();
        if slot.as_ref().is_some_and(|park| park.host != host) {
            // The copied integer may identify an unrelated handle in a worker.
            core::mem::forget(slot.take());
        } else if slot.as_ref().is_some_and(|park| park.domain != domain) {
            // Provider warmup may precede installation of the manager domain.
            drop(slot.take());
        }
        if slot.is_none() {
            let (mut born, mut end, mut kernel, mut user) = unsafe { core::mem::zeroed() };
            if unsafe {
                GetThreadTimes(
                    GetCurrentThread(),
                    &mut born,
                    &mut end,
                    &mut kernel,
                    &mut user,
                )
            } == 0
            {
                return Err(EAGAIN);
            }
            let identity = Identity {
                host,
                thread: unsafe { GetCurrentThreadId() },
                birth: (u64::from(born.dwHighDateTime) << 32) | u64::from(born.dwLowDateTime),
            };
            let name = event_name(domain, identity);
            let event = unsafe { CreateEventW(ptr::null(), 0, 0, name.as_ptr()) };
            if event.is_null() {
                return Err(EAGAIN);
            }
            *slot = Some(Park {
                host,
                domain,
                identity,
                event: event as usize,
                timer: 0,
            });
        }
        let park = slot.as_mut().unwrap();
        if timed && park.timer == 0 {
            let timer = unsafe { CreateWaitableTimerW(ptr::null(), 0, ptr::null()) };
            if timer.is_null() {
                return Err(EAGAIN);
            }
            park.timer = timer as usize;
        }
        Ok((park.event as HANDLE, park.timer as HANDLE))
    })
}

pub(super) fn retire_thread() {
    let _ = PARK.try_with(|slot| drop(slot.borrow_mut().take()));
}

#[derive(Clone, Copy)]
struct Row {
    token: usize,
    thread: usize,
    event: usize,
    selected: bool,
}
type Key = (usize, bool); // address, condition (otherwise a timed mutex)
struct Queues {
    host: u32,
    rows: Mutex<HashMap<Key, Vec<Row>>>,
}
static QUEUES: AtomicPtr<Queues> = AtomicPtr::new(ptr::null_mut());
static TOKENS: AtomicUsize = AtomicUsize::new(1);
static TIMED_WAITERS: AtomicUsize = AtomicUsize::new(0);

fn queues() -> &'static Queues {
    loop {
        let old = QUEUES.load(Ordering::Acquire);
        if !old.is_null() && unsafe { (*old).host } == std::process::id() {
            return unsafe { &*old };
        }
        let next = Box::into_raw(Box::new(Queues {
            host: std::process::id(),
            rows: Mutex::new(HashMap::new()),
        }));
        if QUEUES
            .compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return unsafe { &*next };
        }
        unsafe { drop(Box::from_raw(next)) };
    }
}

pub(super) fn reset_after_fork() {
    QUEUES.store(ptr::null_mut(), Ordering::Release);
    TIMED_WAITERS.store(0, Ordering::SeqCst);
    PARK.with_borrow_mut(|slot| {
        if slot
            .as_ref()
            .is_some_and(|park| park.host != std::process::id())
        {
            core::mem::forget(slot.take());
        }
    });
}

struct Ticket {
    key: Key,
    token: usize,
    active: bool,
}
impl Ticket {
    fn register(key: Key, event: HANDLE) -> Self {
        let mut rows = queues().rows.lock().unwrap_or_else(PoisonError::into_inner);
        // Retire the preceding wait before resetting its per-thread event.
        unsafe { ResetEvent(event) };
        let token = loop {
            let token = TOKENS.fetch_add(1, Ordering::Relaxed);
            if token != 0 {
                break token;
            }
        };
        if !key.1 {
            TIMED_WAITERS.fetch_add(1, Ordering::SeqCst);
        }
        rows.entry(key).or_default().push(Row {
            token,
            thread: pthread_self(),
            event: event as usize,
            selected: false,
        });
        Self {
            key,
            token,
            active: true,
        }
    }

    fn selected(&self) -> bool {
        let rows = queues().rows.lock().unwrap_or_else(PoisonError::into_inner);
        rows.get(&self.key)
            .and_then(|rows| rows.iter().find(|r| r.token == self.token))
            .is_some_and(|row| row.selected)
    }

    fn finish(&mut self) -> bool {
        if !self.active {
            return false;
        }
        let mut rows = queues().rows.lock().unwrap_or_else(PoisonError::into_inner);
        let mut selected = false;
        if let Some(queue) = rows.get_mut(&self.key) {
            if let Some(index) = queue.iter().position(|row| row.token == self.token) {
                selected = queue.remove(index).selected;
            }
            if queue.is_empty() {
                rows.remove(&self.key);
            }
        }
        if !self.key.1 {
            TIMED_WAITERS.fetch_sub(1, Ordering::SeqCst);
        }
        self.active = false;
        selected
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.finish();
    }
}

fn signal_rows(rows: &mut [Row], count: usize, condition: bool) -> i32 {
    let mut signalled = 0;
    for row in rows {
        if signalled == count {
            break;
        }
        if condition && row.selected {
            continue;
        }
        if unsafe { SetEvent(row.event as HANDLE) } == 0 {
            return EINVAL;
        }
        row.selected = true;
        signalled += 1;
    }
    0
}

pub(super) fn signal(cond: *mut usize, all: bool) -> i32 {
    if unsafe { shared::condition::is_cond(cond) } {
        return unsafe { shared::condition::signal(cond, all) };
    }
    let mut rows = queues().rows.lock().unwrap_or_else(PoisonError::into_inner);
    rows.get_mut(&(cond as usize, true)).map_or(0, |rows| {
        signal_rows(rows, if all { usize::MAX } else { 1 }, true)
    })
}

pub(super) fn interrupt(thread: usize) {
    // Requests before registration are caught by the post-registration check.
    let root = QUEUES.load(Ordering::Acquire);
    if root.is_null() || unsafe { (*root).host } != std::process::id() {
        return;
    }
    let rows = unsafe { &*root }
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    for (&(_, condition), queue) in rows.iter() {
        if condition {
            for row in queue.iter().filter(|row| row.thread == thread) {
                unsafe { SetEvent(row.event as HANDLE) };
            }
        }
    }
}

pub(super) fn released(mutex: *mut usize) {
    // This read is AFTER native release. Registration increments BEFORE its
    // final TryAcquire: either it sees the release or this path sees its row.
    if TIMED_WAITERS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let mut rows = queues().rows.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(rows) = rows.get_mut(&(mutex as usize, false)) {
        let _ = signal_rows(rows, usize::MAX, false);
    }
}

pub(super) fn remaining(deadline: &Timespec, clock: i32) -> Result<Option<Duration>, i32> {
    if !matches!(clock, CLOCK_REALTIME | CLOCK_MONOTONIC)
        || !(0..1_000_000_000).contains(&deadline.tv_nsec)
    {
        return Err(EINVAL);
    }
    if deadline.tv_sec < 0 {
        return Ok(None);
    }
    remaining_until(deadline, clock)
}

fn arm(timer: HANDLE, deadline: &Timespec, clock: i32) -> Result<bool, i32> {
    let Some(left) = remaining(deadline, clock)? else {
        return Ok(false);
    };
    let due = if clock == CLOCK_REALTIME {
        // Absolute timers follow wall-clock changes. Round up to 100 ns.
        (116_444_736_000_000_000u128
            + deadline.tv_sec as u128 * 10_000_000
            + (deadline.tv_nsec as u128).div_ceil(100))
        .min(i64::MAX as u128) as i64
    } else {
        -(left.as_nanos().div_ceil(100).clamp(1, i64::MAX as u128) as i64)
    };
    if unsafe { SetWaitableTimer(timer, &due, 0, None, ptr::null(), 0) } == 0 {
        return Err(EINVAL);
    }
    Ok(true)
}

pub(super) fn wait_native(handle: HANDLE, deadline: Option<(&Timespec, i32)>) -> Result<u32, i32> {
    use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_TIMEOUT};
    // An immediately obtainable mutex ignores malformed/expired timespecs.
    let first = unsafe { WaitForSingleObject(handle, 0) };
    if first == WAIT_OBJECT_0 || first == WAIT_ABANDONED {
        return Ok(first);
    }
    if first != WAIT_TIMEOUT {
        return Err(EINVAL);
    }
    let Some((time, clock)) = deadline else {
        let result = unsafe { WaitForSingleObject(handle, INFINITE) };
        return if result == WAIT_OBJECT_0 || result == WAIT_ABANDONED {
            Ok(result)
        } else {
            Err(EINVAL)
        };
    };
    let (_, timer) = handles(true)?;
    let result = loop {
        match arm(timer, time, clock) {
            Ok(true) => {}
            Ok(false) => break Err(ETIMEDOUT),
            Err(error) => break Err(error),
        }
        let result = unsafe { WaitForMultipleObjects(2, [handle, timer].as_ptr(), 0, INFINITE) };
        if result == WAIT_OBJECT_0 || result == WAIT_ABANDONED {
            break Ok(result);
        }
        if result != WAIT_OBJECT_0 + 1 {
            break Err(EINVAL);
        }
    };
    unsafe { CancelWaitableTimer(timer) };
    result
}

pub(super) unsafe fn timed_mutex(mutex: *mut usize, clock: i32, deadline: Timespec) -> i32 {
    match remaining(&deadline, clock) {
        Ok(Some(_)) => {}
        Ok(None) => return ETIMEDOUT,
        Err(error) => return error,
    }
    let (event, timer) = match handles(true) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let mut ticket = Ticket::register((mutex as usize, false), event);
    let result = loop {
        match arm(timer, &deadline, clock) {
            Ok(true) => {}
            Ok(false) => break ETIMEDOUT,
            Err(e) => break e,
        }
        // Registration and the post-release notification close the sleep gap.
        if unsafe { TryAcquireSRWLockExclusive(mutex.cast::<SRWLOCK>()) } {
            break 0;
        }
        let wait = unsafe { WaitForMultipleObjects(2, [event, timer].as_ptr(), 0, INFINITE) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_OBJECT_0 + 1 {
            break EINVAL;
        }
    };
    unsafe { CancelWaitableTimer(timer) };
    ticket.finish();
    if result == 0 {
        record_owner(mutex);
    }
    result
}

enum ConditionTicket {
    Private(Ticket),
    Shared {
        local: Ticket,
        shared: shared::condition::Ticket,
    },
}
impl ConditionTicket {
    unsafe fn register(cond: *mut usize, event: HANDLE) -> Result<Self, i32> {
        let local = Ticket::register((cond as usize, true), event);
        if unsafe { shared::condition::is_cond(cond) } {
            let shared = unsafe { shared::condition::Ticket::register(cond) }?;
            Ok(Self::Shared { local, shared })
        } else {
            Ok(Self::Private(local))
        }
    }
    fn selected(&self) -> Result<bool, i32> {
        match self {
            Self::Private(ticket) => Ok(ticket.selected()),
            Self::Shared { shared, .. } => shared.selected(),
        }
    }
    fn finish(&mut self) -> Result<(bool, bool), i32> {
        match self {
            Self::Private(ticket) => {
                let selected = ticket.finish();
                Ok((selected, selected))
            }
            Self::Shared { local, shared } => {
                let result = shared.finish();
                local.finish();
                result.map(|kind| (kind != 0, kind == 1))
            }
        }
    }
}

pub(super) unsafe fn condition(
    cond: *mut usize,
    mutex: *mut usize,
    deadline: Option<(Timespec, i32)>,
) -> i32 {
    pthread_testcancel();
    if let Some((time, clock)) = &deadline {
        match remaining(time, *clock) {
            Err(e) => return e,
            Ok(None) => {
                // Even an expired condition deadline releases and reacquires.
                let error = unsafe { pthread_mutex_unlock(mutex) };
                if error != 0 {
                    return error;
                }
                let error = unsafe { pthread_mutex_lock(mutex) };
                if error != 0 && error != 130 {
                    return error;
                }
                pthread_testcancel();
                if error != 0 {
                    return error;
                }
                return ETIMEDOUT;
            }
            Ok(Some(_)) => {}
        }
    }
    let (event, timer) = match handles(deadline.is_some()) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let interrupt = kinakaze_vfs::interrupt::current();
    if interrupt.is_null() {
        return EAGAIN;
    }
    let mut ticket = match unsafe { ConditionTicket::register(cond, event) } {
        Ok(ticket) => ticket,
        Err(error) => return error,
    };
    let error = unsafe { pthread_mutex_unlock(mutex) };
    if error != 0 {
        return error;
    }
    let mut result = loop {
        if cancellation_pending() {
            break 0;
        }
        match ticket.selected() {
            Ok(true) => break 0,
            Ok(false) => {}
            Err(error) => break error,
        }
        if kinakaze_vfs::signal::interrupt_pending() {
            break 0;
        }
        if let Some((time, clock)) = &deadline {
            match arm(timer, time, *clock) {
                Ok(true) => {}
                Ok(false) => break ETIMEDOUT,
                Err(e) => break e,
            }
        }
        let wait = unsafe {
            WaitForMultipleObjects(
                if deadline.is_some() { 3 } else { 2 },
                [event, interrupt, timer].as_ptr(),
                0,
                INFINITE,
            )
        };
        if !(WAIT_OBJECT_0..=WAIT_OBJECT_0 + 2).contains(&wait) {
            break EINVAL;
        }
    };
    if deadline.is_some() {
        unsafe { CancelWaitableTimer(timer) };
    }
    let (selected, replace) = match ticket.finish() {
        Ok(value) => value,
        Err(error) => {
            result = error;
            (false, false)
        }
    };
    drop(ticket); // ExitThread and guest cleanup do not unwind Rust locals.
    let error = unsafe { pthread_mutex_lock(mutex) };
    if error != 0 && error != 130 {
        return error;
    }
    if cancellation_pending() {
        if replace {
            let _ = signal(cond, false);
        }
        pthread_testcancel();
    }
    // The caller must repair an abandoned mutex. Releasing it around a guest
    // signal handler here would instead permanently poison its state.
    if error != 0 {
        return error;
    }
    let error = unsafe { deliver_condition_signals(mutex) };
    if error != 0 {
        return error;
    }
    // A handler may itself request cancellation while the mutex was released.
    if cancellation_pending() {
        if replace {
            let _ = signal(cond, false);
        }
        pthread_testcancel();
    }
    if selected { 0 } else { result }
}

#[cfg(test)]
mod tests;
