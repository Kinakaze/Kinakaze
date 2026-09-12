//! User namespace ID maps used by idmapped mounts. Maps are immutable after
//! their first write; mounts own a value copy, independent of the namespace fd.
use crate::mount::shared::{self, Store};
use crate::{EINVAL, EIO, EOVERFLOW, EPERM, FdFlags, FdKind};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
static ANCESTORS: Mutex<BTreeMap<u64, Arc<Store>>> = Mutex::new(BTreeMap::new());
static CAPS: Mutex<[u64; 3]> = Mutex::new([0; 3]);
pub const ALL_CAPS: u64 = (1u64 << 41) - 1;
pub fn capabilities() -> [u64; 3] {
    *CAPS.lock().unwrap_or_else(|p| p.into_inner())
}
pub fn set_capabilities(values: [u64; 3]) -> Result<(), i32> {
    let mut caps = CAPS.lock().map_err(|_| EIO)?;
    if values[0] & !values[1] != 0 || (!is_initial() && values[1] & !caps[1] != 0) {
        return Err(EPERM);
    }
    *caps = values.map(|v| v & ALL_CAPS);
    Ok(())
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Mapping {
    pub uid: Vec<[u32; 3]>,
    pub gid: Vec<[u32; 3]>,
}
impl Mapping {
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for map in [&self.uid, &self.gid] {
            bytes.extend_from_slice(&(map.len() as u32).to_le_bytes());
            for row in map {
                for word in row {
                    bytes.extend_from_slice(&word.to_le_bytes());
                }
            }
        }
        bytes
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, i32> {
        let mut at = 0;
        let mut word = || {
            let v = bytes.get(at..at + 4).ok_or(EIO)?;
            at += 4;
            Ok::<_, i32>(u32::from_le_bytes(v.try_into().unwrap()))
        };
        let mut maps = Vec::new();
        for _ in 0..2 {
            let count = word()?;
            if count > 340 {
                return Err(EIO);
            }
            let mut rows = Vec::new();
            for _ in 0..count {
                rows.push([word()?, word()?, word()?]);
            }
            validate(&rows)?;
            maps.push(rows);
        }
        drop(word);
        if at != bytes.len() {
            return Err(EIO);
        }
        Ok(Self {
            gid: maps.pop().unwrap(),
            uid: maps.pop().unwrap(),
        })
    }
    pub(crate) fn down(&self, value: u32, group: bool) -> Option<u32> {
        translate(if group { &self.gid } else { &self.uid }, value, false)
    }
    pub(crate) fn up(&self, value: u32, group: bool) -> Option<u32> {
        translate(if group { &self.gid } else { &self.uid }, value, true)
    }
    /// Linux map_id_range_up requires the complete range to fit one extent.
    /// A partially visible mount extent must be omitted from statmount output.
    pub(crate) fn range_up(&self, first: u32, count: u32, group: bool) -> Option<u32> {
        if count == 0 {
            return None;
        }
        let rows = if group { &self.gid } else { &self.uid };
        rows.iter().find_map(|row| {
            let offset = first.checked_sub(row[1])?;
            (u64::from(offset) + u64::from(count) <= u64::from(row[2]))
                .then(|| row[0].checked_add(offset))
                .flatten()
        })
    }
    pub(crate) fn stat(&self, stat: &mut crate::fs::Stat) {
        stat.st_uid = self.down(stat.st_uid, false).unwrap_or(65534);
        stat.st_gid = self.down(stat.st_gid, true).unwrap_or(65534);
    }
}
fn translate(rows: &[[u32; 3]], value: u32, reverse: bool) -> Option<u32> {
    rows.iter().find_map(|row| {
        let (from, to) = if reverse {
            (row[1], row[0])
        } else {
            (row[0], row[1])
        };
        value
            .checked_sub(from)
            .filter(|&n| n < row[2])
            .and_then(|n| to.checked_add(n))
    })
}
fn validate(rows: &[[u32; 3]]) -> Result<(), i32> {
    for (index, row) in rows.iter().enumerate() {
        if row[2] == 0
            || row[0] as u64 + row[2] as u64 > u32::MAX as u64
            || row[1] as u64 + row[2] as u64 > u32::MAX as u64
        {
            return Err(EINVAL);
        }
        for previous in &rows[..index] {
            for column in 0..2 {
                if (row[column] as u64) < previous[column] as u64 + previous[2] as u64
                    && (previous[column] as u64) < row[column] as u64 + row[2] as u64
                {
                    return Err(EINVAL);
                }
            }
        }
    }
    Ok(())
}
#[derive(Default)]
struct State {
    owner: u32,
    parent: u64,
    denied_groups: bool,
    maps: Mapping,
}
impl State {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = b"CYUSER01".to_vec();
        bytes.extend_from_slice(&self.owner.to_le_bytes());
        bytes.extend_from_slice(&u32::from(self.denied_groups).to_le_bytes());
        bytes.extend_from_slice(&self.parent.to_le_bytes());
        bytes.extend_from_slice(&self.maps.encode());
        bytes
    }
    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() < 24 || &bytes[..8] != b"CYUSER01" {
            return Err(EIO);
        }
        Ok(Self {
            owner: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            denied_groups: bytes[12..16] != [0; 4],
            parent: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            maps: Mapping::decode(&bytes[24..])?,
        })
    }
}
static INITIAL: Mutex<Option<Arc<Store>>> = Mutex::new(None);
static CURRENT: Mutex<Option<Arc<Store>>> = Mutex::new(None);
fn initial() -> Result<Arc<Store>, i32> {
    let mut slot = INITIAL.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        let store = Arc::new(Store::user_object(1, true)?);
        if store.read()?.1.is_empty() {
            store.update(|bytes| {
                if !bytes.is_empty() {
                    State::decode(bytes)?;
                    return Ok((bytes.to_vec(), ()));
                }
                Ok((
                    State {
                        maps: Mapping {
                            uid: vec![[0, 0, u32::MAX]],
                            gid: vec![[0, 0, u32::MAX]],
                        },
                        ..State::default()
                    }
                    .encode(),
                    (),
                ))
            })?;
        }
        State::decode(&store.read()?.1)?;
        *slot = Some(store);
    }
    Ok(slot.as_ref().unwrap().clone())
}
fn current() -> Result<Arc<Store>, i32> {
    let mut slot = CURRENT.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        *slot = Some(initial()?);
    }
    Ok(slot.as_ref().unwrap().clone())
}
pub fn id(pid: u32) -> Result<u64, i32> {
    if pid == crate::job::process_id() {
        return Ok(current()?.id());
    }
    kinakaze_runtime::job::user_namespace(pid).ok_or(crate::ENOENT)
}
fn process(pid: u32) -> Result<Store, i32> {
    let _ = initial()?;
    Store::user_object(id(pid)?, false)
}
fn open_store(id: u64) -> Result<Arc<Store>, i32> {
    if id == 1 {
        return initial();
    }
    if let Some(store) = ANCESTORS.lock().map_err(|_| EIO)?.get(&id).cloned() {
        return Ok(store);
    }
    let store = Arc::new(Store::user_object(id, false)?);
    State::decode(&store.read()?.1)?;
    ANCESTORS.lock().map_err(|_| EIO)?.insert(id, store.clone());
    Ok(store)
}
fn state_of(id: u64) -> Result<State, i32> {
    State::decode(&open_store(id)?.read()?.1)
}
pub fn current_capable(bit: u32) -> bool {
    current().is_ok_and(|ns| capable(ns.id(), bit))
}
pub fn capable(target: u64, bit: u32) -> bool {
    let Ok(own) = current().map(|s| s.id()) else {
        return false;
    };
    let cred = crate::credentials::current();
    let has_cap = capabilities()[0] & (1u64 << bit) != 0 || (own == 1 && cred.uid == 0);
    let mut ns = target;
    for _ in 0..=32 {
        if ns == own {
            return has_cap;
        }
        let Ok(state) = state_of(ns) else {
            return false;
        };
        if state.parent == own && state.owner == cred.uid {
            return true;
        }
        if state.parent == 0 {
            return false;
        }
        ns = state.parent;
    }
    false
}
pub struct Prepared {
    store: Arc<Store>,
}
impl Prepared {
    pub fn id(&self) -> u64 {
        self.store.id()
    }
    pub fn install(self) -> Result<(), i32> {
        if !kinakaze_runtime::job::set_user_namespace(crate::job::process_id(), self.id()) {
            return Err(EIO);
        }
        ANCESTORS
            .lock()
            .map_err(|_| EIO)?
            .insert(self.id(), self.store.clone());
        *CURRENT.lock().map_err(|_| EIO)? = Some(self.store);
        *CAPS.lock().map_err(|_| EIO)? = [ALL_CAPS, ALL_CAPS, 0];
        Ok(())
    }
}
pub fn prepare_unshare() -> Result<Prepared, i32> {
    if kinakaze_runtime::process_thread_count() != 1 {
        return Err(EINVAL);
    }
    let parent = current()?;
    let caller = crate::credentials::current();
    let map = map_to_initial(parent.id())?;
    if map.up(caller.uid, false).is_none() || map.up(caller.gid, true).is_none() {
        return Err(EPERM);
    }
    if crate::fs_context::read(|s| s.confined) {
        return Err(EPERM);
    }
    let mut ancestor = parent.id();
    for depth in 0..=32 {
        if ancestor == 1 {
            break;
        }
        if depth == 32 {
            return Err(crate::ENOSPC);
        }
        ancestor = state_of(ancestor)?.parent;
    }
    let store = Arc::new(shared::new_object()?);
    store.update(|_| {
        Ok((
            State {
                owner: caller.uid,
                parent: parent.id(),
                denied_groups: !groups_allowed(),
                ..State::default()
            }
            .encode(),
            (),
        ))
    })?;
    ANCESTORS
        .lock()
        .map_err(|_| EIO)?
        .insert(parent.id(), parent);
    Ok(Prepared { store })
}
pub fn unshare() -> Result<(), i32> {
    let prepared = prepare_unshare()?;
    crate::fs_context::unshare();
    prepared.install()
}
pub fn enter(fd: i32) -> Result<(), i32> {
    if crate::get(fd)?.kind != FdKind::UserNamespace {
        return Err(EINVAL);
    }
    let store = Arc::new(shared::object_fd(fd)?);
    State::decode(&store.read()?.1)?;
    if store.id() == current()?.id()
        || kinakaze_runtime::process_thread_count() != 1
        || !crate::fs_context::is_private()
    {
        return Err(EINVAL);
    }
    if !capable(store.id(), 21) {
        return Err(EPERM);
    }
    Prepared { store }.install()
}
fn map_to_initial(id: u64) -> Result<Mapping, i32> {
    let mut chain = Vec::new();
    let mut at = id;
    for _ in 0..=32 {
        let state = state_of(at)?;
        chain.push(state.maps);
        if at == 1 {
            break;
        }
        if state.parent == 0 {
            return Err(EIO);
        }
        at = state.parent;
    }
    if at != 1 {
        return Err(EIO);
    }
    let mut result = chain.pop().ok_or(EIO)?;
    while let Some(child) = chain.pop() {
        let compose = |rows: Vec<[u32; 3]>, parent: &[[u32; 3]]| {
            let mut result = Vec::new();
            for row in rows {
                for p in parent {
                    let lo = u64::from(row[1]).max(u64::from(p[0]));
                    let hi = (u64::from(row[1]) + u64::from(row[2]))
                        .min(u64::from(p[0]) + u64::from(p[2]));
                    if lo < hi {
                        result.push([
                            row[0] + (lo - u64::from(row[1])) as u32,
                            p[1] + (lo - u64::from(p[0])) as u32,
                            (hi - lo) as u32,
                        ]);
                    }
                }
            }
            result
        };
        result = Mapping {
            uid: compose(child.uid, &result.uid),
            gid: compose(child.gid, &result.gid),
        };
    }
    Ok(result)
}
pub fn inode(id: u64) -> u64 {
    if id == 1 {
        4026531837
    } else {
        (1u64 << 61) | id
    }
}
pub fn open_process(pid: u32, flags: FdFlags) -> Result<i32, i32> {
    process(pid)?.descriptor_kind(FdKind::UserNamespace, flags)
}
pub fn descriptor_inode(fd: i32) -> Result<u64, i32> {
    if crate::get(fd)?.kind != FdKind::UserNamespace {
        return Err(EINVAL);
    }
    Ok(inode(shared::object_fd(fd)?.id()))
}
pub(crate) fn descriptor_map(fd: i32) -> Result<Mapping, i32> {
    if crate::get(fd)?.kind != FdKind::UserNamespace {
        return Err(EINVAL);
    }
    let store = shared::object_fd(fd)?;
    if store.id() == 1 {
        return Err(EPERM);
    }
    let maps = map_to_initial(store.id())?;
    if maps.uid.is_empty() || maps.gid.is_empty() {
        return Err(EINVAL);
    }
    Ok(maps)
}
pub fn read_map(pid: u32, group: bool) -> Result<Vec<u8>, i32> {
    let state = State::decode(&process(pid)?.read()?.1)?;
    let rows = if group {
        state.maps.gid
    } else {
        state.maps.uid
    };
    Ok(rows
        .iter()
        .map(|r| format!("{:10} {:10} {:10}\n", r[0], r[1], r[2]))
        .collect::<String>()
        .into_bytes())
}
pub fn write_map(pid: u32, group: bool, bytes: &[u8], offset: u64) -> Result<usize, i32> {
    if offset != 0 || bytes.is_empty() || bytes.len() >= 4096 {
        return Err(EINVAL);
    }
    let store = process(pid)?;
    if store.id() == 1 {
        return Err(EPERM);
    }
    let mut rows = Vec::new();
    for line in crate::procfs::write_text(bytes)?.lines() {
        let values = line
            .split(|c| matches!(c, ' ' | '\t'..='\r'))
            .filter(|field| !field.is_empty())
            .map(|s| s.parse::<u32>().map_err(|_| EINVAL))
            .collect::<Result<Vec<_>, _>>()?;
        if values.len() != 3 {
            return Err(EINVAL);
        }
        rows.push([values[0], values[1], values[2]]);
    }
    if rows.is_empty() || rows.len() > 340 {
        return Err(EINVAL);
    }
    validate(&rows)?;
    store.update(|old| {
        let mut state = State::decode(old)?;
        let current = current()?.id();
        if current != store.id() && current != state.parent {
            return Err(EPERM);
        }
        let caller = crate::credentials::current();
        let parent_map = map_to_initial(state.parent)?;
        let parent_rows = if group {
            &parent_map.gid
        } else {
            &parent_map.uid
        };
        for row in &rows {
            if !parent_rows.iter().any(|p| {
                row[1] >= p[0]
                    && u64::from(row[1]) + u64::from(row[2]) <= u64::from(p[0]) + u64::from(p[2])
            }) {
                return Err(EPERM);
            }
        }
        if !capable(state.parent, if group { 6 } else { 7 }) {
            let own = parent_map
                .up(if group { caller.gid } else { caller.uid }, group)
                .ok_or(EPERM)?;
            if rows.len() != 1
                || rows[0][1] != own
                || rows[0][2] != 1
                || caller.uid != state.owner
                || (group && !state.denied_groups)
            {
                return Err(EPERM);
            }
        }
        let map = if group {
            &mut state.maps.gid
        } else {
            &mut state.maps.uid
        };
        if !map.is_empty() {
            return Err(EPERM);
        }
        *map = rows;
        Ok((state.encode(), bytes.len()))
    })
}
pub fn groups(pid: u32, value: Option<&[u8]>) -> Result<Vec<u8>, i32> {
    let store = process(pid)?;
    store.update(|old| {
        let mut state = State::decode(old)?;
        if let Some(value) = value {
            if store.id() == 1 || !capable(store.id(), 21) {
                return Err(EPERM);
            }
            match crate::procfs::write_text(value)?
                .trim_end_matches(|c| matches!(c, ' ' | '\t'..='\r'))
            {
                "deny" if state.maps.gid.is_empty() => state.denied_groups = true,
                "allow" if !state.denied_groups => (),
                "deny" | "allow" => return Err(EPERM),
                _ => return Err(EINVAL),
            }
        }
        let result = if state.denied_groups {
            b"deny\n".to_vec()
        } else {
            b"allow\n".to_vec()
        };
        Ok((state.encode(), result))
    })
}
pub fn visible(value: u32, group: bool) -> u32 {
    current_map()
        .ok()
        .and_then(|m| m.up(value, group))
        .unwrap_or(65534)
}
pub(crate) fn current_map() -> Result<Mapping, i32> {
    map_to_initial(current()?.id())
}
pub fn kernel(value: u32, group: bool) -> Result<u32, i32> {
    current_map()?.down(value, group).ok_or(EOVERFLOW)
}
pub(crate) fn serialize() -> Result<Vec<u8>, i32> {
    let id = current()?.id();
    map_to_initial(id)?;
    let mut bytes = id.to_le_bytes().to_vec();
    for caps in capabilities() {
        bytes.extend_from_slice(&caps.to_le_bytes());
    }
    Ok(bytes)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    let run = || -> Result<(), i32> {
        if bytes.len() != 8 && bytes.len() != 32 {
            return Err(EINVAL);
        }
        let id = u64::from_le_bytes(bytes[..8].try_into().map_err(|_| EINVAL)?);
        let _ = initial()?;
        let store = Arc::new(Store::user_object(id, false)?);
        State::decode(&store.read()?.1)?;
        if !kinakaze_runtime::job::set_user_namespace(crate::job::process_id(), id) {
            return Err(EIO);
        }
        *CURRENT.lock().map_err(|_| EIO)? = Some(store);
        map_to_initial(id)?;
        if bytes.len() == 32 {
            *CAPS.lock().map_err(|_| EIO)? = std::array::from_fn(|n| {
                u64::from_le_bytes(bytes[8 + n * 8..16 + n * 8].try_into().unwrap())
            });
        }
        Ok(())
    };
    run().is_ok()
}

/// Whether credentials belong to the runtime domain's initial user namespace.
pub fn is_initial() -> bool {
    current().is_ok_and(|store| store.id() == 1)
}
pub fn groups_allowed() -> bool {
    current()
        .and_then(|store| State::decode(&store.read()?.1))
        .is_ok_and(|s| !s.denied_groups)
}
pub fn open_id(id: u64) -> Result<i32, i32> {
    let store = if id == 1 {
        initial()?
    } else {
        Arc::new(Store::user_object(id, false)?)
    };
    State::decode(&store.read()?.1)?;
    store.descriptor_kind(FdKind::UserNamespace, FdFlags::CLOSE_ON_EXEC)
}
pub fn open_initial() -> Result<i32, i32> {
    initial()?.descriptor_kind(FdKind::UserNamespace, FdFlags::CLOSE_ON_EXEC)
}
/// nsfs identity and ownership queries, retaining the native namespace object.
/// # Safety
/// NS_GET_OWNER_UID requires a writable u32 argument.
pub unsafe fn ioctl(fd: i32, request: u64, argument: *mut u32) -> Result<i32, i32> {
    if crate::get(fd)?.kind != FdKind::UserNamespace {
        return Err(crate::ENOTTY);
    }
    let store = shared::object_fd(fd)?;
    let state = State::decode(&store.read()?.1)?;
    match request {
        0xb703 => Ok(0x1000_0000),
        0xb701 | 0xb702 => {
            if state.parent == 0 || !capable(state.parent, 21) {
                return Err(EPERM);
            }
            Store::user_object(state.parent, false)?
                .descriptor_kind(FdKind::UserNamespace, FdFlags::CLOSE_ON_EXEC)
        }
        0xb704 => {
            if argument.is_null() {
                return Err(crate::EFAULT);
            }
            unsafe {
                argument.write(visible(state.owner, false));
            }
            Ok(0)
        }
        _ => Err(crate::ENOTTY),
    }
}
