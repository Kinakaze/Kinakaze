//! Each loader adopts and retires only its own queue resources. Admission has
//! a dedicated event-driven dispatcher; cleanup can wait for a queue publisher
//! without preventing that publisher from finishing an admission request.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::System::Threading::{
    CallbackMayRunLong, CloseThreadpoolWork, CreateThreadpoolWork, PTP_CALLBACK_INSTANCE, PTP_WORK,
    SubmitThreadpoolWork, WaitForThreadpoolWorkCallbacks,
};

#[derive(Clone)]
struct Request {
    request: u64,
    pool: u64,
    token: u64,
    source: Owner,
    handles: [u64; 3],
    result: Option<Result<[u64; 3], i32>>,
}
fn decode(input: &[u8]) -> Result<Vec<Request>, i32> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = Reader(input);
    if r.word()? != 1 {
        return Err(EIO);
    }
    let mut rows = Vec::new();
    while !r.0.is_empty() {
        let request = r.word()?;
        let pool = r.word()?;
        let token = r.word()?;
        let source = Owner {
            pid: r.word()? as u32,
            created: r.word()?,
            anchor: 0,
            inbox: 0,
        };
        let handles = [r.word()?, r.word()?, r.word()?];
        let result = match r.word()? {
            0 => None,
            1 => Some(Ok([r.word()?, r.word()?, r.word()?])),
            2 => Some(Err(r.word()? as i32)),
            _ => return Err(EIO),
        };
        rows.push(Request {
            request,
            pool,
            token,
            source,
            handles,
            result,
        });
    }
    Ok(rows)
}
fn encode(rows: &[Request]) -> Vec<u8> {
    let mut bytes = Vec::new();
    word(&mut bytes, 1);
    for row in rows {
        for value in [
            row.request,
            row.pool,
            row.token,
            row.source.pid as u64,
            row.source.created,
        ] {
            word(&mut bytes, value);
        }
        for raw in row.handles {
            word(&mut bytes, raw);
        }
        match row.result {
            None => word(&mut bytes, 0),
            Some(Ok(handles)) => {
                word(&mut bytes, 1);
                for raw in handles {
                    word(&mut bytes, raw);
                }
            }
            Some(Err(error)) => {
                word(&mut bytes, 2);
                word(&mut bytes, error as u64);
            }
        }
    }
    bytes
}
fn event(kind: &str, id: u64, manual: bool) -> Result<Object, i32> {
    Object::owned(unsafe {
        CreateEventW(
            std::ptr::null(),
            i32::from(manual),
            0,
            wide(&format!(
                "Local\\kinakaze-listener-{kind}-{}-{id:x}",
                kinakaze_runtime::authority::domain_id()
            ))
            .as_ptr(),
        )
    })
}
pub(super) fn notify(id: u64) -> Result<(), i32> {
    let wake = event("owner", id, false)?;
    if unsafe { SetEvent(wake.raw()) } == 0 {
        Err(EIO)
    } else {
        Ok(())
    }
}

#[cfg(test)]
struct ResourceBarrier {
    event: Object,
    result: Mutex<Option<(usize, usize)>>,
}
struct Endpoint {
    pid: u32,
    store: Arc<Store>,
    wake: Object,
    completed: Object,
    #[cfg(test)]
    barriers: Mutex<Vec<Arc<ResourceBarrier>>>,
}
static CURRENT: Mutex<Option<Arc<Endpoint>>> = Mutex::new(None);
pub(super) fn current() -> Result<u64, i32> {
    let mut current = CURRENT.lock().map_err(|_| EIO)?;
    if let Some(endpoint) = current
        .as_ref()
        .filter(|endpoint| endpoint.pid == std::process::id())
    {
        return Ok(endpoint.store.id());
    }
    let store = Arc::new(shared::new_object()?);
    let endpoint = Arc::new(Endpoint {
        pid: std::process::id(),
        wake: event("owner", store.id(), false)?,
        completed: Object::owned(unsafe {
            CreateEventW(std::ptr::null(), 0, 0, std::ptr::null())
        })?,
        store,
        #[cfg(test)]
        barriers: Mutex::new(Vec::new()),
    });
    let worker = endpoint.clone();
    std::thread::Builder::new()
        .name("unix-resource-owner".into())
        .spawn(move || run(worker))
        .map_err(|_| EIO)?;
    let id = endpoint.store.id();
    *current = Some(endpoint);
    Ok(id)
}

pub(super) fn adopt(
    owner: &Owner,
    process: &Object,
    pool: u64,
    token: u64,
    source: &Owner,
    handles: [u64; 3],
) -> Result<[u64; 3], i32> {
    let inbox = Store::user_object(owner.inbox, false)?;
    let request = ancillary::random_id()?;
    let reply = event("adopt", request, true)?;
    inbox.update(|bytes| {
        let mut rows = decode(bytes)?;
        rows.push(Request {
            request,
            pool,
            token,
            source: source.clone(),
            handles,
            result: None,
        });
        Ok((encode(&rows), ()))
    })?;
    notify(owner.inbox)?;
    // The caller owns the queue mutex through this operation. Owner-side
    // cleanup waits for that mutex, while the separate dispatcher stays live.
    let waits = [reply.raw(), process.raw()];
    if unsafe { WaitForMultipleObjects(2, waits.as_ptr(), 0, u32::MAX) } != WAIT_OBJECT_0 {
        return Err(EIO);
    }
    inbox.update(|bytes| {
        let mut rows = decode(bytes)?;
        let index = rows
            .iter()
            .position(|row| row.request == request)
            .ok_or(EIO)?;
        let result = rows.remove(index).result.ok_or(EIO)?;
        Ok((encode(&rows), result))
    })?
}

struct Resources {
    endpoint: Arc<Endpoint>,
    pool: Arc<Store>,
    entries: Mutex<HashMap<u64, [Object; 3]>>,
    running: AtomicBool,
    dirty: AtomicBool,
}
struct Cleanup {
    data: Arc<Resources>,
    // The dedicated dispatcher keeps this context alive until callbacks retire.
    context: Box<Arc<Resources>>,
    work: PTP_WORK,
}
impl Cleanup {
    fn new(endpoint: Arc<Endpoint>, pool: u64) -> Result<Self, i32> {
        let data = Arc::new(Resources {
            endpoint,
            pool: Arc::new(Store::user_object(pool, false)?),
            entries: Mutex::new(HashMap::new()),
            running: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
        });
        let mut context = Box::new(data.clone());
        let work = unsafe {
            CreateThreadpoolWork(Some(cleanup), (&raw mut *context).cast(), std::ptr::null())
        };
        if work == 0 {
            return Err(EIO);
        }
        Ok(Self {
            data,
            context,
            work,
        })
    }
    fn schedule(&self) {
        self.data.dirty.store(true, Ordering::Release);
        if !self.data.running.swap(true, Ordering::AcqRel) {
            self.data.dirty.store(false, Ordering::Release);
            unsafe { SubmitThreadpoolWork(self.work) };
        }
    }
    fn finish(&self) {
        unsafe { WaitForThreadpoolWorkCallbacks(self.work, 0) };
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        self.finish();
        unsafe { CloseThreadpoolWork(self.work) };
        // context and data are dropped only after callbacks can no longer read.
        let _ = &self.context;
    }
}
fn retire(data: &Resources, bytes: &[u8]) -> Result<(), i32> {
    let queue = Queue::decode(bytes)?;
    let index = queue.owners.iter().position(|owner| {
        owner.pid == data.endpoint.pid && owner.inbox == data.endpoint.store.id()
    });
    let mut entries = data.entries.lock().map_err(|_| EIO)?;
    entries.retain(|token, objects| {
        index.is_some_and(|index| {
            queue.pending.iter().any(|entry| {
                entry.token == *token
                    && entry.handles[index] == objects.each_ref().map(|object| object.raw() as u64)
            })
        })
    });
    // A request's caller holds this queue mutex until it consumes its reply.
    // Once we can acquire it, leftover replies belong to abandoned operations.
    data.endpoint.store.update(|bytes| {
        let mut rows = decode(bytes)?;
        rows.retain(|row| row.pool != data.pool.id());
        Ok((encode(&rows), ()))
    })?;
    Ok(())
}
unsafe extern "system" fn cleanup(
    instance: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    _: PTP_WORK,
) {
    let data = unsafe { &*context.cast::<Arc<Resources>>() };
    let result = data
        .pool
        .inspect_exclusive(true, |bytes| retire(data, bytes));
    if matches!(result, Ok(None)) {
        // Native threadpool cleanup may wait on an active/abandoned publisher;
        // it must not consume the dedicated admission dispatcher's thread.
        unsafe { CallbackMayRunLong(instance) };
        let _ = data
            .pool
            .inspect_exclusive(false, |bytes| retire(data, bytes));
    }
    data.running.store(false, Ordering::Release);
    unsafe { SetEvent(data.endpoint.completed.raw()) };
}
fn run(endpoint: Arc<Endpoint>) {
    let mut pools: HashMap<u64, Cleanup> = HashMap::new();
    let waits = [endpoint.wake.raw(), endpoint.completed.raw()];
    loop {
        let wake = unsafe { WaitForMultipleObjects(2, waits.as_ptr(), 0, u32::MAX) };
        if wake == WAIT_OBJECT_0 {
            let requests = endpoint.store.read().and_then(|(_, bytes)| decode(&bytes));
            if let Ok(requests) = requests {
                for request in requests.into_iter().filter(|row| row.result.is_none()) {
                    let result = (|| {
                        if !pools.contains_key(&request.pool) {
                            pools.insert(
                                request.pool,
                                Cleanup::new(endpoint.clone(), request.pool)?,
                            );
                        }
                        let job = pools.get(&request.pool).ok_or(EIO)?;
                        let mut entries = job.data.entries.lock().map_err(|_| EIO)?;
                        if let Some(objects) = entries.get(&request.token) {
                            return Ok(objects.each_ref().map(|object| object.raw() as u64));
                        }
                        let source = request.source.open()?.ok_or(ECONNREFUSED)?;
                        let mut objects = Vec::new();
                        for handle in request.handles {
                            objects.push(ancillary::duplicate(source.raw(), handle, unsafe {
                                GetCurrentProcess()
                            })?);
                        }
                        let objects: [Object; 3] = objects.try_into().map_err(|_| EIO)?;
                        let handles = objects.each_ref().map(|object| object.raw() as u64);
                        entries.insert(request.token, objects);
                        Ok(handles)
                    })();
                    let published = endpoint.store.update(|bytes| {
                        let mut rows = decode(bytes)?;
                        let Some(row) = rows.iter_mut().find(|row| row.request == request.request)
                        else {
                            return Ok((bytes.to_vec(), false));
                        };
                        row.result = Some(result);
                        Ok((encode(&rows), true))
                    });
                    if matches!(published, Ok(true)) {
                        if let Ok(reply) = event("adopt", request.request, true) {
                            unsafe { SetEvent(reply.raw()) };
                        }
                    }
                }
            }
            for job in pools.values() {
                job.schedule();
            }
        } else if wake == WAIT_OBJECT_0 + 1 {
            for job in pools.values() {
                if job.data.dirty.load(Ordering::Acquire)
                    && !job.data.running.load(Ordering::Acquire)
                {
                    job.schedule();
                }
            }
        } else {
            break;
        }
        pools.retain(|_, job| {
            if job.data.running.load(Ordering::Acquire) || job.data.dirty.load(Ordering::Acquire) {
                return true;
            }
            job.finish();
            !job.data
                .entries
                .lock()
                .map(|entries| entries.is_empty())
                .unwrap_or(false)
        });
        #[cfg(test)]
        {
            let barriers = std::mem::take(&mut *endpoint.barriers.lock().unwrap());
            if !barriers.is_empty() {
                for job in pools.values() {
                    job.schedule();
                    job.finish();
                }
                pools.retain(|_, job| !job.data.entries.lock().unwrap().is_empty());
                let handles = pools
                    .values()
                    .map(|job| job.data.entries.lock().unwrap().len() * 3)
                    .sum();
                let retained = (pools.len(), handles);
                for barrier in barriers {
                    *barrier.result.lock().unwrap() = Some(retained);
                    unsafe { SetEvent(barrier.event.raw()) };
                }
            }
        }
    }
}

#[cfg(test)]
pub(super) fn flush() -> (usize, usize) {
    current().unwrap();
    let endpoint = CURRENT.lock().unwrap().as_ref().unwrap().clone();
    let barrier = Arc::new(ResourceBarrier {
        event: event("test-barrier", ancillary::random_id().unwrap(), true).unwrap(),
        result: Mutex::new(None),
    });
    endpoint.barriers.lock().unwrap().push(barrier.clone());
    unsafe { SetEvent(endpoint.wake.raw()) };
    assert_eq!(
        unsafe { WaitForSingleObject(barrier.event.raw(), 5000) },
        WAIT_OBJECT_0
    );
    // The result describes only listener resources. The process-wide handle
    // count includes Windows threadpool and signal-observer handles whose
    // asynchronous lifetime is unrelated to an abandoned queue publication.
    let result = barrier.result.lock().unwrap().take().unwrap();
    result
}
