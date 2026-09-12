//! Kernel-domain queues for unflagged futexes, including MAP_SHARED aliases.
//!
//! Only value records cross process boundaries. A named mutex serializes the
//! value check, enqueue, wake, and two-key operations. Double-buffered commits
//! survive an abandoned mutex; wake events are signaled BEFORE publication,
//! under that mutex, so killing a waker cannot strand a committed wake.

use core::{mem::size_of, ptr};
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kinakaze_vfs::{EAGAIN, EINTR, EIO, ENOMEM, ETIMEDOUT, errno_from_win32, interrupt, signal};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcess, GetCurrentThread,
    GetCurrentThreadId, GetProcessIdOfThread, GetProcessTimes, GetThreadTimes, INFINITE,
    OpenEventW, OpenThread, ReleaseMutex, SetEvent, THREAD_QUERY_LIMITED_INFORMATION,
    THREAD_SYNCHRONIZE, WaitForMultipleObjects, WaitForSingleObject,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Key([u64; 5]);

impl Key {
    pub(crate) fn private(address: usize) -> Result<Self, i32> {
        Ok(Self([
            1,
            u64::from(std::process::id()),
            process_birth()?,
            0,
            address as u64,
        ]))
    }

    pub(crate) fn anonymous(identity: [u64; 3], offset: u64) -> Self {
        Self([2, identity[0], identity[1], identity[2], offset])
    }

    pub(crate) fn file(volume: u64, id: [u8; 16], offset: u64) -> Self {
        Self([
            3,
            volume,
            u64::from_le_bytes(id[..8].try_into().unwrap()),
            u64::from_le_bytes(id[8..].try_into().unwrap()),
            offset,
        ])
    }
}

fn timestamp(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

fn process_birth() -> Result<u64, i32> {
    // Do not cache a PID or birth time in copied fork state.
    let (mut born, mut end, mut kernel, mut user) = unsafe { core::mem::zeroed() };
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut born,
            &mut end,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(timestamp(born))
}

/// Stored in the managed backing record and copied unchanged by fork. New
/// mappings in a child use its new process incarnation, even at a recycled VA.
pub(crate) fn new_backing_id() -> Result<[u64; 3], i32> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    if serial == 0 {
        return Err(ENOMEM);
    }
    Ok([u64::from(std::process::id()), process_birth()?, serial])
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Record {
    key: Key,
    token: u64,
    born: u64,
    host: u32,
    thread: u32,
    bitset: u32,
    reserved: u32,
}

impl Record {
    fn event_name(self, domain: u64) -> Vec<u16> {
        wide(&format!(
            r"Local\kinakaze.futex.wait.v1.{domain:016x}.{:016x}",
            self.token
        ))
    }

    fn dead(self) -> bool {
        let raw = unsafe {
            OpenThread(
                THREAD_QUERY_LIMITED_INFORMATION | THREAD_SYNCHRONIZE,
                0,
                self.thread,
            )
        };
        if raw.is_null() {
            // Access/query failures are not proof of death.
            return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
        }
        let thread = Handle(raw);
        let host = unsafe { GetProcessIdOfThread(thread.0) };
        if host != 0 && host != self.host {
            return true;
        }
        if unsafe { WaitForSingleObject(thread.0, 0) } == WAIT_OBJECT_0 {
            return true;
        }
        thread_birth(thread.0).is_ok_and(|born| born != self.born)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::sync::{Arc, mpsc};

    // This runs in a real separate host process. ExitProcess deliberately skips
    // Guard::drop and abandons the native mutex at a selected transaction step.
    #[test]
    #[ignore = "subprocess helper for futex crash recovery and domain isolation"]
    fn transaction_child() {
        let mode = std::env::var("KINAKAZE_FUTEX_TEST_MODE").unwrap();
        let key: Vec<u64> = std::env::var("KINAKAZE_FUTEX_TEST_KEY")
            .unwrap()
            .split(',')
            .map(|part| part.parse().unwrap())
            .collect();
        let key = Key(key.try_into().unwrap());
        if mode == "other-domain" {
            kinakaze_runtime::authority::install_helper_domain(process_birth().unwrap()).unwrap();
            assert_eq!(wake(key, 1, u32::MAX), Ok(0));
        } else {
            let shared = shared().unwrap();
            let _guard = shared.acquire().unwrap();
            let mut records = shared.load().unwrap();
            assert_eq!(shared.select(&mut records, key, 1, u32::MAX), Ok(1));
            if mode == "after-commit" {
                shared.commit(&records);
            } else if mode == "partial-bank" {
                let next = 1 - shared.header().active.load(Ordering::Acquire);
                unsafe { ptr::addr_of_mut!((*shared.bank(next)).count).write(u64::MAX) };
            }
            // For before-commit and partial-bank, the event is set but the old
            // bank still contains the waiter. It must not report a false wake.
            unsafe { windows_sys::Win32::System::Threading::ExitProcess(77) };
        }
    }

    #[test]
    fn futex_abandoned_waker_and_domain_isolation() {
        for mode in [
            "before-commit",
            "partial-bank",
            "after-commit",
            "other-domain",
        ] {
            let word = Arc::new(AtomicI32::new(0));
            let key = Key::private(word.as_ptr() as usize).unwrap();
            let (tx, rx) = mpsc::channel();
            let target = Arc::clone(&word);
            let sleeper = std::thread::spawn(move || {
                tx.send(wait(0, Some(Duration::from_secs(15)), u32::MAX, || {
                    Ok((key, target.load(Ordering::SeqCst)))
                }))
                .unwrap();
            });
            let started = Instant::now();
            loop {
                let shared = shared().unwrap();
                let _guard = shared.acquire().unwrap();
                if shared
                    .load()
                    .unwrap()
                    .iter()
                    .any(|record| record.key == key)
                {
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(5));
                drop(_guard);
                std::thread::yield_now();
            }
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "futex::tests::transaction_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("KINAKAZE_FUTEX_TEST_MODE", mode)
                .env(
                    "KINAKAZE_FUTEX_TEST_KEY",
                    key.0.map(|part| part.to_string()).join(","),
                )
                .creation_flags(0x0800_0000)
                .output()
                .unwrap();
            let expected = if mode == "other-domain" { 0 } else { 77 };
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if mode != "after-commit" {
                assert!(
                    rx.recv_timeout(Duration::from_millis(30)).is_err(),
                    "false wake: {mode}"
                );
                assert_eq!(wake(key, 1, u32::MAX), Ok(1), "{mode}");
            }
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                Ok(()),
                "{mode}"
            );
            sleeper.join().unwrap();
            assert_eq!(wake(key, 1, u32::MAX), Ok(0));
        }
    }
}

fn thread_birth(thread: HANDLE) -> Result<u64, i32> {
    let (mut born, mut end, mut kernel, mut user) = unsafe { core::mem::zeroed() };
    if unsafe { GetThreadTimes(thread, &mut born, &mut end, &mut kernel, &mut user) } == 0 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(timestamp(born))
}

// A domain has a bounded kernel resource, not a silently truncated queue.
// Exhaustion reports ENOMEM after reclaiming dead waiters.
const CAPACITY: usize = 32_768;
const MAGIC: u64 = u64::from_le_bytes(*b"CYFUT001");

#[repr(C)]
struct Header {
    magic: u64,
    size: u64,
    active: AtomicU32,
    reserved: u32,
    next_token: AtomicU64,
}

#[repr(C)]
struct Bank {
    count: u64,
    records: [Record; CAPACITY],
}

const SECTION_SIZE: usize = size_of::<Header>() + 2 * size_of::<Bank>();
static CACHE: AtomicPtr<Shared> = AtomicPtr::new(ptr::null_mut());

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
    domain: u64,
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

    fn load(&self) -> Result<Vec<Record>, i32> {
        let active = self.header().active.load(Ordering::Acquire);
        if active > 1 {
            return Err(EIO);
        }
        let bank = self.bank(active);
        unsafe {
            let count = ptr::addr_of!((*bank).count).read() as usize;
            if count > CAPACITY {
                return Err(EIO);
            }
            Ok(std::slice::from_raw_parts(ptr::addr_of!((*bank).records).cast(), count).to_vec())
        }
    }

    fn commit(&self, records: &[Record]) {
        assert!(records.len() <= CAPACITY);
        let next = 1 - self.header().active.load(Ordering::Acquire);
        let bank = self.bank(next);
        unsafe {
            ptr::copy_nonoverlapping(
                records.as_ptr(),
                ptr::addr_of_mut!((*bank).records).cast(),
                records.len(),
            );
            ptr::addr_of_mut!((*bank).count).write(records.len() as u64);
        }
        self.header().active.store(next, Ordering::Release);
    }

    fn select(
        &self,
        records: &mut Vec<Record>,
        key: Key,
        count: u32,
        bitset: u32,
    ) -> Result<u32, i32> {
        let mut selected = 0;
        let mut index = 0;
        while index < records.len() && selected < count {
            let record = records[index];
            if record.key != key || record.bitset & bitset == 0 {
                index += 1;
                continue;
            }
            if record.dead() {
                records.remove(index);
                continue;
            }
            let event = match Handle::new(unsafe {
                OpenEventW(
                    EVENT_MODIFY_STATE,
                    0,
                    record.event_name(self.domain).as_ptr(),
                )
            }) {
                Ok(event) => event,
                Err(_) if record.dead() => {
                    records.remove(index);
                    continue;
                }
                Err(error) => return Err(error),
            };
            // The waiter must take this mutex before interpreting the event.
            // Publication is the wake's linearization point, not SetEvent.
            if unsafe { SetEvent(event.0) } == 0 {
                return Err(errno_from_win32(unsafe { GetLastError() }));
            }
            records.remove(index);
            selected += 1;
        }
        Ok(selected)
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn shared() -> Result<&'static Shared, i32> {
    let existing = CACHE.load(Ordering::Acquire);
    if !existing.is_null() {
        return Ok(unsafe { &*existing });
    }
    let domain = kinakaze_runtime::authority::domain_id();
    let mutex = Handle::new(unsafe {
        CreateMutexW(
            ptr::null(),
            0,
            wide(&format!(r"Local\kinakaze.futex.guard.v1.{domain:016x}")).as_ptr(),
        )
    })?;
    let section = Handle::new(unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            SECTION_SIZE as u32,
            wide(&format!(r"Local\kinakaze.futex.v1.{domain:016x}")).as_ptr(),
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
        domain,
    });
    {
        let _guard = candidate.acquire()?;
        let header = candidate.view.Value.cast::<Header>();
        unsafe {
            if (*header).magic == 0 {
                ptr::addr_of_mut!((*header).size).write(SECTION_SIZE as u64);
                ptr::addr_of_mut!((*header).magic).write(MAGIC);
            }
            if (*header).magic != MAGIC || (*header).size != SECTION_SIZE as u64 {
                return Err(EIO);
            }
        }
    }
    let candidate = Box::into_raw(candidate);
    match CACHE.compare_exchange(
        ptr::null_mut(),
        candidate,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => Ok(unsafe { &*candidate }),
        Err(existing) => {
            unsafe { drop(Box::from_raw(candidate)) };
            Ok(unsafe { &*existing })
        }
    }
}

pub(crate) fn reset_after_fork() {
    // Neither the parent's handles nor its local allocations belong to the child.
    CACHE.store(ptr::null_mut(), Ordering::Release);
}

pub(crate) fn wake(key: Key, count: u32, bitset: u32) -> Result<u32, i32> {
    let shared = shared()?;
    let _guard = shared.acquire()?;
    let mut records = shared.load()?;
    let result = shared.select(&mut records, key, count, bitset);
    // A late error must not roll back earlier, already selected wakes.
    shared.commit(&records);
    result
}

pub(crate) fn requeue(
    key: Key,
    destination: Key,
    count: u32,
    move_count: u32,
    comparison: Option<(&AtomicI32, i32)>,
) -> Result<u32, i32> {
    let shared = shared()?;
    let _guard = shared.acquire()?;
    let mut records = shared.load()?;
    if comparison.is_some_and(|(word, expected)| word.load(Ordering::SeqCst) != expected) {
        return Err(EAGAIN);
    }
    let result = (|| {
        let woken = shared.select(&mut records, key, count, u32::MAX)?;
        let mut moved = 0;
        let mut transfers = Vec::new();
        let mut index = 0;
        while index < records.len() && moved < move_count {
            if records[index].key != key {
                index += 1;
                continue;
            }
            let mut record = records.remove(index);
            if record.dead() {
                continue;
            }
            record.key = destination;
            transfers.push(record);
            moved += 1;
        }
        records.extend(transfers);
        Ok(woken + moved)
    })();
    shared.commit(&records);
    result
}

pub(crate) fn wake_op(
    key: Key,
    count: u32,
    destination: Key,
    count2: u32,
    operation: impl FnOnce() -> Result<bool, i32>,
) -> Result<u32, i32> {
    let shared = shared()?;
    let _guard = shared.acquire()?;
    let mut records = shared.load()?;
    let second = operation()?;
    let result = (|| {
        let first = shared.select(&mut records, key, count, u32::MAX)?;
        Ok(first
            + if second {
                shared.select(&mut records, destination, count2, u32::MAX)?
            } else {
                0
            })
    })();
    shared.commit(&records);
    result
}

struct Waiter {
    shared: &'static Shared,
    record: Record,
    event: Handle,
}

impl Waiter {
    fn enqueue(
        expected: i32,
        bitset: u32,
        load: &impl Fn() -> Result<(Key, i32), i32>,
    ) -> Result<Self, i32> {
        let shared = shared()?;
        let _guard = shared.acquire()?;
        let (key, value) = load()?;
        if value != expected {
            return Err(EAGAIN);
        }
        let mut records = shared.load()?;
        // Reap on enqueue so repeated killed waiters cannot consume the domain.
        records.retain(|record| !record.dead());
        if records.len() == CAPACITY {
            return Err(ENOMEM);
        }
        let token = shared.header().next_token.fetch_add(1, Ordering::Relaxed);
        let record = Record {
            key,
            token,
            born: thread_birth(unsafe { GetCurrentThread() })?,
            host: std::process::id(),
            thread: unsafe { GetCurrentThreadId() },
            bitset,
            reserved: 0,
        };
        let event = Handle::new(unsafe {
            CreateEventW(ptr::null(), 0, 0, record.event_name(shared.domain).as_ptr())
        })?;
        records.push(record);
        shared.commit(&records);
        Ok(Self {
            shared,
            record,
            event,
        })
    }

    /// Check selection and optionally cancel, following the token even after
    /// another process requeues it. Returns true only for a committed wake.
    fn finish(&self, cancel: bool) -> Result<bool, i32> {
        let _guard = self.shared.acquire()?;
        let mut records = self.shared.load()?;
        let Some(index) = records
            .iter()
            .position(|record| record.token == self.record.token)
        else {
            return Ok(true);
        };
        if cancel {
            records.remove(index);
            self.shared.commit(&records);
        }
        Ok(false)
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        let _ = self.finish(true);
    }
}

pub(crate) fn wait(
    expected: i32,
    duration: Option<Duration>,
    bitset: u32,
    load: impl Fn() -> Result<(Key, i32), i32>,
) -> Result<(), i32> {
    let started = Instant::now();
    let interrupt = interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    loop {
        // A wait holds its key, not a Rust reference into guest memory. Another
        // thread may unmap that VA while it sleeps. Restart resolves it anew.
        let waiter = Waiter::enqueue(expected, bitset, &load)?;
        signal::register_waiter();
        let outcome = loop {
            let pending = signal::pending() & !signal::blocked_mask() != 0;
            let expired = duration.is_some_and(|limit| started.elapsed() >= limit);
            if pending || expired {
                break waiter.finish(true).and_then(|woken| {
                    if woken {
                        Ok(true)
                    } else if expired {
                        Err(ETIMEDOUT)
                    } else {
                        Ok(false)
                    }
                });
            }
            let milliseconds = duration.map_or(INFINITE, |limit| {
                limit
                    .saturating_sub(started.elapsed())
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min(u128::from(INFINITE - 1)) as u32
            });
            let handles = [waiter.event.0, interrupt];
            let status = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, milliseconds) };
            if status == WAIT_OBJECT_0 || status == WAIT_TIMEOUT {
                match waiter.finish(false) {
                    Ok(true) => break Ok(true),
                    Ok(false) => continue, // pre-commit event from an abandoned waker
                    Err(error) => break Err(error),
                }
            } else if status == WAIT_OBJECT_0 + 1 {
                break waiter.finish(true);
            } else {
                break waiter
                    .finish(true)
                    .and_then(|woken| if woken { Ok(true) } else { Err(EIO) });
            }
        };
        signal::unregister_waiter();
        // A handler may itself block or fork; retire all outer wait resources
        // first, including the parent-only named section/event handles.
        drop(waiter);
        let delivery = signal::deliver_pending();
        if outcome? {
            return Ok(());
        }
        if duration.is_some_and(|limit| started.elapsed() >= limit) {
            return Err(ETIMEDOUT);
        }
        if delivery == signal::Delivery::Interrupted {
            return Err(EINTR);
        }
        // SA_RESTART repeats the original value check with the original budget.
    }
}
