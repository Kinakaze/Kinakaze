//! Ancillary records belong to the pipe connection, independently of payload.
use super::*;
use crate::fs::object::Object;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
#[path = "socket_rpc.rs"]
mod socket_rpc;
use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, WAIT_ABANDONED};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcess, GetProcessTimes, OpenEventW, OpenProcess,
    PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION, ReleaseMutex, SetEvent,
    WaitForSingleObject,
};

struct Lock<'a>(&'a Object);
struct SignalWait;
impl Drop for SignalWait {
    fn drop(&mut self) {
        signal::unregister_waiter();
    }
}
impl<'a> Lock<'a> {
    fn new(object: &'a Object) -> Result<Self, i32> {
        Self::acquire(object, false, false)
    }
    fn acquire(object: &'a Object, nonblocking: bool, interruptible: bool) -> Result<Self, i32> {
        // A connection retains its mutexes. Avoid name lookup, handle creation
        // and interrupt TLS setup on the uncontended transfer path.
        match unsafe { WaitForSingleObject(object.raw(), 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(Self(object)),
            windows_sys::Win32::Foundation::WAIT_TIMEOUT if nonblocking => return Err(EAGAIN),
            windows_sys::Win32::Foundation::WAIT_TIMEOUT => {}
            _ => return Err(EIO),
        }
        let interrupt = if interruptible {
            interrupt::current()
        } else {
            std::ptr::null_mut()
        };
        if interruptible && interrupt.is_null() {
            return Err(EIO);
        }
        let _waiter = interruptible.then(|| {
            signal::register_waiter();
            SignalWait
        });
        let handles = [object.raw(), interrupt];
        loop {
            // Check after enrollment so a process signal queued before the
            // event was registered cannot strand an infinite mutex wait.
            if interruptible && signal::deliver_pending() == signal::Delivery::Interrupted {
                return Err(EINTR);
            }
            let ready = unsafe {
                WaitForMultipleObjects(
                    if interruptible { 2 } else { 1 },
                    handles.as_ptr(),
                    0,
                    u32::MAX,
                )
            };
            match ready {
                WAIT_OBJECT_0 | WAIT_ABANDONED => return Ok(Self(object)),
                value if interruptible && value == WAIT_OBJECT_0 + 1 => {
                    if signal::deliver_pending() == signal::Delivery::Interrupted {
                        return Err(EINTR);
                    }
                }
                _ => return Err(EIO),
            }
        }
    }
}
impl Drop for Lock<'_> {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.raw());
        }
    }
}

struct QueueLocks {
    tx: Object,
    rx: Object,
    queue: Object,
}
impl QueueLocks {
    fn new(id: u64, direction: usize) -> Result<Self, i32> {
        let create = |kind: &str| {
            Object::owned(unsafe {
                CreateMutexW(
                    std::ptr::null(),
                    0,
                    wide(&format!(
                        "Local\\kinakaze-unix-{}-{id:x}-{kind}-{direction}",
                        kinakaze_runtime::authority::domain_id()
                    ))
                    .as_ptr(),
                )
            })
        };
        Ok(Self {
            tx: create("tx")?,
            rx: create("rx")?,
            queue: create("queue")?,
        })
    }
}

pub(super) fn random_id() -> Result<u64, i32> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(provider: HANDLE, output: *mut u8, len: u32, flags: u32) -> i32;
    }
    let mut value = [0; 8];
    if unsafe { BCryptGenRandom(std::ptr::null_mut(), value.as_mut_ptr(), 8, 2) } < 0 {
        return Err(EIO);
    }
    Ok(u64::from_le_bytes(value).max(1))
}

pub(super) struct State {
    queues: [crate::mount::shared::Store; 2],
    pins: [Arc<Object>; 2],
    locks: [QueueLocks; 2],
    filters: [Mutex<Option<(u64, Arc<crate::ofd::Shared>)>>; 2],
}
impl State {
    pub(super) fn set_passcred(&self, server_end: bool, enabled: bool) -> Result<(), i32> {
        self.set_passcred_with(server_end, enabled, || Ok(()))
    }
    pub(super) fn set_passcred_with(
        &self,
        server_end: bool,
        enabled: bool,
        publish: impl FnOnce() -> Result<(), i32>,
    ) -> Result<(), i32> {
        let _first = Lock::new(&self.locks[0].queue)?;
        let _second = Lock::new(&self.locks[1].queue)?;
        publish()?;
        for (direction, store) in self.queues.iter().enumerate() {
            let mut queue = Queue::read(store)?;
            let option = if direction == usize::from(server_end) {
                &mut queue.writer_passcred
            } else {
                &mut queue.reader_passcred
            };
            if *option != enabled {
                *option = enabled;
                queue.write(store)?;
            }
        }
        Ok(())
    }
    fn from_queues(queues: [crate::mount::shared::Store; 2]) -> Result<Arc<Self>, i32> {
        let pins = [Arc::new(queues[0].pin()?), Arc::new(queues[1].pin()?)];
        for pin in &pins {
            crate::platform::try_set_inheritable(pin.raw() as usize, true)?;
        }
        // Restore opens process-local references by the same queue identities;
        // these cached mutex handles do not need native fork inheritance.
        let locks = [
            QueueLocks::new(queues[0].id(), 0)?,
            QueueLocks::new(queues[1].id(), 1)?,
        ];
        Ok(Arc::new(Self {
            queues,
            pins,
            locks,
            filters: [Mutex::new(None), Mutex::new(None)],
        }))
    }
    pub(super) fn pins(&self) -> &[Arc<Object>; 2] {
        &self.pins
    }
    pub(super) fn inherited(&self) -> [u64; 4] {
        [
            self.queues[0].id(),
            self.pins[0].raw() as u64,
            self.queues[1].id(),
            self.pins[1].raw() as u64,
        ]
    }
    pub(super) fn restore(ids: [u64; 2]) -> Result<Arc<Self>, i32> {
        Self::from_queues([
            crate::mount::shared::Store::user_object(ids[0], false)?,
            crate::mount::shared::Store::user_object(ids[1], false)?,
        ])
    }
    pub(super) fn create(handle: HANDLE) -> Result<Arc<Self>, i32> {
        let state = Self::from_queues([
            crate::mount::shared::new_object()?,
            crate::mount::shared::new_object()?,
        ])?;
        let mut bytes = smallvec::SmallVec::<[u8; 16]>::new();
        for s in &state.queues {
            crate::state_codec::word(&mut bytes, s.id());
        }
        let creator = credentials::Sender::current(true)?;
        set_peer_credentials(handle, &creator, &creator)?;
        attribute(handle, b"kinakaze.connection", Some(&bytes))?;
        Ok(state)
    }
    pub(super) fn open(handle: HANDLE) -> Result<Arc<Self>, i32> {
        let bytes = attribute(handle, b"kinakaze.connection", None)?;
        let mut r = crate::state_codec::Reader(&bytes);
        let state = Self::restore([r.word()?, r.word()?])?;
        r.end()?;
        Ok(state)
    }
}

#[derive(Clone)]
struct Record {
    offset: u64,
    length: u64,
    keeper: u32,
    created: u64,
    rpc: u64,
    token: u64,
    fds: Vec<Descriptor>,
}
#[derive(Clone)]
struct Descriptor {
    raw: u64,
    kind: u32,
    flags: u32,
    shared: u64,
    metadata: Vec<u8>,
}
impl Descriptor {
    fn read(r: &mut crate::state_codec::Reader) -> Result<Self, i32> {
        Ok(Self {
            raw: r.word()?,
            kind: r.word()? as u32,
            flags: r.word()? as u32,
            shared: r.word()?,
            metadata: r.bytes()?.to_vec(),
        })
    }
    fn write(&self, bytes: &mut impl Extend<u8>) {
        for w in [self.raw, self.kind as u64, self.flags as u64, self.shared] {
            crate::state_codec::word(bytes, w);
        }
        crate::state_codec::bytes(bytes, &self.metadata);
    }
}
#[derive(Default)]
struct Queue {
    sent: u64,
    received: u64,
    read_closed: bool,
    write_closed: bool,
    records: Vec<Record>,
    reader_async: AsyncOwner,
    writer_async: AsyncOwner,
    writer_blocked: bool,
    reader_filter: u64,
    reader_filter_snapshot: Vec<u8>,
    reader_passcred: bool,
    writer_passcred: bool,
    credentials: smallvec::SmallVec<[CredentialRange; 1]>,
}

struct CredentialRange {
    start: u64,
    end: u64,
    sender: credentials::Sender,
}

#[derive(Clone, Copy, Default)]
struct AsyncOwner {
    owner: i32,
    enabled: bool,
}
impl AsyncOwner {
    fn notify(self) {
        if self.enabled && self.owner != 0 {
            crate::job::notify_io_owner(self.owner);
        }
    }
}
impl Queue {
    fn read(store: &crate::mount::shared::Store) -> Result<Self, i32> {
        store.read_with(Self::decode)
    }

    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        let mut r = crate::state_codec::Reader(bytes);
        let sent = r.word()?;
        let received = r.word()?;
        let read_closed = r.word()? != 0;
        let write_closed = r.word()? != 0;
        let count = r.word()?;
        if count > 253 {
            return Err(EIO);
        }
        let mut records = Vec::new();
        for _ in 0..count {
            let offset = r.word()?;
            let length = r.word()?;
            let keeper = u32::try_from(r.word()?).map_err(|_| EIO)?;
            let created = r.word()?;
            let rpc = r.word()?;
            let token = r.word()?;
            let len = r.word()?;
            if len > 253 {
                return Err(EIO);
            }
            let mut fds = Vec::new();
            for _ in 0..len {
                fds.push(Descriptor::read(&mut r)?);
            }
            records.push(Record {
                offset,
                length,
                keeper,
                created,
                rpc,
                token,
                fds,
            });
        }
        // Older inactive connection records predate async notification state.
        let (reader_async, writer_async, writer_blocked) = if r.0.is_empty() {
            (AsyncOwner::default(), AsyncOwner::default(), false)
        } else {
            let reader = AsyncOwner {
                owner: r.word()? as i32,
                enabled: r.word()? != 0,
            };
            let writer = AsyncOwner {
                owner: r.word()? as i32,
                enabled: r.word()? != 0,
            };
            let blocked = r.word()? != 0;
            (reader, writer, blocked)
        };
        let reader_filter = if r.0.is_empty() { 0 } else { r.word()? };
        let reader_filter_snapshot = if r.0.is_empty() {
            Vec::new()
        } else {
            r.bytes()?.to_vec()
        };
        crate::socket::filter::State::validate_encoding(&reader_filter_snapshot)?;
        let mut reader_passcred = false;
        let mut writer_passcred = false;
        let mut credentials = smallvec::SmallVec::new();
        if !r.0.is_empty() {
            reader_passcred = r.word()? != 0;
            writer_passcred = r.word()? != 0;
            let count = r.word()?;
            if count as usize > r.0.len() / 56 {
                return Err(EIO);
            }
            for _ in 0..count {
                let start = r.word()?;
                let end = r.word()?;
                if start >= end
                    || credentials
                        .last()
                        .is_some_and(|v: &CredentialRange| v.end > start)
                {
                    return Err(EIO);
                }
                credentials.push(CredentialRange {
                    start,
                    end,
                    sender: credentials::Sender::read(&mut r)?,
                });
            }
        }
        r.end()?;
        Ok(Self {
            sent,
            received,
            read_closed,
            write_closed,
            records,
            reader_async,
            writer_async,
            writer_blocked,
            reader_filter,
            reader_filter_snapshot,
            reader_passcred,
            writer_passcred,
            credentials,
        })
    }
    fn write(&self, store: &crate::mount::shared::Store) -> Result<(), i32> {
        if self.records.is_empty()
            && self.reader_filter == 0
            && !self.reader_passcred
            && !self.writer_passcred
            && self.credentials.is_empty()
        {
            let mut bytes = [0u8; 80];
            for (slot, word) in bytes.chunks_exact_mut(8).zip([
                self.sent,
                self.received,
                u64::from(self.read_closed),
                u64::from(self.write_closed),
                0,
                self.reader_async.owner as u64,
                self.reader_async.enabled as u64,
                self.writer_async.owner as u64,
                self.writer_async.enabled as u64,
                self.writer_blocked as u64,
            ]) {
                slot.copy_from_slice(&word.to_le_bytes());
            }
            return store.replace(&bytes);
        }
        // Ordinary credential records fit on the stack. Rights, filters and
        // nested namespace identities can still spill without a wire change.
        let mut bytes = smallvec::SmallVec::<[u8; 256]>::new();
        for word in [
            self.sent,
            self.received,
            u64::from(self.read_closed),
            u64::from(self.write_closed),
            self.records.len() as u64,
        ] {
            crate::state_codec::word(&mut bytes, word);
        }
        for r in &self.records {
            for word in [
                r.offset,
                r.length,
                r.keeper as u64,
                r.created,
                r.rpc,
                r.token,
                r.fds.len() as u64,
            ] {
                crate::state_codec::word(&mut bytes, word);
            }
            for fd in &r.fds {
                fd.write(&mut bytes);
            }
        }
        for word in [
            self.reader_async.owner as u64,
            self.reader_async.enabled as u64,
            self.writer_async.owner as u64,
            self.writer_async.enabled as u64,
            self.writer_blocked as u64,
        ] {
            crate::state_codec::word(&mut bytes, word);
        }
        if self.reader_filter != 0
            || self.reader_passcred
            || self.writer_passcred
            || !self.credentials.is_empty()
        {
            crate::state_codec::word(&mut bytes, self.reader_filter);
            crate::state_codec::bytes(&mut bytes, &self.reader_filter_snapshot);
            crate::state_codec::word(&mut bytes, self.reader_passcred as u64);
            crate::state_codec::word(&mut bytes, self.writer_passcred as u64);
            crate::state_codec::word(&mut bytes, self.credentials.len() as u64);
            for range in &self.credentials {
                crate::state_codec::word(&mut bytes, range.start);
                crate::state_codec::word(&mut bytes, range.end);
                range.sender.write(&mut bytes);
            }
        }
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(crate::ENOBUFS);
        }
        store.replace(&bytes)
    }
}

fn event(token: u64) -> Result<Object, i32> {
    Object::owned(unsafe {
        CreateEventW(
            std::ptr::null(),
            1,
            0,
            wide(&format!("Local\\kinakaze-rights-{token:016x}")).as_ptr(),
        )
    })
}

fn write_space_name(id: u64, received: u64) -> Vec<u16> {
    wide(&format!(
        "Local\\kinakaze-unix-space-{}-{id:x}-{received:x}",
        kinakaze_runtime::authority::domain_id()
    ))
}

fn notify_write_space(id: u64, received: u64) {
    // Only an enrolled waiter owns this epoch event. Absence is the normal
    // uncontended path; do not create persistent objects for every read.
    let handle = unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE,
            0,
            write_space_name(id, received).as_ptr(),
        )
    };
    if !handle.is_null() {
        unsafe {
            SetEvent(handle);
            CloseHandle(handle);
        }
    }
    // Notification is an acceleration hint. Existing bounded readiness checks
    // still cover native consumers, peer crashes between read/publication, and
    // resource failures; a successful read must never become an I/O error here.
}

pub(super) fn prepare_write_wait(fd: i32) -> Result<Option<Object>, i32> {
    let (_, socket, _pin) = pin_socket(fd)?;
    if socket.state != super::State::Connected || socket.message_mode() {
        return Ok(None);
    }
    let state = socket.ancillary.as_ref().ok_or(EIO)?;
    let direction = usize::from(socket.is_server_end);
    let store = &state.queues[direction];
    // Enroll under the same gate that publishes receive progress. Ordinary
    // reads can then skip the named-event lookup unless someone needs a wake.
    let _gate = Lock::new(&state.locks[direction].queue)?;
    let mut observed = Queue::read(store)?;
    let event = Object::owned(unsafe {
        CreateEventW(
            std::ptr::null(),
            1,
            0,
            write_space_name(store.id(), observed.received).as_ptr(),
        )
    })?;
    // Read progress is the epoch, not metadata publication: sends and ancillary
    // bookkeeping cannot skip the epoch on which a blocked writer enrolled.
    if observed.read_closed
        || observed.write_closed
        || pipe_information(socket.handle as HANDLE).is_none_or(|info| {
            info.write_quota_available > 0 || info.named_pipe_state == FILE_PIPE_CLOSING_STATE
        })
    {
        unsafe {
            SetEvent(event.raw());
        }
    } else if !observed.writer_blocked {
        observed.writer_blocked = true;
        observed.write(store)?;
    }
    Ok(Some(event))
}
fn release(record: &Record) {
    if let Ok(e) = event(record.token) {
        unsafe {
            SetEvent(e.raw());
        }
    }
}
struct Pending(Option<Record>);
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(record) = &self.0 {
            release(record);
        }
    }
}
pub(super) fn creation_time(process: HANDLE) -> Result<u64, i32> {
    use windows_sys::Win32::Foundation::FILETIME;
    let mut times = [FILETIME::default(); 4];
    if unsafe {
        GetProcessTimes(
            process,
            &raw mut times[0],
            &raw mut times[1],
            &raw mut times[2],
            &raw mut times[3],
        )
    } == 0
    {
        return Err(EIO);
    }
    Ok((u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime))
}
pub(super) fn duplicate(source: HANDLE, raw: u64, target: HANDLE) -> Result<Object, i32> {
    // Only local handles may be wrapped in an owner whose Drop calls CloseHandle.
    if target != unsafe { GetCurrentProcess() } {
        return Err(EINVAL);
    }
    Object::owned(duplicate_raw(source, raw, target)? as HANDLE)
}
pub(super) fn duplicate_raw(source: HANDLE, raw: u64, target: HANDLE) -> Result<u64, i32> {
    let mut copy = std::ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            source,
            raw as HANDLE,
            target,
            &mut copy,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(copy as u64)
}
fn supported(kind: FdKind) -> bool {
    matches!(
        kind,
        FdKind::PtyMaster
            | FdKind::File
            | FdKind::Directory
            | FdKind::Pipe
            | FdKind::Fifo
            | FdKind::PtySlave
            | FdKind::Null
            | FdKind::Zero
            | FdKind::Random
            | FdKind::Full
            | FdKind::TimerFd
            | FdKind::EventFd
            | FdKind::UnixSocket
            | FdKind::Socket
            | FdKind::Synthetic
            | FdKind::SyntheticDirectory
            | FdKind::CgroupFile
            | FdKind::ProcSysctl
            | FdKind::TmpfsFile
            | FdKind::TmpfsDirectory
            | FdKind::MessageQueue
            | FdKind::SysfsFile
            | FdKind::FsContext
            | FdKind::MountTree
            | FdKind::MountNamespace
            | FdKind::TimeNamespace
            | FdKind::Namespace
            | FdKind::UserNamespace
            | FdKind::BpfProgram
    )
}
fn activate(fd: i32, descriptor: &Descriptor, process: HANDLE) -> Result<(), i32> {
    let kind = FdKind::from_fork_code(descriptor.kind);
    crate::ofd::attach(crate::get(fd)?, descriptor.shared)?;
    if kind == FdKind::BpfProgram {
        let mut input = crate::state_codec::Reader(&descriptor.metadata);
        let id = u32::try_from(input.word()?).map_err(|_| EIO)?;
        input.end()?;
        crate::bpf::import_rights(fd, id)?;
    }
    if matches!(kind, FdKind::File | FdKind::Directory) {
        let mut input = crate::state_codec::Reader(&descriptor.metadata);
        let overlay = input.word()?;
        if overlay == 1 {
            crate::mount::overlay::import_rights(crate::get(fd)?, input.0, |raw| {
                duplicate(process, raw, unsafe { GetCurrentProcess() })
            })?;
        } else if overlay == 0 {
            crate::mount::native::import_rights(crate::get(fd)?, input.0, |raw| {
                duplicate(process, raw, unsafe { GetCurrentProcess() })
            })?;
        } else {
            return Err(EIO);
        }
    }
    if kind == FdKind::Pipe {
        crate::pipe_inode::import_rights(crate::get(fd)?, &descriptor.metadata, |raw| {
            duplicate(process, raw, unsafe { GetCurrentProcess() })
        })?;
    }
    if kind == FdKind::Fifo {
        let mut input = crate::state_codec::Reader(&descriptor.metadata);
        let marker = duplicate(process, input.word()?, unsafe { GetCurrentProcess() })?;
        input.end()?;
        crate::fifo::import_rights(fd, marker)?;
    }
    if kind == FdKind::UnixSocket {
        import_socket(fd, &descriptor.metadata, |raw| {
            duplicate(process, raw, unsafe { GetCurrentProcess() })
        })?;
    }
    if matches!(
        kind,
        FdKind::Synthetic | FdKind::SyntheticDirectory | FdKind::CgroupFile | FdKind::ProcSysctl
    ) {
        crate::import_synthetic_rights(fd, &descriptor.metadata)?;
    }
    match kind {
        FdKind::PtyMaster | FdKind::PtySlave => {
            crate::tty::resolve(fd)?;
        }
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile => {
            crate::tmpfs::fstat(fd)?;
        }
        FdKind::TimerFd => {
            crate::timerfd::gettime(fd)?;
        }
        FdKind::EventFd => {
            crate::eventfd::poll_eventfd(fd)?;
        }
        FdKind::Socket => {
            let mut r = crate::state_codec::Reader(&descriptor.metadata);
            let id = r.word()?;
            let token = r.word()?;
            let token = if token == 0 {
                None
            } else {
                Some(duplicate(process, token, unsafe { GetCurrentProcess() })?)
            };
            crate::usernet::import_rights(crate::get(fd)?, id, token)?;
        }
        _ => {}
    }
    Ok(())
}

/// The escrow process pins terminal mapping and liveness objects until the
/// receiver imports the descriptor or closes its endpoint. The sender may exit
/// immediately after sendmsg without invalidating the queued reference.
fn export(socket: &UnixSocket, rights: &[i32]) -> Result<Record, i32> {
    let mut pinned = Vec::new();
    for &fd in rights {
        let info = crate::get(fd)?;
        let bpf = if info.kind == FdKind::BpfProgram {
            Some(crate::bpf::rights_reference(fd)?)
        } else {
            None
        };
        let terminal = if matches!(info.kind, FdKind::PtyMaster | FdKind::PtySlave) {
            Some(crate::tty::resolve(fd)?.0)
        } else {
            None
        };
        let unix = if info.kind == FdKind::UnixSocket {
            Some(snapshot(fd)?)
        } else {
            None
        };
        let shared = if info.flags.contains(FdFlags::SEEKABLE)
            || matches!(
                info.kind,
                FdKind::File
                    | FdKind::Directory
                    | FdKind::Pipe
                    | FdKind::PtyMaster
                    | FdKind::PtySlave
                    | FdKind::EventFd
                    | FdKind::TimerFd
                    | FdKind::UnixSocket
                    | FdKind::Socket
            ) {
            Some(crate::ofd::promote(fd)?)
        } else {
            None
        };
        let synthetic = if matches!(
            info.kind,
            FdKind::Synthetic
                | FdKind::SyntheticDirectory
                | FdKind::CgroupFile
                | FdKind::ProcSysctl
        ) {
            Some(crate::export_synthetic_rights(fd, info)?)
        } else {
            None
        };
        let table = crate::table().read().map_err(|_| EIO)?;
        let entry = table
            .slots
            .get(usize::try_from(fd).map_err(|_| EBADF)?)
            .and_then(|e| *e)
            .ok_or(EBADF)?;
        if entry.generation != info.generation {
            return Err(EBADF);
        }
        if !supported(entry.kind) {
            return Err(EOPNOTSUPP);
        }
        let socket_pin = if entry.kind == FdKind::Socket {
            Some(crate::socket::import_rights(
                &crate::socket::export_rights(entry.raw, std::process::id())?,
            )?)
        } else {
            None
        };
        let network = crate::usernet::rights_reference(entry)?;
        let pin = if entry.raw == 0 || entry.kind == FdKind::Socket {
            None
        } else {
            Some(duplicate(
                unsafe { GetCurrentProcess() },
                entry.raw as u64,
                unsafe { GetCurrentProcess() },
            )?)
        };
        let native = if matches!(entry.kind, FdKind::File | FdKind::Directory) {
            crate::mount::native::reference(entry)?
        } else {
            None
        };
        let pipe = crate::pipe_inode::reference(entry)?;
        let fifo = crate::fifo::rights_reference(entry)?;
        let overlay = crate::mount::overlay::reference(entry)?;
        pinned.push((
            entry, pin, shared, native, terminal, pipe, overlay, unix, socket_pin, network, fifo,
            synthetic, bpf,
        ));
    }
    let token = random_id()?;
    let _event = event(token)?;
    let rpc = crate::mount::shared::new_object()?;
    let executable = std::env::current_exe().map_err(|_| EIO)?;
    let mut child = crate::with_exec_handle_filter(|| {
        Command::new(executable)
            .arg("--unix-rights-keeper")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
    })
    .map_err(|_| EIO)?
    .map_err(|_| EIO)?;
    let result = (|| {
        let mut bytes = Vec::new();
        let pipe = duplicate_raw(
            unsafe { GetCurrentProcess() },
            socket.handle as u64,
            child.as_raw_handle(),
        )?;
        crate::state_codec::word(&mut bytes, pipe);
        crate::state_codec::word(&mut bytes, token);
        crate::state_codec::word(&mut bytes, rpc.id());
        crate::state_codec::word(&mut bytes, rights.len() as u64);
        let mut fds = Vec::new();
        for (
            entry,
            pin,
            shared,
            native,
            _terminal,
            pipe,
            overlay,
            unix,
            socket_pin,
            network,
            fifo,
            synthetic,
            bpf,
        ) in &pinned
        {
            let raw = if let Some(pin) = pin {
                duplicate_raw(
                    unsafe { GetCurrentProcess() },
                    pin.raw() as u64,
                    child.as_raw_handle(),
                )?
            } else {
                0
            };
            let flags = entry.flags.0 & !FdFlags::CLOSE_ON_EXEC.0;
            let metadata = if matches!(entry.kind, FdKind::File | FdKind::Directory) {
                let mut data = Vec::new();
                crate::state_codec::word(&mut data, u64::from(overlay.is_some()));
                let dup =
                    |raw| duplicate_raw(unsafe { GetCurrentProcess() }, raw, child.as_raw_handle());
                data.extend(if let Some(overlay) = overlay {
                    crate::mount::overlay::export_rights(overlay, dup)?
                } else {
                    crate::mount::native::export_rights_reference(native.clone(), dup)?
                });
                data
            } else if entry.kind == FdKind::Pipe {
                crate::pipe_inode::export_rights(pipe.clone(), |raw| {
                    duplicate_raw(unsafe { GetCurrentProcess() }, raw, child.as_raw_handle())
                })?
            } else if let Some(fifo) = fifo {
                let mut bytes = Vec::new();
                crate::state_codec::word(
                    &mut bytes,
                    duplicate_raw(
                        unsafe { GetCurrentProcess() },
                        fifo.raw(),
                        child.as_raw_handle(),
                    )?,
                );
                bytes
            } else if let Some(unix) = unix {
                export_socket(unix, |raw| {
                    duplicate_raw(unsafe { GetCurrentProcess() }, raw, child.as_raw_handle())
                })?
            } else if let Some(socket_pin) = socket_pin {
                let mut data = Vec::new();
                crate::state_codec::word(
                    &mut data,
                    network.as_ref().map_or(0, |e| crate::usernet::rights_id(e)),
                );
                let token = network
                    .as_ref()
                    .map(|endpoint| crate::usernet::rights_token(endpoint))
                    .transpose()?
                    .flatten();
                crate::state_codec::word(
                    &mut data,
                    match token {
                        Some(token) => duplicate_raw(
                            unsafe { GetCurrentProcess() },
                            token.raw() as u64,
                            child.as_raw_handle(),
                        )?,
                        None => 0,
                    },
                );
                crate::state_codec::bytes(
                    &mut data,
                    &crate::socket::export_rights(socket_pin.0, child.id())?,
                );
                data
            } else if let Some(bpf) = bpf {
                let mut bytes = Vec::new();
                crate::state_codec::word(&mut bytes, bpf.id() as u64);
                bytes
            } else if let Some(synthetic) = synthetic {
                synthetic.clone()
            } else {
                Vec::new()
            };
            let mut descriptor = Descriptor {
                raw,
                kind: entry.kind.fork_code(),
                flags,
                shared: shared.as_ref().map_or(0, |s| s.id()),
                metadata,
            };
            descriptor.write(&mut bytes);
            if entry.kind == FdKind::Socket {
                descriptor.metadata.truncate(16);
            }
            fds.push(descriptor);
        }
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(crate::EMSGSIZE);
        }
        // Only the fixed-size bootstrap ID crosses the anonymous pipe. The
        // bounded shared payload cannot block a sender behind a stuck loader.
        rpc.update(|_| Ok((bytes, ())))?;
        let mut input = child.stdin.take().ok_or(EIO)?;
        input.write_all(&rpc.id().to_le_bytes()).map_err(|_| EIO)?;
        drop(input);
        let mut output = child.stdout.take().ok_or(EIO)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let mut available = 0;
            if unsafe {
                windows_sys::Win32::System::Pipes::PeekNamedPipe(
                    output.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(EIO);
            }
            if available != 0 {
                break;
            }
            if child.try_wait().map_err(|_| EIO)?.is_some() || std::time::Instant::now() >= deadline
            {
                return Err(EIO);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let mut answer = [0];
        output.read_exact(&mut answer).map_err(|_| EIO)?;
        if answer != [1] {
            return Err(EIO);
        }
        Ok(Record {
            offset: 0,
            length: 0,
            keeper: child.id(),
            created: creation_time(child.as_raw_handle())?,
            rpc: rpc.id(),
            token,
            fds,
        })
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(result?)
}

pub(crate) fn run_keeper() -> Result<(), i32> {
    let mut input = std::io::stdin();
    let mut bootstrap = [0; 8];
    input.read_exact(&mut bootstrap).map_err(|_| EIO)?;
    let handoff = crate::mount::shared::Store::user_object(u64::from_le_bytes(bootstrap), false)?;
    let (_, bytes) = handoff.read()?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(EIO);
    }
    let mut r = crate::state_codec::Reader(&bytes);
    let pipe = Object::owned(r.word()? as HANDLE)?;
    let event = event(r.word()?)?;
    let rpc = crate::mount::shared::Store::user_object(r.word()?, false)?;
    let request_event = socket_rpc::notification(rpc.id())?;
    let count = r.word()?;
    if count > 253 {
        return Err(EIO);
    }
    let mut descriptors = Vec::new();
    let mut packet_pins = Vec::new();
    for _ in 0..count {
        let descriptor = Descriptor::read(&mut r)?;
        let raw = descriptor.raw as usize;
        let kind = FdKind::from_fork_code(descriptor.kind);
        let flags = FdFlags(descriptor.flags);
        if !supported(kind) {
            return Err(EOPNOTSUPP);
        }
        let socket = if kind == FdKind::Socket {
            let mut data = crate::state_codec::Reader(&descriptor.metadata);
            data.word()?;
            data.word()?;
            Some(crate::socket::import_rights(data.bytes()?)?)
        } else {
            None
        };
        let fd = crate::install(
            socket.as_ref().map_or(raw, |s| s.0),
            kind,
            flags.union(FdFlags::CLOSE_ON_EXEC),
        )?;
        if let Some(socket) = socket {
            socket.into_raw();
        }
        let activation = if kind == FdKind::Socket && flags.contains(FdFlags::PACKET_SOCKET) {
            // The keeper can be a different PE module than the guest libc.
            // Retain native sections without interpreting its Rust protocol
            // layout; the actual recipient performs the full checked import.
            crate::ofd::attach(crate::get(fd)?, descriptor.shared)?;
            let mut data = crate::state_codec::Reader(&descriptor.metadata);
            let pins = crate::usernet::pin_packet_rights(data.word()?)?;
            let token = data.word()?;
            if token != 0 {
                packet_pins.push(Object::owned(token as HANDLE)?);
            }
            packet_pins.extend(pins);
            Ok(())
        } else {
            activate(fd, &descriptor, unsafe { GetCurrentProcess() })
        };
        if let Err(error) = activation {
            if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
                let path = std::path::PathBuf::from(directory)
                    .join(format!("rights-keeper-{}.log", std::process::id()));
                let _ = std::fs::write(
                    path,
                    format!(
                        "activate fd={fd} kind={kind:?} raw={raw:#x} shared={:#x} errno={error}\n",
                        descriptor.shared,
                    ),
                );
            }
            return Err(error);
        }
        descriptors.push(fd);
    }
    r.end()?;
    let mut output = std::io::stdout().lock();
    output.write_all(&[1]).map_err(|_| EIO)?;
    output.flush().map_err(|_| EIO)?;
    drop(output);
    loop {
        let events = [event.raw(), request_event.raw()];
        let ready = unsafe { WaitForMultipleObjects(2, events.as_ptr(), 0, 100) };
        if ready == WAIT_OBJECT_0 {
            break;
        }
        if ready == WAIT_OBJECT_0 + 1 {
            socket_rpc::serve(&rpc, &descriptors)?;
        }
        if pipe_information(pipe.raw())
            .is_none_or(|i| i.named_pipe_state == FILE_PIPE_CLOSING_STATE)
        {
            break;
        }
    }
    for fd in descriptors {
        let _ = crate::close(fd);
    }
    Ok(())
}

pub(super) fn pin_socket(fd: i32) -> Result<(crate::FdEntry, UnixSocket, Option<Object>), i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(usize::try_from(fd).map_err(|_| EBADF)?)
        .and_then(|slot| *slot)
        .ok_or(EBADF)?;
    if entry.kind != FdKind::UnixSocket {
        return Err(ENOTSOCK);
    }
    let mut socket = sockets()
        .lock()
        .map_err(|_| EIO)?
        .get(&fd)
        .cloned()
        .ok_or(EBADF)?;
    if socket.handle != entry.raw {
        return Err(EBADF);
    }
    let pin = if entry.raw == 0 {
        None
    } else {
        Some(duplicate(
            unsafe { GetCurrentProcess() },
            entry.raw as u64,
            unsafe { GetCurrentProcess() },
        )?)
    };
    if let Some(pin) = &pin {
        socket.handle = pin.raw() as usize;
    }
    // A listener can change native handles without changing its generation.
    // Read both tables and pin that handle under one descriptor-table guard.
    // Shared status flags are refreshed only after dropping the guard, because
    // an open-description transaction can itself acquire the descriptor table.
    drop(table);
    socket.refresh_listener()?;
    Ok((crate::ofd::refresh(entry)?, socket, pin))
}

fn async_state(fd: i32) -> Result<(Arc<State>, usize), i32> {
    let (_, socket, _pin) = pin_socket(fd)?;
    // Connection queues provide cross-process notification for connected Unix
    // sockets. Unconnected/listening sockets need their own readiness source.
    if socket.state != super::State::Connected {
        return Err(EOPNOTSUPP);
    }
    Ok((
        socket.ancillary.ok_or(EIO)?,
        usize::from(!socket.is_server_end),
    ))
}

fn update_async(fd: i32, update: impl Fn(&mut AsyncOwner)) -> Result<(), i32> {
    let (state, input) = async_state(fd)?;
    // Fixed ordering, including when opposite endpoints are configured at once.
    let _first = Lock::new(&state.locks[0].queue)?;
    let _second = Lock::new(&state.locks[1].queue)?;
    for (index, store) in state.queues.iter().enumerate() {
        let mut queue = Queue::read(store)?;
        update(if index == input {
            &mut queue.reader_async
        } else {
            &mut queue.writer_async
        });
        queue.write(store)?;
    }
    Ok(())
}

pub fn set_async(fd: i32, enabled: bool) -> Result<(), i32> {
    update_async(fd, |value| value.enabled = enabled)
}
pub fn async_enabled(fd: i32) -> Result<bool, i32> {
    let (state, input) = async_state(fd)?;
    Ok(Queue::read(&state.queues[input])?.reader_async.enabled)
}
pub fn set_owner(fd: i32, owner: i32) -> Result<(), i32> {
    let owner = crate::job::resolve_io_owner(owner)?;
    update_async(fd, |value| value.owner = owner)
}
pub fn get_owner(fd: i32) -> Result<i32, i32> {
    let (state, input) = async_state(fd)?;
    Ok(crate::job::visible_io_owner(
        Queue::read(&state.queues[input])?.reader_async.owner,
    ))
}

pub(super) fn publish_filter(fd: i32, filter: Arc<crate::ofd::Shared>) -> Result<(), i32> {
    with_filter_publication(fd, &filter, |publish| {
        publish(&crate::socket::filter::snapshot(&filter)?.encode())
    })
}

pub(super) fn with_filter_publication<T>(
    fd: i32,
    filter: &crate::ofd::Shared,
    update: impl FnOnce(&mut dyn FnMut(&[u8]) -> Result<(), i32>) -> Result<T, i32>,
) -> Result<T, i32> {
    let (entry, socket, _pin) = pin_socket(fd)?;
    // Linux runs unix datagram/seqpacket filters in the sender's enqueue path;
    // a Unix byte stream stores the option but does not execute sk_filter.
    if !socket.message_mode() {
        return update(&mut |_| Ok(()));
    }
    if crate::ofd::existing(entry.description_id)?.is_none_or(|current| current.id() != filter.id())
    {
        return Err(EBADF);
    }
    let Some(state) = &socket.ancillary else {
        return update(&mut |_| Ok(()));
    };
    let input = usize::from(!socket.is_server_end);
    let _guard = Lock::new(&state.locks[input].queue)?;
    let mut queue = Queue::read(&state.queues[input])?;
    update(&mut |encoded| {
        // A peer owns the connection queue after the receiving process exits.
        // Retain the last committed rules there, even if that process was the
        // only holder of the separate open-description section. Publish while
        // excluding sends and before publishing the same OFD revision.
        queue.reader_filter = filter.id();
        queue.reader_filter_snapshot = encoded.to_vec();
        queue.write(&state.queues[input])
    })
}

fn filter_length(
    state: &State,
    direction: usize,
    id: u64,
    saved: &[u8],
    data: &[u8],
) -> Result<Option<usize>, i32> {
    if id == 0 {
        return Ok(Some(data.len()));
    }
    let mut cached = state.filters[direction].lock().map_err(|_| EIO)?;
    if cached
        .as_ref()
        .is_none_or(|(cached_id, _)| *cached_id != id)
    {
        match crate::ofd::Shared::open(id) {
            Ok(filter) => *cached = Some((id, filter)),
            Err(error) if !saved.is_empty() => {
                // The rules outlive the receiver just as the peer-held Linux
                // socket does. An allow verdict still reaches the pipe's real
                // closed-peer error; a drop does not enqueue or send anything.
                let _ = error;
                return Ok(crate::socket::filter::State::decode(saved)?
                    .run(&crate::socket::filter::LocalPacket(data), 0));
            }
            Err(error) => return Err(error),
        }
    }
    let filter = crate::socket::filter::snapshot(&cached.as_ref().unwrap().1)?;
    Ok(filter.run(&crate::socket::filter::LocalPacket(data), 0))
}

pub(super) unsafe fn send(
    fd: i32,
    buffer: *const u8,
    len: usize,
    flags: i32,
    rights: &[i32],
    credentials: Option<&credentials::Sender>,
) -> Result<usize, i32> {
    if rights.len() > 253 {
        return Err(EINVAL);
    }
    let (fd_entry, socket, _pin) = pin_socket(fd)?;
    if socket.state != super::State::Connected {
        return Err(ENOTCONN);
    }
    if len == 0 && !socket.message_mode() {
        for fd in rights {
            crate::get(*fd)?;
        }
        if socket.write_shut {
            return Err(EPIPE);
        }
        return Ok(0);
    }
    let state = socket.ancillary.as_ref().ok_or(EIO)?;
    let direction = usize::from(socket.is_server_end);
    let store = &state.queues[direction];
    let nonblocking = fd_entry.flags.contains(FdFlags::NONBLOCK) || flags & MSG_DONTWAIT != 0;
    let _write = Lock::acquire(&state.locks[direction].tx, nonblocking, true)?;
    let gate = &state.locks[direction].queue;
    let original_len = len;
    let len = if socket.message_mode() {
        for fd in rights {
            crate::get(*fd)?;
        }
        let (revision, mut queue) = store.read_with_revision(Queue::decode)?;
        // An installed filter spans the queue and a separate OFD publication.
        // Keep that transaction serialized; unfiltered messages use one snapshot.
        let _lock = if queue.reader_filter != 0 {
            let lock = Lock::new(gate)?;
            if store.revision() != revision {
                queue = Queue::read(store)?;
            }
            Some(lock)
        } else {
            None
        };
        if queue.reader_filter == 0 {
            len
        } else {
            let data = if len == 0 {
                &[]
            } else {
                unsafe { std::slice::from_raw_parts(buffer, len) }
            };
            match filter_length(
                state,
                direction,
                queue.reader_filter,
                &queue.reader_filter_snapshot,
                data,
            )? {
                Some(length) => length,
                None => return Ok(original_len),
            }
        }
    } else {
        len
    };
    let record = if rights.is_empty() {
        None
    } else {
        Some(export(&socket, rights)?)
    };
    let mut pending = Pending(record.clone());
    let (start, has_credentials) = {
        let (revision, mut queue) = store.read_with_revision(Queue::decode)?;
        // A plain send only inspects shutdown and SO_PASSCRED here. The Store
        // already returns one validated atomic snapshot; no queue mutation or
        // native mutex round trip is needed until committing the sent count.
        // Ancillary publication still locks and rereads before changing state.
        let _lock = if record.is_some()
            || credentials.is_some()
            || queue.reader_passcred
            || queue.writer_passcred
        {
            let lock = Lock::new(gate)?;
            if store.revision() != revision {
                queue = Queue::read(store)?;
            }
            Some(lock)
        } else {
            None
        };
        if queue.read_closed || queue.write_closed {
            return Err(EPIPE);
        }
        let sender = match credentials {
            Some(sender) => Some(sender.clone()),
            None if queue.reader_passcred || queue.writer_passcred => {
                Some(credentials::Sender::current(false)?)
            }
            None => None,
        };
        let has_credentials = sender.is_some();
        if let Some(sender) = sender {
            let end = queue
                .sent
                .checked_add(if socket.message_mode() { 1 } else { len as u64 })
                .ok_or(EIO)?;
            if let Some(last) = queue
                .credentials
                .last_mut()
                .filter(|last| last.end == queue.sent && last.sender == sender)
            {
                last.end = end;
            } else {
                queue.credentials.push(CredentialRange {
                    start: queue.sent,
                    end,
                    sender,
                });
            }
        }
        if let Some(mut r) = record.clone() {
            r.offset = queue.sent;
            r.length = len as u64;
            queue.records.push(r);
        }
        if record.is_some() || has_credentials {
            queue.write(store)?;
        }
        (queue.sent, has_credentials)
    };
    let result = unsafe { super::send_payload(fd, fd_entry, &socket, buffer, len, flags) };
    if record.is_none() && !has_credentials && result.is_err() && result != Err(EAGAIN) {
        return result;
    }
    let _lock = Lock::new(gate)?;
    let mut queue = Queue::read(store)?;
    let mut writer = AsyncOwner::default();
    if result == Err(EAGAIN) {
        queue.writer_blocked = true;
        // A concurrent reader may free quota before this EAGAIN is published.
        // Recheck while holding the queue gate so its wake cannot be lost.
        if pipe_information(socket.handle as HANDLE).is_some_and(|p| p.write_quota_available > 0) {
            queue.writer_blocked = false;
            writer = queue.writer_async;
            // A receive may have freed native quota but still be waiting for
            // this gate to publish its epoch. Wake enrolled waiters before
            // clearing their shared progress marker.
            if !socket.message_mode() {
                notify_write_space(store.id(), queue.received);
            }
        }
    }
    if let Ok(sent) = result {
        queue.sent = queue
            .sent
            .checked_add(if socket.message_mode() {
                1
            } else {
                sent as u64
            })
            .ok_or(EIO)?;
        if let Some(record) = &record {
            if let Some(r) = queue.records.iter_mut().find(|r| r.token == record.token) {
                r.length = sent as u64;
            }
        }
    }
    if !result.is_ok_and(|n| n > 0 || socket.message_mode()) {
        if let Some(r) = record {
            queue.records.retain(|v| v.token != r.token);
            release(&r);
        }
    }
    if has_credentials {
        let end = if result.is_ok() { queue.sent } else { start };
        for range in &mut queue.credentials {
            range.end = range.end.min(end);
        }
        queue
            .credentials
            .retain(|range| range.start < range.end && range.end > queue.received);
    }
    queue.write(store)?;
    pending.0 = None;
    let reader = queue.reader_async;
    drop(_lock);
    drop(_write);
    writer.notify();
    if result.is_ok_and(|n| n > 0 || socket.message_mode()) {
        reader.notify();
    }
    result.map(|sent| {
        if socket.message_mode() {
            original_len
        } else {
            sent
        }
    })
}

fn stream_receive_limit(
    store: &crate::mount::shared::Store,
    mut amount: usize,
) -> Result<usize, i32> {
    // The caller owns rx, so received cannot advance. Writers publish ancillary
    // barriers before their payload and Store snapshots cannot tear. Read after
    // inspecting the pipe to include every barrier for the observed bytes.
    let queue = Queue::read(store)?;
    if let Some(record) = queue.records.iter().find(|record| {
        record.offset >= queue.received
            && record.offset < queue.received.saturating_add(amount as u64)
    }) {
        amount =
            amount.min((record.offset - queue.received).saturating_add(record.length) as usize);
    }
    if queue.reader_passcred {
        if let Some(range) = queue
            .credentials
            .iter()
            .find(|range| range.end > queue.received)
        {
            let boundary = if range.start > queue.received {
                range.start
            } else {
                range.end
            };
            amount = amount.min((boundary - queue.received) as usize);
        }
    }
    Ok(amount)
}

pub(super) unsafe fn recv(
    fd: i32,
    buffer: *mut u8,
    len: usize,
    flags: i32,
    control: usize,
    recvmsg: bool,
) -> Result<(usize, Vec<i32>, i32, Option<credentials::Ucred>), i32> {
    let (fd_entry, socket, _pin) = pin_socket(fd)?;
    if !matches!(
        socket.state,
        super::State::Connected | super::State::Disconnected
    ) {
        return Err(ENOTCONN);
    }
    let state = socket.ancillary.as_ref().ok_or(EIO)?;
    let direction = usize::from(!socket.is_server_end);
    let store = &state.queues[direction];
    let id = store.id();
    let nonblocking = fd_entry.flags.contains(FdFlags::NONBLOCK) || flags & MSG_DONTWAIT != 0;
    let _read = Lock::acquire(&state.locks[direction].rx, nonblocking, true)?;
    let gate = &state.locks[direction].queue;
    {
        let queue = Queue::read(store)?;
        if queue.read_closed || (queue.write_closed && queue.received == queue.sent) {
            return Ok((0, Vec::new(), 0, None));
        }
    }
    if socket.read_shut {
        return Ok((0, Vec::new(), 0, None));
    }
    let peeking = flags & MSG_PEEK != 0;
    let limit = |amount| stream_receive_limit(store, amount);
    let present = if len != 0 || socket.message_mode() {
        loop {
            match unsafe {
                inspect_socket(
                    socket.handle as HANDLE,
                    if peeking {
                        buffer
                    } else {
                        std::ptr::null_mut()
                    },
                    if peeking { len } else { 0 },
                    true,
                    socket.message_mode(),
                    true,
                    if peeking && !socket.message_mode() {
                        Some(&limit)
                    } else {
                        None
                    },
                )
            } {
                Ok(Some(n)) => break n,
                Ok(None) => return Ok((0, Vec::new(), 0, None)),
                Err(EAGAIN) => {
                    let closed = {
                        let queue = Queue::read(store)?;
                        queue.read_closed || queue.write_closed
                    };
                    if closed {
                        return Ok((0, Vec::new(), 0, None));
                    }
                    if nonblocking {
                        return Err(EAGAIN);
                    }
                    if signal::deliver_pending() == signal::Delivery::Interrupted {
                        return Err(EINTR);
                    }
                    if !socket.message_mode() {
                        // Arm the existing non-consuming native stream read on
                        // this pinned endpoint, never a possibly reused fd.
                        let pipe = Object::duplicate(socket.handle as HANDLE)?;
                        let mut wait = super::readiness::prepare_stream(pipe)?;
                        // Metadata-only SHUT_RD/SHUT_WR still needs a bounded
                        // recheck; data and native peer closure wake immediately.
                        wait.wait_interruptible(1)?;
                    } else {
                        // A zero-byte native read consumes an empty message.
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    } else {
        0
    };
    let message_length = socket.message_mode().then_some(present);
    let received = if peeking {
        // Inspection already copied the bounded snapshot. Message-mode peeks
        // report the complete first record only when MSG_TRUNC was requested.
        if socket.message_mode() && flags & 0x20 != 0 {
            present
        } else {
            len.min(present)
        }
    } else {
        // The receive mutex excludes other consumers; later writers can only
        // append data beyond the observed snapshot and its metadata barriers.
        let amount = if !socket.message_mode() && len != 0 {
            stream_receive_limit(store, len.min(present))?
        } else {
            len
        };
        unsafe {
            super::recv_payload(
                fd,
                fd_entry,
                &socket,
                buffer,
                amount,
                flags & !0x40000000,
                present,
            )
        }?
    };
    let _lock = Lock::new(gate)?;
    let mut queue = Queue::read(store)?;
    let end = queue
        .received
        .checked_add(if socket.message_mode() {
            1
        } else {
            received as u64
        })
        .ok_or(EIO)?;
    let mut descriptors = Vec::new();
    let mut truncated = false;
    let credentials = if recvmsg && queue.reader_passcred && end > queue.received {
        Some(
            match queue
                .credentials
                .iter()
                .find(|r| r.start <= queue.received && r.end > queue.received)
            {
                Some(range) => range.sender.visible()?,
                None => credentials::absent(),
            },
        )
    } else {
        None
    };
    let max_rights = if recvmsg {
        let available = control.saturating_sub(if credentials.is_some() { 32 } else { 0 });
        (available.saturating_sub(16) / 4).min(253)
    } else {
        control
    };
    for record in queue
        .records
        .iter()
        .filter(|r| r.offset >= queue.received && r.offset < end)
    {
        let process = Object::owned(unsafe {
            OpenProcess(
                PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION | 0x100000,
                0,
                record.keeper,
            )
        })
        .and_then(|p| {
            if creation_time(p.raw())? == record.created {
                Ok(p)
            } else {
                Err(EIO)
            }
        });
        for (index, descriptor) in record.fds.iter().enumerate() {
            let (raw, kind, stored_flags) = (descriptor.raw, descriptor.kind, descriptor.flags);
            if descriptors.len() == max_rights {
                truncated = true;
                continue;
            }
            let result = (|| {
                let process = process.as_ref().map_err(|e| *e)?;
                let socket = if FdKind::from_fork_code(kind) == FdKind::Socket {
                    Some(socket_rpc::request(record.rpc, index, process.raw())?)
                } else {
                    None
                };
                let object = if raw == 0 {
                    None
                } else {
                    Some(duplicate(process.raw(), raw, unsafe {
                        GetCurrentProcess()
                    })?)
                };
                let kind = FdKind::from_fork_code(kind);
                if !supported(kind) {
                    return Err(EOPNOTSUPP);
                }
                let mut fd_flags = FdFlags(stored_flags & !FdFlags::CLOSE_ON_EXEC.0);
                if flags & 0x40000000 != 0 {
                    fd_flags = fd_flags.union(FdFlags::CLOSE_ON_EXEC);
                }
                let fd = crate::install(
                    socket
                        .as_ref()
                        .map_or_else(|| object.as_ref().map_or(0, |o| o.raw() as usize), |s| s.0),
                    kind,
                    fd_flags,
                )?;
                if let Some(object) = object {
                    object.into_raw();
                }
                if let Some(socket) = socket {
                    socket.into_raw();
                }
                if let Err(e) = activate(fd, descriptor, process.raw()) {
                    let _ = crate::close(fd);
                    return Err(e);
                }
                Ok(fd)
            })();
            match result {
                Ok(fd) => descriptors.push(fd),
                Err(_) => truncated = true,
            }
        }
        if flags & MSG_PEEK == 0 {
            release(record);
        }
    }
    let mut writer = AsyncOwner::default();
    if flags & MSG_PEEK == 0 {
        let previous_received = queue.received;
        let wake_writer = end != previous_received && queue.writer_blocked;
        queue.records.retain(|r| r.offset >= end);
        queue.received = end;
        queue.credentials.retain(|range| range.end > end);
        if wake_writer {
            queue.writer_blocked = false;
            writer = queue.writer_async;
        }
        if let Err(e) = queue.write(store) {
            for fd in descriptors {
                let _ = crate::close(fd);
            }
            return Err(e);
        }
        if wake_writer && !socket.message_mode() {
            notify_write_space(id, previous_received);
        }
    }
    drop(_lock);
    drop(_read);
    writer.notify();
    Ok((
        received,
        descriptors,
        (if truncated { 8 } else { 0 })
            | (if message_length.is_some_and(|n| n > len) {
                0x20
            } else {
                0
            }),
        credentials,
    ))
}

pub(super) fn shutdown(socket: &UnixSocket, how: i32) -> Result<(), i32> {
    let state = socket.ancillary.as_ref().ok_or(EIO)?;
    for (index, store) in state.queues.iter().enumerate() {
        let incoming = index == usize::from(!socket.is_server_end);
        if (incoming && how == SHUT_WR) || (!incoming && how == SHUT_RD) {
            continue;
        }
        let _gate = Lock::new(&state.locks[index].queue)?;
        let mut queue = Queue::read(store)?;
        if incoming {
            queue.read_closed = true;
            for record in queue.records.drain(..) {
                release(&record);
            }
            queue.credentials.clear();
        } else {
            queue.write_closed = true;
        }
        queue.write(store)?;
        notify_write_space(store.id(), queue.received);
        let target = if incoming {
            queue.writer_async
        } else {
            queue.reader_async
        };
        drop(_gate);
        target.notify();
    }
    Ok(())
}
pub(super) fn shutdown_state(socket: &UnixSocket) -> Result<(bool, bool, bool), i32> {
    let state = socket.ancillary.as_ref().ok_or(EIO)?;
    let input = Queue::read(&state.queues[usize::from(!socket.is_server_end)])?;
    let output = Queue::read(&state.queues[usize::from(socket.is_server_end)])?;
    Ok((
        input.read_closed || input.write_closed,
        output.read_closed || output.write_closed,
        input.write_closed && output.write_closed,
    ))
}

fn export_socket(
    socket: &UnixSocket,
    mut duplicate: impl FnMut(u64) -> Result<u64, i32>,
) -> Result<Vec<u8>, i32> {
    let mut bytes = Vec::new();
    let state = match socket.state {
        super::State::Idle => 0,
        super::State::Bound => 1,
        super::State::Listening => 2,
        super::State::Connected => 3,
        super::State::Disconnected => 4,
    };
    let flags = u64::from(socket.owns_file)
        | (u64::from(socket.read_shut) << 1)
        | (u64::from(socket.write_shut) << 2)
        | (u64::from(socket.is_server_end) << 3);
    for value in [
        socket.socket_type as u64,
        state,
        flags,
        socket.network,
        match &socket.inode {
            Some(i) => duplicate(i.raw() as u64)?,
            None => 0,
        },
        socket.record.id(),
        duplicate(socket.record.pin.raw() as u64)?,
        socket.listener.as_ref().map_or(0, |listener| listener.id()),
        match &socket.listener {
            Some(listener) => duplicate(listener.pin.raw() as u64)?,
            None => 0,
        },
    ] {
        crate::state_codec::word(&mut bytes, value);
    }
    for address in [&socket.local, &socket.peer] {
        crate::state_codec::word(&mut bytes, namespace_code(&address.namespace) as u64);
        crate::state_codec::bytes(&mut bytes, &address.name);
    }
    Ok(bytes)
}
fn import_socket(
    fd: i32,
    bytes: &[u8],
    mut duplicate: impl FnMut(u64) -> Result<Object, i32>,
) -> Result<(), i32> {
    let mut input = crate::state_codec::Reader(bytes);
    let socket_type = input.word()? as i32;
    if !matches!(socket_type, SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET) {
        return Err(EIO);
    }
    let state = match input.word()? {
        0 => super::State::Idle,
        1 => super::State::Bound,
        2 => super::State::Listening,
        3 => super::State::Connected,
        4 => super::State::Disconnected,
        _ => return Err(EIO),
    };
    let flags = input.word()?;
    let network = input.word()?;
    let inode_raw = input.word()?;
    let inode = if inode_raw == 0 {
        None
    } else {
        Some(Arc::new(SocketInode(
            duplicate(inode_raw)?.into_raw() as usize
        )))
    };
    let record_id = input.word()?;
    let _record_pin = duplicate(input.word()?)?;
    let record = super::procnet::Record::restore(record_id)?;
    let listener_id = input.word()?;
    let listener_pin = input.word()?;
    let _listener_pin = if listener_pin == 0 {
        None
    } else {
        Some(duplicate(listener_pin)?)
    };
    let listener = if listener_id == 0 {
        None
    } else {
        Some(super::listener::Lease::restore(listener_id)?)
    };
    let mut addresses = Vec::new();
    for _ in 0..2 {
        let namespace = match input.word()? {
            1 => Namespace::Pathname,
            2 => Namespace::Abstract,
            3 => Namespace::Unnamed,
            _ => return Err(EIO),
        };
        let name = input.bytes()?.to_vec();
        if name.len() > 108 {
            return Err(EIO);
        }
        addresses.push(UnixAddress { namespace, name });
    }
    input.end()?;
    let entry = get(fd)?;
    let socket = UnixSocket {
        record,
        ancillary: if entry.raw == 0
            || (listener.is_some()
                && matches!(state, super::State::Bound | super::State::Listening))
        {
            None
        } else {
            Some(State::open(entry.raw as HANDLE)?)
        },
        listener,
        network,
        _network_pin: crate::namespaces::pin_network(network)?,
        socket_type,
        state,
        handle: entry.raw,
        local: addresses.remove(0),
        peer: addresses.remove(0),
        owns_file: flags & 1 != 0,
        inode,
        read_shut: flags & 2 != 0,
        write_shut: flags & 4 != 0,
        is_server_end: flags & 8 != 0,
    };
    sockets().lock().map_err(|_| EIO)?.insert(fd, socket);
    Ok(())
}

pub(super) fn set_peer_credentials(
    handle: HANDLE,
    server: &credentials::Sender,
    client: &credentials::Sender,
) -> Result<(), i32> {
    let mut bytes = smallvec::SmallVec::<[u8; 128]>::new();
    server.write(&mut bytes);
    client.write(&mut bytes);
    attribute(handle, b"kinakaze.peers", Some(&bytes)).map(|_| ())
}
pub(super) fn peer_credentials(
    handle: HANDLE,
    server_end: bool,
) -> Result<credentials::Sender, i32> {
    let bytes = attribute(handle, b"kinakaze.peers", None)?;
    let mut input = crate::state_codec::Reader(&bytes);
    let server = credentials::Sender::read(&mut input)?;
    let client = credentials::Sender::read(&mut input)?;
    input.end()?;
    Ok(if server_end { client } else { server })
}
fn attribute(handle: HANDLE, name: &[u8], value: Option<&[u8]>) -> Result<Vec<u8>, i32> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtFsControlFile(
            file: HANDLE,
            event: HANDLE,
            apc: *const c_void,
            context: *const c_void,
            io: *mut IoStatusBlock,
            code: u32,
            input: *const c_void,
            input_len: u32,
            output: *mut c_void,
            output_len: u32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }
    let mut input = smallvec::SmallVec::<[u8; 128]>::from_slice(name);
    input.push(0);
    if let Some(value) = value {
        input.extend_from_slice(value);
    }
    // Connection IDs and peer credentials fit in 128 bytes. Do not allocate
    // and clear 64 KiB on every connection/credential query. Larger attributes
    // still work: retry a non-consuming query with the original maximum size.
    let mut output = smallvec::SmallVec::from_buf([0u8; 128]);
    let event = IoEvent::new().ok_or(EIO)?;
    loop {
        let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
        operation.hEvent = event.0;
        let mut status = unsafe {
            NtFsControlFile(
                handle,
                event.0,
                std::ptr::null(),
                std::ptr::null(),
                (&raw mut operation).cast(),
                if value.is_some() { 0x110034 } else { 0x110030 },
                input.as_ptr().cast(),
                input.len() as u32,
                output.as_mut_ptr().cast(),
                output.len() as u32,
            )
        };
        if status == 0x103 {
            unsafe {
                wait_interruptible(handle, &raw mut operation, event.0)?;
            }
            status = operation.Internal as i32;
        }
        if status as u32 == 0xc0000225 {
            return Err(crate::ENOENT);
        }
        if value.is_none()
            && matches!(status as u32, 0x80000005 | 0xc0000023)
            && output.len() < 65536
        {
            output.resize(65536, 0);
            continue;
        }
        if status < 0 {
            #[cfg(test)]
            eprintln!(
                "pipe attribute status={status:x} name={:?}",
                String::from_utf8_lossy(name)
            );
            return Err(crate::errno_from_win32(unsafe {
                RtlNtStatusToDosError(status)
            }));
        }
        if operation.InternalHigh > output.len() {
            return Err(EIO);
        }
        output.truncate(operation.InternalHigh);
        return Ok(output.into_vec());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn credential_and_rights_queue_wire_survives_inline_buffer_spill() {
        use super::*;
        let store = crate::mount::shared::new_object().unwrap();
        for namespace_count in [1, 2, 32] {
            let mut identity = Vec::new();
            for word in [40001, 41002, namespace_count] {
                crate::state_codec::word(&mut identity, word);
            }
            for index in 0..namespace_count {
                crate::state_codec::word(&mut identity, namespace_count - index);
                crate::state_codec::word(&mut identity, index + 17);
            }
            for metadata_len in [0, 4097] {
                let mut expected = Vec::new();
                let record_count = u64::from(metadata_len != 0);
                // Saved wire layout, including one rights record when present.
                for word in [19, 7, 0, 0, record_count] {
                    crate::state_codec::word(&mut expected, word);
                }
                if record_count != 0 {
                    for word in [7, 12, 42, 99, 5, 6, 1, 123, 4, 3, 456] {
                        crate::state_codec::word(&mut expected, word);
                    }
                    let metadata: Vec<_> = (0..metadata_len).map(|n| (n * 17) as u8).collect();
                    crate::state_codec::bytes(&mut expected, &metadata);
                }
                for word in [0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 7, 19] {
                    crate::state_codec::word(&mut expected, word);
                }
                expected.extend_from_slice(&identity);
                store.replace(&expected).unwrap();
                let revision = store.revision();
                let queue = Queue::read(&store).unwrap();
                queue.write(&store).unwrap();
                assert_eq!(store.read().unwrap().1, expected);
                assert_eq!(store.revision(), revision);
            }
        }
    }

    #[test]
    fn rights_metadata_preserves_proc_socket_identity_after_sender_close() {
        use super::*;
        let (left, right) = socketpair(SOCK_STREAM).unwrap();
        let id = super::super::procnet::inode(left).unwrap();
        let payload = export_socket(&snapshot(left).unwrap(), Ok).unwrap();
        let source = get(left).unwrap();
        let raw = Object::duplicate(source.raw as HANDLE).unwrap();
        let received =
            crate::install_duplicate(raw.raw() as usize, source.kind, source.flags, source)
                .unwrap();
        let _ = raw.into_raw();
        import_socket(received, &payload, |handle| {
            Object::duplicate(handle as HANDLE)
        })
        .unwrap();
        crate::close(left).unwrap();
        assert_eq!(super::super::procnet::inode(received).unwrap(), id);
        crate::write(received, b"retained").unwrap();
        let mut bytes = [0; 8];
        assert_eq!(crate::read(right, &mut bytes), Ok(8));
        assert_eq!(&bytes, b"retained");
        crate::close(received).unwrap();
        crate::close(right).unwrap();
    }
    use super::*;
    #[test]
    fn async_write_space_notifies_once_after_backpressure_and_can_be_disabled() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "sysv64" fn handler(_: i32) {
            CALLS.fetch_add(1, Ordering::SeqCst);
        }
        let _serialized = signal::test_lock();
        let old_mask = signal::swap_blocked_mask(0);
        let old_action = signal::sigaction(
            29,
            Some(signal::Action {
                disposition: signal::Disposition::Handle(handler, 0),
                ..signal::Action::default()
            }),
        )
        .unwrap();
        CALLS.store(0, Ordering::SeqCst);
        let (a, b) = socketpair(SOCK_STREAM | SOCK_NONBLOCK).unwrap();
        set_owner(a, crate::job::process_id() as i32).unwrap();
        set_async(a, true).unwrap();
        let bytes = [1; 4096];
        let mut full = false;
        for _ in 0..1024 {
            match crate::write(a, &bytes) {
                Ok(_) => {}
                Err(EAGAIN) => {
                    full = true;
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(full);
        signal::deliver_pending();
        assert_eq!(CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(crate::read(b, &mut [0; 1]), Ok(1));
        signal::deliver_pending();
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(crate::read(b, &mut [0; 1]), Ok(1));
        signal::deliver_pending();
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        set_async(a, false).unwrap();
        super::super::shutdown(b, SHUT_RDWR).unwrap();
        signal::deliver_pending();
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        crate::close(a).unwrap();
        crate::close(b).unwrap();
        signal::sigaction(29, Some(old_action)).unwrap();
        signal::swap_blocked_mask(old_mask);
    }
    #[test]
    fn process_signal_interrupts_a_contended_connection_mutex() {
        let _serialized = signal::test_lock();
        unsafe extern "sysv64" fn handler(_: i32) {}
        let old = signal::sigaction(
            signal::SIGUSR1,
            Some(signal::Action {
                disposition: signal::Disposition::Handle(handler, 0),
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        for pending_before_wait in [true, false] {
            let (a, b) = socketpair(SOCK_STREAM).unwrap();
            let state = snapshot(a).unwrap().ancillary.unwrap();
            let held = Lock::new(&state.locks[0].tx).unwrap();
            let other = state.clone();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let waiter = std::thread::spawn(move || {
                let mask = signal::sigprocmask(signal::SIG_SETMASK, 0).unwrap();
                if pending_before_wait {
                    signal::raise_signal(signal::SIGUSR1).unwrap();
                }
                ready_tx.send(()).unwrap();
                let result = Lock::acquire(&other.locks[0].tx, false, true).map(|_| ());
                signal::sigprocmask(signal::SIG_SETMASK, mask).unwrap();
                done_tx.send(result).unwrap();
            });
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            if !pending_before_wait {
                // Let the waiter park; the timeout below checks wakeup while
                // this thread still owns the mutex, not scheduling latency.
                std::thread::sleep(std::time::Duration::from_millis(20));
                signal::raise_signal(signal::SIGUSR1).unwrap();
            }
            let result = done_rx.recv_timeout(std::time::Duration::from_secs(2));
            drop(held);
            waiter.join().unwrap();
            crate::close(a).unwrap();
            crate::close(b).unwrap();
            assert_eq!(result.unwrap(), Err(EINTR));
        }
        signal::sigaction(signal::SIGUSR1, Some(old)).unwrap();
    }

    #[test]
    fn restored_connection_mutexes_share_ownership_and_wake_on_release() {
        let (a, b) = socketpair(SOCK_STREAM).unwrap();
        let original = snapshot(a).unwrap().ancillary.unwrap();
        let reopened = State::restore([original.queues[0].id(), original.queues[1].id()]).unwrap();
        let held = Lock::new(&original.locks[0].tx).unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            assert!(matches!(
                Lock::acquire(&reopened.locks[0].tx, true, true),
                Err(EAGAIN)
            ));
            ready_tx.send(()).unwrap();
            let _acquired = Lock::acquire(&reopened.locks[0].tx, false, true).unwrap();
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        drop(held);
        waiter.join().unwrap();
        crate::close(a).unwrap();
        crate::close(b).unwrap();
    }
    #[test]
    fn connection_attributes_are_shared_and_do_not_enter_payload() {
        let (a, b) = socketpair(SOCK_STREAM).unwrap();
        let ah = get(a).unwrap().raw as HANDLE;
        let bh = get(b).unwrap().raw as HANDLE;
        attribute(ah, b"KinakazeTest", Some(b"value")).unwrap();
        assert_eq!(attribute(bh, b"KinakazeTest", None).unwrap(), b"value");
        for length in [127, 128, 129, 8192] {
            let value: Vec<_> = (0..length).map(|index| (index * 17) as u8).collect();
            attribute(ah, b"KinakazeTest", Some(&value)).unwrap();
            assert_eq!(attribute(bh, b"KinakazeTest", None).unwrap(), value);
        }
        // NPFS treats an empty value as deletion.
        attribute(ah, b"KinakazeTest", Some(&[])).unwrap();
        assert_eq!(attribute(bh, b"KinakazeTest", None), Err(crate::ENOENT));
        assert_eq!(
            unsafe { super::super::send(a, b"data".as_ptr(), 4, 0) },
            Ok(4)
        );
        let mut bytes = [0; 4];
        assert_eq!(
            unsafe { super::super::recv(b, bytes.as_mut_ptr(), 4, 0) },
            Ok(4)
        );
        assert_eq!(&bytes, b"data");
        crate::close(a).unwrap();
        crate::close(b).unwrap();
    }
}
