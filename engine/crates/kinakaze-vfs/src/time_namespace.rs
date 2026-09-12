//! Time namespaces backed by reference-counted native section objects.
//! Offsets become immutable when the first process enters. A namespace fd pins
//! the section independently of its members, including across fork and exec.
use crate::{EACCES, EINVAL, EIO, EOVERFLOW, EPERM, ERANGE, FdFlags, FdKind, errno_from_win32};
use std::ptr;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, INFINITE, ReleaseMutex, WaitForSingleObject,
};

const MAGIC: u64 = u64::from_le_bytes(*b"CYTIME01");
const SIZE: usize = 4096;
const NSEC: i64 = 1_000_000_000;
#[repr(C)]
struct Header {
    magic: AtomicU64,
    domain: u64,
    id: u64,
    next: AtomicU64,
    frozen: AtomicU64,
    monotonic: AtomicI64,
    boottime: AtomicI64,
    owner: AtomicU64,
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Namespace {
    section: Handle,
    mutex: Handle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}
unsafe impl Send for Namespace {}
unsafe impl Sync for Namespace {}
impl Drop for Namespace {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}
struct Guard<'a>(&'a Namespace);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.mutex.0);
        }
    }
}
impl Namespace {
    fn header(&self) -> &Header {
        unsafe { &*self.view.Value.cast::<Header>() }
    }
    fn lock(&self) -> Result<Guard<'_>, i32> {
        match unsafe { WaitForSingleObject(self.mutex.0, INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Guard(self)),
            _ => Err(EIO),
        }
    }
    fn open(id: u64, create: bool) -> Result<Arc<Self>, i32> {
        if id == 0 {
            return Err(EINVAL);
        }
        let domain = kinakaze_runtime::authority::domain_id();
        let name = |suffix| {
            format!(r"Local\kinakaze.time.v1.{domain}.{id}.{suffix}")
                .encode_utf16()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        let checked = |handle: HANDLE| {
            if handle.is_null() {
                Err(errno_from_win32(unsafe { GetLastError() }))
            } else {
                Ok(Handle(handle))
            }
        };
        let mutex = checked(unsafe { CreateMutexW(ptr::null(), 0, name("mutex").as_ptr()) })?;
        let section = checked(unsafe {
            if create {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_READWRITE,
                    0,
                    SIZE as u32,
                    name("state").as_ptr(),
                )
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name("state").as_ptr())
            }
        })?;
        let view = unsafe { MapViewOfFile(section.0, FILE_MAP_ALL_ACCESS, 0, 0, SIZE) };
        if view.Value.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let result = Arc::new(Self {
            section,
            mutex,
            view,
        });
        if result.header().magic.load(Ordering::Acquire) == 0 && create {
            let _guard = result.lock()?;
            if result.header().magic.load(Ordering::Acquire) == 0 {
                unsafe {
                    ptr::write(
                        result.view.Value.cast::<Header>(),
                        Header {
                            magic: AtomicU64::new(0),
                            domain: domain as u64,
                            id,
                            next: AtomicU64::new(2),
                            frozen: AtomicU64::new(u64::from(id == 1)),
                            monotonic: AtomicI64::new(0),
                            boottime: AtomicI64::new(0),
                            owner: AtomicU64::new(1),
                        },
                    );
                }
                result.header().magic.store(MAGIC, Ordering::Release);
            }
        }
        let h = result.header();
        if h.magic.load(Ordering::Acquire) != MAGIC || h.id != id || h.domain != domain as u64 {
            return Err(EIO);
        }
        Ok(result)
    }
    fn freeze(&self) -> Result<(), i32> {
        if self.header().frozen.load(Ordering::Acquire) != 0 {
            return Ok(());
        }
        let _guard = self.lock()?;
        self.header().frozen.store(1, Ordering::Release);
        Ok(())
    }
    fn offsets(&self) -> (i64, i64) {
        (
            self.header().monotonic.load(Ordering::Acquire),
            self.header().boottime.load(Ordering::Acquire),
        )
    }
}
static INITIAL: Mutex<Option<Arc<Namespace>>> = Mutex::new(None);
static CURRENT: Mutex<Option<(Arc<Namespace>, Arc<Namespace>)>> = Mutex::new(None);
fn initial() -> Result<Arc<Namespace>, i32> {
    let mut slot = INITIAL.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        *slot = Some(Namespace::open(1, true)?);
    }
    Ok(slot.as_ref().unwrap().clone())
}
pub(crate) fn allocate_object_id() -> Result<u64, i32> {
    initial()?
        .header()
        .next
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            n.checked_add(1).filter(|n| *n < (1u64 << 60))
        })
        .map_err(|_| EOVERFLOW)
}
fn state() -> Result<(Arc<Namespace>, Arc<Namespace>), i32> {
    let mut slot = CURRENT.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        let root = initial()?;
        *slot = Some((root.clone(), root));
    }
    Ok(slot.as_ref().unwrap().clone())
}
fn privileged(bit: u32) -> bool {
    crate::user_namespace::id(crate::job::process_id())
        .is_ok_and(|id| crate::user_namespace::capable(id, bit))
}
fn publish(current: Arc<Namespace>, children: Arc<Namespace>) -> Result<(), i32> {
    if !kinakaze_runtime::job::set_time_namespaces(
        crate::job::process_id(),
        current.header().id,
        children.header().id,
    ) {
        return Err(EIO);
    }
    *CURRENT.lock().map_err(|_| EIO)? = Some((current, children));
    Ok(())
}
/// Build first so an allocation error cannot change either namespace membership.
pub fn prepare_unshare() -> Result<impl FnOnce() -> Result<(), i32>, i32> {
    prepare_with_user(None)
}
pub fn prepare_with_user(
    user: Option<&crate::user_namespace::Prepared>,
) -> Result<impl FnOnce() -> Result<(), i32> + use<>, i32> {
    if user.is_none() && !privileged(21) {
        return Err(EPERM);
    }
    let (current, _) = state()?;
    let root = initial()?;
    let id = root
        .header()
        .next
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            n.checked_add(1).filter(|n| *n < (1u64 << 60))
        })
        .map_err(|_| EOVERFLOW)?;
    let child = Namespace::open(id, true)?;
    child.header().owner.store(
        match user {
            Some(u) => u.id(),
            None => crate::user_namespace::id(crate::job::process_id())?,
        },
        Ordering::Release,
    );
    let (monotonic, boottime) = current.offsets();
    child.header().monotonic.store(monotonic, Ordering::Release);
    child.header().boottime.store(boottime, Ordering::Release);
    Ok(move || publish(current, child))
}
pub fn unshare() -> Result<(), i32> {
    prepare_unshare()?()
}
pub fn process_id(pid: u32, children: bool) -> Result<u64, i32> {
    if pid == crate::job::process_id() {
        let (current, child) = state()?;
        return Ok(if children { child } else { current }.header().id);
    }
    let (current, child) = kinakaze_runtime::job::time_namespaces(pid).ok_or(crate::ENOENT)?;
    Ok(if children { child } else { current })
}
pub fn inode(id: u64) -> u64 {
    if id == 1 {
        4026531834
    } else {
        (1u64 << 62) | id
    }
}
fn descriptor(fd: i32) -> Result<Arc<Namespace>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|entry| *entry)
        .ok_or(crate::EBADF)?;
    if entry.kind != FdKind::TimeNamespace {
        return Err(EINVAL);
    }
    let view = unsafe { MapViewOfFile(entry.raw as HANDLE, FILE_MAP_ALL_ACCESS, 0, 0, SIZE) };
    if view.Value.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let h = unsafe { &*view.Value.cast::<Header>() };
    let result = if h.magic.load(Ordering::Acquire) == MAGIC
        && h.domain == kinakaze_runtime::authority::domain_id()
    {
        Namespace::open(h.id, false)
    } else {
        Err(EIO)
    };
    unsafe {
        UnmapViewOfFile(view);
    }
    result
}
pub fn descriptor_inode(fd: i32) -> Result<u64, i32> {
    Ok(inode(descriptor(fd)?.header().id))
}
pub fn open_process(pid: u32, children: bool, flags: FdFlags) -> Result<i32, i32> {
    let namespace = Namespace::open(process_id(pid, children)?, false)?;
    let mut handle = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            namespace.section.0,
            GetCurrentProcess(),
            &mut handle,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    match crate::install(handle as usize, FdKind::TimeNamespace, flags) {
        Ok(fd) => Ok(fd),
        Err(error) => {
            unsafe {
                CloseHandle(handle);
            }
            Err(error)
        }
    }
}
/// A process created by clone3(NEWTIME) enters its newly created time namespace.
pub fn enter_children() -> Result<(), i32> {
    let (_, child) = state()?;
    child.freeze()?;
    publish(child.clone(), child)
}
pub fn enter(fd: i32) -> Result<(), i32> {
    let ns = descriptor(fd)?;
    if !crate::user_namespace::capable(ns.header().owner.load(Ordering::Acquire).max(1), 21) {
        return Err(EPERM);
    }
    if kinakaze_runtime::process_thread_count() != 1 {
        return Err(87);
    } // EUSERS
    ns.freeze()?;
    publish(ns.clone(), ns)
}
pub fn read_offsets(pid: u32) -> Result<Vec<u8>, i32> {
    let ns = Namespace::open(process_id(pid, true)?, false)?;
    let _guard = ns.lock()?;
    let (m, b) = ns.offsets();
    Ok(format!(
        "monotonic {} {}\nboottime {} {}\n",
        m.div_euclid(NSEC),
        m.rem_euclid(NSEC),
        b.div_euclid(NSEC),
        b.rem_euclid(NSEC)
    )
    .into_bytes())
}
// sscanf in procfs accepts a signed decimal prefix and leaves trailing input
// after its third conversion. Keep the parser independent of namespace state.
fn decimal_prefix(text: &mut &str) -> Result<i64, i32> {
    *text = text.trim_start_matches(|c| matches!(c, ' ' | '\t'..='\r'));
    let bytes = text.as_bytes();
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut end = sign;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == sign {
        return Err(EINVAL);
    }
    let value = text[..end].parse().map_err(|_| ERANGE)?;
    *text = &text[end..];
    Ok(value)
}

fn parse_offsets(bytes: &[u8], offset: u64) -> Result<(Vec<(i32, i64)>, usize), i32> {
    if offset != 0 || bytes.len() >= 4096 {
        return Err(EINVAL);
    }
    let text = crate::procfs::write_text(bytes)?;
    let mut updates = Vec::new();
    let mut consumed = 0;
    let mut written = bytes.len();
    for line in text.split_inclusive('\n') {
        consumed += line.len();
        let mut tail = line.trim_start_matches(|c| matches!(c, ' ' | '\t'..='\r'));
        let clock_end = tail
            .bytes()
            .position(|c| matches!(c, b' ' | b'\t'..=b'\r'))
            .unwrap_or(tail.len());
        let clock = match &tail[..clock_end] {
            "monotonic" | "1" => 1,
            "boottime" | "7" => 7,
            _ => return Err(EINVAL),
        };
        tail = &tail[clock_end..];
        let sec = decimal_prefix(&mut tail)?;
        let nsec = decimal_prefix(&mut tail)?;
        if !(0..NSEC).contains(&nsec) {
            return Err(EINVAL);
        }
        let value = sec
            .checked_mul(NSEC)
            .and_then(|s| s.checked_add(nsec))
            .ok_or(ERANGE)?;
        updates.push((clock, value));
        // Linux consumes at most two lines per write. Repeated clock IDs are
        // legal and the final assignment wins. A following line is left for
        // the caller to resubmit after the short write.
        if updates.len() == 2 {
            if consumed < text.len() {
                written = consumed;
            }
            break;
        }
    }
    if updates.is_empty() {
        return Err(EINVAL);
    }
    Ok((updates, written))
}

pub fn write_offsets(pid: u32, bytes: &[u8], offset: u64) -> Result<usize, i32> {
    let (updates, written) = parse_offsets(bytes, offset)?;
    let ns = Namespace::open(process_id(pid, true)?, false)?;
    if !crate::user_namespace::capable(ns.header().owner.load(Ordering::Acquire).max(1), 25) {
        return Err(EPERM);
    }
    for &(clock, value) in &updates {
        let seconds = (host_nanoseconds(clock)? as i128 + value as i128).div_euclid(NSEC as i128);
        if !(0..=((i64::MAX / NSEC) / 2) as i128).contains(&seconds) {
            return Err(ERANGE);
        }
    }
    let ns = Namespace::open(process_id(pid, true)?, false)?;
    let _guard = ns.lock()?;
    if ns.header().frozen.load(Ordering::Acquire) != 0 {
        return Err(EACCES);
    }
    for (clock, value) in updates {
        let target = if clock == 1 {
            &ns.header().monotonic
        } else {
            &ns.header().boottime
        };
        target.store(value, Ordering::Release);
    }
    Ok(written)
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryUnbiasedInterruptTimePrecise(time: *mut u64);
    fn QueryInterruptTimePrecise(time: *mut u64);
}
/// Host elapsed time: unbiased excludes suspend; interrupt time includes it.
pub fn host_nanoseconds(clock: i32) -> Result<i64, i32> {
    let mut ticks = 0u64;
    match clock {
        1 | 4 | 6 => unsafe {
            QueryUnbiasedInterruptTimePrecise(&mut ticks);
        },
        7 | 9 => unsafe {
            QueryInterruptTimePrecise(&mut ticks);
        },
        _ => return Err(EINVAL),
    }
    i64::try_from(ticks)
        .ok()
        .and_then(|t| t.checked_mul(100))
        .ok_or(EOVERFLOW)
}
pub fn offset(clock: i32) -> Result<i64, i32> {
    let ns = state()?.0;
    match clock {
        1 | 4 | 6 => Ok(ns.offsets().0),
        7 | 9 => Ok(ns.offsets().1),
        _ => Ok(0),
    }
}
pub fn clock(clock: i32) -> Result<(i64, i64), i32> {
    let value = host_nanoseconds(clock)?
        .checked_add(offset(clock)?)
        .ok_or(EOVERFLOW)?;
    Ok((value.div_euclid(NSEC), value.rem_euclid(NSEC)))
}
pub(crate) fn serialize(_fork: bool) -> Result<Vec<u8>, i32> {
    let (_, children) = state()?;
    // Prepare before the coordinator freezes sibling threads. Child restore
    // must never wait for an offset writer in a frozen parent.
    children.freeze()?;
    let mut bytes = Vec::new();
    for word in [
        MAGIC,
        kinakaze_runtime::authority::domain_id(),
        children.header().id,
        children.header().id,
        1,
    ] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    Ok(bytes)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    let run = || -> Result<(), i32> {
        if bytes.len() != 40 {
            return Err(EINVAL);
        }
        let word = |n| u64::from_le_bytes(bytes[n..n + 8].try_into().unwrap());
        if word(0) != MAGIC || word(8) != kinakaze_runtime::authority::domain_id() || word(32) > 1 {
            return Err(EINVAL);
        }
        let _initial = initial()?;
        let current = Namespace::open(word(16), false)?;
        let children = Namespace::open(word(24), false)?;
        if word(32) == 1 {
            current.freeze()?;
        }
        publish(current, children)
    };
    run().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proc_offsets_accept_runc_terminators_and_consume_two_lines() {
        let input = b"monotonic 120 0\nboottime 240 0\n\0\xff";
        assert_eq!(
            parse_offsets(input, 0),
            Ok((vec![(1, 120 * NSEC), (7, 240 * NSEC)], input.len()))
        );
        let input = b"1 1 0\n1 2 0\n7 3 0\n";
        let (updates, written) = parse_offsets(input, 0).unwrap();
        assert_eq!(updates, vec![(1, NSEC), (1, 2 * NSEC)]);
        assert_eq!(written, 12);
        assert_eq!(
            parse_offsets(&input[written..], 0),
            Ok((vec![(7, 3 * NSEC)], 6))
        );
        assert_eq!(
            parse_offsets(b"1 1 0 ignored", 0),
            Ok((vec![(1, NSEC)], 13))
        );
        assert_eq!(parse_offsets(b"1 1 0", 1), Err(EINVAL));
        assert_eq!(parse_offsets(b"\0", 0), Err(EINVAL));
        assert_eq!(parse_offsets(b"1 1 -1", 0), Err(EINVAL));
        assert_eq!(parse_offsets(b"1 1 1000000000", 0), Err(EINVAL));
        assert_eq!(parse_offsets(b"1 nope 0", 0), Err(EINVAL));
    }
    #[test]
    fn reopening_a_frozen_namespace_does_not_wait_on_its_writer_mutex() {
        let id = allocate_object_id().unwrap();
        let namespace = Namespace::open(id, true).unwrap();
        namespace.freeze().unwrap();
        let guard = namespace.lock().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = Namespace::open(id, false).and_then(|ns| ns.freeze());
            sender.send(result).unwrap();
        });
        let result = receiver.recv_timeout(std::time::Duration::from_secs(1));
        drop(guard);
        worker.join().unwrap();
        assert_eq!(result.unwrap(), Ok(()));
    }
}

pub fn owner(fd: i32) -> Result<u64, i32> {
    Ok(descriptor(fd)?
        .header()
        .owner
        .load(Ordering::Acquire)
        .max(1))
}
