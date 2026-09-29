//! Inotify open descriptions share watches and queues through native sections.
//! Init owns directory I/O; a worker exiting never cancels an inherited watch.
use crate::mount::shared::{self, Store};
use crate::state_codec::{Reader, bytes, word};
use kinakaze_v2_host_win::{DirectoryQueue, DirectoryWatch};
use kinakaze_v2_protocol::kernel::{KernelCommand, ObjectKey};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

pub const IN_ACCESS: u32 = 0x0000_0001;
pub const IN_MODIFY: u32 = 0x0000_0002;
pub const IN_ATTRIB: u32 = 0x0000_0004;
pub const IN_CREATE: u32 = 0x0000_0100;
pub const IN_DELETE: u32 = 0x0000_0200;
pub const IN_DELETE_SELF: u32 = 0x0000_0400;
pub const IN_MOVE_SELF: u32 = 0x0000_0800;
pub const IN_MOVED_FROM: u32 = 0x0000_0040;
pub const IN_MOVED_TO: u32 = 0x0000_0080;
pub const IN_ALL_EVENTS: u32 = 0x0000_0fff;
pub const IN_ISDIR: u32 = 0x4000_0000;
pub const IN_IGNORED: u32 = 0x0000_8000;
pub const IN_ONLYDIR: u32 = 0x0100_0000;
pub const IN_DONT_FOLLOW: u32 = 0x0200_0000;
pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
pub const IN_MASK_CREATE: u32 = 0x1000_0000;
pub const IN_MASK_ADD: u32 = 0x2000_0000;
pub const IN_ONESHOT: u32 = 0x8000_0000;

pub const IN_CLOEXEC: i32 = 0o2000000;
pub const IN_NONBLOCK: i32 = 0o0004000;

const FILE_ACTION_ADDED: u32 = 1;
const FILE_ACTION_REMOVED: u32 = 2;
const FILE_ACTION_MODIFIED: u32 = 3;
const FILE_ACTION_RENAMED_OLD_NAME: u32 = 4;
const FILE_ACTION_RENAMED_NEW_NAME: u32 = 5;
const MAGIC: u64 = u64::from_le_bytes(*b"CYINOFD1");
const QUEUE_LIMIT: usize = 4096;

struct Watch {
    id: u64,
    volume: u64,
    node: u64,
    path: PathBuf,
    filter: Vec<u8>,
    mask: u32,
    cookie: u32,
}
struct State {
    retired: Vec<Watch>,
    next: i32,
    watches: BTreeMap<i32, Watch>,
    queue: VecDeque<Vec<u8>>,
}
impl State {
    fn decode(data: &[u8]) -> Result<Self, i32> {
        let mut r = Reader(data);
        if r.word()? != MAGIC {
            return Err(crate::EIO);
        }
        let mut s = Self {
            retired: Vec::new(),
            next: r.word()? as i32,
            watches: BTreeMap::new(),
            queue: VecDeque::new(),
        };
        let count = r.word()?;
        if count > 16384 {
            return Err(crate::EIO);
        }
        for _ in 0..count {
            let wd = r.word()? as i32;
            let item = Watch {
                id: r.word()?,
                volume: r.word()?,
                node: r.word()?,
                mask: r.word()? as u32,
                cookie: r.word()? as u32,
                path: PathBuf::from(std::str::from_utf8(r.bytes()?).map_err(|_| crate::EIO)?),
                filter: r.bytes()?.to_vec(),
            };
            if s.watches.insert(wd, item).is_some() {
                return Err(crate::EIO);
            }
        }
        let count = r.word()?;
        if count > (QUEUE_LIMIT + 1) as u64 {
            return Err(crate::EIO);
        }
        for _ in 0..count {
            s.queue.push_back(r.bytes()?.to_vec());
        }
        r.end()?;
        Ok(s)
    }
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        word(&mut out, MAGIC);
        word(&mut out, self.next as u64);
        word(&mut out, self.watches.len() as u64);
        for (&wd, w) in &self.watches {
            for value in [
                wd as u64,
                w.id,
                w.volume,
                w.node,
                w.mask as u64,
                w.cookie as u64,
            ] {
                word(&mut out, value);
            }
            bytes(&mut out, w.path.to_string_lossy().as_bytes());
            bytes(&mut out, &w.filter);
        }
        word(&mut out, self.queue.len() as u64);
        for event in &self.queue {
            bytes(&mut out, event);
        }
        out
    }
    fn push(&mut self, event: Vec<u8>) {
        if self.queue.back() == Some(&event) {
            return;
        }
        if self.queue.len() < QUEUE_LIMIT {
            self.queue.push_back(event);
        } else if self.queue.len() == QUEUE_LIMIT {
            self.queue.push_back(encode_event(-1, 0x4000, 0, &[]));
        }
    }
}
// Standalone provider tests have no init. Production never keeps watch owners
// in a worker; only this fallback uses the same native owning wrappers locally.
#[allow(dead_code)]
enum LocalWatch {
    Native(DirectoryWatch),
    Memory(crate::tmpfs::watch::Watch),
}
fn local() -> &'static Mutex<HashMap<(u64, u64), LocalWatch>> {
    static VALUE: OnceLock<Mutex<HashMap<(u64, u64), LocalWatch>>> = OnceLock::new();
    VALUE.get_or_init(Default::default)
}
fn owner(fd: i32) -> Result<Store, i32> {
    let table = crate::table().read().map_err(|_| crate::EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|s| *s)
        .ok_or(crate::EBADF)?;
    if entry.kind != crate::FdKind::Inotify {
        return Err(crate::EINVAL);
    }
    shared::object_entry(entry)
}
fn update<T>(owner: &Store, action: impl FnOnce(&mut State) -> Result<T, i32>) -> Result<T, i32> {
    update_observed(owner, action).map(|(_, result)| result)
}
fn update_observed<T>(
    owner: &Store,
    action: impl FnOnce(&mut State) -> Result<T, i32>,
) -> Result<(u64, T), i32> {
    let (revision, (result, retired)) = owner.update_with_revision(|data| {
        let mut state = State::decode(data)?;
        let result = action(&mut state);
        Ok((state.encode(), (result, state.retired)))
    })?;
    // Publish removal before cancelling I/O. An abandoned update can never
    // leave a published watch pointing at an already destroyed native queue.
    for watch in retired {
        release(owner, &watch)?;
    }
    result.map(|value| (revision, value))
}
pub fn create_inotify(flags: i32) -> Result<i32, i32> {
    if flags & !(IN_CLOEXEC | IN_NONBLOCK) != 0 {
        return Err(crate::EINVAL);
    }
    let store = shared::new_object()?;
    store.replace(
        &State {
            retired: Vec::new(),
            next: 1,
            watches: BTreeMap::new(),
            queue: VecDeque::new(),
        }
        .encode(),
    )?;
    let mut fd_flags = crate::FdFlags::NONE;
    if flags & IN_CLOEXEC != 0 {
        fd_flags.0 |= crate::FdFlags::CLOSE_ON_EXEC.0;
    }
    if flags & IN_NONBLOCK != 0 {
        fd_flags.0 |= crate::FdFlags::NONBLOCK.0;
    }
    let fd = store.descriptor_kind(crate::FdKind::Inotify, fd_flags)?;
    crate::pipe_inode::finish_created(fd)
}
/// Resolves a guest pathname with the same cwd/root/symlink rules as the rest
/// of the VFS, then installs a watch on the resolved object.
pub fn add_watch_linux(fd: i32, path: &str, mask: u32) -> Result<i32, i32> {
    let absolute = crate::fs::absolute_linux(path);
    if let Some((pid, target)) = crate::procfs::fd_magic_link(&absolute) {
        if pid != crate::job::process_id() || mask & IN_DONT_FOLLOW != 0 {
            return Err(crate::EOPNOTSUPP);
        }
        // sd-event uses O_PATH fds to watch symlink inodes themselves. Resolve
        // the pinned native object, not the textual target of that symlink.
        if matches!(
            crate::get(target)?.kind,
            crate::FdKind::TmpfsFile | crate::FdKind::TmpfsDirectory | crate::FdKind::SysfsFile
        ) {
            return add_memory_watch(fd, crate::tmpfs::watch::locate_fd(target)?, mask);
        }
        let object = crate::fs::object::Object::from_fd(target)?;
        return add_watch(fd, object.path()?, mask);
    }
    if let Some(location) = crate::tmpfs::watch::locate(path, mask & IN_DONT_FOLLOW == 0)? {
        return add_memory_watch(fd, location, mask);
    }
    let resolved = if mask & IN_DONT_FOLLOW != 0 {
        crate::fs::resolve_no_follow(path)?
    } else {
        crate::fs::resolve(path)?
    };
    add_watch(fd, resolved, mask)
}

fn valid_mask(mask: u32) -> Result<(), i32> {
    if mask & IN_ALL_EVENTS == 0 || mask & IN_MASK_ADD != 0 && mask & IN_MASK_CREATE != 0 {
        Err(crate::EINVAL)
    } else {
        Ok(())
    }
}
fn change_mask(watch: &mut Watch, mask: u32) -> Result<(), i32> {
    if mask & IN_MASK_CREATE != 0 {
        return Err(crate::EEXIST);
    }
    if watch.volume != 0 {
        crate::tmpfs::watch::Watch::open(watch.volume, watch.node, watch.id)?.mask(mask)?;
    }
    watch.mask = if mask & IN_MASK_ADD != 0 {
        watch.mask | mask
    } else {
        mask
    };
    Ok(())
}
fn add_memory_watch(fd: i32, (volume, node, mode): (u64, u64, u32), mask: u32) -> Result<i32, i32> {
    valid_mask(mask)?;
    if mask & IN_ONLYDIR != 0 && mode & crate::fs::S_IFMT != crate::fs::S_IFDIR {
        return Err(crate::ENOTDIR);
    }
    let store = owner(fd)?;
    update(&store, |s| {
        for (&wd, watch) in &mut s.watches {
            if watch.volume == volume && watch.node == node {
                change_mask(watch, mask)?;
                return Ok(wd);
            }
        }
        let next = s.next.checked_add(1).ok_or(crate::ENOSPC)?;
        let memory = crate::tmpfs::watch::Watch::new(volume, node, mask)?;
        memory.retain(store.kernel_key())?;
        let id = memory.id();
        if kinakaze_runtime::authority::get().is_none() {
            local()
                .lock()
                .map_err(|_| crate::EIO)?
                .insert((store.id(), id), LocalWatch::Memory(memory));
        }
        let wd = s.next;
        s.next = next;
        s.watches.insert(
            wd,
            Watch {
                id,
                volume,
                node,
                path: PathBuf::new(),
                filter: Vec::new(),
                mask,
                cookie: 0,
            },
        );
        Ok(wd)
    })
}
pub fn add_watch(fd: i32, path: PathBuf, mask: u32) -> Result<i32, i32> {
    valid_mask(mask)?;
    let store = owner(fd)?;
    update(&store, |s| {
        for (&wd, watch) in &mut s.watches {
            if watch.volume == 0 && watch.path == path {
                change_mask(watch, mask)?;
                return Ok(wd);
            }
        }
        if s.watches.len() >= 16384 {
            return Err(crate::ENOSPC);
        }
        let next = s.next.checked_add(1).ok_or(crate::ENOSPC)?;
        let metadata = std::fs::metadata(&path).map_err(errno_from_io)?;
        if mask & IN_ONLYDIR != 0 && !metadata.is_dir() {
            return Err(crate::ENOTDIR);
        }
        let (root, filter) = if metadata.is_dir() {
            (path.clone(), Vec::new())
        } else {
            (
                path.parent().ok_or(crate::ENOENT)?.to_path_buf(),
                path.file_name()
                    .ok_or(crate::ENOENT)?
                    .to_string_lossy()
                    .as_bytes()
                    .to_vec(),
            )
        };
        let id = shared::new_object()?.id();
        if kinakaze_runtime::authority::get().is_some() {
            kinakaze_runtime::authority::kernel(KernelCommand::WatchDirectory {
                owner: store.id(),
                watch: id,
                path: root.to_string_lossy().into_owned(),
            })?;
        } else {
            let watch = DirectoryWatch::start(kinakaze_runtime::authority::domain_id(), id, &root)
                .map_err(errno_from_io)?;
            local()
                .lock()
                .map_err(|_| crate::EIO)?
                .insert((store.id(), id), LocalWatch::Native(watch));
        }
        let wd = s.next;
        s.next = next;
        s.watches.insert(
            wd,
            Watch {
                id,
                volume: 0,
                node: 0,
                path,
                filter,
                mask,
                cookie: 0,
            },
        );
        Ok(wd)
    })
}
fn release(store: &Store, watch: &Watch) -> Result<(), i32> {
    if kinakaze_runtime::authority::get().is_none() {
        local()
            .lock()
            .map_err(|_| crate::EIO)?
            .remove(&(store.id(), watch.id));
        return Ok(());
    }
    kinakaze_runtime::authority::kernel(if watch.volume == 0 {
        KernelCommand::RemoveDirectoryWatch {
            owner: store.id(),
            watch: watch.id,
        }
    } else {
        KernelCommand::Unlease {
            object: ObjectKey::Shared(watch.id),
            owner: store.kernel_key(),
        }
    })
}
fn collect(_store: &Store, s: &mut State) -> Result<(), i32> {
    collect_with_queues(s, &mut HashMap::new(), &mut HashMap::new())
}

fn collect_with_queues(
    s: &mut State,
    queues: &mut HashMap<i32, (u64, DirectoryQueue)>,
    memory_queues: &mut HashMap<i32, (u64, crate::tmpfs::watch::Watch)>,
) -> Result<(), i32> {
    let _observation = crate::tmpfs::Observation::enter();
    let mut events = Vec::new();
    let mut ignored = Vec::new();
    for (&wd, watch) in &mut s.watches {
        if watch.volume != 0 {
            if memory_queues.get(&wd).is_none_or(|(id, _)| *id != watch.id) {
                memory_queues.insert(
                    wd,
                    (
                        watch.id,
                        crate::tmpfs::watch::Watch::open(watch.volume, watch.node, watch.id)?,
                    ),
                );
            }
            for event in memory_queues.get(&wd).ok_or(crate::EIO)?.1.drain(wd)? {
                if u32::from_ne_bytes(event[4..8].try_into().unwrap()) & IN_IGNORED != 0 {
                    ignored.push(wd);
                }
                events.push(event);
            }
            continue;
        }
        if queues.get(&wd).is_none_or(|(id, _)| *id != watch.id) {
            queues.insert(
                wd,
                (
                    watch.id,
                    DirectoryQueue::open(kinakaze_runtime::authority::domain_id(), watch.id, false)
                        .map_err(errno_from_io)?,
                ),
            );
        }
        let (_, queue) = queues.get(&wd).ok_or(crate::EIO)?;
        let (packets, overflow, ended) = queue.drain().map_err(errno_from_io)?;
        if overflow {
            events.push(encode_event(-1, 0x4000, 0, &[]));
        }
        'packets: for packet in packets {
            let mut offset = 0;
            while offset + 12 <= packet.len() {
                let next =
                    u32::from_le_bytes(packet[offset..offset + 4].try_into().unwrap()) as usize;
                let action = u32::from_le_bytes(packet[offset + 4..offset + 8].try_into().unwrap());
                let length = u32::from_le_bytes(packet[offset + 8..offset + 12].try_into().unwrap())
                    as usize;
                let name = packet
                    .get(offset + 12..offset + 12 + length)
                    .ok_or(crate::EIO)?;
                let name = wide_to_utf8(name);
                if watch.filter.is_empty() || watch.filter == name {
                    let mask = action_to_mask(action, watch.mask);
                    if mask != 0 {
                        let cookie = match action {
                            FILE_ACTION_RENAMED_OLD_NAME => {
                                watch.cookie = watch.cookie.wrapping_add(1).max(1);
                                watch.cookie
                            }
                            FILE_ACTION_RENAMED_NEW_NAME => watch.cookie,
                            _ => 0,
                        };
                        events.push(encode_event(
                            wd,
                            mask,
                            cookie,
                            if watch.filter.is_empty() { &name } else { &[] },
                        ));
                        if watch.mask & IN_ONESHOT != 0 {
                            ignored.push(wd);
                            break 'packets;
                        }
                    }
                }
                if next == 0 {
                    break;
                }
                if next < 12 || next > packet.len() - offset {
                    return Err(crate::EIO);
                }
                offset += next;
            }
        }
        if ended {
            if watch.mask & IN_DELETE_SELF != 0 {
                events.push(encode_event(wd, IN_DELETE_SELF, 0, &[]));
            }
            ignored.push(wd);
        }
    }
    for event in events {
        s.push(event);
    }
    ignored.sort_unstable();
    ignored.dedup();
    for wd in ignored {
        if let Some(watch) = s.watches.remove(&wd) {
            if watch.volume == 0 {
                s.push(encode_event(wd, IN_IGNORED, 0, &[]));
            }
            s.retired.push(watch);
        }
    }
    queues.retain(|wd, (id, _)| {
        s.watches
            .get(wd)
            .is_some_and(|watch| watch.volume == 0 && watch.id == *id)
    });
    memory_queues.retain(|wd, (id, _)| {
        s.watches
            .get(wd)
            .is_some_and(|watch| watch.volume != 0 && watch.id == *id)
    });
    Ok(())
}
pub fn rm_watch(fd: i32, wd: i32) -> Result<(), i32> {
    let store = owner(fd)?;
    update(&store, |s| {
        collect(&store, s)?;
        let watch = s.watches.remove(&wd).ok_or(crate::EINVAL)?;
        s.retired.push(watch);
        s.push(encode_event(wd, IN_IGNORED, 0, &[]));
        Ok(())
    })
}
pub fn read_inotify(fd: i32, buf: &mut [u8], nonblock: bool) -> Result<usize, i32> {
    let store = owner(fd)?;
    loop {
        let result = update(&store, |s| {
            collect(&store, s)?;
            if s.queue.is_empty() {
                return Ok(Err(crate::EAGAIN));
            }
            let mut written = 0;
            while let Some(event) = s.queue.front() {
                if event.len() > buf.len() - written {
                    if written == 0 {
                        return Ok(Err(crate::EINVAL));
                    }
                    break;
                }
                buf[written..written + event.len()].copy_from_slice(event);
                written += event.len();
                s.queue.pop_front();
            }
            Ok(Ok(written))
        })?;
        if result != Err(crate::EAGAIN)
            || nonblock
            || crate::get(fd)?.flags.contains(crate::FdFlags::NONBLOCK)
        {
            return result;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        if matches!(
            crate::signal::deliver_pending(),
            crate::signal::Delivery::Interrupted
        ) {
            return Err(crate::EINTR);
        }
    }
}
pub fn poll_inotify(fd: i32) -> Result<bool, i32> {
    let store = owner(fd)?;
    poll_store(&store)
}
pub(crate) fn poll_store(store: &Store) -> Result<bool, i32> {
    update(store, |s| {
        collect(store, s)?;
        Ok(!s.queue.is_empty())
    })
}

/// Mappings borrowed for one epoll wait. Init still owns directory I/O and
/// shared queue lifetime. An unchanged, empty native watch set can inspect
/// queue headers without decoding and re-encoding every watch on each scan.
pub(crate) struct PollView {
    store: Store,
    queues: HashMap<i32, (u64, DirectoryQueue)>,
    memory_queues: HashMap<i32, (u64, crate::tmpfs::watch::Watch)>,
    empty_native_revision: Option<u64>,
}
impl PollView {
    pub(crate) fn new(fd: i32) -> Result<Self, i32> {
        Ok(Self {
            store: owner(fd)?,
            queues: HashMap::new(),
            memory_queues: HashMap::new(),
            empty_native_revision: None,
        })
    }

    pub(crate) fn poll(&mut self) -> Result<bool, i32> {
        if let Some(revision) = self.empty_native_revision
            && self.store.revision() == revision
        {
            let mut pending = false;
            for (_, queue) in self.queues.values() {
                pending |= queue.pending().map_err(errno_from_io)?;
            }
            // A concurrent reader may have moved packets into the shared
            // inotify queue, or another worker may have changed the watches.
            if !pending && self.store.revision() == revision {
                return Ok(false);
            }
        }
        self.empty_native_revision = None;
        let (revision, (ready, native_only)) = update_observed(&self.store, |state| {
            collect_with_queues(state, &mut self.queues, &mut self.memory_queues)?;
            Ok((
                !state.queue.is_empty(),
                state.watches.values().all(|watch| watch.volume == 0),
            ))
        })?;
        if !ready && native_only {
            self.empty_native_revision = Some(revision);
        }
        Ok(ready)
    }
}
pub fn close_inotify(entry: crate::FdEntry, last_local: bool) {
    if kinakaze_runtime::authority::get().is_some() || !last_local {
        return;
    }
    if let Ok(store) = shared::object_entry(entry) {
        if let Ok(mut items) = local().lock() {
            items.retain(|(owner, _), _| *owner != store.id());
        }
    }
}

pub(crate) fn encode_event(wd: i32, mask: u32, cookie: u32, name: &[u8]) -> Vec<u8> {
    let name_len = if name.is_empty() {
        0u32
    } else {
        let with_nul = name.len() + 1;
        ((with_nul + 3) & !3) as u32
    };
    let total = 16 + name_len as usize;
    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&wd.to_ne_bytes());
    buf[4..8].copy_from_slice(&mask.to_ne_bytes());
    buf[8..12].copy_from_slice(&cookie.to_ne_bytes());
    buf[12..16].copy_from_slice(&name_len.to_ne_bytes());
    if !name.is_empty() {
        let copy_len = name.len().min(name_len as usize - 1);
        buf[16..16 + copy_len].copy_from_slice(&name[..copy_len]);
    }
    buf
}

fn action_to_mask(action: u32, watch_mask: u32) -> u32 {
    let candidate = match action {
        FILE_ACTION_ADDED => IN_CREATE,
        FILE_ACTION_REMOVED => IN_DELETE,
        FILE_ACTION_MODIFIED => IN_MODIFY | IN_ATTRIB,
        FILE_ACTION_RENAMED_OLD_NAME => IN_MOVED_FROM,
        FILE_ACTION_RENAMED_NEW_NAME => IN_MOVED_TO,
        _ => 0,
    };
    candidate & watch_mask
}

fn wide_to_utf8(wide: &[u8]) -> Vec<u8> {
    if wide.len() < 2 {
        return Vec::new();
    }
    let words: Vec<u16> = wide
        .chunks_exact(2)
        .map(|c| u16::from_ne_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&words).into_bytes()
}

fn errno_from_io(error: std::io::Error) -> i32 {
    error
        .raw_os_error()
        .map(|code| crate::errno_from_win32(code as u32))
        .unwrap_or(crate::EIO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inotify_init1_flags_and_descriptor_allocation() {
        let fd = create_inotify(IN_CLOEXEC | IN_NONBLOCK).expect("create_inotify");
        assert!(fd >= 0);

        let mut buf = [0u8; 64];
        // In nonblocking mode with empty queue, read returns EAGAIN
        assert_eq!(read_inotify(fd, &mut buf, true), Err(crate::EAGAIN));

        crate::close(fd).expect("close");
    }

    #[test]
    fn inotify_init1_rejects_unknown_flags() {
        assert_eq!(create_inotify(1), Err(crate::EINVAL));
    }

    #[test]
    fn inotify_encode_event_wire_format() {
        let encoded = encode_event(1, IN_CREATE, 0, b"test.txt");
        // inotify_event header is 16 bytes. "test.txt" is 8 bytes + 1 nul = 9 -> aligned to 12 bytes.
        // total size = 28 bytes.
        assert_eq!(encoded.len(), 28);

        let wd = i32::from_ne_bytes(encoded[0..4].try_into().unwrap());
        let mask = u32::from_ne_bytes(encoded[4..8].try_into().unwrap());
        let cookie = u32::from_ne_bytes(encoded[8..12].try_into().unwrap());
        let len = u32::from_ne_bytes(encoded[12..16].try_into().unwrap());

        assert_eq!(wd, 1);
        assert_eq!(mask, IN_CREATE);
        assert_eq!(cookie, 0);
        assert_eq!(len, 12);
        assert_eq!(&encoded[16..24], b"test.txt");
        assert_eq!(encoded[24], 0); // nul terminator
    }

    #[test]
    fn inotify_watch_lifecycle_and_rm_watch() {
        let temp_dir =
            std::env::temp_dir().join(format!("kinakaze-inotify-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);

        let fd = create_inotify(IN_NONBLOCK).expect("create_inotify");
        let wd =
            add_watch(fd, temp_dir.clone(), IN_CREATE | IN_DELETE | IN_MODIFY).expect("add_watch");
        assert!(wd >= 1);

        // Removing the watch should generate an IN_IGNORED event
        assert_eq!(rm_watch(fd, wd), Ok(()));

        let mut buf = [0u8; 128];
        let n = read_inotify(fd, &mut buf, false).expect("read IN_IGNORED event");
        assert!(n >= 16);

        let r_wd = i32::from_ne_bytes(buf[0..4].try_into().unwrap());
        let r_mask = u32::from_ne_bytes(buf[4..8].try_into().unwrap());
        assert_eq!(r_wd, wd);
        assert_ne!(r_mask & IN_IGNORED, 0);

        crate::close(fd).expect("close");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn poll_view_refreshes_watches_and_releases_retired_queues() {
        let temp_dir =
            std::env::temp_dir().join(format!("kinakaze-inotify-view-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let fd = create_inotify(IN_NONBLOCK).unwrap();
        let mut view = PollView::new(fd).unwrap();
        assert!(!view.poll().unwrap());
        for _ in 0..2 {
            let wd = add_watch(fd, temp_dir.clone(), IN_CREATE).unwrap();
            assert!(!view.poll().unwrap());
            assert_eq!(view.queues.len(), 1);
            // Exercise the unchanged empty fast path, then a native publisher
            // whose queue changed without changing the inotify watch table.
            assert!(!view.poll().unwrap());
            let path = temp_dir.join("published");
            std::fs::write(&path, b"event").unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !view.poll().unwrap() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let mut buffer = [0; 128];
            read_inotify(fd, &mut buffer, true).unwrap();
            std::fs::remove_file(path).unwrap();
            rm_watch(fd, wd).unwrap();
            assert!(view.poll().unwrap());
            assert!(view.queues.is_empty());
            let mut buffer = [0; 128];
            read_inotify(fd, &mut buffer, true).unwrap();
            assert!(!view.poll().unwrap());
        }
        drop(view);
        crate::close(fd).unwrap();
        std::fs::remove_dir(temp_dir).unwrap();
    }

    #[test]
    fn inotify_rejects_invalid_watch_operations() {
        let temp_dir = std::env::temp_dir().join(format!(
            "kinakaze-inotify-invalid-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_dir).expect("create temp directory");

        let fd = create_inotify(IN_NONBLOCK).expect("create_inotify");
        assert_eq!(add_watch(fd, temp_dir.clone(), 0), Err(crate::EINVAL));
        let wd = add_watch(fd, temp_dir.clone(), IN_CREATE).expect("add_watch");
        assert_eq!(
            add_watch(fd, temp_dir.clone(), IN_CREATE | IN_MASK_CREATE),
            Err(crate::EEXIST)
        );
        assert_eq!(rm_watch(fd, wd + 1), Err(crate::EINVAL));

        crate::close(fd).expect("close");
        std::fs::remove_dir_all(&temp_dir).expect("remove temp directory");
    }
}
