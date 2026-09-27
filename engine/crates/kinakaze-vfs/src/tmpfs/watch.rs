//! Inode watches delivered by the same shared transaction as tmpfs mutations.
//! Each live watch pins its queue; another guest process can publish events
//! without a polling thread or a pathname lookup in that process's namespace.
use super::*;
use crate::inotify::*;

pub(crate) struct Watch {
    pub(crate) volume: u64,
    pub(crate) node: u64,
    store: Store,
}
pub(crate) fn locate(path: &str, follow: bool) -> Result<Option<(u64, u64, u32)>, i32> {
    let Some(l) = resolve_location(path, follow, 0)? else {
        return Ok(None);
    };
    let state = volume(l.volume)?.snapshot()?;
    let n = state.nodes.get(&l.node).ok_or(ENOENT)?;
    n.access(4)?;
    Ok(Some((l.volume, l.node, n.mode)))
}
pub(crate) fn locate_fd(fd: i32) -> Result<(u64, u64, u32), i32> {
    let (_, l, _, _) = descriptor(fd)?;
    let state = volume(l.volume)?.snapshot()?;
    let n = state.nodes.get(&l.node).ok_or(ENOENT)?;
    n.access(4)?;
    Ok((l.volume, l.node, n.mode))
}
struct Queue {
    mask: u32,
    events: Vec<Vec<u8>>,
}
impl Queue {
    fn decode(b: &[u8]) -> Result<Self, i32> {
        let mut r = Reader(b);
        if r.word()? != u64::from_le_bytes(*b"KINO0001") {
            return Err(EIO);
        }
        let mask = r.word()? as u32;
        let count = r.word()?;
        if count > 4097 {
            return Err(EIO);
        }
        let mut events = Vec::new();
        for _ in 0..count {
            events.push(r.bytes()?.to_vec());
        }
        r.end()?;
        Ok(Self { mask, events })
    }
    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        word(&mut b, u64::from_le_bytes(*b"KINO0001"));
        word(&mut b, self.mask as u64);
        word(&mut b, self.events.len() as u64);
        for event in &self.events {
            bytes(&mut b, event);
        }
        b
    }
    fn push(&mut self, mask: u32, cookie: u32, name: &[u8]) {
        if self.mask == 0 || mask & self.mask & IN_ALL_EVENTS == 0 {
            return;
        }
        if self.events.len() >= 4096 {
            if self.events.len() == 4096 {
                self.events
                    .push(crate::inotify::encode_event(-1, 0x4000, 0, &[]));
            }
            return;
        }
        let event = crate::inotify::encode_event(0, mask & (self.mask | IN_ISDIR), cookie, name);
        if self.events.last() != Some(&event) {
            self.events.push(event);
        }
        if self.mask & IN_ONESHOT != 0 {
            self.events
                .push(crate::inotify::encode_event(0, IN_IGNORED, 0, &[]));
            self.mask = 0;
        }
    }
}
impl Watch {
    pub(crate) fn new(volume_id: u64, node: u64, mask: u32) -> Result<Self, i32> {
        let store = shared::new_object()?;
        store.replace(
            &Queue {
                mask,
                events: Vec::new(),
            }
            .encode(),
        )?;
        volume(volume_id)?.change(|s| {
            if !s.nodes.contains_key(&node) {
                return Err(ENOENT);
            }
            s.watches.push((store.id(), node));
            Ok(())
        })?;
        Ok(Self {
            volume: volume_id,
            node,
            store,
        })
    }
    pub(crate) fn mask(&self, mask: u32) -> Result<(), i32> {
        self.store.update(|b| {
            let mut q = Queue::decode(b)?;
            if mask & IN_MASK_ADD != 0 {
                q.mask |= mask;
            } else {
                q.mask = mask;
            }
            Ok((q.encode(), ()))
        })
    }
    pub(crate) fn drain(&self, wd: i32) -> Result<Vec<Vec<u8>>, i32> {
        // Cgroup counters can change without a write to this mount. Refreshing
        // its projection publishes the corresponding file modifications too.
        let _ = volume(self.volume)?.snapshot()?;
        self.store.update(|b| {
            let mut q = Queue::decode(b)?;
            let mut events = std::mem::take(&mut q.events);
            for event in &mut events {
                if event[..4] != (-1i32).to_ne_bytes() {
                    event[..4].copy_from_slice(&wd.to_ne_bytes());
                }
            }
            Ok((q.encode(), events))
        })
    }
}
pub(super) struct Observation {
    store: Store,
    node: u64,
    nodes: BTreeMap<u64, Node>,
}
pub(super) fn before(s: &mut State) -> Result<Vec<Observation>, i32> {
    let mut observations = Vec::new();
    s.watches.retain(|(id, node)| {
        let Ok(store) = Store::user_object(*id, false) else {
            return false;
        };
        let mut nodes = BTreeMap::new();
        if let Some(n) = s.nodes.get(node) {
            nodes.insert(*node, n.clone());
            for id in n.children.values() {
                if let Some(n) = s.nodes.get(id) {
                    nodes.insert(*id, n.clone());
                }
            }
        }
        observations.push(Observation {
            store,
            node: *node,
            nodes,
        });
        true
    });
    Ok(observations)
}
fn changes(a: &Node, b: &Node) -> u32 {
    let mut mask = 0;
    if a.atime != b.atime {
        mask |= IN_ACCESS;
    }
    if a.mode & S_IFMT != S_IFDIR && (a.mtime != b.mtime || a.size != b.size) {
        mask |= IN_MODIFY;
    }
    if a.mode != b.mode || a.uid != b.uid || a.gid != b.gid || a.links != b.links {
        mask |= IN_ATTRIB;
    }
    mask | if b.mode & S_IFMT == S_IFDIR {
        IN_ISDIR
    } else {
        0
    }
}
pub(super) fn after(observations: Vec<Observation>, s: &State) -> Result<(), i32> {
    for observation in observations {
        let Some(old) = observation.nodes.get(&observation.node) else {
            continue;
        };
        observation.store.update(|b| {
            let mut q = Queue::decode(b)?;
            let new = s.nodes.get(&observation.node);
            if new.is_none_or(|n| n.links == 0) && old.links != 0 {
                q.push(IN_DELETE_SELF, 0, &[]);
                if q.mask != 0 {
                    q.events
                        .push(crate::inotify::encode_event(0, IN_IGNORED, 0, &[]));
                    q.mask = 0;
                }
            } else if let Some(new) = new {
                q.push(changes(old, new), 0, &[]);
                if old.parent != new.parent {
                    q.push(IN_MOVE_SELF, 0, &[]);
                }
                for (name, id) in &old.children {
                    if new.children.get(name) != Some(id) {
                        let moved = s
                            .nodes
                            .values()
                            .any(|parent| parent.children.values().any(|v| v == id));
                        let isdir = observation
                            .nodes
                            .get(id)
                            .is_some_and(|n| n.mode & S_IFMT == S_IFDIR);
                        q.push(
                            (if moved { IN_MOVED_FROM } else { IN_DELETE })
                                | if isdir { IN_ISDIR } else { 0 },
                            if moved { *id as u32 } else { 0 },
                            name.as_bytes(),
                        );
                    }
                }
                for (name, id) in &new.children {
                    if old.children.get(name) != Some(id) {
                        let moved = observation.nodes.contains_key(id);
                        let isdir = s.nodes.get(id).is_some_and(|n| n.mode & S_IFMT == S_IFDIR);
                        q.push(
                            (if moved { IN_MOVED_TO } else { IN_CREATE })
                                | if isdir { IN_ISDIR } else { 0 },
                            if moved { *id as u32 } else { 0 },
                            name.as_bytes(),
                        );
                    } else if let (Some(a), Some(b)) = (observation.nodes.get(id), s.nodes.get(id))
                    {
                        q.push(changes(a, b), 0, name.as_bytes());
                    }
                }
            }
            Ok((q.encode(), ()))
        })?;
    }
    Ok(())
}
