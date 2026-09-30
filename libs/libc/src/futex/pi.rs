//! PI futex ownership and waiters in the same crash-recoverable domain bank.
//! Native identities include thread creation time; guest TIDs are resolved in
//! their PID namespace. No pointer or owning handle is published cross-process.

use super::*;
use kinakaze_vfs::{EINVAL, EPERM};
const EDEADLK: i32 = 35;
use windows_sys::Win32::System::Threading::{
    GetPriorityClass, GetThreadPriority, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    SetThreadPriority, THREAD_SET_INFORMATION,
};

pub(super) const WAIT: u32 = 1 << 30;
pub(super) const STATE: u32 = 1 << 29;
pub(super) const JOURNAL: u32 = 1 << 28;
const DEAD: u32 = 1 << 27;
const WAITERS: u32 = 1 << 31;
const OWNER_DIED: u32 = 1 << 30;
const TID_MASK: u32 = OWNER_DIED - 1;
const ESRCH: i32 = 3;
const TASK_CAPACITY: usize = 4096;
const TASK_MAGIC: u64 = u64::from_le_bytes(*b"CYFPI001");

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Identity {
    born: u64,
    host: u32,
    thread: u32,
}

impl Identity {
    fn current() -> Result<Self, i32> {
        Ok(Self {
            born: current_thread_birth()?,
            host: std::process::id(),
            thread: unsafe { GetCurrentThreadId() },
        })
    }
    fn record(self, key: Key, token: u64, tid: u32, role: u32) -> Record {
        Record {
            key,
            token,
            born: self.born,
            host: self.host,
            thread: self.thread,
            bitset: tid,
            reserved: role,
        }
    }
    fn handle(self, access: u32) -> Result<Handle, i32> {
        let handle = Handle::new(unsafe {
            OpenThread(
                access | THREAD_QUERY_LIMITED_INFORMATION | THREAD_SYNCHRONIZE,
                0,
                self.thread,
            )
        })
        .map_err(|_| {
            if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
                ESRCH
            } else {
                EPERM
            }
        })?;
        if unsafe { GetProcessIdOfThread(handle.0) } != self.host
            || thread_birth(handle.0)? != self.born
            || unsafe { WaitForSingleObject(handle.0, 0) } == WAIT_OBJECT_0
        {
            return Err(ESRCH);
        }
        Ok(handle)
    }
}

impl From<Record> for Identity {
    fn from(record: Record) -> Self {
        Self {
            born: record.born,
            host: record.host,
            thread: record.thread,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Task {
    identity: Identity,
    namespace: u64,
    tid: u32,
    base: i32,
    applied: i32,
    class: i32,
}

impl Task {
    fn new(identity: Identity, namespace: u64, tid: u32) -> Result<Self, i32> {
        let handle = identity.handle(0)?;
        let base = unsafe { GetThreadPriority(handle.0) };
        if base == i32::MAX {
            return Err(EPERM);
        }
        let process = Handle::new(unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, identity.host)
        })?;
        let class = match unsafe { GetPriorityClass(process.0) } {
            0x40 => 4,
            0x4000 => 6,
            0x20 => 8,
            0x8000 => 10,
            0x80 => 13,
            0x100 => 24,
            _ => return Err(EPERM),
        };
        Ok(Self {
            identity,
            namespace,
            tid,
            base,
            applied: base,
            class,
        })
    }
    fn absolute(self, relative: i32) -> i32 {
        match relative {
            -15 => {
                if self.class == 24 {
                    16
                } else {
                    1
                }
            }
            15 => {
                if self.class == 24 {
                    31
                } else {
                    15
                }
            }
            _ => self.class + relative,
        }
    }
    fn relative(self, floor: i32) -> i32 {
        [-15, -2, -1, 0, 1, 2, 15]
            .into_iter()
            .filter(|&level| {
                self.absolute(level) >= floor && self.absolute(level) >= self.absolute(self.base)
            })
            .min_by_key(|&level| self.absolute(level))
            .unwrap_or(15)
    }
}

#[repr(C)]
struct TaskBank {
    count: u64,
    tasks: [Task; TASK_CAPACITY],
}
const TASK_SIZE: usize = size_of::<Header>() + 2 * size_of::<TaskBank>();
struct Tasks {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _section: Handle,
}
impl Drop for Tasks {
    fn drop(&mut self) {
        unsafe { UnmapViewOfFile(self.view) };
    }
}
static TASKS: AtomicPtr<Tasks> = AtomicPtr::new(ptr::null_mut());

impl Tasks {
    /// The caller holds the common futex domain guard throughout access.
    fn shared() -> Result<&'static Self, i32> {
        let pointer = TASKS.load(Ordering::Acquire);
        if !pointer.is_null() {
            return Ok(unsafe { &*pointer });
        }
        let domain = kinakaze_runtime::authority::domain_id();
        let section = Handle::new(unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                TASK_SIZE as u32,
                wide(&format!(r"Local\kinakaze.futex.pi.tasks.v1.{domain:016x}")).as_ptr(),
            )
        })?;
        let view = unsafe { MapViewOfFile(section.0, FILE_MAP_ALL_ACCESS, 0, 0, TASK_SIZE) };
        if view.Value.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let tasks = Box::new(Self {
            view,
            _section: section,
        });
        let header = view.Value.cast::<Header>();
        unsafe {
            if (*header).magic == 0 {
                ptr::addr_of_mut!((*header).size).write(TASK_SIZE as u64);
                ptr::addr_of_mut!((*header).magic).write(TASK_MAGIC);
            }
            if (*header).magic != TASK_MAGIC || (*header).size != TASK_SIZE as u64 {
                return Err(EIO);
            }
        }
        let pointer = Box::into_raw(tasks);
        // Initialization is serialized by the domain mutex.
        TASKS.store(pointer, Ordering::Release);
        Ok(unsafe { &*pointer })
    }
    fn header(&self) -> &Header {
        unsafe { &*self.view.Value.cast::<Header>() }
    }
    fn bank(&self, index: u32) -> *mut TaskBank {
        unsafe {
            self.view
                .Value
                .cast::<u8>()
                .add(size_of::<Header>() + index as usize * size_of::<TaskBank>())
                .cast()
        }
    }
    fn load(&self) -> Result<Vec<Task>, i32> {
        let active = self.header().active.load(Ordering::Acquire);
        if active > 1 {
            return Err(EIO);
        }
        let bank = self.bank(active);
        let count = unsafe { (*bank).count as usize };
        if count > TASK_CAPACITY {
            return Err(EIO);
        }
        Ok(unsafe {
            std::slice::from_raw_parts(ptr::addr_of!((*bank).tasks).cast::<Task>(), count)
        }
        .to_vec())
    }
    fn commit(&self, tasks: &[Task]) {
        assert!(tasks.len() <= TASK_CAPACITY);
        let next = 1 - self.header().active.load(Ordering::Acquire);
        let bank = self.bank(next);
        unsafe {
            ptr::copy_nonoverlapping(
                tasks.as_ptr(),
                ptr::addr_of_mut!((*bank).tasks).cast(),
                tasks.len(),
            );
            ptr::addr_of_mut!((*bank).count).write(tasks.len() as u64);
        }
        self.header().active.store(next, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
struct Registered {
    identity: Identity,
    tid: u32,
    domain: u64,
}
struct Participant(Cell<Option<Registered>>);
impl Drop for Participant {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            retire(task);
        }
    }
}
thread_local! { static PARTICIPANT: Participant = const { Participant(Cell::new(None)) }; }

fn namespace() -> u64 {
    kinakaze_runtime::job::namespaces::memberships(kinakaze_runtime::job::current_pid())
        .map_or(1, |ns| ns[4])
}

pub(crate) fn register_current(tid: i32) -> Result<(), i32> {
    let host = std::process::id();
    let thread = unsafe { GetCurrentThreadId() };
    let domain = kinakaze_runtime::authority::domain_id();
    if PARTICIPANT
        .try_with(|slot| {
            slot.0.get().is_some_and(|task| {
                task.identity.host == host
                    && task.identity.thread == thread
                    && task.tid == tid as u32
                    && task.domain == domain
            })
        })
        .unwrap_or(false)
    {
        return Ok(());
    }
    let identity = Identity::current()?;
    let namespace = namespace();
    let _transaction = Transaction::begin()?;
    let shared = Tasks::shared()?;
    let mut tasks = shared.load()?;
    if !tasks.iter().any(|task| {
        task.identity == identity && task.namespace == namespace && task.tid == tid as u32
    }) {
        // Prune only when space is needed; failed handle access is not proof
        // of death. Record::dead compares creation time as well as native IDs.
        if tasks.len() == TASK_CAPACITY {
            tasks.retain(|task| !task.identity.record(Key([0; 5]), 0, 0, 0).dead());
        }
        if tasks.len() == TASK_CAPACITY {
            return Err(ENOMEM);
        }
        tasks.push(Task::new(identity, namespace, tid as u32)?);
        shared.commit(&tasks);
    }
    PARTICIPANT.with(|slot| {
        slot.0.set(Some(Registered {
            identity,
            tid: tid as u32,
            domain,
        }))
    });
    crate::sysadmin::robust::install_exit_hook();
    libpthread::install_priority_base_hook(priority_base);
    Ok(())
}

pub(crate) fn reset_after_fork() {
    TASKS.store(ptr::null_mut(), Ordering::Release);
    let _ = PARTICIPANT.try_with(|slot| slot.0.set(None));
}

pub(crate) fn exit_current() {
    let _ = PARTICIPANT.try_with(|slot| {
        if let Some(task) = slot.0.take() {
            retire(task);
        }
    });
}

fn notify(record: Record, domain: u64) -> Result<(), i32> {
    let event = Handle::new(unsafe {
        OpenEventW(EVENT_MODIFY_STATE, 0, record.event_name(domain).as_ptr())
    })?;
    if unsafe { SetEvent(event.0) } == 0 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(())
}

fn retire(task: Registered) {
    if task.identity.host != std::process::id()
        || task.domain != kinakaze_runtime::authority::domain_id()
    {
        return;
    }
    let Ok(mut transaction) = Transaction::begin() else {
        return;
    };
    transaction
        .records
        .retain(|record| !(record.pi_wait() && Identity::from(*record) == task.identity));
    for record in &mut transaction.records {
        if record.reserved & STATE != 0 && Identity::from(*record) == task.identity {
            record.reserved |= DEAD;
            record.token = 0;
        }
    }
    let keys: Vec<Key> = transaction
        .records
        .iter()
        .filter(|record| record.reserved & STATE != 0)
        .map(|record| record.key)
        .collect();
    for key in keys {
        remove_unused_state(&mut transaction, key);
    }
    transaction.dirty = true;
    // Notification precedes bank publication. A waiter interprets it only
    // after acquiring the same guard and observing the committed DEAD state.
    for record in &transaction.records {
        if record.pi_wait()
            && transaction.records.iter().any(|state| {
                state.key == record.key && state.reserved & (STATE | DEAD) == STATE | DEAD
            })
        {
            let _ = notify(*record, transaction.shared.domain);
        }
    }
    transaction.commit();
    if let Ok(shared) = Tasks::shared()
        && let Ok(mut tasks) = shared.load()
    {
        tasks.retain(|entry| entry.identity != task.identity);
        shared.commit(&tasks);
        let _ = priorities(&transaction, shared, &mut tasks);
    }
}

fn priorities(transaction: &Transaction, shared: &Tasks, tasks: &mut [Task]) -> Result<(), i32> {
    let mut effective: Vec<i32> = tasks.iter().map(|task| task.absolute(task.base)).collect();
    // Fixed-point propagation covers nested PI locks in either enqueue order.
    for _ in 0..tasks.len() {
        let mut changed = false;
        for waiter in transaction
            .records
            .iter()
            .filter(|record| record.pi_wait() && !record.dead())
        {
            let Some(state) = transaction.records.iter().find(|state| {
                state.key == waiter.key && state.reserved & STATE != 0 && state.reserved & DEAD == 0
            }) else {
                continue;
            };
            if state.token == waiter.token {
                continue;
            }
            let Some(owner) = tasks
                .iter()
                .position(|task| task.identity == Identity::from(*state))
            else {
                continue;
            };
            let Some(donor) = tasks
                .iter()
                .position(|task| task.identity == Identity::from(*waiter))
            else {
                continue;
            };
            if effective[owner] < effective[donor] {
                effective[owner] = effective[donor];
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (index, task) in tasks.iter_mut().enumerate() {
        let desired = task.relative(effective[index]);
        task.applied = desired;
    }
    // Persist intended priority before touching the native scheduler. Replaying
    // the intended value never treats a former inherited boost as base priority.
    shared.commit(tasks);
    for task in tasks {
        match task.identity.handle(THREAD_SET_INFORMATION) {
            Ok(handle) if unsafe { GetThreadPriority(handle.0) } == task.applied => {}
            Ok(handle) if unsafe { SetThreadPriority(handle.0, task.applied) } != 0 => {}
            Err(ESRCH) => {}
            _ => return Err(EPERM),
        }
    }
    Ok(())
}

extern "sysv64" fn priority_base(thread: u32, base: i32) -> i32 {
    let result = (|| {
        let transaction = Transaction::begin()?;
        let shared = Tasks::shared()?;
        let mut tasks = shared.load()?;
        if let Some(task) = tasks
            .iter_mut()
            .find(|task| task.identity.host == std::process::id() && task.identity.thread == thread)
        {
            task.base = base;
            // Force scheduler replay even if the inherited value is unchanged.
            task.applied = i32::MAX;
            priorities(&transaction, shared, &mut tasks)?;
        } else {
            let handle = Handle::new(unsafe { OpenThread(THREAD_SET_INFORMATION, 0, thread) })?;
            if unsafe { SetThreadPriority(handle.0, base) } == 0 {
                return Err(EPERM);
            }
        }
        Ok(())
    })();
    result.map_or_else(|error| error, |()| 0)
}

fn owner(tasks: &mut Vec<Task>, namespace: u64, tid: u32) -> Result<Task, i32> {
    for task in tasks
        .iter()
        .filter(|task| task.namespace == namespace && task.tid == tid)
        .copied()
    {
        match task.identity.handle(0) {
            Ok(_) => return Ok(task),
            Err(ESRCH) => {}
            Err(error) => return Err(error),
        }
    }
    // A native pthread may use its TCB's cached TID without calling gettid.
    // Only resolve that fallback inside our own host process; a foreign guest
    // process leader's namespace PID is never passed blindly to OpenThread.
    let thread = crate::fsextra::native_signal_tid(tid as i32);
    let handle = Handle::new(unsafe {
        OpenThread(
            THREAD_QUERY_LIMITED_INFORMATION | THREAD_SYNCHRONIZE,
            0,
            thread,
        )
    })
    .map_err(|_| ESRCH)?;
    if unsafe { GetProcessIdOfThread(handle.0) } != std::process::id() {
        return Err(ESRCH);
    }
    let identity = Identity {
        born: thread_birth(handle.0)?,
        host: std::process::id(),
        thread,
    };
    if tasks.len() == TASK_CAPACITY {
        return Err(ENOMEM);
    }
    let task = Task::new(identity, namespace, tid)?;
    tasks.push(task);
    Ok(task)
}

fn state(transaction: &Transaction, key: Key) -> Option<Record> {
    transaction
        .records
        .iter()
        .copied()
        .find(|record| record.key == key && record.reserved & STATE != 0)
}

fn top(transaction: &Transaction, key: Key) -> Option<Record> {
    transaction
        .records
        .iter()
        .copied()
        .filter(|record| record.key == key && !record.metadata() && !record.dead())
        // All admitted guest scheduling policies are OTHER/0. Linux clamps
        // non-RT futex queue priorities to the same band and keeps FIFO order.
        // Native donation below does not invent Linux FIFO/RR admission.
        .min_by_key(|record| record.token)
}

fn cycle(transaction: &Transaction, waiting: Identity, mut owner: Identity) -> bool {
    for _ in 0..=transaction.records.len() {
        if owner == waiting {
            return true;
        }
        let Some(waiter) = transaction
            .records
            .iter()
            .find(|record| record.pi_wait() && Identity::from(**record) == owner)
        else {
            return false;
        };
        let Some(state) = state(transaction, waiter.key) else {
            return false;
        };
        if state.token == waiter.token || state.reserved & DEAD != 0 {
            return false;
        }
        owner = Identity::from(state);
    }
    true
}

fn remove_unused_state(transaction: &mut Transaction, key: Key) {
    if !transaction
        .records
        .iter()
        .any(|record| record.key == key && record.pi_wait())
    {
        transaction
            .records
            .retain(|record| !(record.key == key && record.reserved & (STATE | JOURNAL) != 0));
        transaction.dirty = true;
    }
}

fn prune(transaction: &mut Transaction, key: Key) {
    let journal = transaction
        .records
        .iter()
        .find(|record| record.key == key && record.reserved & JOURNAL != 0)
        .map(|record| record.token);
    let before = transaction.records.len();
    transaction.records.retain(|record| {
        !(record.key == key && record.pi_wait() && Some(record.token) != journal && record.dead())
    });
    transaction.dirty |= before != transaction.records.len();
}

/// A journal is published before the user-word CAS. A killed unlocker cannot
/// lose its selected waiter: the next caller completes the recorded transition
/// through its own alias and signals the already-existing named park event.
fn finish_journal(transaction: &mut Transaction, key: Key, address: usize) -> Result<(), i32> {
    let Some(journal) = transaction
        .records
        .iter()
        .copied()
        .find(|record| record.key == key && record.reserved & JOURNAL != 0)
    else {
        return Ok(());
    };
    let waiter = transaction
        .records
        .iter()
        .copied()
        .find(|record| record.pi_wait() && record.token == journal.token)
        .ok_or(EINVAL)?;
    let desired = (journal.bitset & OWNER_DIED)
        | if journal.reserved & DEAD != 0 {
            OWNER_DIED
        } else {
            0
        }
        | WAITERS
        | waiter.bitset;
    #[cfg(test)]
    tests::crash_at("before-cas");
    let observed = atomic_word::compare_exchange(address, journal.bitset, desired)?;
    if observed != journal.bitset && observed != desired {
        return Err(EINVAL);
    }
    #[cfg(test)]
    tests::crash_at("after-cas");
    if !waiter.dead() {
        notify(waiter, transaction.shared.domain)?;
    }
    for record in transaction.records.iter().filter(|record| {
        record.key == key && record.pi_wait() && record.token != waiter.token && !record.dead()
    }) {
        notify(*record, transaction.shared.domain)?;
    }
    let mut found = false;
    for record in &mut transaction.records {
        if record.key == key && record.reserved & STATE != 0 {
            *record = Identity::from(waiter).record(key, waiter.token, waiter.bitset, STATE);
            found = true;
        }
    }
    if !found {
        return Err(EINVAL);
    }
    transaction
        .records
        .retain(|record| !(record.key == key && record.reserved & JOURNAL != 0));
    transaction.dirty = true;
    transaction.commit();
    #[cfg(test)]
    tests::crash_at("after-publication");
    Ok(())
}

fn grant(
    transaction: &mut Transaction,
    key: Key,
    address: usize,
    old: u32,
    waiter: Record,
) -> Result<(), i32> {
    #[cfg(test)]
    tests::crash_at("before-journal");
    transaction.reserve(1)?;
    transaction
        .records
        .push(Identity::from(waiter).record(key, waiter.token, old, JOURNAL));
    transaction.dirty = true;
    transaction.commit();
    finish_journal(transaction, key, address)
}

pub(crate) enum Acquisition {
    Owned,
    Queued(u64),
}

pub(crate) struct OwnerHandle(Handle);
impl OwnerHandle {
    pub(crate) fn raw(&self) -> HANDLE {
        self.0.0
    }
}
pub(crate) fn owner_handle(transaction: &Transaction, key: Key) -> Result<OwnerHandle, i32> {
    let owner = state(transaction, key).ok_or(EINVAL)?;
    Identity::from(owner).handle(0).map(OwnerHandle)
}

pub(crate) fn cancel_error(transaction: &mut Transaction, key: Key, token: u64) {
    transaction.cancel(token);
    for record in &mut transaction.records {
        if record.key == key && record.reserved & STATE != 0 && record.token == token {
            record.reserved |= DEAD;
            record.token = 0;
        }
    }
    remove_unused_state(transaction, key);
    transaction.dirty = true;
    for record in transaction
        .records
        .iter()
        .filter(|record| record.key == key && record.pi_wait())
    {
        let _ = notify(*record, transaction.shared.domain);
    }
    transaction.commit();
    if let Ok(shared) = Tasks::shared()
        && let Ok(mut tasks) = shared.load()
    {
        let _ = priorities(transaction, shared, &mut tasks);
    }
}

pub(crate) fn acquire(
    transaction: &mut Transaction,
    key: Key,
    address: usize,
    tid: u32,
    try_only: bool,
) -> Result<Acquisition, i32> {
    let identity = Identity::current()?;
    finish_journal(transaction, key, address)?;
    prune(transaction, key);
    loop {
        let old = atomic_word::read(address)?;
        if old & TID_MASK == tid {
            return Err(EDEADLK);
        }
        let queued = top(transaction, key);
        if queued.is_some_and(|record| !record.pi_wait()) {
            return Err(EINVAL);
        }
        if queued.is_none() {
            remove_unused_state(transaction, key);
            if old & TID_MASK == 0 {
                let desired = (old & OWNER_DIED) | tid;
                if atomic_word::compare_exchange(address, old, desired)? != old {
                    continue;
                }
                return Ok(Acquisition::Owned);
            }
        }
        if atomic_word::compare_exchange(address, old, old | WAITERS)? != old {
            continue;
        }
        let old = old | WAITERS;
        let shared = Tasks::shared()?;
        let mut tasks = shared.load()?;
        let existing = state(transaction, key);
        let owner = if let Some(existing) = existing {
            if existing.reserved & DEAD == 0
                && existing.bitset != old & TID_MASK
                && !(old & TID_MASK == 0 && old & OWNER_DIED != 0)
            {
                return Err(EINVAL);
            }
            if existing.reserved & DEAD != 0 || existing.dead() {
                if try_only {
                    return Err(EAGAIN);
                }
                Identity::from(existing)
            } else {
                Identity::from(existing)
            }
        } else {
            let owner = owner(&mut tasks, namespace(), old & TID_MASK)?;
            shared.commit(&tasks);
            transaction.reserve(1)?;
            transaction
                .records
                .push(owner.identity.record(key, 0, owner.tid, STATE));
            transaction.dirty = true;
            owner.identity
        };
        if cycle(transaction, identity, owner) {
            remove_unused_state(transaction, key);
            return Err(EDEADLK);
        }
        if try_only {
            remove_unused_state(transaction, key);
            priorities(transaction, shared, &mut tasks)?;
            return Err(EAGAIN);
        }
        transaction.reserve(1)?;
        let park = park_identity()?;
        let token = transaction.append_private(key, park, tid);
        let record = transaction.records.last_mut().unwrap();
        record.reserved |= WAIT;
        // Publish the donor before changing a native priority. If this process
        // dies during donation, the owner's next unlock can reclaim this dead
        // queue entry and restore its base; an unpublished boost cannot linger.
        transaction.commit();
        if let Err(error) = priorities(transaction, shared, &mut tasks) {
            transaction.cancel(token);
            remove_unused_state(transaction, key);
            transaction.commit();
            let _ = priorities(transaction, shared, &mut tasks);
            return Err(error);
        }
        transaction.commit();
        return Ok(Acquisition::Queued(token));
    }
}

pub(crate) fn unlock(
    transaction: &mut Transaction,
    key: Key,
    address: usize,
    tid: u32,
) -> Result<(), i32> {
    finish_journal(transaction, key, address)?;
    prune(transaction, key);
    let old = atomic_word::read(address)?;
    if old & TID_MASK != tid {
        return Err(EPERM);
    }
    let had_state = state(transaction, key).is_some();
    let queued = top(transaction, key);
    if let Some(next) = queued {
        if !next.pi_wait() {
            return Err(EINVAL);
        }
        let state = state(transaction, key).ok_or(EINVAL)?;
        if state.bitset != tid || Identity::from(state) != Identity::current()? {
            return Err(EINVAL);
        }
        grant(transaction, key, address, old, next)?;
    } else {
        if atomic_word::compare_exchange(address, old, 0)? != old {
            return Err(EAGAIN);
        }
        remove_unused_state(transaction, key);
        transaction.commit();
    }
    if had_state {
        let shared = Tasks::shared()?;
        let mut tasks = shared.load()?;
        priorities(transaction, shared, &mut tasks)?;
    }
    Ok(())
}

/// Ownership wins timeout/signal; cancellation removes only a queued waiter.
/// Dead-owner fixup accesses the waiter's alias, never the exited owner's VA.
pub(crate) fn poll(
    transaction: &mut Transaction,
    key: Key,
    address: usize,
    token: u64,
    cancel: bool,
) -> Result<bool, i32> {
    finish_journal(transaction, key, address)?;
    prune(transaction, key);
    let shared = Tasks::shared()?;
    let mut tasks = shared.load()?;
    let owner = state(transaction, key).ok_or(EINVAL)?;
    if owner.token == token {
        transaction.cancel(token);
        for record in &mut transaction.records {
            if record.key == key && record.reserved & STATE != 0 {
                record.token = 0;
            }
        }
        remove_unused_state(transaction, key);
        transaction.commit();
        priorities(transaction, shared, &mut tasks)?;
        return Ok(true);
    }
    if owner.reserved & DEAD != 0 || owner.dead() {
        let next = top(transaction, key).ok_or(EINVAL)?;
        if !next.pi_wait() {
            return Err(EINVAL);
        }
        let old = atomic_word::read(address)?;
        if old & TID_MASK != owner.bitset && !(old & TID_MASK == 0 && old & OWNER_DIED != 0) {
            return Err(EINVAL);
        }
        // Preserve the old value in the CAS expectation; OWNER_DIED belongs to
        // the desired value only, even for non-robust contended owner exit.
        transaction.reserve(1)?;
        let mut journal = Identity::from(next).record(key, next.token, old, JOURNAL | DEAD);
        journal.bitset = old;
        transaction.records.push(journal);
        transaction.dirty = true;
        transaction.commit();
        finish_journal(transaction, key, address)?;
        return poll(transaction, key, address, token, cancel);
    }
    if cancel {
        transaction.cancel(token);
        remove_unused_state(transaction, key);
        transaction.commit();
        priorities(transaction, shared, &mut tasks)?;
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
