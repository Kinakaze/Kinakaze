//! Linux SQ/CQ mmap ABI over the native asynchronous ring implementation.
//! Guest mappings alias owned sections; completion publication never retains
//! a guest virtual address, and closing the fd retires native I/O first.
use super::*;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub const OFF_SQ_RING: u64 = 0;
pub const OFF_CQ_RING: u64 = 0x0800_0000;
pub const OFF_SQES: u64 = 0x1000_0000;
const SQ_ARRAY: usize = 128;
const CQ_HEAD: usize = 64;
const CQ_TAIL: usize = 68;
const CQ_FLAGS: usize = 84;

fn duplicate(raw: HANDLE) -> Result<Object, i32> {
    let mut handle = std::ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut handle,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(EIO);
    }
    Object::owned(handle)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Offsets {
    pub head: u32,
    pub tail: u32,
    pub mask: u32,
    pub entries: u32,
    pub flags_or_overflow: u32,
    pub dropped_or_cqes: u32,
    pub array_or_flags: u32,
    pub reserved: u32,
    pub user_address: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Params {
    pub sq_entries: u32,
    pub cq_entries: u32,
    pub flags: u32,
    pub sq_thread_cpu: u32,
    pub sq_thread_idle: u32,
    pub features: u32,
    pub workqueue_fd: u32,
    pub reserved: [u32; 3],
    pub sq: Offsets,
    pub cq: Offsets,
}

struct Section {
    object: Object,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    length: usize,
}
unsafe impl Send for Section {}
unsafe impl Sync for Section {}
impl Section {
    fn new(length: usize) -> Result<Self, i32> {
        let length = length.checked_add(4095).ok_or(EINVAL)? & !4095;
        let object = Object::owned(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                (length as u64 >> 32) as u32,
                length as u32,
                std::ptr::null(),
            )
        })?;
        let view = unsafe { MapViewOfFile(object.raw(), FILE_MAP_ALL_ACCESS, 0, 0, length) };
        if view.Value.is_null() {
            return Err(crate::ENOMEM);
        }
        Ok(Self {
            object,
            view,
            length,
        })
    }
    fn word(&self, offset: usize) -> &AtomicU32 {
        assert!(offset % 4 == 0 && offset + 4 <= self.length);
        unsafe { &*self.view.Value.cast::<u8>().add(offset).cast::<AtomicU32>() }
    }
    fn address(&self, offset: usize) -> usize {
        self.view.Value as usize + offset
    }
    fn duplicate(&self) -> Result<Object, i32> {
        duplicate(self.object.raw())
    }
}
impl Drop for Section {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
        }
    }
}

pub(super) struct Queues {
    rings: Section,
    sqes: Section,
    sq_entries: u32,
    cq_entries: u32,
    cq_offset: usize,
    submit: Mutex<()>,
    ready: Event,
    failed: AtomicI32,
    files: Mutex<Option<Vec<Option<FixedFile>>>>,
    eventfd: Mutex<Option<crate::eventfd::CompletionSignal>>,
}
struct FixedFile {
    object: Object,
    descriptor: FdEntry,
}
fn fixed_file(fd: i32) -> Result<Option<FixedFile>, i32> {
    if fd == -1 {
        return Ok(None);
    }
    let (object, descriptor) = Object::from_fd_with_entry(fd)?;
    if !matches!(descriptor.kind, FdKind::File | FdKind::Directory) {
        return Err(crate::EOPNOTSUPP);
    }
    Ok(Some(FixedFile { object, descriptor }))
}
impl Queues {
    fn new(sq_entries: u32, cq_entries: u32) -> Result<Self, i32> {
        let cq_offset = (SQ_ARRAY + sq_entries as usize * 4 + 63) & !63;
        let rings = Section::new(cq_offset + cq_entries as usize * 16)?;
        let sqes = Section::new(sq_entries as usize * 64)?;
        for (offset, value) in [
            (8, sq_entries - 1),
            (12, sq_entries),
            (72, cq_entries - 1),
            (76, cq_entries),
        ] {
            rings.word(offset).store(value, Ordering::Release);
        }
        Ok(Self {
            rings,
            sqes,
            sq_entries,
            cq_entries,
            cq_offset,
            submit: Mutex::new(()),
            ready: Event::new(true)?,
            failed: AtomicI32::new(0),
            files: Mutex::new(None),
            eventfd: Mutex::new(None),
        })
    }
    fn count(&self) -> u32 {
        self.rings
            .word(CQ_TAIL)
            .load(Ordering::Acquire)
            .wrapping_sub(self.rings.word(CQ_HEAD).load(Ordering::Acquire))
    }
    // Called under Ring.state. Tail publication orders all CQE and read-buffer
    // writes before a liburing consumer can observe the completion.
    fn publish(&self, state: &mut State) -> Result<(), i32> {
        let head = self.rings.word(CQ_HEAD).load(Ordering::Acquire);
        let mut tail = self.rings.word(CQ_TAIL).load(Ordering::Relaxed);
        let original_tail = tail;
        if tail.wrapping_sub(head) > self.cq_entries {
            return Err(EINVAL);
        }
        while tail.wrapping_sub(head) < self.cq_entries {
            let Some(entry) = state.completions.pop_front() else {
                break;
            };
            let address = self
                .rings
                .address(self.cq_offset + (tail & (self.cq_entries - 1)) as usize * 16);
            unsafe {
                std::ptr::write_volatile(address as *mut CompletionEntry, entry);
            }
            tail = tail.wrapping_add(1);
        }
        self.rings.word(CQ_TAIL).store(tail, Ordering::Release);
        if tail != original_tail {
            if self.rings.word(CQ_FLAGS).load(Ordering::Acquire) & 1 == 0 {
                if let Some(event) = self.eventfd.lock().map_err(|_| EIO)?.as_ref() {
                    event.signal()?;
                }
            }
            state.notify();
        }
        if tail != self.rings.word(CQ_HEAD).load(Ordering::Acquire) {
            self.ready.signal();
        } else {
            unsafe {
                ResetEvent(self.ready.raw());
            }
        }
        // A full CQ keeps excess completions in the owned queue. Enter flushes
        // that queue when userspace advances head; no CQE is silently dropped.
        self.rings.word(16).store(
            if state.completions.is_empty() { 0 } else { 2 },
            Ordering::Release,
        );
        Ok(())
    }
}

fn import<T: Copy + Default>(address: usize) -> Result<T, i32> {
    let mut value = T::default();
    let size = std::mem::size_of::<T>();
    if memory::copy(address, &mut value as *mut T as usize, size)? != size {
        return Err(crate::EFAULT);
    }
    Ok(value)
}
fn export<T>(address: usize, value: &T) -> Result<(), i32> {
    let size = std::mem::size_of::<T>();
    if memory::copy(value as *const T as usize, address, size)? != size {
        return Err(crate::EFAULT);
    }
    Ok(())
}

pub fn setup(entries: u32, address: usize) -> Result<i32, i32> {
    let mut params: Params = import(address)?;
    const CQSIZE: u32 = 1 << 3;
    if params.reserved != [0; 3] || params.flags & !(IORING_SETUP_CLAMP | CQSIZE) != 0 {
        return Err(EINVAL);
    }
    if entries == 0 || (entries > 32768 && params.flags & IORING_SETUP_CLAMP == 0) {
        return Err(EINVAL);
    }
    let entries = entries.min(32768).next_power_of_two();
    let cq_entries = if params.flags & CQSIZE != 0 {
        if params.cq_entries < entries || params.cq_entries > 65536 {
            return Err(EINVAL);
        }
        params.cq_entries.next_power_of_two()
    } else {
        entries * 2
    };
    let queues = Arc::new(Queues::new(entries, cq_entries)?);
    let fd = super::setup(entries, 0)?;
    let result = (|| {
        let ring = lookup(fd)?;
        params.sq_entries = entries;
        params.cq_entries = cq_entries;
        params.features = 1 | 2 | 4; // SINGLE_MMAP, NODROP, SUBMIT_STABLE
        params.sq = Offsets {
            head: 0,
            tail: 4,
            mask: 8,
            entries: 12,
            flags_or_overflow: 16,
            dropped_or_cqes: 20,
            array_or_flags: SQ_ARRAY as u32,
            ..Offsets::default()
        };
        params.cq = Offsets {
            head: CQ_HEAD as u32,
            tail: CQ_TAIL as u32,
            mask: 72,
            entries: 76,
            flags_or_overflow: 80,
            dropped_or_cqes: queues.cq_offset as u32,
            array_or_flags: CQ_FLAGS as u32,
            ..Offsets::default()
        };
        export(address, &params)?;
        ring.linux.set(queues.clone()).map_err(|_| EIO)?;
        let weak = Arc::downgrade(&ring);
        // Keep native wait handles alive without keeping the ring registry alive.
        let completion = duplicate(ring.completion_event.raw())?;
        let closed = duplicate(ring.closed_event.raw())?;
        std::thread::Builder::new()
            .name("kinakaze-uring-cq".into())
            .spawn(move || {
                signal::swap_blocked_mask(u64::MAX);
                let handles = [closed.raw(), completion.raw()];
                while unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) }
                    == WAIT_OBJECT_0 + 1
                {
                    let Some(ring) = weak.upgrade() else {
                        break;
                    };
                    let Ok(mut state) = ring.state.lock() else {
                        break;
                    };
                    if state.closing {
                        break;
                    }
                    if let Err(error) = state
                        .harvest(&ring.completion_event, true)
                        .and_then(|_| queues.publish(&mut state))
                    {
                        queues.failed.store(error, Ordering::Release);
                        queues.ready.signal();
                        state.notify();
                        // Corrupt userspace indices fail the next syscall; keep the
                        // I/O owners until close rather than freeing kernel buffers.
                        break;
                    }
                }
            })
            .map_err(|_| crate::EAGAIN)?;
        Ok(fd)
    })();
    if result.is_err() {
        let _ = crate::close(fd);
    }
    result
}

/// Return a retained native section, its maximum size, and its section offset.
pub fn mmap_section(fd: i32, offset: u64) -> Result<(usize, usize), i32> {
    let ring = lookup(fd)?;
    let queues = ring.linux.get().ok_or(crate::ENODEV)?;
    let section = match offset {
        OFF_SQ_RING | OFF_CQ_RING => &queues.rings,
        OFF_SQES => &queues.sqes,
        _ => return Err(EINVAL),
    };
    Ok((section.duplicate()?.into_raw() as usize, section.length))
}

pub fn enter(
    fd: i32,
    to_submit: u32,
    minimum: u32,
    flags: u32,
    sigmask: usize,
    sigset_size: usize,
) -> Result<u32, i32> {
    if flags & !IORING_ENTER_GETEVENTS != 0 {
        return Err(EINVAL);
    }
    let mask = if sigmask != 0 {
        if sigset_size != 8 {
            return Err(EINVAL);
        }
        Some(import::<u64>(sigmask)?)
    } else {
        None
    };
    let previous = mask.map(signal::swap_blocked_mask);
    let result = enter_inner(fd, to_submit, minimum, flags);
    // Retire operation-local owners before a guest handler can fork or jump.
    // EINTR accepts its signal under the temporary mask, like ppoll.
    if result == Err(crate::EINTR) {
        signal::deliver_pending();
    }
    if let Some(previous) = previous {
        signal::swap_blocked_mask(previous);
    }
    if result.is_ok() {
        signal::deliver_pending();
    }
    result
}
fn enter_inner(fd: i32, to_submit: u32, minimum: u32, flags: u32) -> Result<u32, i32> {
    let ring = lookup(fd)?;
    let queues = ring.linux.get().ok_or(EINVAL)?;
    let error = queues.failed.load(Ordering::Acquire);
    if error != 0 {
        return Err(error);
    }
    if minimum > queues.cq_entries {
        return Err(EINVAL);
    }
    let mut submitted = 0;
    {
        let _submission = queues.submit.lock().map_err(|_| EIO)?;
        let mut head = queues.rings.word(0).load(Ordering::Relaxed);
        let tail = queues.rings.word(4).load(Ordering::Acquire);
        let count = tail
            .wrapping_sub(head)
            .min(queues.sq_entries)
            .min(to_submit);
        for _ in 0..count {
            let slot = queues
                .rings
                .word(SQ_ARRAY + (head & (queues.sq_entries - 1)) as usize * 4)
                .load(Ordering::Acquire);
            if slot >= queues.sq_entries {
                queues.rings.word(20).fetch_add(1, Ordering::Relaxed);
            } else {
                let mut entry: SubmissionEntry = import(queues.sqes.address(slot as usize * 64))?;
                let pinned = if entry.flags & 1 != 0 {
                    entry.flags &= !1;
                    let files = queues.files.lock().map_err(|_| EIO)?;
                    let file = files
                        .as_ref()
                        .and_then(|files| files.get(entry.fd as usize))
                        .and_then(Option::as_ref);
                    match file {
                        Some(file) => {
                            Some((Object::duplicate(file.object.raw())?, file.descriptor))
                        }
                        None => {
                            entry.fd = -1;
                            None
                        }
                    }
                } else {
                    None
                };
                if let Err(error) = unsafe { super::push_with_file(fd, &entry, pinned) } {
                    if submitted == 0 {
                        return Err(error);
                    }
                    break;
                }
                submitted += 1;
            }
            head = head.wrapping_add(1);
            queues.rings.word(0).store(head, Ordering::Release);
        }
        let mut state = ring.state.lock().map_err(|_| EIO)?;
        if state.closing {
            return Err(EBADF);
        }
        state.submit()?;
        state.harvest(&ring.completion_event, true)?;
        queues.publish(&mut state)?;
    }
    if flags & IORING_ENTER_GETEVENTS == 0 || minimum == 0 {
        return Ok(submitted);
    }
    let interrupt = interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    let notification = Arc::new(Event::new(false)?);
    {
        let mut state = ring.state.lock().map_err(|_| EIO)?;
        state.waiters.retain(|waiter| waiter.strong_count() != 0);
        state.waiters.push(Arc::downgrade(&notification));
    }
    signal::register_waiter();
    struct Waiter;
    impl Drop for Waiter {
        fn drop(&mut self) {
            signal::unregister_waiter();
        }
    }
    let _waiter = Waiter;
    loop {
        {
            let mut state = ring.state.lock().map_err(|_| EIO)?;
            if state.closing {
                return Err(EBADF);
            }
            let error = queues.failed.load(Ordering::Acquire);
            if error != 0 {
                return Err(error);
            }
            queues.publish(&mut state)?;
            if queues.count() >= minimum {
                return Ok(submitted);
            }
        }
        if signal::interrupt_pending() {
            return if submitted > 0 {
                Ok(submitted)
            } else {
                Err(crate::EINTR)
            };
        }
        let handles = [ring.closed_event.raw(), notification.raw(), interrupt];
        match unsafe { WaitForMultipleObjects(3, handles.as_ptr(), 0, INFINITE) } {
            WAIT_OBJECT_0 => return Err(EBADF),
            value if value == WAIT_OBJECT_0 + 1 => {}
            value if value == WAIT_OBJECT_0 + 2 => {
                if signal::interrupt_pending() {
                    return if submitted > 0 {
                        Ok(submitted)
                    } else {
                        Err(crate::EINTR)
                    };
                }
            }
            _ => return Err(EIO),
        }
    }
}

pub fn register(fd: i32, opcode: u32, address: usize, count: u32) -> Result<u32, i32> {
    let ring = lookup(fd)?;
    let queues = ring.linux.get().ok_or(EINVAL)?;
    match opcode {
        4 => {
            if count != 1 {
                return Err(EINVAL);
            }
            let fd = import::<i32>(address)?;
            let signal = crate::eventfd::completion_signal(fd)?;
            let mut registered = queues.eventfd.lock().map_err(|_| EIO)?;
            if registered.is_some() {
                return Err(crate::EBUSY);
            }
            *registered = Some(signal);
            return Ok(0);
        }
        5 => {
            if address != 0 || count != 0 {
                return Err(EINVAL);
            }
            return if queues.eventfd.lock().map_err(|_| EIO)?.take().is_some() {
                Ok(0)
            } else {
                Err(crate::ENXIO)
            };
        }
        2 => {
            if count == 0 || count > 32768 {
                return Err(EINVAL);
            }
            let mut files = queues.files.lock().map_err(|_| EIO)?;
            if files.is_some() {
                return Err(crate::EBUSY);
            }
            let mut registered = Vec::with_capacity(count as usize);
            for i in 0..count {
                let value =
                    import::<i32>(address.checked_add(i as usize * 4).ok_or(crate::EFAULT)?)?;
                registered.push(fixed_file(value)?);
            }
            *files = Some(registered);
            return Ok(0);
        }
        3 => {
            if address != 0 || count != 0 {
                return Err(EINVAL);
            }
            let mut files = queues.files.lock().map_err(|_| EIO)?;
            return if files.take().is_some() {
                Ok(0)
            } else {
                Err(crate::ENXIO)
            };
        }
        6 => {
            #[repr(C)]
            #[derive(Clone, Copy, Default)]
            struct Update {
                offset: u32,
                reserved: u32,
                fds: u64,
            }
            let update: Update = import(address)?;
            if update.reserved != 0 || count == 0 {
                return Err(EINVAL);
            }
            let mut guard = queues.files.lock().map_err(|_| EIO)?;
            let files = guard.as_mut().ok_or(crate::ENXIO)?;
            let end = update.offset.checked_add(count).ok_or(EINVAL)? as usize;
            if end > files.len() {
                return Err(EINVAL);
            }
            for index in 0..count {
                let result = (|| {
                    let fd = import::<i32>(
                        (update.fds as usize)
                            .checked_add(index as usize * 4)
                            .ok_or(crate::EFAULT)?,
                    )?;
                    if fd != -2 {
                        files[update.offset as usize + index as usize] = fixed_file(fd)?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    return if index != 0 { Ok(index) } else { Err(error) };
                }
            }
            return Ok(count);
        }
        _ => {}
    }
    if opcode == 8 {
        // IORING_REGISTER_PROBE
        if count > 256 {
            return Err(EINVAL);
        }
        let mut bytes = vec![0u8; 16 + count as usize * 8];
        bytes[0] = IORING_OP_WRITE;
        bytes[1] = count.min(IORING_OP_WRITE as u32 + 1) as u8;
        for op in 0..u32::from(bytes[1]) {
            let offset = 16 + op as usize * 8;
            bytes[offset] = op as u8;
            if opcode_supported(fd, op as u8)? {
                bytes[offset + 2] = 1;
            }
        }
        if memory::copy(bytes.as_ptr() as usize, address, bytes.len())? != bytes.len() {
            return Err(crate::EFAULT);
        }
        return Ok(0);
    }
    // No claim of registration success without owning the corresponding pins.
    Err(if opcode == 6 {
        crate::ENXIO
    } else {
        crate::EOPNOTSUPP
    })
}

pub fn supported(fd: i32) -> bool {
    lookup(fd).is_ok_and(|ring| ring.linux.get().is_some())
}
pub fn poll(fd: i32) -> Result<(bool, bool), i32> {
    let ring = lookup(fd)?;
    let queues = ring.linux.get().ok_or(crate::EOPNOTSUPP)?;
    let mut state = ring.state.lock().map_err(|_| EIO)?;
    if state.closing {
        return Err(EBADF);
    }
    state.harvest(&ring.completion_event, true)?;
    queues.publish(&mut state)?;
    let pending = queues
        .rings
        .word(4)
        .load(Ordering::Acquire)
        .wrapping_sub(queues.rings.word(0).load(Ordering::Acquire));
    Ok((queues.count() != 0, pending < queues.sq_entries))
}
pub(crate) fn read_wait(fd: i32) -> Result<crate::epoll::native_wait::Source, i32> {
    let ring = lookup(fd)?;
    let queues = ring.linux.get().ok_or(crate::EOPNOTSUPP)?;
    unsafe { crate::epoll::native_wait::Source::duplicate(queues.ready.raw()) }
}
