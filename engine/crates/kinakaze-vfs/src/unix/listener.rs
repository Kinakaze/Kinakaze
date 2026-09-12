//! One namespace-visible accept queue per bound stream/seqpacket description.
//! The binding pipe stays stable; pending data pipes are allocated on connect.
use super::*;
use crate::fs::object::Object;
use crate::mount::shared::{self, Store};
use crate::state_codec::{Reader, bytes, word};
use std::sync::Weak;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;

mod owner;

#[cfg(test)]
pub(super) fn flush_test_resources() -> (usize, usize) {
    owner::flush()
}
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    SetEvent, WaitForSingleObject,
};

#[derive(Clone)]
struct Owner {
    pid: u32,
    created: u64,
    anchor: u64,
    inbox: u64,
}
impl Owner {
    fn open(&self) -> Result<Option<Object>, i32> {
        let raw = unsafe {
            OpenProcess(
                PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION | 0x100000,
                0,
                self.pid,
            )
        };
        if raw.is_null() {
            return if unsafe { GetLastError() } == 87 {
                Ok(None)
            } else {
                Err(EIO)
            };
        }
        let process = Object::owned(raw)?;
        if ancillary::creation_time(raw)? != self.created
            || unsafe { WaitForSingleObject(raw, 0) } == WAIT_OBJECT_0
        {
            Ok(None)
        } else {
            Ok(Some(process))
        }
    }
}
struct Pending {
    token: u64,
    ids: [u64; 2],
    peer: UnixAddress,
    // Server endpoint and two ancillary section references, indexed by owner.
    handles: Vec<[u64; 3]>,
}
struct Queue {
    kind: i32,
    listening: bool,
    backlog: u32,
    server_credentials: credentials::Sender,
    owners: Vec<Owner>,
    pending: Vec<Pending>,
}
impl Queue {
    fn decode(input: &[u8]) -> Result<Self, i32> {
        let mut r = Reader(input);
        if r.word()? != 3 {
            return Err(EIO);
        }
        let kind = r.word()? as i32;
        let listening = r.word()? != 0;
        let backlog = r.word()? as u32;
        let server_credentials = credentials::Sender::read(&mut r)?;
        let count = r.word()? as usize;
        if count > r.0.len() / 32 {
            return Err(EIO);
        }
        let mut owners = Vec::with_capacity(count);
        for _ in 0..count {
            owners.push(Owner {
                pid: r.word()? as u32,
                created: r.word()?,
                anchor: r.word()?,
                inbox: r.word()?,
            });
        }
        let count = r.word()? as usize;
        if count > r.0.len() / 32 {
            return Err(EIO);
        }
        let mut pending = Vec::with_capacity(count);
        for _ in 0..count {
            let token = r.word()?;
            let ids = [r.word()?, r.word()?];
            let namespace = match r.word()? {
                1 => Namespace::Pathname,
                2 => Namespace::Abstract,
                3 => Namespace::Unnamed,
                _ => return Err(EIO),
            };
            let name = r.bytes()?.to_vec();
            if name.len() > SUN_PATH_MAX {
                return Err(EIO);
            }
            let mut handles = Vec::with_capacity(owners.len());
            for _ in &owners {
                handles.push([r.word()?, r.word()?, r.word()?]);
            }
            pending.push(Pending {
                token,
                ids,
                peer: UnixAddress { namespace, name },
                handles,
            });
        }
        r.end()?;
        Ok(Self {
            kind,
            listening,
            backlog,
            server_credentials,
            owners,
            pending,
        })
    }
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for value in [
            3,
            self.kind as u64,
            u64::from(self.listening),
            self.backlog as u64,
        ] {
            word(&mut out, value);
        }
        self.server_credentials.write(&mut out);
        word(&mut out, self.owners.len() as u64);
        for owner in &self.owners {
            for value in [owner.pid as u64, owner.created, owner.anchor, owner.inbox] {
                word(&mut out, value);
            }
        }
        word(&mut out, self.pending.len() as u64);
        for entry in &self.pending {
            for value in [
                entry.token,
                entry.ids[0],
                entry.ids[1],
                namespace_code(&entry.peer.namespace) as u64,
            ] {
                word(&mut out, value);
            }
            bytes(&mut out, &entry.peer.name);
            for handles in &entry.handles {
                for &value in handles {
                    word(&mut out, value);
                }
            }
        }
        out
    }
    fn live(&mut self) -> Result<Vec<Object>, i32> {
        let mut processes = Vec::new();
        let mut index = 0;
        while index < self.owners.len() {
            if let Some(process) = self.owners[index].open()? {
                processes.push(process);
                index += 1;
            } else {
                self.owners.remove(index);
                for entry in &mut self.pending {
                    entry.handles.remove(index);
                }
            }
        }
        if self.owners.is_empty() {
            self.pending.clear();
        }
        Ok(processes)
    }
}

pub(super) struct Pool {
    store: Store,
    ready: Object,
    space: Object,
}
pub(super) struct Lease {
    pub(super) pool: Arc<Pool>,
    pub(super) pin: Arc<Object>,
    pid: u32,
    created: u64,
}
static LOCAL: Mutex<Option<HashMap<u64, Weak<Lease>>>> = Mutex::new(None);
static DIRECTORY: Mutex<Option<Arc<Store>>> = Mutex::new(None);
struct Handoff {
    leases: Vec<Arc<Lease>>,
    token: u64,
    event: Option<Object>,
}
thread_local! {
    static HANDOFF: std::cell::RefCell<Option<Handoff>> = const { std::cell::RefCell::new(None) };
}
fn handoff_event(token: u64) -> Result<Object, i32> {
    Object::owned(unsafe {
        CreateEventW(
            std::ptr::null(),
            1,
            0,
            wide(&format!("Local\\kinakaze-listener-handoff-{token:x}")).as_ptr(),
        )
    })
}
pub(super) fn prepare_handoff() -> Result<(), i32> {
    HANDOFF.with(|slot| {
        let mut handoff = slot.borrow_mut();
        if handoff.is_some() {
            return Err(crate::EBUSY);
        }
        *handoff = Some(Handoff {
            leases: Vec::new(),
            token: 0,
            event: None,
        });
        Ok(())
    })
}
pub(super) fn handoff_token(leases: Vec<Arc<Lease>>) -> Result<u64, i32> {
    HANDOFF.with(|slot| {
        let mut handoff = slot.borrow_mut();
        let Some(handoff) = handoff.as_mut() else {
            return Ok(0);
        };
        for lease in leases {
            if !handoff
                .leases
                .iter()
                .any(|retained| retained.id() == lease.id())
            {
                handoff.leases.push(lease);
            }
        }
        if handoff.token == 0 && !handoff.leases.is_empty() {
            handoff.token = ancillary::random_id()?;
            handoff.event = Some(handoff_event(handoff.token)?);
        }
        Ok(handoff.token)
    })
}
pub(super) fn acknowledge_handoff(token: u64) -> Result<(), i32> {
    if token != 0 {
        let event = handoff_event(token)?;
        if unsafe { SetEvent(event.raw()) } == 0 {
            return Err(EIO);
        }
    }
    Ok(())
}
pub(super) fn finish_handoff(pid: i32) {
    let handoff = HANDOFF.with(|slot| slot.borrow_mut().take());
    if let Some(handoff) = handoff {
        if pid > 0 {
            if let Some(event) = &handoff.event {
                if let Ok(child) = Object::owned(unsafe { OpenProcess(0x100000, 0, pid as u32) }) {
                    let handles = [event.raw(), child.raw()];
                    unsafe {
                        WaitForMultipleObjects(2, handles.as_ptr(), 0, 10_000);
                    }
                }
            }
        }
        // Child restore has adopted the listener rows, or this handoff failed.
        // Parent copies outlive the complete snapshot-to-adoption interval.
        drop(handoff);
    }
}
fn directory() -> Result<Arc<Store>, i32> {
    let mut slot = DIRECTORY.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        *slot = Some(Arc::new(Store::user_object(u64::MAX - 42, true)?));
    }
    Ok(slot.as_ref().unwrap().clone())
}
fn names(input: &[u8]) -> Result<Vec<(String, u64)>, i32> {
    let mut r = Reader(input);
    let mut entries = Vec::new();
    while !r.0.is_empty() {
        entries.push((r.text()?, r.word()?));
    }
    Ok(entries)
}
fn register(name: &str, id: u64) -> Result<(), i32> {
    directory()?.update(|input| {
        let mut entries = names(input)?;
        entries.retain(|(old, id)| old != name && Store::user_object(*id, false).is_ok());
        entries.push((name.to_owned(), id));
        let mut out = Vec::new();
        for (name, id) in entries {
            bytes(&mut out, name.as_bytes());
            word(&mut out, id);
        }
        Ok((out, ()))
    })
}

impl Pool {
    fn from_store(store: Store) -> Result<Arc<Self>, i32> {
        let event = |suffix| {
            Object::owned(unsafe {
                CreateEventW(
                    std::ptr::null(),
                    1,
                    0,
                    wide(&format!(
                        "Local\\kinakaze-listener-{}-{}-{suffix}",
                        kinakaze_runtime::authority::domain_id(),
                        store.id()
                    ))
                    .as_ptr(),
                )
            })
        };
        Ok(Arc::new(Self {
            ready: event("ready")?,
            space: event("space")?,
            store,
        }))
    }
    pub(super) fn lookup(name: &str) -> Result<Option<Arc<Self>>, i32> {
        let id = names(&directory()?.read()?.1)?
            .into_iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, id)| id);
        match id {
            None => Ok(None),
            Some(id) => match Store::user_object(id, false) {
                Ok(store) => Self::from_store(store).map(Some),
                Err(crate::ENOENT) => Ok(None),
                Err(error) => Err(error),
            },
        }
    }
    fn update<T>(&self, action: impl FnOnce(&mut Queue) -> Result<T, i32>) -> Result<T, i32> {
        self.store.update(|input| {
            let mut queue = Queue::decode(input)?;
            let previous_owners = queue.owners.clone();
            let result = action(&mut queue)?;
            // These condition events are changed while holding the same native
            // mutex as all admission/dequeue operations. Waiters recheck state.
            unsafe {
                if queue.pending.is_empty() {
                    ResetEvent(self.ready.raw());
                } else {
                    SetEvent(self.ready.raw());
                }
                if queue.owners.is_empty() || queue.pending.len() <= queue.backlog as usize {
                    SetEvent(self.space.raw());
                } else {
                    ResetEvent(self.space.raw());
                }
            }
            let encoded = queue.encode();
            if encoded != input {
                // Enroll reclamation before publishing. Cleanup owns only the
                // recipient's native references and waits for this mutex, even
                // if the publishing process dies before releasing it.
                let mut notified = std::collections::HashSet::new();
                for owner in previous_owners.iter().chain(&queue.owners) {
                    if notified.insert(owner.inbox) {
                        owner::notify(owner.inbox)?;
                    }
                }
            }
            Ok((encoded, result))
        })
    }
    pub(super) fn is_listening(&self) -> Result<bool, i32> {
        Ok(Queue::decode(&self.store.read()?.1)?.listening)
    }
    pub(super) fn credentials(&self) -> Result<credentials::Sender, i32> {
        Ok(Queue::decode(&self.store.read()?.1)?.server_credentials)
    }
    pub(super) fn readable(&self) -> Result<bool, i32> {
        Ok(!Queue::decode(&self.store.read()?.1)?.pending.is_empty())
    }
    pub(super) fn notification(&self) -> Result<Object, i32> {
        // Repair a speculative wake if a publisher failed or died before the
        // metadata commit. Rechecking under the queue's native mutex prevents
        // an empty listener from retaining a permanently signaled wait event.
        self.update(|_| Ok(()))?;
        Object::duplicate(self.ready.raw())
    }
    pub(super) fn listen(&self, backlog: i32) -> Result<(), i32> {
        self.update(|queue| {
            queue.server_credentials = credentials::Sender::current(true)?;
            queue.listening = true;
            queue.backlog = (backlog as u32).min(4096);
            Ok(())
        })
    }
    fn wait(&self, event: HANDLE, process: Option<&Object>) -> Result<(), i32> {
        let interrupt = interrupt::current();
        if interrupt.is_null() {
            return Err(EIO);
        }
        signal::register_waiter();
        if signal::deliver_pending() == signal::Delivery::Interrupted {
            signal::unregister_waiter();
            return Err(EINTR);
        }
        let mut handles = vec![event, interrupt];
        if let Some(process) = process {
            handles.push(process.raw());
        }
        let ready =
            unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, u32::MAX) };
        signal::unregister_waiter();
        if ready == WAIT_OBJECT_0 || (process.is_some() && ready == WAIT_OBJECT_0 + 2) {
            Ok(())
        } else if ready == WAIT_OBJECT_0 + 1 {
            Err(EINTR)
        } else {
            Err(EIO)
        }
    }
    pub(super) fn connect(
        &self,
        kind: i32,
        peer: &UnixAddress,
        nonblocking: bool,
    ) -> Result<(Object, Arc<ancillary::State>), i32> {
        loop {
            let mut wait_process = None;
            let result = self.update(|queue| {
                let processes = queue.live()?;
                if !queue.listening || processes.is_empty() {
                    return Err(ECONNREFUSED);
                }
                if queue.kind != kind {
                    return Err(EPROTOTYPE);
                }
                if queue.pending.len() > queue.backlog as usize {
                    wait_process = processes.into_iter().next();
                    return Ok(None);
                }
                let name = format!(
                    "{PIPE_PREFIX}{:016x}-pending-{:016x}",
                    kinakaze_runtime::authority::domain_id(),
                    ancillary::random_id()?
                );
                let (raw, state) = create_instance(&name, kind != SOCK_STREAM, true)?;
                let server = Object::owned(raw)?;
                let client = Object::owned(unsafe {
                    CreateFileW(
                        wide(&name).as_ptr(),
                        GENERIC_READ | GENERIC_WRITE,
                        0,
                        std::ptr::null(),
                        OPEN_EXISTING,
                        FILE_FLAG_OVERLAPPED,
                        std::ptr::null_mut(),
                    )
                })?;
                if kind != SOCK_STREAM {
                    let mode = PIPE_READMODE_MESSAGE;
                    if unsafe {
                        SetNamedPipeHandleState(
                            client.raw(),
                            &mode,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                        )
                    } == 0
                    {
                        return Err(EIO);
                    }
                }
                ancillary::set_peer_credentials(
                    server.raw(),
                    &queue.server_credentials,
                    &credentials::Sender::current(true)?,
                )?;
                // Pending Linux stream sockets collect credentials before
                // accept attaches the listener's current socket options.
                state.set_passcred(true, true)?;
                let inherited = state.inherited();
                let refs = [server.raw() as u64, inherited[1], inherited[3]];
                let source = Owner {
                    pid: std::process::id(),
                    created: ancillary::creation_time(unsafe { GetCurrentProcess() })?,
                    anchor: 0,
                    inbox: 0,
                };
                let token = ancillary::random_id()?;
                let mut handles = Vec::new();
                for (owner, process) in queue.owners.iter().zip(&processes) {
                    handles.push(owner::adopt(
                        owner,
                        process,
                        self.store.id(),
                        token,
                        &source,
                        refs,
                    )?);
                }
                #[cfg(test)]
                if std::env::var_os("KINAKAZE_LISTENER_DIE_BEFORE_PUBLISH").is_some() {
                    std::process::exit(77);
                }
                queue.pending.push(Pending {
                    token,
                    ids: [inherited[0], inherited[2]],
                    peer: peer.clone(),
                    handles,
                });
                Ok(Some((client, state)))
            })?;
            if let Some(result) = result {
                return Ok(result);
            }
            if nonblocking {
                return Err(EAGAIN);
            }
            self.wait(self.space.raw(), wait_process.as_ref())?;
        }
    }
}
impl Lease {
    pub(super) fn id(&self) -> u64 {
        self.pool.store.id()
    }
    pub(super) fn create(name: &str, kind: i32, anchor: HANDLE) -> Result<Arc<Self>, i32> {
        let pool = Pool::from_store(shared::new_object()?)?;
        let owned = Object::duplicate(anchor)?;
        let created = ancillary::creation_time(unsafe { GetCurrentProcess() })?;
        let owner = Owner {
            pid: std::process::id(),
            created,
            anchor: owned.raw() as u64,
            inbox: owner::current()?,
        };
        let queue = Queue {
            kind,
            listening: false,
            backlog: 0,
            server_credentials: credentials::Sender::current(true)?,
            owners: vec![owner],
            pending: Vec::new(),
        };
        pool.store.update(|_| Ok((queue.encode(), ())))?;
        let lease = Self::local(pool, created)?;
        owned.into_raw();
        register(name, lease.id())?;
        Ok(lease)
    }
    fn local(pool: Arc<Pool>, created: u64) -> Result<Arc<Self>, i32> {
        let pin = Arc::new(pool.store.pin()?);
        crate::platform::try_set_inheritable(pin.raw() as usize, true)?;
        let lease = Arc::new(Self {
            pool,
            pin,
            pid: std::process::id(),
            created,
        });
        LOCAL
            .lock()
            .map_err(|_| EIO)?
            .get_or_insert_with(HashMap::new)
            .insert(lease.id(), Arc::downgrade(&lease));
        Ok(lease)
    }
    pub(super) fn restore(id: u64) -> Result<Arc<Self>, i32> {
        // The well-known name directory must outlive the original binder too.
        // The parent's handoff lease pins it until this process has opened it.
        directory()?;
        let mut local = LOCAL.lock().map_err(|_| EIO)?;
        if let Some(lease) = local
            .as_ref()
            .and_then(|map| map.get(&id))
            .and_then(Weak::upgrade)
        {
            if lease.pid == std::process::id() {
                return Ok(lease);
            }
        }
        let pool = Pool::from_store(Store::user_object(id, false)?)?;
        let created = ancillary::creation_time(unsafe { GetCurrentProcess() })?;
        let pin = Arc::new(pool.store.pin()?);
        crate::platform::try_set_inheritable(pin.raw() as usize, true)?;
        let mut retained = Vec::new();
        let inbox = owner::current()?;
        let local_process = Object::duplicate(unsafe { GetCurrentProcess() })?;
        pool.update(|queue| {
            let processes = queue.live()?;
            let source = processes.first().ok_or(ECONNREFUSED)?;
            let anchor = ancillary::duplicate(source.raw(), queue.owners[0].anchor, unsafe {
                GetCurrentProcess()
            })?;
            let owner = Owner {
                pid: std::process::id(),
                created,
                anchor: anchor.raw() as u64,
                inbox,
            };
            retained.push(anchor);
            for entry in &mut queue.pending {
                let copies = owner::adopt(
                    &owner,
                    &local_process,
                    id,
                    entry.token,
                    &queue.owners[0],
                    entry.handles[0],
                )?;
                entry.handles.push(copies);
            }
            queue.owners.push(owner);
            Ok(())
        })?;
        for pin in retained {
            pin.into_raw();
        }
        let lease = Arc::new(Self {
            pool,
            pin,
            pid: std::process::id(),
            created,
        });
        local
            .get_or_insert_with(HashMap::new)
            .insert(id, Arc::downgrade(&lease));
        Ok(lease)
    }
    pub(super) fn accept(
        &self,
        nonblocking: bool,
    ) -> Result<(Object, Arc<ancillary::State>, UnixAddress, bool), i32> {
        loop {
            let result = self.pool.update(|queue| {
                queue.live()?;
                let index = queue
                    .owners
                    .iter()
                    .position(|owner| owner.pid == self.pid && owner.created == self.created)
                    .ok_or(EBADF)?;
                if !queue.listening {
                    return Err(EINVAL);
                }
                let Some(first) = queue.pending.first() else {
                    return Ok(None);
                };
                // Duplicate locally before publication; failure leaves the queue
                // untouched, and all other owners retain their existing copies.
                let pipe = Object::duplicate(first.handles[index][0] as HANDLE)?;
                let state = ancillary::State::restore(first.ids)?;
                let entry = queue.pending.remove(0);
                #[cfg(test)]
                if std::env::var_os("KINAKAZE_LISTENER_DIE_BEFORE_DEQUEUE_PUBLISH").is_some() {
                    std::process::exit(79);
                }
                Ok(Some((pipe, state, entry.peer, queue.pending.is_empty())))
            })?;
            #[cfg(test)]
            if result.is_some() && std::env::var_os("KINAKAZE_LISTENER_DIE_AFTER_DEQUEUE").is_some()
            {
                std::process::exit(78);
            }
            if let Some(result) = result {
                return Ok(result);
            }
            if nonblocking {
                return Err(EAGAIN);
            }
            self.pool.wait(self.pool.ready.raw(), None)?;
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        // Only this process's managed queue references belong to this lease.
        if self.pid != std::process::id() {
            return;
        }
        let retired = self.pool.update(|queue| {
            let Some(index) = queue
                .owners
                .iter()
                .position(|owner| owner.pid == self.pid && owner.created == self.created)
            else {
                return Ok(Vec::new());
            };
            let handles = vec![queue.owners.remove(index).anchor];
            for pending in &mut queue.pending {
                pending.handles.remove(index);
            }
            if queue.owners.is_empty() {
                queue.pending.clear();
            }
            Ok(handles)
        });
        if let Ok(handles) = retired {
            for raw in handles {
                unsafe {
                    CloseHandle(raw as HANDLE);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_handoffs_do_not_replace_or_reject_each_other() {
        prepare_handoff().unwrap();
        let second = std::thread::spawn(|| {
            let result = prepare_handoff();
            if result.is_ok() {
                finish_handoff(0);
            }
            result
        });
        let result = second.join().unwrap();
        // This thread's transaction still exists until its own completion.
        assert_eq!(prepare_handoff(), Err(crate::EBUSY));
        finish_handoff(0);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn enrollment_repairs_a_wake_without_a_published_connection() {
        let pool = Pool::from_store(shared::new_object().unwrap()).unwrap();
        let queue = Queue {
            kind: SOCK_STREAM,
            listening: true,
            backlog: 1,
            server_credentials: credentials::Sender::current(true).unwrap(),
            owners: Vec::new(),
            pending: Vec::new(),
        };
        pool.store.update(|_| Ok((queue.encode(), ()))).unwrap();
        unsafe {
            SetEvent(pool.ready.raw());
        }
        let event = pool.notification().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(event.raw(), 0) },
            windows_sys::Win32::Foundation::WAIT_TIMEOUT
        );
    }
}
