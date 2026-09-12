//! Linux user/logon keys and keyrings, shared across native fork/exec hosts.
//! Keys live in a pagefile section, never in guest files or environment strings.
use crate::mount::shared::Store;
use crate::state_codec::{Reader, bytes, word};
use crate::{EACCES, EINVAL, EIO, ENOENT, EOPNOTSUPP, EPERM};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
const ENOKEY: i32 = 126;
const EKEYEXPIRED: i32 = 127;
const EKEYREVOKED: i32 = 128;
const CATALOG: u64 = u64::MAX - 21;
static STORE: Mutex<Option<Arc<Store>>> = Mutex::new(None);
#[derive(Clone, Copy, Default)]
pub(crate) struct Task {
    session: i32,
    thread: i32,
    process: i32,
}
thread_local! { static TASK: RefCell<Task> = RefCell::new(Task::default()); }
#[derive(Clone)]
struct Key {
    id: i32,
    ns: u64,
    uid: u32,
    gid: u32,
    perm: u32,
    revoked: bool,
    expires: u64,
    kind: String,
    name: String,
    data: Vec<u8>,
    links: Vec<i32>,
}
#[derive(Default)]
struct State {
    next: i32,
    keys: BTreeMap<i32, Key>,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn namespace() -> Result<u64, i32> {
    crate::user_namespace::id(crate::job::process_id())
}
fn store() -> Result<Arc<Store>, i32> {
    let mut slot = STORE.lock().map_err(|_| EIO)?;
    if slot.is_none() {
        *slot = Some(Arc::new(Store::user_object(CATALOG, true)?));
    }
    Ok(slot.as_ref().unwrap().clone())
}
impl State {
    fn decode(value: &[u8]) -> Result<Self, i32> {
        if value.is_empty() {
            return Ok(Self {
                next: 1,
                ..Self::default()
            });
        }
        let mut r = Reader(value);
        if r.word()? != 0x43594b4559533031 {
            return Err(EIO);
        }
        let next = r.word()? as i32;
        let mut keys = BTreeMap::new();
        let count = r.word()?;
        if count > 1_000_000 {
            return Err(EIO);
        }
        for _ in 0..count {
            let mut k = Key {
                id: r.word()? as i32,
                ns: r.word()?,
                uid: r.word()? as u32,
                gid: r.word()? as u32,
                perm: r.word()? as u32,
                revoked: r.word()? != 0,
                expires: r.word()?,
                kind: r.text()?,
                name: r.text()?,
                data: r.bytes()?.to_vec(),
                links: Vec::new(),
            };
            let n = r.word()?;
            if n > count {
                return Err(EIO);
            }
            for _ in 0..n {
                k.links.push(r.word()? as i32);
            }
            keys.insert(k.id, k);
        }
        r.end()?;
        Ok(Self { next, keys })
    }
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        word(&mut out, 0x43594b4559533031);
        word(&mut out, self.next as u64);
        word(&mut out, self.keys.len() as u64);
        for k in self.keys.values() {
            for v in [
                k.id as u64,
                k.ns,
                k.uid as u64,
                k.gid as u64,
                k.perm as u64,
                k.revoked as u64,
                k.expires,
            ] {
                word(&mut out, v);
            }
            for v in [k.kind.as_bytes(), k.name.as_bytes(), &k.data] {
                bytes(&mut out, v);
            }
            word(&mut out, k.links.len() as u64);
            for id in &k.links {
                word(&mut out, *id as u64);
            }
        }
        out
    }
    fn create(&mut self, kind: &str, name: &str, data: &[u8]) -> Result<i32, i32> {
        if !matches!(kind, "keyring" | "user" | "logon") {
            return Err(ENODEV);
        }
        if name.len() > 4095
            || name.is_empty()
            || name.contains(';')
            || (kind == "logon" && !name.contains(':'))
        {
            return Err(EINVAL);
        }
        if data.len() > 32767 || (kind == "keyring" && !data.is_empty()) {
            return Err(EINVAL);
        }
        let cred = crate::credentials::current();
        let owned: Vec<_> = self.keys.values().filter(|k| k.uid == cred.uid).collect();
        let (max_keys, max_bytes) = if cred.uid == 0 {
            (1_000_000, 25_000_000)
        } else {
            (200, 20_000)
        };
        if owned.len() >= max_keys
            || owned
                .iter()
                .map(|k| k.data.len() + k.name.len())
                .sum::<usize>()
                + data.len()
                + name.len()
                > max_bytes
        {
            return Err(122);
        }
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or(crate::ENOSPC)?;
        self.keys.insert(
            id,
            Key {
                id,
                ns: namespace()?,
                uid: cred.uid,
                gid: cred.gid,
                perm: 0x3f01_0000,
                revoked: false,
                expires: 0,
                kind: kind.into(),
                name: name.into(),
                data: data.into(),
                links: Vec::new(),
            },
        );
        Ok(id)
    }
    fn live(&self, id: i32) -> Result<&Key, i32> {
        let k = self.keys.get(&id).ok_or(ENOKEY)?;
        if k.revoked {
            return Err(EKEYREVOKED);
        }
        if k.expires != 0 && k.expires <= now() {
            return Err(EKEYEXPIRED);
        }
        Ok(k)
    }
    fn contains(&self, root: i32, wanted: i32, seen: &mut BTreeSet<i32>) -> bool {
        if root == wanted {
            return true;
        }
        if !seen.insert(root) || seen.len() > 1024 {
            return false;
        }
        self.live(root)
            .is_ok_and(|k| k.links.iter().any(|id| self.contains(*id, wanted, seen)))
    }
    fn permission(&self, id: i32, need: u32, task: Task) -> Result<(), i32> {
        let k = self.live(id)?;
        let cred = crate::credentials::current();
        let class = if k.uid == cred.uid {
            (k.perm >> 16) & 0x3f
        } else if crate::credentials::group_member(k.gid) {
            (k.perm >> 8) & 0x3f
        } else {
            k.perm & 0x3f
        };
        let possessed = [task.session, task.thread, task.process]
            .iter()
            .any(|root| self.contains(*root, id, &mut BTreeSet::new()));
        if (class | if possessed { k.perm >> 24 } else { 0 }) & need == need {
            Ok(())
        } else {
            Err(EACCES)
        }
    }
    fn special(&mut self, id: i32, create: bool, task: &mut Task) -> Result<i32, i32> {
        if id > 0 {
            self.live(id)?;
            return Ok(id);
        }
        let name = match id {
            -1 => "_tid".to_string(),
            -2 => "_pid".to_string(),
            -3 => "_ses".to_string(),
            -4 => format!("_uid.{}", crate::credentials::current().uid),
            -5 => format!("_uid_ses.{}", crate::credentials::current().uid),
            _ => return Err(ENOKEY),
        };
        let old = match id {
            -1 => task.thread,
            -2 => task.process,
            -3 => task.session,
            _ => {
                let ns = namespace()?;
                self.keys
                    .values()
                    .find(|k| k.ns == ns && k.name == name && k.kind == "keyring" && !k.revoked)
                    .map_or(0, |k| k.id)
            }
        };
        if old != 0 {
            self.live(old)?;
            return Ok(old);
        }
        if !create {
            return Err(ENOKEY);
        }
        let new = self.create("keyring", &name, &[])?;
        match id {
            -1 => task.thread = new,
            -2 => task.process = new,
            -3 => task.session = new,
            _ => {}
        }
        Ok(new)
    }
    fn link(&mut self, id: i32, ring: i32, task: Task) -> Result<(), i32> {
        self.permission(id, 16, task)?;
        self.permission(ring, 4, task)?;
        if self.live(ring)?.kind != "keyring" {
            return Err(crate::ENOTDIR);
        }
        if self.contains(id, ring, &mut BTreeSet::new()) {
            return Err(35);
        }
        let key = self.live(id)?.clone();
        let replaced: Vec<_> = self
            .live(ring)?
            .links
            .iter()
            .copied()
            .filter(|other| {
                self.keys
                    .get(other)
                    .is_some_and(|k| k.kind == key.kind && k.name == key.name)
            })
            .collect();
        let ring = self.keys.get_mut(&ring).unwrap();
        ring.links.retain(|k| !replaced.contains(k));
        ring.links.push(id);
        Ok(())
    }
}
const ENODEV: i32 = 19;
fn transact<T>(run: impl FnOnce(&mut State, &mut Task) -> Result<T, i32>) -> Result<T, i32> {
    let mut task = capture();
    let result = store()?.update(|old| {
        let mut state = State::decode(old)?;
        let value = run(&mut state, &mut task)?;
        Ok((state.encode(), value))
    })?;
    adopt(task);
    Ok(result)
}
pub fn join(name: Option<&str>) -> Result<i32, i32> {
    if name.is_some_and(|s| s.starts_with('.')) {
        return Err(EPERM);
    }
    transact(|state, task| {
        let ns = namespace()?;
        let found = name.and_then(|name| {
            state
                .keys
                .values()
                .find(|k| {
                    k.ns == ns
                        && k.name == name
                        && k.kind == "keyring"
                        && !k.revoked
                        && state.permission(k.id, 8, *task).is_ok()
                })
                .map(|k| k.id)
        });
        let id = if let Some(id) = found {
            id
        } else {
            state.create("keyring", name.unwrap_or("_ses"), &[])?
        };
        task.session = id;
        Ok(id)
    })
}
pub fn add(kind: &str, name: &str, data: &[u8], ring: i32) -> Result<i32, i32> {
    transact(|s, t| {
        let ring = s.special(ring, true, t)?;
        s.permission(ring, 4, *t)?;
        if s.live(ring)?.kind != "keyring" {
            return Err(crate::ENOTDIR);
        }
        if let Some(id) = s.live(ring)?.links.iter().copied().find(|id| {
            s.keys
                .get(id)
                .is_some_and(|k| k.kind == kind && k.name == name)
        }) {
            s.permission(id, 4, *t)?;
            if data.len() > 32767 {
                return Err(EINVAL);
            }
            s.keys.get_mut(&id).unwrap().data = data.to_vec();
            return Ok(id);
        }
        let id = s.create(kind, name, data)?;
        s.keys.get_mut(&ring).unwrap().links.push(id);
        Ok(id)
    })
}
pub fn search(ring: i32, kind: &str, name: &str, destination: i32) -> Result<i32, i32> {
    transact(|s, t| {
        let root = s.special(ring, false, t)?;
        s.permission(root, 8, *t)?;
        if s.live(root)?.kind != "keyring" {
            return Err(crate::ENOTDIR);
        }
        let mut pending = vec![root];
        let mut seen = BTreeSet::new();
        let mut found = None;
        while let Some(ring) = pending.pop() {
            if !seen.insert(ring) || seen.len() > 1024 || s.permission(ring, 8, *t).is_err() {
                continue;
            }
            for id in &s.live(ring)?.links {
                if let Ok(k) = s.live(*id) {
                    if k.kind == kind && k.name == name && s.permission(*id, 8, *t).is_ok() {
                        found = Some(*id);
                        break;
                    }
                    if k.kind == "keyring" {
                        pending.push(*id);
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        let id = found.ok_or(ENOKEY)?;
        if destination != 0 {
            let destination = s.special(destination, true, t)?;
            s.link(id, destination, *t)?;
        }
        Ok(id)
    })
}
/// Commands with scalar inputs; data buffers are copied by the ABI boundary.
pub fn command(op: u32, id: i32, a: u64, b: u64, input: &[u8]) -> Result<(i64, Vec<u8>), i32> {
    if op == 31 {
        return Ok((2, vec![0x21, 0x01]));
    }
    if !matches!(op, 0 | 2..=9 | 11 | 15 | 17 | 21) {
        return Err(EOPNOTSUPP);
    }
    transact(|s, t| {
        let id = s.special(id, op == 0 && a != 0, t)?;
        if op == 0 {
            s.permission(id, 8, *t)?;
            return Ok((id as i64, vec![]));
        }
        let need = match op {
            6 | 17 => 1,
            11 => 2,
            2 | 3 | 7 | 9 => 4,
            4 | 5 | 15 => 32,
            8 => 16,
            21 => 8,
            _ => 0,
        };
        s.permission(id, need, *t)?;
        let mut out = Vec::new();
        match op {
            2 => {
                let k = s.keys.get_mut(&id).unwrap();
                if k.kind == "keyring" {
                    return Err(EOPNOTSUPP);
                }
                if input.len() > 32767 {
                    return Err(EINVAL);
                }
                k.data = input.into();
            }
            3 | 21 => {
                let k = s.keys.get_mut(&id).unwrap();
                k.revoked = true;
                k.data.clear();
                k.links.clear();
            }
            4 => {
                let k = s.keys.get_mut(&id).unwrap();
                let cred = crate::credentials::current();
                if (a as u32 != u32::MAX && a as u32 != k.uid)
                    || (b as u32 != u32::MAX && !crate::credentials::group_member(b as u32))
                {
                    if !crate::user_namespace::capable(k.ns, 0) {
                        return Err(EPERM);
                    }
                }
                if cred.uid != k.uid && !crate::user_namespace::capable(k.ns, 0) {
                    return Err(EPERM);
                }
                if a as u32 != u32::MAX {
                    k.uid = a as u32;
                }
                if b as u32 != u32::MAX {
                    k.gid = b as u32;
                }
            }
            5 => {
                if a & !0x3f3f3f3f != 0 {
                    return Err(EINVAL);
                }
                s.keys.get_mut(&id).unwrap().perm = a as u32;
            }
            6 => {
                let k = s.live(id)?;
                out = format!("{};{};{};{:08x};{}\0", k.kind, k.uid, k.gid, k.perm, k.name)
                    .into_bytes();
            }
            7 => {
                let k = s.keys.get_mut(&id).unwrap();
                if k.kind != "keyring" {
                    return Err(crate::ENOTDIR);
                }
                k.links.clear();
            }
            8 => {
                let ring = s.special(a as i32, true, t)?;
                s.link(id, ring, *t)?;
            }
            9 => {
                let ring = s.special(a as i32, false, t)?;
                s.permission(ring, 4, *t)?;
                let k = s.keys.get_mut(&ring).unwrap();
                if k.kind != "keyring" {
                    return Err(crate::ENOTDIR);
                }
                if !k.links.contains(&id) {
                    return Err(ENOENT);
                }
                k.links.retain(|v| *v != id);
            }
            11 => {
                let k = s.live(id)?;
                if k.kind == "logon" {
                    return Err(EACCES);
                }
                if k.kind == "keyring" {
                    for id in &k.links {
                        out.extend_from_slice(&id.to_le_bytes());
                    }
                } else {
                    out = k.data.clone();
                }
            }
            15 => {
                s.keys.get_mut(&id).unwrap().expires = if a == 0 {
                    0
                } else {
                    now().checked_add(a).ok_or(EINVAL)?
                }
            }
            17 => out.push(0),
            _ => unreachable!(),
        }
        Ok((out.len() as i64, out))
    })
}
pub(crate) fn capture() -> Task {
    TASK.with(|t| *t.borrow())
}
pub(crate) fn capture_thread() -> Task {
    let mut task = capture();
    task.thread = 0;
    task
}
pub(crate) fn adopt(task: Task) {
    TASK.with(|t| *t.borrow_mut() = task);
}
pub(crate) fn serialize(_fork: bool) -> Result<Vec<u8>, i32> {
    let task = capture();
    let mut out = Vec::new();
    for id in [task.session, 0] {
        word(&mut out, id as u64);
    }
    Ok(out)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    let run = || {
        let mut r = Reader(bytes);
        let task = Task {
            session: r.word()? as i32,
            process: r.word()? as i32,
            thread: 0,
        };
        r.end()?;
        if task.session != 0 || task.process != 0 {
            let _ = store()?;
        }
        adopt(task);
        Ok::<_, i32>(())
    };
    run().is_ok()
}
