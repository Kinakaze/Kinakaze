//! Native UTS, IPC, network, cgroup and PID namespace objects.
//! Descriptors pin the object and shared registry rows publish membership.
use crate::mount::shared::{self, Store};
use crate::{EINVAL, EIO, EPERM, FdFlags, FdKind};
use kinakaze_runtime::job::namespaces as registry;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub use registry::{CGROUP, IPC, NET, PID, PID_CHILDREN, UTS};
pub const FLAGS: [u32; 5] = [0x04000000, 0x08000000, 0x40000000, 0x02000000, 0x20000000];
const NAMES: [&str; 6] = ["uts", "ipc", "net", "cgroup", "pid", "pid_for_children"];
const MAGIC: &[u8; 8] = b"CRYNS001";
static OBJECTS: Mutex<BTreeMap<(usize, u64), Arc<Store>>> = Mutex::new(BTreeMap::new());
// Linux attaches network namespaces to tasks. Go's netns package consequently
// locks one OS thread, enters a namespace, opens a netlink/socket descriptor and
// restores that thread without stopping the process's other threads. The shared
// process registry remains the inherited default; this override models the
// calling task and lets each newly opened descriptor pin the namespace it saw.
thread_local! { static NET_OVERRIDE: Cell<u64> = const { Cell::new(0) }; }

#[derive(Clone)]
struct State {
    kind: usize,
    id: u64,
    owner: u64,
    parent: u64,
    data: Vec<u8>,
}
impl State {
    fn encode(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        for word in [self.kind as u64, self.id, self.owner, self.parent] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&self.data);
        out
    }
    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() < 40 || &bytes[..8] != MAGIC {
            return Err(EIO);
        }
        let word = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        let kind = word(8) as usize;
        if kind >= 5 || word(16) == 0 {
            return Err(EIO);
        }
        Ok(Self {
            kind,
            id: word(16),
            owner: word(24),
            parent: word(32),
            data: bytes[40..].to_vec(),
        })
    }
}
pub fn kind(name: &str) -> Option<usize> {
    NAMES.iter().position(|value| *value == name)
}
pub fn name(kind: usize) -> &'static str {
    NAMES[kind]
}
fn object_kind(kind: usize) -> usize {
    if kind == PID_CHILDREN { PID } else { kind }
}
pub fn memberships(pid: u32) -> Result<[u64; 6], i32> {
    registry::memberships(pid).ok_or(crate::ESRCH)
}
pub fn current_id(kind: usize) -> Result<u64, i32> {
    if kind == NET {
        let id = NET_OVERRIDE.get();
        if id != 0 {
            return Ok(id);
        }
    }
    Ok(memberships(crate::job::process_id())?[kind])
}
pub fn process_id(pid: u32, kind: usize) -> Result<u64, i32> {
    Ok(memberships(pid)?[kind])
}
pub fn inode(kind: usize, id: u64) -> u64 {
    let kind = object_kind(kind);
    if id == 1 {
        [4026531838, 4026531839, 4026531992, 4026531835, 4026531836][kind]
    } else {
        (1u64 << 61) | id
    }
}
fn initial_data(kind: usize) -> Result<Vec<u8>, i32> {
    Ok(match kind {
        UTS => {
            let host = std::env::var("KINAKAZE_HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .unwrap_or_else(|_| "localhost".into());
            let mut data = host.into_bytes();
            data.truncate(64);
            data.push(0);
            data.extend_from_slice(b"(none)");
            data
        }
        CGROUP => b"/sys/fs/cgroup".to_vec(),
        NET => crate::route_state::encode_namespace(&crate::route_state::Network::default())?,
        _ => Vec::new(),
    })
}
fn open(kind: usize, id: u64) -> Result<Arc<Store>, i32> {
    let kind = object_kind(kind);
    if let Some(store) = OBJECTS.lock().map_err(|_| EIO)?.get(&(kind, id)).cloned() {
        return Ok(store);
    }
    // Initial objects need distinct keys from the initial user namespace.
    let key = if id == 1 { u64::MAX - kind as u64 } else { id };
    let store = Arc::new(Store::user_object(key, id == 1)?);
    if id == 1 && store.read()?.1.is_empty() {
        store.update(|old| {
            if !old.is_empty() {
                return Ok((old.to_vec(), ()));
            }
            Ok((
                State {
                    kind,
                    id,
                    owner: 1,
                    parent: 0,
                    data: initial_data(kind)?,
                }
                .encode(),
                (),
            ))
        })?;
    }
    let state = State::decode(&store.read()?.1)?;
    if state.kind != kind || state.id != id {
        return Err(EIO);
    }
    OBJECTS
        .lock()
        .map_err(|_| EIO)?
        .insert((kind, id), store.clone());
    Ok(store)
}
pub fn privileged() -> bool {
    crate::user_namespace::id(crate::job::process_id())
        .is_ok_and(|id| crate::user_namespace::capable(id, 21))
}
pub struct Prepared {
    values: [u64; 6],
    stores: Vec<(usize, Arc<Store>)>,
    thread_scoped: bool,
    committed: bool,
}
impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.committed {
            for (kind, store) in &self.stores {
                if *kind == PID {
                    registry::discard_empty_pid_namespace(store.id());
                }
            }
        }
    }
}
pub fn prepare(flags: u32) -> Result<Prepared, i32> {
    prepare_with_user(flags, None)
}
pub fn prepare_with_user(
    flags: u32,
    user: Option<&crate::user_namespace::Prepared>,
) -> Result<Prepared, i32> {
    if user.is_none() && !privileged() {
        return Err(EPERM);
    }
    let thread_scoped = kinakaze_runtime::process_thread_count() != 1;
    if thread_scoped && flags & !FLAGS[NET] != 0 {
        return Err(EINVAL);
    }
    let mut values = memberships(crate::job::process_id())?;
    if thread_scoped {
        values[NET] = current_id(NET)?;
    }
    if flags & FLAGS[PID] != 0 && values[PID] != values[PID_CHILDREN] {
        return Err(EINVAL);
    }
    let mut stores = Vec::new();
    for (kind, flag) in FLAGS.into_iter().enumerate() {
        if flags & flag == 0 {
            continue;
        }
        let old = State::decode(&open(kind, values[kind])?.read()?.1)?;
        let store = Arc::new(shared::new_object()?);
        let id = store.id();
        let data = match kind {
            UTS => old.data,
            CGROUP => kinakaze_runtime::job::cgroup_path(crate::job::process_id())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "/sys/fs/cgroup".into())
                .into_bytes(),
            NET => crate::route_state::encode_namespace(&crate::route_state::Network::default())?,
            _ => Vec::new(),
        };
        store.update(|_| {
            Ok((
                State {
                    kind,
                    id,
                    owner: match user {
                        Some(u) => u.id(),
                        None => crate::user_namespace::id(crate::job::process_id())?,
                    },
                    parent: values[kind],
                    data,
                }
                .encode(),
                (),
            ))
        })?;
        if kind == PID {
            registry::create_pid_namespace(id, values[PID])?;
            values[PID_CHILDREN] = id;
        } else {
            values[kind] = id;
        }
        if kind == NET {
            crate::usernet::register_namespace(id)?;
        }
        stores.push((kind, store));
    }
    Ok(Prepared {
        values,
        stores,
        thread_scoped,
        committed: false,
    })
}
impl Prepared {
    pub fn install(mut self) -> Result<(), i32> {
        let mut objects = OBJECTS.lock().map_err(|_| EIO)?;
        if self.thread_scoped {
            NET_OVERRIDE.set(self.values[NET]);
        } else if !registry::set_memberships(crate::job::process_id(), self.values) {
            return Err(EIO);
        }
        for (kind, store) in self.stores.drain(..) {
            objects.insert((kind, store.id()), store);
        }
        objects.retain(|(kind, id), _| *id == 1 || *kind == PID || self.values[*kind] == *id);
        self.committed = true;
        Ok(())
    }
}
pub fn open_process(pid: u32, kind: usize, flags: FdFlags) -> Result<i32, i32> {
    let id = if pid == crate::job::process_id() && kind == NET {
        current_id(NET)?
    } else {
        process_id(pid, kind)?
    };
    open(kind, id)?.descriptor_kind(FdKind::Namespace, flags)
}
fn descriptor(fd: i32) -> Result<(Store, State), i32> {
    if crate::get(fd)?.kind != FdKind::Namespace {
        return Err(EINVAL);
    }
    let store = shared::object_fd(fd)?;
    let state = State::decode(&store.read()?.1)?;
    Ok((store, state))
}
pub fn descriptor_inode(fd: i32) -> Result<u64, i32> {
    let (_, state) = descriptor(fd)?;
    Ok(inode(state.kind, state.id))
}
pub fn descriptor_link(fd: i32) -> Result<String, i32> {
    let (_, state) = descriptor(fd)?;
    Ok(format!(
        "{}:[{}]",
        name(state.kind),
        inode(state.kind, state.id)
    ))
}
pub fn descriptor_id(fd: i32, kind: usize) -> Result<u64, i32> {
    let (_, state) = descriptor(fd)?;
    if state.kind != object_kind(kind) {
        return Err(EINVAL);
    }
    Ok(state.id)
}
pub fn enter(fd: i32, flag: u32, before: impl FnOnce(usize) -> Result<(), i32>) -> Result<(), i32> {
    let (store, state) = descriptor(fd)?;
    if flag != 0 && flag != FLAGS[state.kind] {
        return Err(EINVAL);
    }
    if !crate::user_namespace::capable(state.owner, 21) {
        return Err(EPERM);
    }
    let thread_scoped = kinakaze_runtime::process_thread_count() != 1;
    if thread_scoped && state.kind != NET {
        return Err(EINVAL);
    }
    let mut values = memberships(crate::job::process_id())?;
    if thread_scoped {
        values[NET] = current_id(NET)?;
    }
    let kind = if state.kind == PID {
        if !registry::pid_descendant(state.id, values[PID]) {
            return Err(EINVAL);
        }
        PID_CHILDREN
    } else {
        state.kind
    };
    values[kind] = state.id;
    before(state.kind)?;
    OBJECTS
        .lock()
        .map_err(|_| EIO)?
        .insert((state.kind, state.id), Arc::new(store));
    if thread_scoped {
        NET_OVERRIDE.set(values[NET]);
    } else if !registry::set_memberships(crate::job::process_id(), values) {
        return Err(EIO);
    }
    Ok(())
}
pub fn uts(domain: bool) -> Result<Vec<u8>, i32> {
    let state = State::decode(&open(UTS, current_id(UTS)?)?.read()?.1)?;
    let split = state.data.iter().position(|b| *b == 0).ok_or(EIO)?;
    Ok(if domain {
        state.data[split + 1..].to_vec()
    } else {
        state.data[..split].to_vec()
    })
}
pub fn set_uts(domain: bool, value: &[u8]) -> Result<(), i32> {
    if value.len() > 64 {
        return Err(EINVAL);
    }
    let owner = State::decode(&open(UTS, current_id(UTS)?)?.read()?.1)?.owner;
    if !crate::user_namespace::capable(owner, 21) {
        return Err(EPERM);
    }
    let end = value.iter().position(|b| *b == 0).unwrap_or(value.len());
    open(UTS, current_id(UTS)?)?.update(|old| {
        let mut state = State::decode(old)?;
        let split = state.data.iter().position(|b| *b == 0).ok_or(EIO)?;
        state.data = if domain {
            [&state.data[..split + 1], &value[..end]].concat()
        } else {
            [&value[..end], &state.data[split..]].concat()
        };
        Ok((state.encode(), ()))
    })
}
pub fn cgroup_root() -> Result<String, i32> {
    String::from_utf8(State::decode(&open(CGROUP, current_id(CGROUP)?)?.read()?.1)?.data)
        .map_err(|_| EIO)
}
pub fn cgroup_relative(path: &str) -> Result<String, i32> {
    let path = if path.is_empty() {
        "/sys/fs/cgroup"
    } else {
        path
    };
    if path.contains(['\n', '\r', '\0'])
        || !(path == "/sys/fs/cgroup" || path.starts_with("/sys/fs/cgroup/"))
    {
        return Err(EIO);
    }
    let root = cgroup_root()?;
    let source: Vec<_> = root.split('/').filter(|p| !p.is_empty()).collect();
    let target: Vec<_> = path.split('/').filter(|p| !p.is_empty()).collect();
    let common = source
        .iter()
        .zip(&target)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts = vec![".."; source.len() - common];
    parts.extend_from_slice(&target[common..]);
    Ok(format!("/{}", parts.join("/")))
}
pub unsafe fn ioctl(fd: i32, request: u64, _argument: *mut u32) -> Result<i32, i32> {
    let (_, state) = descriptor(fd)?;
    match request {
        0xb703 => Ok(FLAGS[state.kind] as i32),
        0xb701 => crate::user_namespace::open_id(state.owner),
        0xb702 if state.kind == PID && state.parent != 0 => {
            open(PID, state.parent)?.descriptor_kind(FdKind::Namespace, FdFlags::CLOSE_ON_EXEC)
        }
        0xb702 => Err(EPERM),
        _ => Err(crate::ENOTTY),
    }
}
pub(crate) fn serialize() -> Result<Vec<u8>, i32> {
    let values = memberships(crate::job::process_id())?;
    for (kind, id) in values.into_iter().enumerate() {
        open(kind, id)?;
        if kind == PID || kind == PID_CHILDREN {
            let mut ancestor = id;
            while let Some(parent) = registry::pid_parent(ancestor) {
                open(PID, parent)?;
                ancestor = parent;
            }
        }
    }
    Ok(MAGIC.to_vec())
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    bytes == MAGIC && serialize().is_ok()
}

pub(crate) fn pin_network(id: u64) -> Result<Arc<Store>, i32> {
    if id == 1 {
        return open(NET, id);
    }
    let store = Arc::new(Store::user_object(id, false)?);
    let state = State::decode(&store.read()?.1)?;
    if state.kind != NET || state.id != id {
        return Err(EINVAL);
    }
    Ok(store)
}
pub(crate) fn network_data(id: u64) -> Result<Vec<u8>, i32> {
    Ok(State::decode(&pin_network(id)?.read()?.1)?.data)
}
pub(crate) fn network_update<T>(
    id: u64,
    action: impl FnOnce(&[u8]) -> Result<(Vec<u8>, T), i32>,
) -> Result<T, i32> {
    pin_network(id)?.update(|bytes| {
        let mut state = State::decode(bytes)?;
        let (data, value) = action(&state.data)?;
        state.data = data;
        Ok((state.encode(), value))
    })
}

pub(crate) fn network_owner(id: u64) -> Result<u64, i32> {
    Ok(State::decode(&pin_network(id)?.read()?.1)?.owner)
}

pub(crate) fn owner(kind: usize, id: u64) -> Result<u64, i32> {
    Ok(State::decode(&open(kind, id)?.read()?.1)?.owner)
}

/// The hidden mqueue superblock is owned by its IPC namespace, not a mount.
pub(crate) fn ipc_update<T>(
    id: u64,
    action: impl FnOnce(&[u8]) -> Result<(Vec<u8>, T), i32>,
) -> Result<T, i32> {
    open(IPC, id)?.update(|bytes| {
        let mut s = State::decode(bytes)?;
        let (data, out) = action(&s.data)?;
        s.data = data;
        Ok((s.encode(), out))
    })
}
