//! Value-only shared condition queues, with committed selection before return.
//! Reuse each thread's named park event; private cancellation rows remain local.
use super::*;
use core::{mem::size_of, ptr};
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    EVENT_MODIFY_STATE, GetProcessIdOfThread, GetThreadTimes, INFINITE, OpenEventW, OpenThread,
    SetEvent, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SYNCHRONIZE,
};

const CAPACITY: usize = 512;
const MAGIC: u64 = u64::from_le_bytes(*b"CYCOND01");
const EIO: i32 = 5;
const ENOMEM: i32 = 12;
static CACHE: AtomicPtr<Cache> = AtomicPtr::new(ptr::null_mut());

#[repr(C)]
#[derive(Clone, Copy)]
struct Row {
    token: u64,
    birth: u64,
    host: u32,
    thread: u32,
    selected: u32, // 0 waiting, 1 directed signal, 2 broadcast
    reserved: u32,
}
impl Row {
    fn identity(self) -> parking::Identity {
        parking::Identity {
            host: self.host,
            thread: self.thread,
            birth: self.birth,
        }
    }
    fn dead(self) -> bool {
        let value = unsafe {
            OpenThread(
                THREAD_QUERY_LIMITED_INFORMATION | THREAD_SYNCHRONIZE,
                0,
                self.thread,
            )
        };
        if value.is_null() {
            return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
        }
        let handle = NativeHandle {
            host: std::process::id(),
            value: value as usize,
        };
        if unsafe { WaitForSingleObject(handle.raw(), 0) } == WAIT_OBJECT_0 {
            return true;
        }
        let host = unsafe { GetProcessIdOfThread(handle.raw()) };
        if host != 0 && host != self.host {
            return true;
        }
        let (mut born, mut end, mut kernel, mut user) = unsafe { core::mem::zeroed() };
        (unsafe { GetThreadTimes(handle.raw(), &mut born, &mut end, &mut kernel, &mut user) }) != 0
            && ((u64::from(born.dwHighDateTime) << 32) | u64::from(born.dwLowDateTime))
                != self.birth
    }
}
#[repr(C)]
struct Header {
    magic: u64,
    size: u64,
    active: AtomicU32,
    cursor: u32,
    next: AtomicU64,
}
#[repr(C)]
struct Rows {
    count: u64,
    rows: [Row; CAPACITY],
}
const SIZE: usize = size_of::<Header>() + 2 * size_of::<Rows>();
struct Bank {
    host: u32,
    domain: u64,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _section: NativeHandle,
    gate: NativeHandle,
}
// The view is used exclusively while holding the named gate.
unsafe impl Send for Bank {}
unsafe impl Sync for Bank {}
impl Drop for Bank {
    fn drop(&mut self) {
        if self.host == std::process::id() {
            unsafe { UnmapViewOfFile(self.view) };
        }
    }
}
struct Guard<'a>(&'a Bank);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe { ReleaseMutex(self.0.gate.raw()) };
    }
}
impl Bank {
    fn acquire(&self) -> Result<Guard<'_>, i32> {
        match unsafe { WaitForSingleObject(self.gate.raw(), INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Guard(self)),
            _ => Err(EIO),
        }
    }
    fn header(&self) -> &Header {
        unsafe { &*self.view.Value.cast() }
    }
    fn rows(&self, index: u32) -> *mut Rows {
        unsafe {
            self.view
                .Value
                .cast::<u8>()
                .add(size_of::<Header>() + index as usize * size_of::<Rows>())
                .cast()
        }
    }
    fn load(&self) -> Result<Vec<Row>, i32> {
        let active = self.header().active.load(Ordering::Acquire);
        if active > 1 {
            return Err(EIO);
        }
        let rows = self.rows(active);
        let count = unsafe { ptr::addr_of!((*rows).count).read() } as usize;
        if count > CAPACITY {
            return Err(EIO);
        }
        Ok(
            unsafe { std::slice::from_raw_parts(ptr::addr_of!((*rows).rows).cast(), count) }
                .to_vec(),
        )
    }
    fn commit(&self, rows: &[Row]) {
        assert!(rows.len() <= CAPACITY);
        let next = 1 - self.header().active.load(Ordering::Acquire);
        let target = self.rows(next);
        unsafe {
            ptr::copy_nonoverlapping(
                rows.as_ptr(),
                ptr::addr_of_mut!((*target).rows).cast(),
                rows.len(),
            );
            ptr::addr_of_mut!((*target).count).write(rows.len() as u64);
        }
        self.header().active.store(next, Ordering::Release);
    }
    fn notify(&self, rows: &mut [Row], count: usize, broadcast: bool) -> Result<(), i32> {
        let mut selected = 0;
        for row in rows {
            if selected == count {
                break;
            }
            if row.selected != 0 || row.dead() {
                continue;
            }
            let name = parking::event_name(self.domain, row.identity());
            let value = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
            if value.is_null() {
                if row.dead() {
                    continue;
                }
                return Err(EIO);
            }
            let event = NativeHandle {
                host: self.host,
                value: value as usize,
            };
            // A pre-commit event does not select a waiter. The active bank is
            // authoritative when a signaler dies during a broadcast.
            if unsafe { SetEvent(event.raw()) } == 0 {
                return Err(EIO);
            }
            row.selected = if broadcast { 2 } else { 1 };
            selected += 1;
        }
        Ok(())
    }
    fn sweep(&self, rows: &mut Vec<Row>, full: bool) -> Result<(), i32> {
        let mut replacement = 0;
        if full {
            rows.retain(|row| {
                if !row.dead() {
                    return true;
                }
                replacement += usize::from(row.selected == 1);
                false
            });
        } else if !rows.is_empty() {
            let cursor = unsafe { &mut (*self.view.Value.cast::<Header>()).cursor };
            let index = *cursor as usize % rows.len();
            *cursor = cursor.wrapping_add(1);
            if rows[index].dead() {
                replacement += usize::from(rows.remove(index).selected == 1);
            }
        }
        self.notify(rows, replacement, false)
    }
}
struct Cache {
    host: u32,
    rows: Mutex<HashMap<Key, Arc<Bank>>>,
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
            return unsafe { &*next };
        }
        unsafe { drop(Box::from_raw(next)) };
    }
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn native(value: HANDLE) -> Result<NativeHandle, i32> {
    if value.is_null() {
        Err(EAGAIN)
    } else {
        Ok(NativeHandle {
            host: std::process::id(),
            value: value as usize,
        })
    }
}
fn open(key: Key) -> Result<Arc<Bank>, i32> {
    let name = format!(
        "Local\\kinakaze.pthread.condition.v1.{:016x}.{:08x}.{:016x}.{:016x}",
        key.domain, key.creator, key.birth, key.generation
    );
    let gate =
        native(unsafe { CreateMutexW(ptr::null(), 0, wide(&(name.clone() + ".gate")).as_ptr()) })?;
    let section = native(unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            SIZE as u32,
            wide(&name).as_ptr(),
        )
    })?;
    let view = unsafe { MapViewOfFile(section.raw(), FILE_MAP_ALL_ACCESS, 0, 0, SIZE) };
    if view.Value.is_null() {
        return Err(EAGAIN);
    }
    let bank = Arc::new(Bank {
        host: std::process::id(),
        domain: key.domain,
        view,
        _section: section,
        gate,
    });
    {
        let _guard = bank.acquire()?;
        let header = view.Value.cast::<Header>();
        unsafe {
            if (*header).magic == 0 {
                ptr::addr_of_mut!((*header).size).write(SIZE as u64);
                ptr::addr_of_mut!((*header).magic).write(MAGIC);
            }
            if (*header).magic != MAGIC || (*header).size != SIZE as u64 {
                return Err(EIO);
            }
        }
    }
    Ok(bank)
}
fn bank(key: Key) -> Result<Arc<Bank>, i32> {
    if !cached() {
        return open(key);
    }
    let mut rows = cache().rows.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(bank) = rows.get(&key) {
        return Ok(bank.clone());
    }
    let bank = open(key)?;
    if rows.len() >= CACHE_LIMIT {
        let idle = rows
            .iter()
            .find_map(|(key, value)| (Arc::strong_count(value) == 1).then_some(*key));
        if let Some(idle) = idle {
            rows.remove(&idle);
        }
    }
    if rows.len() < CACHE_LIMIT {
        rows.insert(key, bank.clone());
    }
    Ok(bank)
}
pub(crate) unsafe fn is_cond(cond: *mut usize) -> bool {
    unsafe { field(cond, 40) }.load(Ordering::Acquire) & 1 != 0
}
pub(crate) unsafe fn clock(cond: *mut usize) -> i32 {
    ((unsafe { field(cond, 40) }.load(Ordering::Acquire) >> 1) & 1) as i32
}
pub(crate) unsafe fn init(cond: *mut usize, clock: i32) -> i32 {
    let identity = match new_identity() {
        Ok(key) => key,
        Err(e) => return e,
    };
    if let Err(e) = bank(identity) {
        return e;
    }
    unsafe {
        ptr::write_bytes(cond.cast::<u8>(), 0, 48);
        let bytes = cond.cast::<u8>();
        bytes.add(20).cast::<u32>().write(identity.creator);
        bytes.add(24).cast::<u64>().write(identity.birth);
        bytes.add(32).cast::<u64>().write(identity.generation);
        bytes.add(40).cast::<u32>().write(1 | ((clock as u32) << 1));
    }
    0
}
pub(crate) unsafe fn signal(cond: *mut usize, all: bool) -> i32 {
    // Register increments before publication/release; retire decrements after
    // removal. Dead processes leave only false positives, never missed waiters.
    if cached() && unsafe { field(cond, 40) }.load(Ordering::Acquire) >> 3 == 0 {
        return 0;
    }
    let key = match unsafe { key(cond) } {
        Ok(key) => key,
        Err(e) => return e,
    };
    let bank = match bank(key) {
        Ok(bank) => bank,
        Err(e) => return e,
    };
    let _guard = match bank.acquire() {
        Ok(g) => g,
        Err(e) => return e,
    };
    let mut rows = match bank.load() {
        Ok(rows) => rows,
        Err(e) => return e,
    };
    let result = bank
        .sweep(&mut rows, true)
        .and_then(|()| bank.notify(&mut rows, if all { usize::MAX } else { 1 }, all));
    bank.commit(&rows);
    result.err().unwrap_or(0)
}
pub(crate) unsafe fn destroy(cond: *mut usize) -> i32 {
    let key = match unsafe { key(cond) } {
        Ok(key) => key,
        Err(e) => return e,
    };
    let bank = match bank(key) {
        Ok(bank) => bank,
        Err(e) => return e,
    };
    {
        let _guard = match bank.acquire() {
            Ok(g) => g,
            Err(e) => return e,
        };
        let mut rows = match bank.load() {
            Ok(rows) => rows,
            Err(e) => return e,
        };
        let result = bank.sweep(&mut rows, true);
        bank.commit(&rows);
        if let Err(e) = result {
            return e;
        }
        if !rows.is_empty() {
            return EBUSY;
        }
    }
    cache()
        .rows
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&key);
    unsafe { cond.cast::<u8>().add(40).cast::<u32>().write(0) };
    0
}

pub(crate) struct Ticket {
    bank: Arc<Bank>,
    token: u64,
    cond: usize,
    active: bool,
}
impl Ticket {
    pub(crate) unsafe fn register(cond: *mut usize) -> Result<Self, i32> {
        let bank = bank(unsafe { key(cond) }?)?;
        let identity = parking::identity()?;
        let token;
        {
            let _guard = bank.acquire()?;
            let mut rows = bank.load()?;
            let full = rows.len() >= CAPACITY;
            let result = bank.sweep(&mut rows, full);
            bank.commit(&rows);
            result?;
            if rows.len() >= CAPACITY {
                return Err(ENOMEM);
            }
            token = bank
                .header()
                .next
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| EAGAIN)?
                + 1;
            unsafe { field(cond, 40) }
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(8)
                })
                .map_err(|_| EAGAIN)?;
            rows.push(Row {
                token,
                birth: identity.birth,
                host: identity.host,
                thread: identity.thread,
                selected: 0,
                reserved: 0,
            });
            bank.commit(&rows);
        }
        Ok(Self {
            bank,
            token,
            cond: cond as usize,
            active: true,
        })
    }
    pub(crate) fn selected(&self) -> Result<bool, i32> {
        let _guard = self.bank.acquire()?;
        Ok(self
            .bank
            .load()?
            .iter()
            .any(|row| row.token == self.token && row.selected != 0))
    }
    pub(crate) fn finish(&mut self) -> Result<u32, i32> {
        if !self.active {
            return Ok(0);
        }
        let _guard = self.bank.acquire()?;
        let mut rows = self.bank.load()?;
        let selected = rows
            .iter()
            .find(|row| row.token == self.token)
            .map_or(0, |row| row.selected);
        rows.retain(|row| row.token != self.token);
        self.bank.commit(&rows);
        self.active = false;
        unsafe { field(self.cond as _, 40) }.fetch_sub(8, Ordering::AcqRel);
        Ok(selected)
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if self.active
            && let Ok(1) = self.finish()
            && let Ok(_guard) = self.bank.acquire()
            && let Ok(mut rows) = self.bank.load()
        {
            let _ = self.bank.notify(&mut rows, 1, false);
            self.bank.commit(&rows);
        }
    }
}

#[cfg(test)]
mod tests;
