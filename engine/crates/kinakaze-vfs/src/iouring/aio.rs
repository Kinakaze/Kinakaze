//! Linux legacy AIO syscalls over native Windows IoRing. The context is local
//! to this host process (neither fork nor exec inherits a native queue).
use super::*;
use std::time::Duration;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IoCb {
    data: u64,
    key: u32,
    rw_flags: u32,
    opcode: u16,
    priority: i16,
    fd: u32,
    buffer: u64,
    length: u64,
    offset: i64,
    reserved: u64,
    flags: u32,
    resfd: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct IoEvent {
    data: u64,
    object: u64,
    result: i64,
    result2: i64,
}
struct Pending {
    data: u64,
    object: usize,
    signal: Option<crate::eventfd::CompletionSignal>,
}
struct Queue {
    native: State,
    pending: HashMap<u64, Pending>,
    completed: VecDeque<IoEvent>,
    serial: u64,
    limit: usize,
    closing: bool,
    failure: Option<i32>,
}
struct Context {
    // A readable zero-magic header makes libaio use the syscall path. Never
    // expose the Rust queue to guest writes or advertise a userspace CQ ring.
    header: usize,
    native_event: Event,
    changed: Event,
    stopped: Event,
    queue: Mutex<Queue>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Drop for Context {
    fn drop(&mut self) {
        self.queue
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .native
            .shutdown(&self.native_event);
        unsafe { VirtualFree(self.header as *mut _, 0, MEM_RELEASE) };
    }
}
static CONTEXTS: OnceLock<Mutex<HashMap<usize, Arc<Context>>>> = OnceLock::new();
fn contexts() -> &'static Mutex<HashMap<usize, Arc<Context>>> {
    CONTEXTS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn lookup(id: usize) -> Result<Arc<Context>, i32> {
    contexts()
        .lock()
        .map_err(|_| EIO)?
        .get(&id)
        .cloned()
        .ok_or(EINVAL)
}
fn copy_in<T: Copy + Default>(address: usize) -> Result<T, i32> {
    let mut value = T::default();
    if memory::copy(
        address,
        &mut value as *mut T as usize,
        std::mem::size_of::<T>(),
    )? != std::mem::size_of::<T>()
    {
        return Err(crate::EFAULT);
    }
    Ok(value)
}
fn copy_out<T>(address: usize, value: &T) -> Result<(), i32> {
    if memory::copy(
        value as *const T as usize,
        address,
        std::mem::size_of::<T>(),
    )? != std::mem::size_of::<T>()
    {
        return Err(crate::EFAULT);
    }
    Ok(())
}
fn trace(message: std::fmt::Arguments<'_>) {
    if std::env::var_os("KINAKAZE_AIO_TRACE").is_some() {
        eprintln!("kinakaze aio: {message}");
    }
}

pub fn setup(entries: u32, output: usize) -> Result<i64, i32> {
    if copy_in::<usize>(output)? != 0 || entries == 0 {
        return Err(EINVAL);
    }
    if entries > 32768 {
        return Err(EAGAIN);
    }
    let capacity = entries.next_power_of_two();
    let native_event = Event::new(true)?;
    let changed = Event::new(true)?;
    let stopped = Event::new(true)?;
    let handle = create_best_ring(capacity, capacity * 2).ok_or(ENOSYS)?;
    if let Err(error) = hresult(unsafe { SetIoRingCompletionEvent(handle, native_event.raw()) }) {
        unsafe { CloseIoRing(handle) };
        return Err(error);
    }
    let header = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            4096,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if header.is_null() {
        unsafe { CloseIoRing(handle) };
        return Err(crate::ENOMEM);
    }
    let context = Arc::new(Context {
        header: header as usize,
        native_event,
        changed,
        stopped,
        queue: Mutex::new(Queue {
            native: State {
                handle: handle as usize,
                closing: false,
                pending: 0,
                capacity,
                requests: HashMap::new(),
                next_cookie: 1,
                local_pending: VecDeque::new(),
                completions: VecDeque::new(),
                waiters: Vec::new(),
            },
            pending: HashMap::new(),
            completed: VecDeque::new(),
            serial: 1,
            limit: entries as usize,
            closing: false,
            failure: None,
        }),
        worker: Mutex::new(None),
    });
    // Reserve registry storage before starting an owner thread.
    let mut registry = contexts().lock().map_err(|_| EIO)?;
    registry.try_reserve(1).map_err(|_| crate::ENOMEM)?;
    let owner = context.clone();
    let worker = std::thread::Builder::new()
        .name("linux-aio".into())
        .spawn(move || owner.run())
        .map_err(|_| EAGAIN)?;
    *context.worker.lock().map_err(|_| EIO)? = Some(worker);
    registry.insert(context.header, context.clone());
    drop(registry);
    if let Err(error) = copy_out(output, &context.header) {
        let _ = destroy(context.header);
        return Err(error);
    }
    trace(format_args!(
        "setup context={:#x} entries={entries}",
        context.header
    ));
    Ok(0)
}
impl Context {
    fn publish(&self, queue: &mut Queue) -> Result<(), i32> {
        queue.native.harvest(&self.native_event, true)?;
        while let Some(completion) = queue.native.completions.pop_front() {
            let pending = queue.pending.remove(&completion.user_data).ok_or(EIO)?;
            queue.completed.push_back(IoEvent {
                data: pending.data,
                object: pending.object as u64,
                result: completion.result as i64,
                result2: 0,
            });
            // The complete event and read buffer are visible before eventfd.
            if let Some(signal) = pending.signal {
                signal.signal()?;
            }
            trace(format_args!(
                "complete context={:#x} result={}",
                self.header, completion.result
            ));
            self.changed.signal();
        }
        Ok(())
    }
    fn run(&self) {
        loop {
            let handles = [self.stopped.raw(), self.native_event.raw()];
            let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            if result == WAIT_OBJECT_0 {
                return;
            }
            let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if queue.closing {
                return;
            }
            if result != WAIT_OBJECT_0 + 1 || self.publish(&mut queue).is_err() {
                queue.failure = Some(EIO);
                self.changed.signal();
                return;
            }
        }
    }
    fn submit_one(&self, queue: &mut Queue, address: usize) -> Result<(), i32> {
        if queue.closing {
            return Err(EINVAL);
        }
        if let Some(error) = queue.failure {
            return Err(error);
        }
        if queue.pending.len() + queue.completed.len() >= queue.limit {
            return Err(EAGAIN);
        }
        let cb = copy_in::<IoCb>(address)?;
        if cb.reserved != 0 || cb.flags & !1 != 0 {
            return Err(EINVAL);
        }
        if cb.rw_flags != 0 {
            return Err(crate::EOPNOTSUPP);
        }
        if cb.offset < 0 || cb.length > isize::MAX as u64 {
            return Err(EINVAL);
        }
        let opcode = match cb.opcode {
            0 => IORING_OP_READ,
            1 => IORING_OP_WRITE,
            7 => IORING_OP_READV,
            8 => IORING_OP_WRITEV,
            _ => return Err(EINVAL),
        };
        if unsafe {
            IsIoRingOpSupported(
                queue.native.handle as HIORING,
                native_opcode(opcode).ok_or(EINVAL)?,
            )
        } == 0
        {
            return Err(crate::EOPNOTSUPP);
        }
        let length = if cb.opcode >= 7 {
            if cb.length > 1024 {
                return Err(EINVAL);
            }
            cb.length as u32
        } else {
            cb.length.min(0x7fff_f000) as u32
        };
        let serial = queue.serial;
        queue.serial = serial.checked_add(1).ok_or(EAGAIN)?;
        let request = prepare(&SubmissionEntry {
            opcode,
            fd: cb.fd as i32,
            offset: cb.offset as u64,
            address: cb.buffer,
            length,
            user_data: serial,
            ..Default::default()
        })?;
        let notification = if cb.flags & 1 != 0 {
            Some(crate::eventfd::completion_signal(cb.resfd as i32)?)
        } else {
            None
        };
        // Reserve all completion storage before any native SQE can own memory.
        queue.pending.try_reserve(1).map_err(|_| crate::ENOMEM)?;
        queue
            .completed
            .try_reserve(queue.pending.len() + 1)
            .map_err(|_| crate::ENOMEM)?;
        queue
            .native
            .requests
            .try_reserve(1)
            .map_err(|_| crate::ENOMEM)?;
        queue
            .native
            .local_pending
            .try_reserve(1)
            .map_err(|_| crate::ENOMEM)?;
        queue
            .native
            .completions
            .try_reserve(queue.pending.len() + 1)
            .map_err(|_| crate::ENOMEM)?;
        copy_out(address.checked_add(8).ok_or(crate::EFAULT)?, &0u32)?;
        queue.native.queue_native(request)?;
        queue.native.pending += 1;
        queue.pending.insert(
            serial,
            Pending {
                data: cb.data,
                object: address,
                signal: notification,
            },
        );
        if let Err(error) = queue.native.submit() {
            // SubmitIoRing failure retains every unsubmitted SQE. Close before
            // releasing buffers, and report accepted operations as failed CQEs.
            queue.native.shutdown(&self.native_event);
            queue.failure = Some(error);
            for (_, pending) in queue.pending.drain() {
                queue.completed.push_back(IoEvent {
                    data: pending.data,
                    object: pending.object as u64,
                    result: -(error as i64),
                    result2: 0,
                });
                if let Some(signal) = pending.signal {
                    signal.signal()?;
                }
            }
            self.changed.signal();
        } else {
            // Also wakes the consumer for zero-length, software completions.
            self.native_event.signal();
        }
        trace(format_args!(
            "submit context={:#x} opcode={} bytes={length}",
            self.header, cb.opcode
        ));
        Ok(())
    }
}
pub fn submit(id: usize, count: i64, array: usize) -> Result<i64, i32> {
    if count < 0 {
        return Err(EINVAL);
    }
    let context = lookup(id)?;
    let mut queue = context.queue.lock().map_err(|_| EIO)?;
    let mut submitted = 0;
    while submitted < count {
        let result = (|| {
            let slot = (submitted as usize)
                .checked_mul(8)
                .and_then(|n| array.checked_add(n))
                .ok_or(crate::EFAULT)?;
            context.submit_one(&mut queue, copy_in::<usize>(slot)?)
        })();
        if let Err(error) = result {
            return if submitted == 0 {
                Err(error)
            } else {
                Ok(submitted)
            };
        }
        submitted += 1;
    }
    Ok(submitted)
}
pub fn getevents(
    id: usize,
    minimum: i64,
    maximum: i64,
    output: usize,
    timeout: usize,
) -> Result<i64, i32> {
    let mut interrupted = false;
    let result = getevents_owned(id, minimum, maximum, output, timeout, &mut interrupted);
    if interrupted {
        signal::deliver_pending();
    }
    result
}
fn getevents_owned(
    id: usize,
    minimum: i64,
    maximum: i64,
    output: usize,
    timeout: usize,
    interrupted: &mut bool,
) -> Result<i64, i32> {
    if minimum < 0 || maximum < minimum {
        return Err(EINVAL);
    }
    let context = lookup(id)?;
    let duration = if timeout == 0 {
        None
    } else {
        let [seconds, nanos] = copy_in::<[i64; 2]>(timeout)?;
        if seconds < 0 || !(0..1_000_000_000).contains(&nanos) {
            return Err(EINVAL);
        }
        Some(Duration::new(seconds as u64, nanos as u32))
    };
    let started = Instant::now();
    let mut copied = 0i64;
    loop {
        let mut queue = context.queue.lock().map_err(|_| EIO)?;
        if queue.closing {
            return if copied > 0 { Ok(copied) } else { Err(EINVAL) };
        }
        while copied < maximum {
            let Some(event) = queue.completed.front() else {
                break;
            };
            let result = (|| {
                let address = (copied as usize)
                    .checked_mul(32)
                    .and_then(|n| output.checked_add(n))
                    .ok_or(crate::EFAULT)?;
                copy_out(address, event)
            })();
            if let Err(error) = result {
                return if copied > 0 { Ok(copied) } else { Err(error) };
            }
            queue.completed.pop_front();
            copied += 1;
        }
        if copied >= minimum || duration.is_some_and(|d| started.elapsed() >= d) {
            return Ok(copied);
        }
        if let Some(error) = queue.failure {
            return if copied > 0 { Ok(copied) } else { Err(error) };
        }
        unsafe { ResetEvent(context.changed.raw()) };
        drop(queue);
        let interrupt = interrupt::current();
        let mut handles = vec![context.stopped.raw(), context.changed.raw()];
        if !interrupt.is_null() {
            handles.push(interrupt);
        }
        let wait = duration
            .map(|d| {
                d.saturating_sub(started.elapsed())
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min((INFINITE - 1) as u128) as u32
            })
            .unwrap_or(INFINITE);
        signal::register_waiter();
        let pending = signal::pending() & !signal::blocked_mask() != 0;
        let result = if pending {
            WAIT_OBJECT_0 + 2
        } else {
            unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, wait) }
        };
        signal::unregister_waiter();
        if result == WAIT_OBJECT_0 + 2 {
            *interrupted = true;
            return if copied > 0 {
                Ok(copied)
            } else {
                Err(crate::EINTR)
            };
        }
        if result != WAIT_OBJECT_0 && result != WAIT_OBJECT_0 + 1 && result != WAIT_TIMEOUT {
            return Err(EIO);
        }
    }
}
pub fn destroy(id: usize) -> Result<i64, i32> {
    let context = contexts()
        .lock()
        .map_err(|_| EIO)?
        .remove(&id)
        .ok_or(EINVAL)?;
    {
        let mut queue = context.queue.lock().map_err(|_| EIO)?;
        queue.closing = true;
        context.stopped.signal();
        context.changed.signal();
        queue.native.shutdown(&context.native_event);
        queue.pending.clear();
        queue.completed.clear();
    }
    if let Some(worker) = context.worker.lock().map_err(|_| EIO)?.take() {
        worker.join().map_err(|_| EIO)?;
    }
    trace(format_args!("destroy context={id:#x}"));
    Ok(0)
}
pub fn cancel(id: usize, iocb: usize) -> Result<i64, i32> {
    let _context = lookup(id)?;
    let _cb = copy_in::<IoCb>(iocb)?;
    // Linux regular-file read/write requests do not register a ki_cancel
    // callback. io_destroy still cancels and retires all native operations.
    Err(EINVAL)
}
pub fn destroy_all() {
    let ids: Vec<_> = contexts()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .copied()
        .collect();
    for id in ids {
        let _ = destroy(id);
    }
}
