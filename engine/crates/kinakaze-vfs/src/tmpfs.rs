//! Shared tmpfs inodes and sparse, pagefile-backed file pages.
//! No native directory or disk file is used for either contents or metadata.
use crate::{
    fs::*,
    mount::shared::{self, Store},
    state_codec::{Reader, bytes, word},
    *,
};
use std::{
    collections::BTreeMap,
    ptr,
    sync::{Arc, Mutex},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE},
    System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEM_COMMIT, MEMORY_MAPPED_VIEW_ADDRESS,
        MapViewOfFile, OpenFileMappingW, PAGE_READWRITE, SEC_RESERVE, UnmapViewOfFile,
        VirtualAlloc,
    },
};
pub mod cgroupfs;
pub mod devpts;
pub mod mqueue;
mod observation;
pub mod sysfs;
pub(crate) use observation::Scope as Observation;
const PAGE: u64 = 4096;
static VOLUMES: Mutex<BTreeMap<u64, Arc<Volume>>> = Mutex::new(BTreeMap::new());
struct Volume {
    meta: Store,
    section: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    capacity: u64,
    message_queues: std::sync::atomic::AtomicBool,
    _network: Option<Arc<Store>>,
}
unsafe impl Send for Volume {}
unsafe impl Sync for Volume {}
impl Drop for Volume {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
            CloseHandle(self.section);
        }
    }
}
#[derive(Clone)]
struct Node {
    inode_number: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    size: u64,
    device: u64,
    atime: u64,
    mtime: u64,
    ctime: u64,
    parent: u64,
    target: String,
    children: BTreeMap<String, u64>,
    pages: BTreeMap<u64, u64>,
    opens: Vec<u64>,
}
struct State {
    limit: u64,
    capacity: u64,
    inodes: u64,
    next: u64,
    page_next: u64,
    free: Vec<u64>,
    nodes: BTreeMap<u64, Node>,
    pts: Option<devpts::Instance>,
    mq: Option<mqueue::Instance>,
    sys: Option<sysfs::Instance>,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}
impl Node {
    fn new(mode: u32, parent: u64) -> Self {
        let c = credentials::filesystem();
        let t = now();
        Self {
            inode_number: 0,
            mode,
            uid: c.uid,
            gid: c.gid,
            links: if mode & S_IFMT == S_IFDIR { 2 } else { 1 },
            size: 0,
            device: 0,
            atime: t,
            mtime: t,
            ctime: t,
            parent,
            target: String::new(),
            children: BTreeMap::new(),
            pages: BTreeMap::new(),
            opens: Vec::new(),
        }
    }
    fn stat(&self, id: u64, volume: u64) -> Stat {
        Stat {
            st_dev: device(volume),
            st_ino: if self.inode_number == 0 {
                id
            } else {
                self.inode_number
            },
            st_mode: self.mode,
            st_uid: user_namespace::visible(self.uid, false),
            st_gid: user_namespace::visible(self.gid, true),
            st_nlink: self.links,
            st_rdev: self.device,
            st_size: self.size as i64,
            st_blksize: PAGE as i64,
            st_blocks: self.pages.len() as i64 * 8,
            st_atime: (self.atime as i64).div_euclid(1_000_000_000),
            st_atime_nsec: (self.atime as i64).rem_euclid(1_000_000_000),
            st_mtime: (self.mtime as i64).div_euclid(1_000_000_000),
            st_mtime_nsec: (self.mtime as i64).rem_euclid(1_000_000_000),
            st_ctime: (self.ctime as i64).div_euclid(1_000_000_000),
            st_ctime_nsec: (self.ctime as i64).rem_euclid(1_000_000_000),
            ..Stat::default()
        }
    }
    fn access(&self, need: u32) -> Result<(), i32> {
        let c = credentials::filesystem();
        let granted = if c.uid == self.uid {
            self.mode >> 6
        } else if credentials::group_member(self.gid) {
            self.mode >> 3
        } else {
            self.mode
        };
        if granted & need == need
            || (c.uid == 0
                && (need & 1 == 0 || self.mode & S_IFMT == S_IFDIR || self.mode & 0o111 != 0))
        {
            Ok(())
        } else {
            Err(EACCES)
        }
    }
}
impl State {
    fn decode(data: &[u8]) -> Result<Self, i32> {
        let mut r = Reader(data);
        if r.word()? != 0x4359544d50465337 {
            return Err(EIO);
        }
        let mut s = Self {
            limit: r.word()?,
            capacity: r.word()?,
            inodes: r.word()?,
            next: r.word()?,
            page_next: r.word()?,
            free: Vec::new(),
            nodes: BTreeMap::new(),
            pts: None,
            mq: None,
            sys: None,
        };
        let count = r.word()?;
        if count > s.capacity / PAGE {
            return Err(EIO);
        }
        for _ in 0..count {
            s.free.push(r.word()?);
        }
        let count = r.word()?;
        if count > 1_000_000 {
            return Err(EIO);
        }
        for _ in 0..count {
            let id = r.word()?;
            let mut n = Node {
                inode_number: r.word()?,
                mode: r.word()? as u32,
                uid: r.word()? as u32,
                gid: r.word()? as u32,
                links: r.word()?,
                size: r.word()?,
                device: r.word()?,
                atime: r.word()?,
                mtime: r.word()?,
                ctime: r.word()?,
                parent: r.word()?,
                target: r.text()?,
                children: BTreeMap::new(),
                pages: BTreeMap::new(),
                opens: Vec::new(),
            };
            let children = r.word()?;
            if children > 1_000_000 {
                return Err(EIO);
            }
            for _ in 0..children {
                n.children.insert(r.text()?, r.word()?);
            }
            let pages = r.word()?;
            if pages > s.capacity / PAGE {
                return Err(EIO);
            }
            for _ in 0..pages {
                n.pages.insert(r.word()?, r.word()?);
            }
            let opens = r.word()?;
            if opens > 1_000_000 {
                return Err(EIO);
            }
            for _ in 0..opens {
                n.opens.push(r.word()?);
            }
            s.nodes.insert(id, n);
        }
        s.pts = devpts::Instance::decode(&mut r)?;
        s.mq = mqueue::Instance::decode(&mut r)?;
        s.sys = sysfs::Instance::decode(&mut r)?;
        r.end()?;
        Ok(s)
    }
    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [
            0x4359544d50465337,
            self.limit,
            self.capacity,
            self.inodes,
            self.next,
            self.page_next,
            self.free.len() as u64,
        ] {
            word(&mut b, v);
        }
        for v in &self.free {
            word(&mut b, *v);
        }
        word(&mut b, self.nodes.len() as u64);
        for (id, n) in &self.nodes {
            for v in [
                *id,
                n.inode_number,
                n.mode as u64,
                n.uid as u64,
                n.gid as u64,
                n.links,
                n.size,
                n.device,
                n.atime,
                n.mtime,
                n.ctime,
                n.parent,
            ] {
                word(&mut b, v);
            }
            bytes(&mut b, n.target.as_bytes());
            word(&mut b, n.children.len() as u64);
            for (name, id) in &n.children {
                bytes(&mut b, name.as_bytes());
                word(&mut b, *id);
            }
            word(&mut b, n.pages.len() as u64);
            for (page, slot) in &n.pages {
                word(&mut b, *page);
                word(&mut b, *slot);
            }
            word(&mut b, n.opens.len() as u64);
            for id in &n.opens {
                word(&mut b, *id);
            }
        }
        devpts::Instance::encode(&self.pts, &mut b);
        mqueue::Instance::encode(&self.mq, &mut b);
        sysfs::Instance::encode(&self.sys, &mut b);
        b
    }
    fn used(&self) -> u64 {
        self.page_next - self.free.len() as u64
    }
    fn collect(&mut self) {
        devpts::collect(self);
        mqueue::collect(self);
        for n in self.nodes.values_mut() {
            n.opens
                .retain(|id| !matches!(Store::user_object(*id, false), Err(ENOENT)));
        }
        let dead: Vec<_> = self
            .nodes
            .iter()
            .filter(|(id, n)| {
                n.links == 0
                    && n.opens.is_empty()
                    && self
                        .pts
                        .as_ref()
                        .is_none_or(|p| !p.terminals.values().any(|t| t.1 == **id))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in dead {
            if let Some(mq) = &mut self.mq {
                mq.queues.remove(&id);
            }
            if let Some(n) = self.nodes.remove(&id) {
                self.free.extend(n.pages.into_values());
            }
        }
    }
    fn resolve(&self, start: u64, path: &str) -> Result<u64, i32> {
        let mut id = start;
        for part in path.split('/').filter(|s| !s.is_empty() && *s != ".") {
            let n = self.nodes.get(&id).ok_or(ENOENT)?;
            if n.mode & S_IFMT != S_IFDIR {
                return Err(ENOTDIR);
            }
            n.access(1)?;
            id = if part == ".." {
                n.parent
            } else {
                *n.children.get(part).ok_or(ENOENT)?
            };
        }
        Ok(id)
    }
    fn parent(&self, start: u64, path: &str) -> Result<(u64, String), i32> {
        let path = path.trim_end_matches('/');
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        if name.is_empty() || matches!(name, "." | "..") {
            return Err(EINVAL);
        }
        if name.len() > 255 {
            return Err(ENAMETOOLONG);
        }
        let parent = self.resolve(start, parent)?;
        let n = self.nodes.get(&parent).ok_or(ENOENT)?;
        if n.mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
        n.access(3)?;
        Ok((parent, name.into()))
    }
    fn insert(&mut self, start: u64, path: &str, mut n: Node) -> Result<u64, i32> {
        if self.sys.as_ref().is_some_and(|i| i.cgroup) {
            return cgroupfs::create(self, start, path, n.mode);
        }
        if self.sys.is_some() {
            return Err(EPERM);
        }
        if self.mq.is_some() {
            return mqueue::insert(self, start, path, n);
        }
        if self.pts.is_some() {
            return Err(EPERM);
        }
        let (parent, name) = self.parent(start, path)?;
        if self.nodes[&parent].children.contains_key(&name) {
            return Err(EEXIST);
        }
        if self.nodes.len() as u64 >= self.inodes {
            return Err(ENOSPC);
        }
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or(ENOSPC)?;
        n.parent = parent;
        if self.nodes[&parent].mode & 0o2000 != 0 {
            n.gid = self.nodes[&parent].gid;
            if n.mode & S_IFMT == S_IFDIR {
                n.mode |= 0o2000;
            }
        }
        let p = self.nodes.get_mut(&parent).unwrap();
        p.children.insert(name, id);
        p.mtime = now();
        p.ctime = p.mtime;
        if n.mode & S_IFMT == S_IFDIR {
            p.links += 1;
        }
        self.nodes.insert(id, n);
        Ok(id)
    }
    fn sticky(&self, parent: u64, id: u64) -> Result<(), i32> {
        let c = credentials::filesystem();
        let p = &self.nodes[&parent];
        if p.mode & 0o1000 != 0 && c.uid != 0 && c.uid != p.uid && c.uid != self.nodes[&id].uid {
            Err(EPERM)
        } else {
            Ok(())
        }
    }
}
impl Volume {
    fn map(meta: Store, create: bool) -> Result<Arc<Self>, i32> {
        let s = State::decode(&meta.read()?.1)?;
        let network = s.sys.as_ref().map(sysfs::retain_namespace).transpose()?;
        let name: Vec<u16> = format!(
            "Local\\kinakaze.tmpfs.v1.{}.{}",
            kinakaze_runtime::authority::domain_id(),
            meta.id()
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let section = unsafe {
            if create {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_READWRITE | SEC_RESERVE,
                    (s.capacity >> 32) as u32,
                    s.capacity as u32,
                    name.as_ptr(),
                )
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name.as_ptr())
            }
        };
        if section.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let view = unsafe { MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
        if view.Value.is_null() {
            unsafe {
                CloseHandle(section);
            }
            return Err(ENOMEM);
        }
        Ok(Arc::new(Self {
            meta,
            section,
            view,
            capacity: s.capacity,
            message_queues: std::sync::atomic::AtomicBool::new(s.mq.is_some()),
            _network: network,
        }))
    }
    fn change<T>(&self, f: impl FnOnce(&mut State) -> Result<T, i32>) -> Result<T, i32> {
        self.change_observed(f).map(|(_, result)| result)
    }
    fn change_observed<T>(
        &self,
        f: impl FnOnce(&mut State) -> Result<T, i32>,
    ) -> Result<(Arc<State>, T), i32> {
        if !self
            .message_queues
            .load(std::sync::atomic::Ordering::Acquire)
            && self.decoded()?.mq.is_some()
        {
            self.message_queues
                .store(true, std::sync::atomic::Ordering::Release);
        }
        let _transaction = if self
            .message_queues
            .load(std::sync::atomic::Ordering::Acquire)
        {
            Some(mqueue::transaction(self.meta.id())?)
        } else {
            None
        };
        let (revision, (state, out)) = self.meta.update_with_revision(|bytes| {
            let mut s = State::decode(bytes)?;
            if s.sys.is_some() && observation::get(&self.meta, true).is_none() {
                sysfs::keep_catalog()?;
                sysfs::refresh(&mut s)?;
            }
            s.collect();
            let out = f(&mut s)?;
            Ok((s.encode(), (Arc::new(s), out)))
        })?;
        observation::put(&self.meta, revision, &state, true);
        Ok((state, out))
    }
    fn decoded(&self) -> Result<Arc<State>, i32> {
        if let Some(state) = observation::get(&self.meta, false) {
            return Ok(state);
        }
        let (revision, bytes) = self.meta.read()?;
        let state = Arc::new(State::decode(&bytes)?);
        observation::put(&self.meta, revision, &state, state.sys.is_none());
        Ok(state)
    }
    fn snapshot(&self) -> Result<Arc<State>, i32> {
        if let Some(state) = observation::get(&self.meta, true) {
            return Ok(state);
        }
        let state = self.decoded()?;
        if state.sys.is_none() {
            return Ok(state);
        }
        self.change_observed(|_| Ok(())).map(|(state, _)| state)
    }
    fn page(&self, slot: u64) -> Result<*mut u8, i32> {
        if slot >= self.capacity / PAGE {
            return Err(EIO);
        }
        Ok(unsafe { self.view.Value.cast::<u8>().add((slot * PAGE) as usize) })
    }
    fn allocate(&self, s: &mut State) -> Result<u64, i32> {
        if s.used() >= s.limit / PAGE {
            return Err(ENOSPC);
        }
        let slot = if let Some(slot) = s.free.pop() {
            slot
        } else {
            let slot = s.page_next;
            s.page_next += 1;
            slot
        };
        let address = self.page(slot)?;
        if unsafe { VirtualAlloc(address.cast(), PAGE as usize, MEM_COMMIT, PAGE_READWRITE) }
            .is_null()
        {
            return Err(ENOMEM);
        }
        unsafe {
            ptr::write_bytes(address, 0, PAGE as usize);
        }
        Ok(slot)
    }
    fn truncate(&self, s: &mut State, id: u64, length: u64) -> Result<(), i32> {
        if s.mq.is_some() || s.sys.is_some() {
            return Err(EINVAL);
        }
        let n = s.nodes.get_mut(&id).ok_or(ENOENT)?;
        if n.mode & S_IFMT != S_IFREG {
            return Err(EINVAL);
        }
        let removed = n.pages.split_off(&length.div_ceil(PAGE));
        s.free.extend(removed.into_values());
        if length % PAGE != 0 {
            if let Some(slot) = n.pages.get(&(length / PAGE)) {
                unsafe {
                    ptr::write_bytes(
                        self.page(*slot)?.add((length % PAGE) as usize),
                        0,
                        (PAGE - length % PAGE) as usize,
                    );
                }
            }
        }
        n.size = length;
        n.mtime = now();
        n.ctime = n.mtime;
        Ok(())
    }
}
fn volume(id: u64) -> Result<Arc<Volume>, i32> {
    let mut all = VOLUMES.lock().map_err(|_| EIO)?;
    if let Some(v) = all.get(&id) {
        return Ok(v.clone());
    }
    if std::env::var_os("KINAKAZE_TMPFS_TRACE").is_some() {
        eprintln!("tmpfs volume open host={} id={id}", std::process::id());
    }
    let v = Volume::map(Store::user_object(id, false)?, false)?;
    all.insert(id, v.clone());
    Ok(v)
}
pub(crate) fn device(id: u64) -> u64 {
    0x7400_0000_0000 | id
}
pub(crate) fn parse(source: &str) -> Result<(u64, u64), i32> {
    let (id, node) = source
        .strip_prefix("tmpfs:")
        .ok_or(EINVAL)?
        .split_once(':')
        .ok_or(EINVAL)?;
    Ok((
        id.parse().map_err(|_| EINVAL)?,
        node.parse().map_err(|_| EINVAL)?,
    ))
}
pub(crate) fn subtree(source: &str, tail: &str) -> Result<String, i32> {
    let (id, node) = parse(source)?;
    let v = volume(id)?;
    let s = v.snapshot()?;
    Ok(format!("tmpfs:{id}:{}", s.resolve(node, tail)?))
}
pub(crate) fn options(id: u64) -> Result<String, i32> {
    let v = volume(id)?;
    let s = v.snapshot()?;
    if s.mq.is_some() || s.sys.is_some() {
        return Ok("rw".into());
    }
    if let Some(p) = &s.pts {
        return Ok(p.options());
    }
    Ok(format!(
        "rw,size={}k,nr_inodes={},mode={:o}",
        s.limit / 1024,
        s.inodes,
        s.nodes[&1].mode & 0o7777
    ))
}
pub fn reopen(fd: i32, flags: i32) -> Result<i32, i32> {
    let (_, l, _, _) = descriptor(fd)?;
    let v = volume(l.volume)?;
    let s = v.snapshot()?;
    let n = s.nodes.get(&l.node).ok_or(ENOENT)?;
    if s.sys.is_some()
        && flags & O_PATH == 0
        && flags & O_ACCMODE != O_RDONLY
        && n.mode & S_IFMT == S_IFREG
    {
        sysfs::writable(&s, l.node)?;
    }
    if flags & O_DIRECTORY != 0 && n.mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    if flags & O_PATH == 0 {
        n.access(match flags & O_ACCMODE {
            O_RDONLY => 4,
            O_WRONLY => 2,
            O_RDWR => 6,
            _ => return Err(EINVAL),
        })?;
    }
    if n.mode & S_IFMT == S_IFCHR && flags & O_PATH == 0 {
        if l.flags & 4 != 0 {
            return Err(EACCES);
        }
        return devpts::open_character(&l, n, flags);
    }
    if flags & O_ACCMODE != O_RDONLY {
        l.writable()?;
    }
    let d = shared::new_object()?;
    d.update(|_| Ok((encode_descriptor(&l, 0, flags), ())))?;
    let f = fs::special_fd_flags(flags).union(FdFlags::SEEKABLE);
    v.change(|s| {
        s.nodes.get_mut(&l.node).ok_or(ENOENT)?.opens.push(d.id());
        Ok(())
    })?;
    d.descriptor_kind(
        if n.mode & S_IFMT == S_IFDIR {
            FdKind::TmpfsDirectory
        } else {
            match filesystem(l.volume)? {
                "sysfs" | "cgroup2" => FdKind::SysfsFile,
                "mqueue" => FdKind::MessageQueue,
                _ => FdKind::TmpfsFile,
            }
        },
        f,
    )
}
fn number(text: &str, percent: bool) -> Result<u64, i32> {
    let total = || crate::memory::snapshot().map(|m| m.total);
    if percent && text.ends_with('%') {
        let n: u64 = text[..text.len() - 1].parse().map_err(|_| EINVAL)?;
        return total()?.checked_mul(n).map(|v| v / 100).ok_or(EINVAL);
    }
    let (n, scale) = match text.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&text[..text.len() - 1], 1024),
        Some(b'm' | b'M') => (&text[..text.len() - 1], 1024 * 1024),
        Some(b'g' | b'G') => (&text[..text.len() - 1], 1024 * 1024 * 1024),
        _ => (text, 1),
    };
    n.parse::<u64>()
        .map_err(|_| EINVAL)?
        .checked_mul(scale)
        .ok_or(EINVAL)
}
pub(crate) fn prepare(options: &str) -> Result<String, i32> {
    let source = prepare_unkept(options)?;
    start_keeper(parse(&source)?.0)?;
    Ok(source)
}
fn prepare_unkept(options: &str) -> Result<String, i32> {
    let total = crate::memory::snapshot()?.total;
    let mut s = State {
        limit: total / 2,
        capacity: 0,
        inodes: (total / PAGE / 2).max(1),
        next: 2,
        page_next: 0,
        free: vec![],
        nodes: BTreeMap::new(),
        pts: None,
        mq: None,
        sys: None,
    };
    let mut root = Node::new(S_IFDIR | 0o1777, 1);
    for option in options.split(',').filter(|s| !s.is_empty()) {
        let (key, value) = option.split_once('=').unwrap_or((option, ""));
        match key {
            "size" => {
                s.limit = number(value, true)?
                    .checked_next_multiple_of(PAGE)
                    .ok_or(EINVAL)?
            }
            "nr_blocks" => s.limit = number(value, false)?.checked_mul(PAGE).ok_or(EINVAL)?,
            "nr_inodes" => s.inodes = number(value, false)?,
            "mode" => {
                root.mode = S_IFDIR | (u32::from_str_radix(value, 8).map_err(|_| EINVAL)? & 0o7777)
            }
            "uid" => root.uid = value.parse().map_err(|_| EINVAL)?,
            "gid" => root.gid = value.parse().map_err(|_| EINVAL)?,
            "inode64" | "inode32" if value.is_empty() => {}
            "huge" if value == "never" => {}
            "mpol" if value == "default" => {}
            "noswap" | "huge" | "mpol" => return Err(EOPNOTSUPP),
            _ => return Err(EINVAL),
        }
    }
    // Zero is Linux's unlimited setting; reserve up to the host's physical
    // memory and fail allocation explicitly if the host cannot commit pages.
    if s.limit == 0 {
        s.limit = total;
    }
    if s.inodes == 0 {
        s.inodes = u64::MAX;
    }
    s.capacity = s
        .limit
        .max(PAGE)
        .checked_next_multiple_of(65536)
        .ok_or(EINVAL)?;
    if s.capacity > isize::MAX as u64 {
        return Err(EINVAL);
    }
    s.nodes.insert(1, root);
    let meta = shared::new_object()?;
    let id = meta.id();
    meta.update(|_| Ok((s.encode(), ())))?;
    let v = Volume::map(meta, true)?;
    VOLUMES.lock().map_err(|_| EIO)?.insert(id, v);
    Ok(format!("tmpfs:{id}:1"))
}
#[derive(Clone)]
pub(crate) struct Location {
    pub volume: u64,
    pub node: u64,
    pub tail: String,
    pub namespace: u64,
    pub mount: u64,
    pub flags: u64,
    pub path: String,
}
impl Location {
    fn writable(&self) -> Result<(), i32> {
        if crate::mount::policy::get(self.namespace, self.mount, self.flags)?.flags() & 1 != 0 {
            Err(EROFS)
        } else {
            Ok(())
        }
    }
}
pub(crate) fn location(path: &str) -> Result<Option<Location>, i32> {
    let l = crate::mount::tmpfs_location(path)?;
    if let Some(l) = &l {
        if l.flags & crate::mount::MS_DEVPTS != 0 {
            volume(l.volume)?.change(|_| Ok(()))?;
        }
    }
    Ok(l)
}
pub fn filesystem(id: u64) -> Result<&'static str, i32> {
    let state = volume(id)?.decoded()?;
    if let Some(i) = &state.sys {
        return Ok(if i.cgroup { "cgroup2" } else { "sysfs" });
    }
    if state.mq.is_some() {
        return Ok("mqueue");
    }
    Ok(if state.pts.is_some() {
        "devpts"
    } else {
        "tmpfs"
    })
}
pub fn owns(path: &str) -> bool {
    location(path).is_ok_and(|p| p.is_some())
}
fn follow_path(path: &str, follow: bool) -> Result<String, i32> {
    let mut path = fs::absolute_linux(path);
    for _ in 0..40 {
        let Some(l) = location(&path)? else {
            return Ok(path);
        };
        let v = volume(l.volume)?;
        let s = v.snapshot()?;
        let parts: Vec<_> = l.tail.split('/').filter(|s| !s.is_empty()).collect();
        let mut id = l.node;
        let mut next = None;
        for (index, part) in parts.iter().enumerate() {
            let n = s.nodes.get(&id).ok_or(ENOENT)?;
            if n.mode & S_IFMT != S_IFDIR {
                return Err(ENOTDIR);
            }
            n.access(1)?;
            id = if *part == "." {
                id
            } else if *part == ".." {
                n.parent
            } else {
                match n.children.get(*part) {
                    Some(id) => *id,
                    None => return Ok(path),
                }
            };
            let n = s.nodes.get(&id).ok_or(EIO)?;
            if n.mode & S_IFMT == S_IFLNK && (follow || index + 1 != parts.len()) {
                let base = path.strip_suffix(&l.tail).ok_or(EIO)?;
                let mut target = if n.target.starts_with('/') {
                    n.target.clone()
                } else {
                    format!("{}{}/{}", base, parts[..index].join("/"), n.target)
                };
                if index + 1 < parts.len() {
                    target.push('/');
                    target.push_str(&parts[index + 1..].join("/"));
                }
                next = Some(fs::absolute_linux(&target));
                break;
            }
        }
        if let Some(next) = next {
            path = next;
        } else {
            return Ok(path);
        }
    }
    Err(ELOOP)
}
fn resolve_location(path: &str, follow: bool, _depth: u32) -> Result<Option<Location>, i32> {
    let path = follow_path(path, follow)?;
    let Some(mut l) = location(&path)? else {
        return Ok(None);
    };
    let v = volume(l.volume)?;
    let s = v.snapshot()?;
    l.node = s.resolve(l.node, &l.tail)?;
    l.tail.clear();
    Ok(Some(l))
}
pub fn stat(path: &str, follow: bool) -> Result<Option<Stat>, i32> {
    let _observation = Observation::enter();
    if !owns(path) {
        return Ok(None);
    }
    let destination = follow_path(path, follow)?;
    if !owns(&destination) {
        return (if follow {
            fs::stat(&destination)
        } else {
            fs::lstat(&destination)
        })
        .map(Some);
    }
    let path = destination.as_str();
    let Some(l) = resolve_location(path, follow, 0)? else {
        return Ok(None);
    };
    let v = volume(l.volume)?;
    let s = v.snapshot()?;
    Ok(Some(
        s.nodes.get(&l.node).ok_or(ENOENT)?.stat(l.node, l.volume),
    ))
}
fn descriptor(fd: i32) -> Result<(Store, Location, u64, i32), i32> {
    let entry = crate::get(fd)?;
    if !matches!(
        entry.kind,
        FdKind::TmpfsFile | FdKind::TmpfsDirectory | FdKind::MessageQueue | FdKind::SysfsFile
    ) {
        return Err(EBADF);
    }
    let store = shared::object_fd(fd)?;
    let data = store.read()?.1;
    let mut r = Reader(&data);
    let l = Location {
        volume: r.word()?,
        node: r.word()?,
        namespace: r.word()?,
        mount: r.word()?,
        flags: r.word()?,
        tail: String::new(),
        path: r.text()?,
    };
    let offset = r.word()?;
    let flags = r.word()? as i32;
    r.end()?;
    Ok((store, l, offset, flags))
}
fn encode_descriptor(l: &Location, offset: u64, flags: i32) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [l.volume, l.node, l.namespace, l.mount, l.flags] {
        word(&mut out, v);
    }
    bytes(&mut out, l.path.as_bytes());
    word(&mut out, offset);
    word(&mut out, flags as u64);
    out
}
pub fn descriptor_path(fd: i32) -> Result<String, i32> {
    Ok(descriptor(fd)?.1.path)
}
pub(crate) fn set_status(fd: i32, mask: i32, value: i32) -> Result<(), i32> {
    let (store, _, _, _) = descriptor(fd)?;
    store.update(|old| {
        let at = old.len().checked_sub(8).ok_or(EIO)?;
        let flags = u64::from_le_bytes(old[at..].try_into().map_err(|_| EIO)?) as i32;
        let mut bytes = old.to_vec();
        bytes[at..].copy_from_slice(&(((flags & !mask) | (value & mask)) as u64).to_le_bytes());
        Ok((bytes, ()))
    })
}
pub fn fstat(fd: i32) -> Result<Stat, i32> {
    let _observation = Observation::enter();
    let (_, l, _, _) = descriptor(fd)?;
    let v = volume(l.volume)?;
    let s = v.snapshot()?;
    Ok(s.nodes.get(&l.node).ok_or(ENOENT)?.stat(l.node, l.volume))
}
pub fn open(path: &str, flags: i32, mode: u32) -> Result<Option<i32>, i32> {
    let _observation = Observation::enter();
    if !owns(path) {
        return Ok(None);
    }
    if flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL)
        && resolve_location(path, false, 0).is_ok_and(|l| l.is_some())
    {
        return Err(EEXIST);
    }
    let destination = follow_path(path, flags & O_NOFOLLOW == 0)?;
    if !owns(&destination) {
        return fs::open(&destination, flags, mode).map(Some);
    }
    let path = destination.as_str();
    let Some(raw) = location(path)? else {
        return Ok(None);
    };
    let v = volume(raw.volume)?;
    let _transaction = if filesystem(raw.volume)? == "mqueue" {
        Some(mqueue::transaction(raw.volume)?)
    } else {
        None
    };
    let l = match resolve_location(path, flags & O_NOFOLLOW == 0, 0) {
        Ok(Some(l)) => {
            if flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL) {
                return Err(EEXIST);
            }
            l
        }
        Ok(None) => return Err(EOPNOTSUPP),
        Err(ENOENT) if flags & O_CREAT != 0 => {
            raw.writable()?;
            let mut l = raw.clone();
            l.node = v.change(|s| {
                s.insert(
                    raw.node,
                    &raw.tail,
                    Node::new(S_IFREG | (mode & 0o7777 & !fs_context::umask()), raw.node),
                )
            })?;
            l.tail.clear();
            l
        }
        Err(e) => return Err(e),
    };
    let n = v.snapshot()?.nodes.get(&l.node).ok_or(ENOENT)?.clone();
    let kind = n.mode & S_IFMT;
    if matches!(filesystem(l.volume)?, "sysfs" | "cgroup2")
        && flags & O_PATH == 0
        && flags & O_ACCMODE != O_RDONLY
        && kind == S_IFREG
    {
        sysfs::writable(&*v.snapshot()?, l.node)?;
    }
    if kind == S_IFLNK && flags & O_PATH == 0 {
        return Err(ELOOP);
    }
    if flags & O_DIRECTORY != 0 && kind != S_IFDIR {
        return Err(ENOTDIR);
    }
    if flags & O_PATH == 0 {
        if kind == S_IFDIR && flags & O_ACCMODE != O_RDONLY {
            return Err(EISDIR);
        }
        n.access(match flags & O_ACCMODE {
            O_RDONLY => 4,
            O_WRONLY => 2,
            O_RDWR => 6,
            _ => return Err(EINVAL),
        })?;
        if kind != S_IFCHR && (flags & O_ACCMODE != O_RDONLY || flags & O_TRUNC != 0) {
            l.writable()?;
        }
        if kind == S_IFCHR {
            if l.flags & 4 != 0 {
                return Err(EACCES);
            }
            return devpts::open_character(&l, &n, flags).map(Some);
        }
        if kind != S_IFDIR && kind != S_IFREG {
            return Err(EOPNOTSUPP);
        }
        if flags & O_TRUNC != 0
            && kind == S_IFREG
            && !matches!(filesystem(l.volume)?, "mqueue" | "sysfs" | "cgroup2")
        {
            v.change(|s| v.truncate(s, l.node, 0))?;
        }
    }
    let mut fdflags = fs::special_fd_flags(flags).union(FdFlags::SEEKABLE);
    if flags & O_CLOEXEC != 0 {
        fdflags = fdflags.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & O_PATH != 0 {
        fdflags = fdflags.union(FdFlags::PATH_ONLY);
    }
    if flags & O_APPEND != 0 {
        fdflags = fdflags.union(FdFlags::APPEND);
    }
    let description = shared::new_object()?;
    description.update(|_| Ok((encode_descriptor(&l, 0, flags), ())))?;
    v.change(|s| {
        s.nodes
            .get_mut(&l.node)
            .ok_or(ENOENT)?
            .opens
            .push(description.id());
        Ok(())
    })?;
    description
        .descriptor_kind(
            if kind == S_IFDIR {
                FdKind::TmpfsDirectory
            } else {
                match filesystem(l.volume)? {
                    "sysfs" | "cgroup2" => FdKind::SysfsFile,
                    "mqueue" => FdKind::MessageQueue,
                    _ => FdKind::TmpfsFile,
                }
            },
            fdflags,
        )
        .map(Some)
}
pub fn openat(fd: i32, path: &str, flags: i32, mode: u32) -> Result<i32, i32> {
    let _observation = Observation::enter();
    let (_, mut l, _, _) = descriptor(fd)?;
    if fstat(fd)?.st_mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    // Normal attachment resolution sees submounts; retained directory fds use
    // their pinned inode after detach, rather than the now-uncovered directory.
    let guest = format!("{}/{path}", l.path.trim_end_matches('/'));
    if resolve_location(&l.path, true, 0)?.is_some_and(|p| p.volume == l.volume && p.node == l.node)
    {
        return fs::open(&guest, flags, mode);
    }
    l.tail = path.into();
    open_pinned(l, flags, mode)
}
fn open_pinned(mut l: Location, flags: i32, mode: u32) -> Result<i32, i32> {
    let v = volume(l.volume)?;
    let _transaction = if filesystem(l.volume)? == "mqueue" {
        Some(mqueue::transaction(l.volume)?)
    } else {
        None
    };
    let id = v.change(|s| match s.resolve(l.node, &l.tail) {
        Ok(id) => {
            if flags & (O_CREAT | O_EXCL) == (O_CREAT | O_EXCL) {
                Err(EEXIST)
            } else {
                Ok(id)
            }
        }
        Err(ENOENT) if flags & O_CREAT != 0 => {
            l.writable()?;
            s.insert(
                l.node,
                &l.tail,
                Node::new(S_IFREG | (mode & 0o777 & !fs_context::umask()), l.node),
            )
        }
        Err(e) => Err(e),
    })?;
    let s = v.snapshot()?;
    let n = &s.nodes[&id];
    let kind = n.mode & S_IFMT;
    if matches!(filesystem(l.volume)?, "sysfs" | "cgroup2")
        && flags & O_PATH == 0
        && flags & O_ACCMODE != O_RDONLY
        && kind == S_IFREG
    {
        sysfs::writable(&*v.snapshot()?, id)?;
    }
    if flags & O_DIRECTORY != 0 && kind != S_IFDIR {
        return Err(ENOTDIR);
    }
    if kind == S_IFLNK && flags & (O_PATH | O_NOFOLLOW) != (O_PATH | O_NOFOLLOW) {
        return Err(ELOOP);
    }
    if flags & O_PATH == 0 {
        if kind == S_IFDIR && flags & O_ACCMODE != O_RDONLY {
            return Err(EISDIR);
        }
        n.access(match flags & O_ACCMODE {
            O_RDONLY => 4,
            O_WRONLY => 2,
            O_RDWR => 6,
            _ => return Err(EINVAL),
        })?;
    }
    if kind != S_IFCHR && flags & (O_TRUNC | O_WRONLY | O_RDWR) != 0 {
        l.writable()?;
    }
    if flags & O_TRUNC != 0
        && kind == S_IFREG
        && !matches!(filesystem(l.volume)?, "mqueue" | "sysfs" | "cgroup2")
    {
        v.change(|s| v.truncate(s, id, 0))?;
    }
    l.node = id;
    l.path = fs::absolute_linux(&format!("{}/{}", l.path.trim_end_matches('/'), l.tail));
    l.tail.clear();
    if kind == S_IFCHR && flags & O_PATH == 0 {
        if l.flags & 4 != 0 {
            return Err(EACCES);
        }
        return devpts::open_character(&l, n, flags);
    }
    let d = shared::new_object()?;
    d.update(|_| Ok((encode_descriptor(&l, 0, flags), ())))?;
    let mut f = fs::special_fd_flags(flags).union(FdFlags::SEEKABLE);
    if flags & O_CLOEXEC != 0 {
        f = f.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & O_PATH != 0 {
        f = f.union(FdFlags::PATH_ONLY);
    }
    v.change(|s| {
        s.nodes.get_mut(&l.node).ok_or(ENOENT)?.opens.push(d.id());
        Ok(())
    })?;
    d.descriptor_kind(
        if kind == S_IFDIR {
            FdKind::TmpfsDirectory
        } else {
            match filesystem(l.volume)? {
                "sysfs" | "cgroup2" => FdKind::SysfsFile,
                "mqueue" => FdKind::MessageQueue,
                _ => FdKind::TmpfsFile,
            }
        },
        f,
    )
}
pub fn openat_resolved(
    fd: i32,
    path: &str,
    flags: i32,
    mode: u32,
    resolve: u64,
) -> Result<i32, i32> {
    let _observation = Observation::enter();
    let (_, mut l, _, _) = descriptor(fd)?;
    if resolve & !63 != 0 || resolve & 24 == 24 {
        return Err(EINVAL);
    }
    if resolve & 32 != 0 {
        return Err(EAGAIN);
    }
    if path.starts_with('/') && resolve & 8 != 0 {
        return Err(EXDEV);
    }
    if path.starts_with('/') && resolve & 16 == 0 {
        return Err(EOPNOTSUPP);
    }
    let v = volume(l.volume)?;
    let s = v.snapshot()?;
    let mut parents = vec![l.node];
    let parts: Vec<_> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == ".." {
            if parents.len() == 1 {
                if resolve & 8 != 0 {
                    return Err(EXDEV);
                }
                if resolve & 16 != 0 {
                    continue;
                }
                return Err(EOPNOTSUPP);
            }
            parents.pop();
            continue;
        }
        let n = s.nodes.get(parents.last().ok_or(EIO)?).ok_or(ENOENT)?;
        if n.mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
        n.access(1)?;
        let candidate = format!("{}/{}", l.path.trim_end_matches('/'), parts[..=i].join("/"));
        if let Some(next) = location(&candidate)? {
            if next.mount != l.mount && next.volume != l.volume {
                if resolve & 1 != 0 {
                    return Err(EXDEV);
                }
                if parts[i + 1..].iter().any(|p| *p == "..") {
                    return Err(EOPNOTSUPP);
                }
                let fd = open(
                    &candidate,
                    if i + 1 == parts.len() {
                        flags
                    } else {
                        O_PATH | O_DIRECTORY
                    },
                    mode,
                )?
                .ok_or(ENOENT)?;
                if i + 1 == parts.len() {
                    return Ok(fd);
                }
                let result = openat_resolved(fd, &parts[i + 1..].join("/"), flags, mode, resolve);
                let _ = crate::close(fd);
                return result;
            }
        }
        let id = *n.children.get(*part).ok_or(ENOENT)?;
        let n = &s.nodes[&id];
        if n.mode & S_IFMT == S_IFLNK {
            let return_link =
                i + 1 == parts.len() && flags & (O_PATH | O_NOFOLLOW) == (O_PATH | O_NOFOLLOW);
            if !return_link {
                return Err(if resolve & 4 != 0 { ELOOP } else { EOPNOTSUPP });
            }
        }
        parents.push(id);
    }
    l.node = *parents.last().ok_or(EIO)?;
    l.path = format!(
        "{}/{}",
        l.path.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    l.tail.clear();
    open_pinned(l, flags, mode)
}
pub fn read(fd: i32, buffer: &mut [u8], position: Option<u64>) -> Result<usize, i32> {
    let _observation = Observation::enter();
    if crate::get(fd)?.kind == FdKind::MessageQueue {
        return mqueue::read_status(fd, buffer, position);
    }
    let (d, l, _, flags) = descriptor(fd)?;
    if flags & O_PATH != 0 || flags & O_ACCMODE == O_WRONLY {
        return Err(EBADF);
    }
    let v = volume(l.volume)?;
    d.update(|old| {
        let flags = u64::from_le_bytes(old[old.len() - 8..].try_into().unwrap()) as i32;
        let offset = u64::from_le_bytes(old[old.len() - 16..old.len() - 8].try_into().unwrap());
        let at = position.unwrap_or(offset);
        let count = v.change(|s| {
            if s.sys.is_some() {
                return sysfs::read(s, l.node, d.id(), buffer, at);
            }
            let n = s.nodes.get_mut(&l.node).ok_or(ENOENT)?;
            if n.mode & S_IFMT == S_IFDIR {
                return Err(EISDIR);
            }
            let count = buffer.len().min(n.size.saturating_sub(at) as usize);
            buffer[..count].fill(0);
            let mut done = 0;
            while done < count {
                let pos = at + done as u64;
                let part = (PAGE - pos % PAGE).min((count - done) as u64) as usize;
                if let Some(slot) = n.pages.get(&(pos / PAGE)) {
                    unsafe {
                        ptr::copy_nonoverlapping(
                            v.page(*slot)?.add((pos % PAGE) as usize),
                            buffer[done..].as_mut_ptr(),
                            part,
                        );
                    }
                }
                done += part;
            }
            if count != 0
                && flags & O_NOATIME == 0
                && l.flags & 1024 == 0
                && (l.flags & (1 << 24) != 0
                    || n.atime <= n.mtime
                    || n.atime <= n.ctime
                    || now().saturating_sub(n.atime) > 86_400_000_000_000)
            {
                n.atime = now();
            }
            Ok(count)
        })?;
        Ok((
            encode_descriptor(
                &l,
                if position.is_none() {
                    at + count as u64
                } else {
                    offset
                },
                flags,
            ),
            count,
        ))
    })
}
pub fn write(fd: i32, buffer: &[u8], position: Option<u64>) -> Result<usize, i32> {
    let _observation = Observation::enter();
    if crate::get(fd)?.kind == FdKind::MessageQueue {
        return Err(EINVAL);
    }
    let (d, l, _, flags) = descriptor(fd)?;
    if flags & O_PATH != 0 || flags & O_ACCMODE == O_RDONLY {
        return Err(EBADF);
    }
    l.writable()?;
    let v = volume(l.volume)?;
    d.update(|old| {
        let flags = u64::from_le_bytes(old[old.len() - 8..].try_into().unwrap()) as i32;
        let offset = u64::from_le_bytes(old[old.len() - 16..old.len() - 8].try_into().unwrap());
        let mut end = offset;
        let count = v.change(|s| {
            if s.sys.is_some() {
                let count = sysfs::write(s, l.node, buffer)?;
                s.sys.as_mut().unwrap().buffers_invalidate(d.id());
                end = position.unwrap_or(offset) + count as u64;
                return Ok(count);
            }
            let mut n = s.nodes.get(&l.node).ok_or(ENOENT)?.clone();
            if n.mode & S_IFMT != S_IFREG {
                return Err(EISDIR);
            }
            let at = if flags & O_APPEND != 0 {
                n.size
            } else {
                position.unwrap_or(offset)
            };
            let buffer = &buffer[..crate::limits::write_length(at, buffer.len())?];
            at.checked_add(buffer.len() as u64)
                .filter(|v| *v <= i64::MAX as u64)
                .ok_or(EINVAL)?;
            let mut done = 0;
            while done < buffer.len() {
                let pos = at + done as u64;
                let page = pos / PAGE;
                let part = (PAGE - pos % PAGE).min((buffer.len() - done) as u64) as usize;
                let slot = if let Some(slot) = n.pages.get(&page) {
                    *slot
                } else {
                    match v.allocate(s) {
                        Ok(slot) => {
                            n.pages.insert(page, slot);
                            slot
                        }
                        Err(e) if done == 0 => return Err(e),
                        Err(_) => break,
                    }
                };
                unsafe {
                    ptr::copy_nonoverlapping(
                        buffer[done..].as_ptr(),
                        v.page(slot)?.add((pos % PAGE) as usize),
                        part,
                    );
                }
                done += part;
            }
            end = at + done as u64;
            if done != 0 {
                n.size = n.size.max(end);
                n.mtime = now();
                n.ctime = n.mtime;
                n.mode &= !0o6000;
            }
            s.nodes.insert(l.node, n);
            Ok(done)
        })?;
        Ok((
            encode_descriptor(&l, if position.is_none() { end } else { offset }, flags),
            count,
        ))
    })
}
pub fn seek(fd: i32, offset: i64, whence: i32) -> Result<u64, i32> {
    let _observation = Observation::enter();
    let (d, l, _, flags) = descriptor(fd)?;
    if get(fd)?.kind == FdKind::SysfsFile
        && volume(l.volume)?
            .snapshot()?
            .nodes
            .get(&l.node)
            .is_none_or(|n| n.links == 0)
    {
        return Err(ENODEV);
    }
    if flags & O_PATH != 0 {
        return Err(EBADF);
    }
    d.update(|old| {
        let position = u64::from_le_bytes(old[old.len() - 16..old.len() - 8].try_into().unwrap());
        let base = match whence {
            0 => 0,
            1 => position,
            2 => fstat(fd)?.st_size as u64,
            _ => return Err(EINVAL),
        };
        let next = base
            .checked_add_signed(offset)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or(EINVAL)?;
        Ok((encode_descriptor(&l, next, flags), next))
    })
}
pub fn truncate(fd: i32, length: i64) -> Result<(), i32> {
    if crate::get(fd)?.kind == FdKind::MessageQueue {
        return Err(EINVAL);
    }
    if length < 0 {
        return Err(EINVAL);
    }
    let (_, l, _, flags) = descriptor(fd)?;
    if flags & O_PATH != 0 {
        return Err(EBADF);
    }
    if flags & O_ACCMODE == O_RDONLY {
        return Err(EINVAL);
    }
    l.writable()?;
    crate::limits::truncate(length as u64)?;
    let v = volume(l.volume)?;
    v.change(|s| v.truncate(s, l.node, length as u64))
}
pub fn create(path: &str, mode: u32, device: u64, target: &str) -> Result<bool, i32> {
    let Some(l) = location(path)? else {
        return Ok(false);
    };
    l.writable()?;
    let mut n = Node::new(mode, l.node);
    n.device = device;
    n.target = target.into();
    if mode & S_IFMT == S_IFLNK {
        n.size = target.len() as u64;
    }
    volume(l.volume)?.change(|s| {
        s.insert(l.node, &l.tail, n)?;
        Ok(())
    })?;
    Ok(true)
}
pub fn unlink(path: &str, directory: bool) -> Result<bool, i32> {
    let Some(l) = location(path)? else {
        return Ok(false);
    };
    l.writable()?;
    volume(l.volume)?.change(|s| {
        if s.sys.as_ref().is_some_and(|i| i.cgroup) {
            return cgroupfs::remove(s, l.node, &l.tail, directory);
        }
        if s.pts.is_some() || s.sys.is_some() {
            return Err(EPERM);
        }
        let (parent, name) = s.parent(l.node, &l.tail)?;
        let id = *s.nodes[&parent].children.get(&name).ok_or(ENOENT)?;
        s.sticky(parent, id)?;
        let n = &s.nodes[&id];
        if directory {
            if n.mode & S_IFMT != S_IFDIR {
                return Err(ENOTDIR);
            }
            if !n.children.is_empty() {
                return Err(ENOTEMPTY);
            }
        } else if n.mode & S_IFMT == S_IFDIR {
            return Err(EISDIR);
        }
        let p = s.nodes.get_mut(&parent).unwrap();
        p.children.remove(&name);
        if directory {
            p.links -= 1;
        }
        p.mtime = now();
        p.ctime = p.mtime;
        let n = s.nodes.get_mut(&id).unwrap();
        n.links = if directory { 0 } else { n.links - 1 };
        n.ctime = now();
        Ok(())
    })?;
    Ok(true)
}
pub fn hard_link(from: &str, to: &str) -> Result<bool, i32> {
    let (a, b) = (resolve_location(from, false, 0)?, location(to)?);
    if a.is_none() && b.is_none() {
        return Ok(false);
    }
    let (a, b) = (a.ok_or(EXDEV)?, b.ok_or(EXDEV)?);
    if a.volume != b.volume {
        return Err(EXDEV);
    }
    b.writable()?;
    volume(a.volume)?.change(|s| {
        if s.pts.is_some() || s.mq.is_some() || s.sys.is_some() {
            return Err(EPERM);
        }
        if s.nodes[&a.node].mode & S_IFMT == S_IFDIR {
            return Err(EPERM);
        }
        let (parent, name) = s.parent(b.node, &b.tail)?;
        if s.nodes[&parent].children.contains_key(&name) {
            return Err(EEXIST);
        }
        s.nodes
            .get_mut(&parent)
            .unwrap()
            .children
            .insert(name, a.node);
        s.nodes.get_mut(&a.node).unwrap().links += 1;
        Ok(())
    })?;
    Ok(true)
}
pub fn rename(from: &str, to: &str, flags: u32) -> Result<bool, i32> {
    let (a, b) = (location(from)?, location(to)?);
    if a.is_none() && b.is_none() {
        return Ok(false);
    }
    let (a, b) = (a.ok_or(EXDEV)?, b.ok_or(EXDEV)?);
    if a.volume != b.volume || a.mount != b.mount {
        return Err(EXDEV);
    }
    if flags & !3 != 0 || flags == 3 {
        return Err(EINVAL);
    }
    a.writable()?;
    b.writable()?;
    volume(a.volume)?.change(|s| {
        if s.pts.is_some() || s.mq.is_some() || s.sys.is_some() {
            return Err(EPERM);
        }
        let (ap, an) = s.parent(a.node, &a.tail)?;
        let (bp, bn) = s.parent(b.node, &b.tail)?;
        let id = *s.nodes[&ap].children.get(&an).ok_or(ENOENT)?;
        let other = s.nodes[&bp].children.get(&bn).copied();
        if other == Some(id) {
            return Ok(());
        }
        s.sticky(ap, id)?;
        if flags & 1 != 0 && other.is_some() {
            return Err(EEXIST);
        }
        if flags & 2 != 0 && other.is_none() {
            return Err(ENOENT);
        }
        let is_dir = s.nodes[&id].mode & S_IFMT == S_IFDIR;
        let mut ancestor = bp;
        while ancestor != 1 {
            if ancestor == id {
                return Err(EINVAL);
            }
            ancestor = s.nodes[&ancestor].parent;
        }
        if let Some(other) = other {
            s.sticky(bp, other)?;
            let n = &s.nodes[&other];
            if flags & 2 == 0 {
                if is_dir && n.mode & S_IFMT != S_IFDIR {
                    return Err(ENOTDIR);
                }
                if !is_dir && n.mode & S_IFMT == S_IFDIR {
                    return Err(EISDIR);
                }
                if !n.children.is_empty() {
                    return Err(ENOTEMPTY);
                }
                let n = s.nodes.get_mut(&other).unwrap();
                n.links = if is_dir { 0 } else { n.links - 1 };
                if is_dir {
                    s.nodes.get_mut(&bp).unwrap().links -= 1;
                }
            } else {
                let mut ancestor = ap;
                while ancestor != 1 {
                    if ancestor == other {
                        return Err(EINVAL);
                    }
                    ancestor = s.nodes[&ancestor].parent;
                }
                s.nodes
                    .get_mut(&ap)
                    .unwrap()
                    .children
                    .insert(an.clone(), other);
                s.nodes.get_mut(&other).unwrap().parent = ap;
            }
        }
        if flags & 2 == 0 {
            s.nodes.get_mut(&ap).unwrap().children.remove(&an);
        }
        s.nodes.get_mut(&bp).unwrap().children.insert(bn, id);
        s.nodes.get_mut(&id).unwrap().parent = bp;
        if ap != bp && is_dir {
            s.nodes.get_mut(&ap).unwrap().links -= 1;
            s.nodes.get_mut(&bp).unwrap().links += 1;
        }
        if flags & 2 != 0
            && ap != bp
            && other.is_some_and(|id| s.nodes[&id].mode & S_IFMT == S_IFDIR)
        {
            s.nodes.get_mut(&bp).unwrap().links -= 1;
            s.nodes.get_mut(&ap).unwrap().links += 1;
        }
        for id in [ap, bp, id] {
            let n = s.nodes.get_mut(&id).unwrap();
            n.ctime = now();
            n.mtime = n.ctime;
        }
        s.collect();
        Ok(())
    })?;
    Ok(true)
}
pub(crate) fn resize(source: &str, options: &str) -> Result<(), i32> {
    let (id, _) = parse(source)?;
    let v = volume(id)?;
    v.change(|s| {
        for option in options.split(',').filter(|s| !s.is_empty()) {
            let (key, value) = option.split_once('=').ok_or(EINVAL)?;
            match key {
                "size" => {
                    let limit = number(value, true)?
                        .checked_next_multiple_of(PAGE)
                        .ok_or(EINVAL)?;
                    if limit < s.used() * PAGE {
                        return Err(EINVAL);
                    }
                    if limit > s.capacity {
                        return Err(EOPNOTSUPP);
                    }
                    s.limit = limit;
                }
                "nr_blocks" => {
                    let limit = number(value, false)?.checked_mul(PAGE).ok_or(EINVAL)?;
                    if limit < s.used() * PAGE {
                        return Err(EINVAL);
                    }
                    if limit > s.capacity {
                        return Err(EOPNOTSUPP);
                    }
                    s.limit = limit;
                }
                "nr_inodes" => {
                    let count = number(value, false)?;
                    if count < s.nodes.len() as u64 {
                        return Err(EINVAL);
                    }
                    s.inodes = count;
                }
                "mode" | "uid" | "gid" => {} // Linux ignores root metadata on remount.
                _ => return Err(EINVAL),
            }
        }
        Ok(())
    })
}
pub fn read_link(path: &str) -> Result<Option<String>, i32> {
    let _observation = Observation::enter();
    let Some(l) = resolve_location(path, false, 0)? else {
        return Ok(None);
    };
    let v = volume(l.volume)?;
    let s = v.snapshot()?;
    let n = &s.nodes[&l.node];
    if n.mode & S_IFMT != S_IFLNK {
        return Err(EINVAL);
    }
    Ok(Some(n.target.clone()))
}
pub fn read_link_fd(fd: i32) -> Result<String, i32> {
    let _observation = Observation::enter();
    let (_, l, _, _) = descriptor(fd)?;
    let v = volume(l.volume)?;
    let s = v.snapshot()?;
    let n = &s.nodes[&l.node];
    if n.mode & S_IFMT != S_IFLNK {
        return Err(EINVAL);
    }
    Ok(n.target.clone())
}
fn directory(l: &Location) -> Result<Vec<DirectoryEntry>, i32> {
    let v = volume(l.volume)?;
    v.change(|_| Ok(()))?;
    let s = v.snapshot()?;
    let n = s.nodes.get(&l.node).ok_or(ENOENT)?;
    if n.mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    n.access(4)?;
    let mut out = Vec::new();
    for (name, id) in [(".".to_owned(), l.node), ("..".to_owned(), n.parent)]
        .into_iter()
        .chain(n.children.iter().map(|(name, id)| (name.clone(), *id)))
    {
        let node = &s.nodes[&id];
        out.push(DirectoryEntry {
            name,
            is_directory: node.mode & S_IFMT == S_IFDIR,
            is_symlink: node.mode & S_IFMT == S_IFLNK,
        });
    }
    Ok(out)
}
pub fn read_directory(path: &str) -> Result<Option<Vec<DirectoryEntry>>, i32> {
    let _observation = Observation::enter();
    resolve_location(path, true, 0)?
        .map(|l| directory(&l))
        .transpose()
}
pub fn read_directory_fd(fd: i32) -> Result<Vec<DirectoryEntry>, i32> {
    let _observation = Observation::enter();
    directory(&descriptor(fd)?.1)
}
pub fn read_directory_bytes(fd: i32, buffer: &mut [u8], wide: bool) -> Result<usize, i32> {
    let _observation = Observation::enter();
    let (d, l, _, flags) = descriptor(fd)?;
    if flags & O_PATH != 0 {
        return Err(EBADF);
    }
    let v = volume(l.volume)?;
    v.change(|_| Ok(()))?;
    let s = v.snapshot()?;
    let n = s.nodes.get(&l.node).ok_or(ENOENT)?;
    if n.mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    n.access(4)?;
    let entries: Vec<_> = [(".".to_string(), l.node), ("..".to_string(), n.parent)]
        .into_iter()
        .chain(n.children.iter().map(|(name, id)| (name.clone(), *id)))
        .collect();
    d.update(|old| {
        let mut offset = u64::from_le_bytes(old[old.len() - 16..old.len() - 8].try_into().unwrap());
        let mut count = 0;
        for (name, id) in entries.iter().skip(offset as usize) {
            let len = (name.len() + 20).next_multiple_of(8);
            if count + len > buffer.len() {
                if count == 0 {
                    return Err(EINVAL);
                }
                break;
            }
            let record = &mut buffer[count..count + len];
            record.fill(0);
            offset += 1;
            record[..8].copy_from_slice(&s.nodes[id].stat(*id, l.volume).st_ino.to_le_bytes());
            record[8..16].copy_from_slice(&offset.to_le_bytes());
            record[16..18].copy_from_slice(&(len as u16).to_le_bytes());
            let kind = ((s.nodes[id].mode & S_IFMT) >> 12) as u8;
            let start = if wide {
                record[18] = kind;
                19
            } else {
                record[len - 1] = kind;
                18
            };
            record[start..start + name.len()].copy_from_slice(name.as_bytes());
            count += len;
        }
        Ok((encode_descriptor(&l, offset, flags), count))
    })
}
pub fn access(path: &str, mode: i32) -> Result<bool, i32> {
    let _observation = Observation::enter();
    let Some(l) = resolve_location(path, true, 0)? else {
        return Ok(false);
    };
    if mode & !7 != 0 {
        return Err(EINVAL);
    }
    if mode & 2 != 0 {
        l.writable()?;
    }
    let v = volume(l.volume)?;
    let s = v.snapshot()?;
    s.nodes.get(&l.node).ok_or(ENOENT)?.access(mode as u32)?;
    Ok(true)
}
pub fn set_times(
    path: Option<&str>,
    fd: i32,
    follow: bool,
    times: [[i64; 2]; 2],
) -> Result<(), i32> {
    let l = match path {
        Some(path) => resolve_location(path, follow, 0)?.ok_or(ENOENT)?,
        None => descriptor(fd)?.1,
    };
    l.writable()?;
    volume(l.volume)?.change(|s| {
        let n = s.nodes.get_mut(&l.node).ok_or(ENOENT)?;
        let caller = credentials::filesystem().uid;
        if caller != 0 && caller != n.uid {
            if times
                .iter()
                .all(|t| matches!(t[1], 1073741822 | 1073741823))
            {
                n.access(2)?;
            } else {
                return Err(EPERM);
            }
        }
        let mut values = [n.atime, n.mtime];
        for (i, t) in times.iter().enumerate() {
            values[i] = match t[1] {
                1073741822 => values[i],
                1073741823 => now(),
                0..1_000_000_000 => t[0]
                    .checked_mul(1_000_000_000)
                    .and_then(|v| v.checked_add(t[1]))
                    .ok_or(EOVERFLOW)? as u64,
                _ => return Err(EINVAL),
            };
        }
        n.atime = values[0];
        n.mtime = values[1];
        n.ctime = now();
        Ok(())
    })
}
pub fn chmod(path: &str, mode: u32) -> Result<bool, i32> {
    let Some(l) = resolve_location(path, true, 0)? else {
        return Ok(false);
    };
    change_mode(l, mode)?;
    Ok(true)
}
fn change_mode(l: Location, mode: u32) -> Result<(), i32> {
    l.writable()?;
    if matches!(filesystem(l.volume)?, "sysfs" | "cgroup2") {
        return Err(EPERM);
    }
    volume(l.volume)?.change(|s| {
        let n = s.nodes.get_mut(&l.node).ok_or(ENOENT)?;
        if credentials::filesystem().uid != n.uid && credentials::filesystem().uid != 0 {
            return Err(EPERM);
        }
        n.mode = (n.mode & S_IFMT) | (mode & 0o7777);
        n.ctime = now();
        Ok(())
    })
}
pub fn fchmod(fd: i32, mode: u32) -> Result<(), i32> {
    chmod_descriptor(fd, mode, false)
}
pub(crate) fn chmod_descriptor(fd: i32, mode: u32, allow_path: bool) -> Result<(), i32> {
    let (_, l, _, flags) = descriptor(fd)?;
    if !allow_path && flags & O_PATH != 0 {
        return Err(EBADF);
    }
    if volume(l.volume)?
        .snapshot()?
        .nodes
        .get(&l.node)
        .ok_or(ENOENT)?
        .mode
        & S_IFMT
        == S_IFLNK
    {
        return Err(EOPNOTSUPP);
    }
    change_mode(l, mode)
}
fn change_owner(l: Location, owner: &Ownership) -> Result<(), i32> {
    l.writable()?;
    if matches!(filesystem(l.volume)?, "sysfs" | "cgroup2") {
        return Err(EPERM);
    }
    volume(l.volume)?.change(|s| {
        let n = s.nodes.get_mut(&l.node).ok_or(ENOENT)?;
        owner.check(&n.stat(l.node, l.volume))?;
        if owner.uid != u32::MAX {
            n.uid = user_namespace::kernel(owner.uid, false)?;
        }
        if owner.gid != u32::MAX {
            n.gid = user_namespace::kernel(owner.gid, true)?;
        }
        n.mode &= !0o6000;
        n.ctime = now();
        Ok(())
    })
}
pub fn chown(path: &str, follow: bool, owner: &Ownership) -> Result<bool, i32> {
    let Some(l) = resolve_location(path, follow, 0)? else {
        return Ok(false);
    };
    change_owner(l, owner)?;
    Ok(true)
}
pub fn fchown(fd: i32, owner: &Ownership) -> Result<(), i32> {
    change_owner(descriptor(fd)?.1, owner)
}
pub struct Statistics {
    pub magic: i64,
    pub device: u64,
    pub blocks: u64,
    pub free: u64,
    pub inodes: u64,
    pub free_inodes: u64,
    pub flags: u64,
}
fn statistics(l: &Location) -> Result<Statistics, i32> {
    let v = volume(l.volume)?;
    let s = v.change(|s| Ok(State::decode(&s.encode())?))?;
    if s.sys.is_some() {
        return Ok(Statistics {
            magic: if s.sys.as_ref().unwrap().cgroup {
                0x63677270
            } else {
                0x62656572
            },
            device: device(l.volume),
            blocks: 0,
            free: 0,
            inodes: 0,
            free_inodes: 0,
            flags: l.flags,
        });
    }
    if s.mq.is_some() {
        return Ok(Statistics {
            magic: 0x19800202,
            device: device(l.volume),
            blocks: 0,
            free: 0,
            inodes: 0,
            free_inodes: 0,
            flags: l.flags,
        });
    }
    if s.pts.is_some() {
        return Ok(Statistics {
            magic: 0x1cd1,
            device: device(l.volume),
            blocks: 0,
            free: 0,
            inodes: 0,
            free_inodes: 0,
            flags: crate::mount::policy::get(l.namespace, l.mount, l.flags)?.flags(),
        });
    }
    Ok(Statistics {
        magic: if s.pts.is_some() { 0x1cd1 } else { 0x01021994 },
        device: device(l.volume),
        blocks: s.limit / PAGE,
        free: (s.limit / PAGE).saturating_sub(s.used()),
        inodes: s.inodes,
        free_inodes: s.inodes.saturating_sub(s.nodes.len() as u64),
        flags: crate::mount::policy::get(l.namespace, l.mount, l.flags)?.flags(),
    })
}
pub fn statfs(path: &str) -> Result<Option<Statistics>, i32> {
    resolve_location(path, true, 0)?
        .map(|l| statistics(&l))
        .transpose()
}
pub fn fstatfs(fd: i32) -> Result<Statistics, i32> {
    statistics(&descriptor(fd)?.1)
}
pub(crate) fn serialize() -> Result<Vec<u8>, i32> {
    let mut b = Vec::new();
    for id in VOLUMES.lock().map_err(|_| EIO)?.keys() {
        word(&mut b, *id);
    }
    Ok(b)
}
pub(crate) fn restore(data: &[u8]) -> bool {
    if data.len() % 8 != 0 {
        return false;
    }
    data.chunks_exact(8)
        .all(|b| volume(u64::from_le_bytes(b.try_into().unwrap())).is_ok())
}
pub(crate) fn reference(source: &str, object: u64) -> Result<(), i32> {
    let (id, _) = parse(source)?;
    volume(id)?.change(|s| {
        s.nodes.get_mut(&1).ok_or(EIO)?.opens.push(object);
        Ok(())
    })
}
fn start_keeper(id: u64) -> Result<(), i32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ, GENERIC_WRITE},
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{
            CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile,
        },
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE},
            Pipes::{CreatePipe, PeekNamedPipe},
            Threading::*,
        },
    };
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    struct Attributes(Vec<usize>);
    impl Drop for Attributes {
        fn drop(&mut self) {
            unsafe {
                DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast());
            }
        }
    }
    let executable = std::env::current_exe().map_err(|_| EIO)?;
    if executable.file_stem().is_none_or(|s| s != "elf-loader") {
        return Ok(());
    }
    let trace = std::env::var_os("KINAKAZE_TMPFS_TRACE").is_some();
    if trace {
        eprintln!("tmpfs keeper start host={} id={id}", std::process::id());
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let (mut input, mut output) = (ptr::null_mut(), ptr::null_mut());
    if unsafe { CreatePipe(&mut input, &mut output, &security, 0) } == 0 {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let (input, output) = (Handle(input), Handle(output));
    crate::platform::try_set_inheritable(input.0 as usize, false)?;
    let null = unsafe {
        CreateFileW(
            windows_sys::core::w!("NUL"),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &security,
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    };
    if null == INVALID_HANDLE_VALUE {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let null = Handle(null);
    let mut error = None;
    if trace {
        let mut raw = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetStdHandle(STD_ERROR_HANDLE),
                GetCurrentProcess(),
                &mut raw,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            )
        } != 0
        {
            error = Some(Handle(raw));
        }
    }
    let mut inherited = vec![output.0, null.0];
    if let Some(error) = &error {
        inherited.push(error.0);
    }
    let mut size = 0;
    unsafe {
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
    }
    if size == 0 {
        return Err(EIO);
    }
    let mut buffer = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
    if unsafe { InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), 1, 0, &mut size) }
        == 0
    {
        return Err(EIO);
    }
    let mut attributes = Attributes(buffer);
    if unsafe {
        UpdateProcThreadAttribute(
            attributes.0.as_mut_ptr().cast(),
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inherited.as_ptr().cast(),
            inherited.len() * std::mem::size_of::<HANDLE>(),
            ptr::null_mut(),
            ptr::null(),
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let mut startup = unsafe { std::mem::zeroed::<STARTUPINFOEXW>() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = null.0;
    startup.StartupInfo.hStdOutput = output.0;
    startup.StartupInfo.hStdError = error.as_ref().map_or(null.0, |e| e.0);
    startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
    let executable_wide: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut command: Vec<u16> = format!("\"{}\" --tmpfs-keeper {id}", executable.display())
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut process = unsafe { std::mem::zeroed::<PROCESS_INFORMATION>() };
    if unsafe {
        CreateProcessW(
            executable_wide.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            ptr::null(),
            ptr::null(),
            &startup.StartupInfo,
            &mut process,
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    unsafe {
        CloseHandle(process.hThread);
    }
    let process = Handle(process.hProcess);
    drop(output);
    // A helper must acknowledge its mapping before the creator releases it.
    let start = std::time::Instant::now();
    loop {
        let mut available = 0;
        let peek = unsafe {
            PeekNamedPipe(
                input.0,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        };
        if peek != 0 && available != 0 {
            let (mut ready, mut count) = (0u8, 0);
            if unsafe {
                ReadFile(
                    input.0,
                    (&mut ready as *mut u8).cast(),
                    1,
                    &mut count,
                    ptr::null_mut(),
                )
            } != 0
                && count == 1
                && ready == 1
            {
                return Ok(());
            }
            break;
        }
        if start.elapsed().as_secs() >= 5 || unsafe { WaitForSingleObject(process.0, 5) } != 258 {
            break;
        }
    }
    unsafe {
        TerminateProcess(process.0, 1);
        WaitForSingleObject(process.0, 5000);
    }
    Err(EIO)
}
/// Keep kernel sections alive independently of the process which mounted them.
/// This helper does not register a guest PID. A sysfs volume pins its network
/// namespace until its mounts and open descriptions are no longer referenced.
pub fn run_keeper() {
    use std::io::Write;
    // Keep the ID allocator alive for exactly as long as its volume objects.
    let Ok(_allocator) = shared::initial() else {
        return;
    };
    let Some(id) = std::env::args().nth(2).and_then(|s| s.parse::<u64>().ok()) else {
        return;
    };
    let Ok(v) = volume(id) else {
        return;
    };
    let mut output = std::io::stdout().lock();
    if output.write_all(&[1]).and_then(|_| output.flush()).is_err() {
        return;
    }
    drop(output);
    if std::env::var_os("KINAKAZE_TMPFS_TRACE").is_some() {
        eprintln!("tmpfs keeper ready host={} id={id}", std::process::id());
    }
    let start = std::time::Instant::now();
    let mut attached_namespace = None;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if kinakaze_runtime::job::all_process_info().is_empty() {
            if std::env::var_os("KINAKAZE_TMPFS_TRACE").is_some() {
                eprintln!("tmpfs keeper exit id={id}: no guest rows");
            }
            break;
        }
        if start.elapsed().as_secs() < 3 {
            continue;
        }
        let opened = v
            .change(|s| {
                Ok(s.nodes.values().any(|n| !n.opens.is_empty())
                    || s.pts.as_ref().is_some_and(|p| !p.terminals.is_empty())
                    || s.mq
                        .as_ref()
                        .is_some_and(|m| mqueue::namespace_alive(m.ipc)))
            })
            .unwrap_or(true);
        if !opened && !crate::mount::tmpfs_attached(id, &mut attached_namespace).unwrap_or(true) {
            if std::env::var_os("KINAKAZE_TMPFS_TRACE").is_some() {
                eprintln!("tmpfs keeper exit id={id}: no mount/open refs");
            }
            break;
        }
    }
}
