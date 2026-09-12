//! FD-based filesystem contexts and detached mount trees.
//! Configuration updates use the same two-bank native publication mechanism as
//! mount namespaces, including after a descriptor crosses fork or exec.
use super::*;
use crate::fs::{self, object::Object};
use crate::xattr::ENODATA;
use crate::{EBUSY, EMSGSIZE, ENODEV, ENOENT, EXDEV, FdFlags, FdKind};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_EA};

const ACCESS: u32 = FILE_READ_ATTRIBUTES | FILE_READ_EA;
const CLOEXEC: u32 = 1;
const PARAMS: u32 = 0;
const READY: u32 = 1;
const RECONFIGURE: u32 = 2;
const FAILED: u32 = 3;
const TREE: u32 = 4;

#[derive(Clone)]
struct Layer {
    native: String,
    device: u64,
    inode: u64,
    guest: String,
}
impl Layer {
    fn pin(object: Object, guest: String) -> Result<(Self, Arc<Object>), i32> {
        let stat = fs::stat_handle(object.raw(), false)?;
        if stat.st_mode & fs::S_IFMT != fs::S_IFDIR {
            return Err(crate::ENOTDIR);
        }
        let native = object.path()?.to_str().ok_or(EIO)?.to_owned();
        crate::platform::try_set_inheritable(object.raw() as usize, true)?;
        Ok((
            Self {
                native,
                device: stat.st_dev,
                inode: stat.st_ino,
                guest,
            },
            Arc::new(object),
        ))
    }
    fn open(&self) -> Result<Object, i32> {
        let path = PathBuf::from(&self.native);
        if let Ok(object) = Object::open(&path, ACCESS) {
            let stat = fs::stat_handle(object.raw(), false)?;
            if (stat.st_dev, stat.st_ino) == (self.device, self.inode) {
                return Ok(object);
            }
        }
        let mut volume = PathBuf::new();
        for part in path.components() {
            if matches!(
                part,
                std::path::Component::Prefix(_) | std::path::Component::RootDir
            ) {
                volume.push(part);
            } else {
                break;
            }
        }
        let object = Object::by_id(&volume, self.inode, ACCESS)?;
        if fs::stat_handle(object.raw(), false)?.st_dev != self.device {
            return Err(crate::ESTALE);
        }
        Ok(object)
    }
}

#[derive(Clone)]
struct State {
    phase: u32,
    flags: u64,
    options: BTreeMap<String, String>,
    layers: Vec<(String, Layer)>,
    source: String,
    logs: Vec<String>,
    tree: Vec<MountPoint>,
    attached_namespace: u64,
    attached_id: u64,
}
impl Default for State {
    fn default() -> Self {
        Self {
            phase: PARAMS,
            flags: 0,
            options: BTreeMap::new(),
            layers: Vec::new(),
            source: String::new(),
            logs: Vec::new(),
            tree: Vec::new(),
            attached_namespace: 0,
            attached_id: 0,
        }
    }
}
fn word(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
fn string(bytes: &mut Vec<u8>, value: &str) {
    word(bytes, value.len() as u64);
    bytes.extend_from_slice(value.as_bytes());
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8], i32> {
        if size > self.0.len() {
            return Err(EIO);
        }
        let (a, b) = self.0.split_at(size);
        self.0 = b;
        Ok(a)
    }
    fn word(&mut self) -> Result<u64, i32> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn count(&mut self) -> Result<usize, i32> {
        let n = usize::try_from(self.word()?).map_err(|_| EIO)?;
        if n > self.0.len() {
            return Err(EIO);
        }
        Ok(n)
    }
    fn string(&mut self) -> Result<String, i32> {
        let n = self.count()?;
        Ok(std::str::from_utf8(self.take(n)?).map_err(|_| EIO)?.into())
    }
}
impl State {
    fn encode(&self) -> Result<Vec<u8>, i32> {
        let mut b = b"CYMOBJ01".to_vec();
        for value in [
            self.phase as u64,
            self.flags,
            self.attached_namespace,
            self.attached_id,
        ] {
            word(&mut b, value);
        }
        string(&mut b, &self.source);
        word(&mut b, self.options.len() as u64);
        for (k, v) in &self.options {
            string(&mut b, k);
            string(&mut b, v);
        }
        word(&mut b, self.layers.len() as u64);
        for (k, v) in &self.layers {
            string(&mut b, k);
            string(&mut b, &v.native);
            word(&mut b, v.device);
            word(&mut b, v.inode);
            string(&mut b, &v.guest);
        }
        word(&mut b, self.logs.len() as u64);
        for log in &self.logs {
            string(&mut b, log);
        }
        let next = self
            .tree
            .iter()
            .map(|p| p.id + 1)
            .max()
            .unwrap_or(FIRST_DYNAMIC_MOUNT_ID)
            .max(FIRST_DYNAMIC_MOUNT_ID);
        let tree = encode_table_with_next_id(&self.tree, next)?;
        word(&mut b, tree.len() as u64);
        b.extend_from_slice(&tree);
        Ok(b)
    }
    fn decode(b: &[u8]) -> Result<Self, i32> {
        let mut r = Reader(b);
        if r.take(8)? != b"CYMOBJ01" {
            return Err(EIO);
        }
        let phase = u32::try_from(r.word()?).map_err(|_| EIO)?;
        if phase > TREE {
            return Err(EIO);
        }
        let flags = r.word()?;
        let attached_namespace = r.word()?;
        let attached_id = r.word()?;
        let source = r.string()?;
        let mut options = BTreeMap::new();
        for _ in 0..r.count()? {
            options.insert(r.string()?, r.string()?);
        }
        let mut layers = Vec::new();
        for _ in 0..r.count()? {
            layers.push((
                r.string()?,
                Layer {
                    native: r.string()?,
                    device: r.word()?,
                    inode: r.word()?,
                    guest: r.string()?,
                },
            ));
        }
        let mut logs = Vec::new();
        for _ in 0..r.count()? {
            logs.push(r.string()?);
        }
        let size = r.count()?;
        let tree = decode_table_view(r.take(size)?, true)?;
        if !r.0.is_empty() {
            return Err(EIO);
        }
        Ok(Self {
            phase,
            flags,
            options,
            layers,
            source,
            logs,
            tree,
            attached_namespace,
            attached_id,
        })
    }
}

fn pins() -> &'static Mutex<HashMap<u64, Vec<Arc<Object>>>> {
    static PINS: OnceLock<Mutex<HashMap<u64, Vec<Arc<Object>>>>> = OnceLock::new();
    PINS.get_or_init(Default::default)
}
pub(crate) fn auxiliary_handles(
    entries: impl Iterator<Item = crate::FdEntry>,
) -> Result<Vec<(u64, Arc<Object>)>, i32> {
    let mut ids = Vec::new();
    for entry in entries.filter(|e| matches!(e.kind, FdKind::FsContext | FdKind::MountTree)) {
        ids.push((entry.description_id, shared::object_entry(entry)?.id()));
    }
    let pins = pins().lock().map_err(|_| EIO)?;
    Ok(ids
        .into_iter()
        .flat_map(|(description, id)| {
            pins.get(&id)
                .into_iter()
                .flatten()
                .map(move |pin| (description, pin.clone()))
        })
        .collect())
}
pub(crate) fn serialize(keep: impl Fn(i32) -> bool) -> Result<Vec<u8>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let mut ids = std::collections::HashSet::new();
    for (fd, entry) in table.slots.enumerated() {
        if keep(fd as i32)
            && let Some(entry) = entry
            && matches!(entry.kind, FdKind::FsContext | FdKind::MountTree)
        {
            ids.insert(shared::object_entry(*entry)?.id());
        }
    }
    let pins = pins().lock().map_err(|_| EIO)?;
    let selected: Vec<_> = ids
        .into_iter()
        .filter_map(|id| pins.get(&id).map(|pins| (id, pins)))
        .collect();
    let mut bytes = Vec::new();
    word(&mut bytes, selected.len() as u64);
    for (id, pins) in selected {
        word(&mut bytes, id);
        word(&mut bytes, pins.len() as u64);
        for pin in pins {
            word(&mut bytes, pin.raw() as u64);
        }
    }
    Ok(bytes)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    fn inner(bytes: &[u8]) -> Result<(), i32> {
        let mut r = Reader(bytes);
        let mut restored = HashMap::new();
        let mut inherited = std::collections::HashSet::new();
        for _ in 0..r.count()? {
            let id = r.word()?;
            let mut objects = Vec::new();
            for _ in 0..r.count()? {
                let raw = r.word()? as usize;
                let object = unsafe { Object::reopen(raw as _, ACCESS)? };
                crate::platform::try_set_inheritable(object.raw() as usize, true)?;
                objects.push(Arc::new(object));
                inherited.insert(raw);
            }
            if restored.insert(id, objects).is_some() {
                return Err(EIO);
            }
        }
        if !r.0.is_empty() {
            return Err(EIO);
        }
        *pins().lock().map_err(|_| EIO)? = restored;
        for raw in inherited {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(raw as _) };
        }
        Ok(())
    }
    inner(bytes).is_ok()
}
pub(crate) fn closed(entry: crate::FdEntry, survivors: impl Iterator<Item = crate::FdEntry>) {
    if !matches!(entry.kind, FdKind::FsContext | FdKind::MountTree) {
        return;
    }
    let Ok(store) = shared::object_entry(entry) else {
        return;
    };
    let id = store.id();
    if survivors
        .filter(|e| matches!(e.kind, FdKind::FsContext | FdKind::MountTree))
        .any(|entry| shared::object_entry(entry).is_ok_and(|s| s.id() == id))
    {
        return;
    }
    if let Ok(mut pins) = pins().lock() {
        pins.remove(&id);
    }
}
fn read_state(fd: i32) -> Result<State, i32> {
    State::decode(&shared::object_fd(fd)?.read()?.1)
}
fn change<T>(fd: i32, action: impl FnOnce(&mut State, u64) -> Result<T, i32>) -> Result<T, i32> {
    let store = if let Some(anchor) = overlay::directory_anchor(fd)? {
        anchor.store()?
    } else {
        shared::object_fd(fd)?
    };
    store.update(|bytes| {
        let mut state = State::decode(bytes)?;
        let original = state.clone();
        let pins_before = pins()
            .lock()
            .map_err(|_| EIO)?
            .get(&store.id())
            .map_or(0, Vec::len);
        let result = action(&mut state, store.id());
        if result.is_err() && state.phase != FAILED {
            state = original;
            if let Some(pins) = pins().lock().map_err(|_| EIO)?.get_mut(&store.id()) {
                pins.truncate(pins_before);
            }
        }
        if let Err(error) = result {
            if state.logs.len() == 16 {
                state.logs.remove(0);
            }
            state.logs.push(format!("e overlay: errno {error}"));
        }
        Ok((state.encode()?, result))
    })?
}
fn check_kind(fd: i32, kind: FdKind) -> Result<(), i32> {
    if crate::get(fd)?.kind != kind {
        return Err(EINVAL);
    }
    Ok(())
}
fn descriptor(state: State, flags: u32, kind: FdKind) -> Result<i32, i32> {
    if flags & !CLOEXEC != 0 {
        return Err(EINVAL);
    }
    let store = shared::new_object()?;
    store.update(|_| Ok((state.encode()?, ())))?;
    if state.flags & MS_TMPFS != 0 && !state.source.is_empty() {
        crate::tmpfs::reference(&state.source, store.id())?;
    }
    let mut fdflags = if kind == FdKind::MountTree {
        FdFlags::PATH_ONLY
    } else {
        FdFlags::READ_ACCESS.union(FdFlags::WRITE_ACCESS)
    };
    if flags & CLOEXEC != 0 {
        fdflags = fdflags.union(FdFlags::CLOSE_ON_EXEC);
    }
    store.descriptor_kind(kind, fdflags)
}
pub fn fsopen(filesystem: &str, flags: u32) -> Result<i32, i32> {
    if flags & !CLOEXEC != 0 {
        return Err(EINVAL);
    }
    if !matches!(
        filesystem,
        "overlay" | "proc" | "tmpfs" | "devpts" | "mqueue" | "sysfs" | "cgroup2"
    ) {
        return Err(ENODEV);
    }
    descriptor(
        State {
            flags: if filesystem == "proc" {
                MS_PROC
            } else if filesystem == "cgroup2" {
                MS_TMPFS | MS_CGROUP | 2 | 4 | 8
            } else if filesystem == "sysfs" {
                MS_TMPFS | MS_SYSFS | 2 | 4 | 8
            } else if filesystem == "mqueue" {
                MS_TMPFS | MS_MQUEUE | 4 | 8
            } else if filesystem == "devpts" {
                MS_TMPFS | MS_DEVPTS
            } else if filesystem == "tmpfs" {
                MS_TMPFS
            } else {
                0
            },
            ..State::default()
        },
        flags,
        FdKind::FsContext,
    )
}
pub fn read_log(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    check_kind(fd, FdKind::FsContext)?;
    let store = shared::object_fd(fd)?;
    store.update(|bytes| {
        let mut state = State::decode(bytes)?;
        let line = state.logs.first().ok_or(ENODATA)?;
        if line.len() > buffer.len() {
            return Err(EMSGSIZE);
        }
        let count = line.len();
        buffer[..count].copy_from_slice(line.as_bytes());
        state.logs.remove(0);
        Ok((state.encode()?, count))
    })
}
fn escaped(value: &str) -> String {
    let mut s = String::new();
    for ch in value.chars() {
        if matches!(ch, ',' | ':' | '\\') {
            s.push('\\')
        }
        s.push(ch)
    }
    s
}

/// Values are typed by the syscall adapter; FD parameters never become a
/// pathname to whichever inode later occupies the descriptor's old name.
pub enum Parameter<'a> {
    Flag,
    String(&'a str),
    Path(i32, &'a str, bool),
    Fd(i32),
    Binary(&'a [u8]),
}
pub fn configure(
    fd: i32,
    command: u32,
    key: Option<&str>,
    value: Option<Parameter<'_>>,
) -> Result<(), i32> {
    check_kind(fd, FdKind::FsContext)?;
    let mut changes = None;
    let result = change(fd, |state, id| {
        if command >= 6 {
            if key.is_some() || value.is_some() {
                return Err(EINVAL);
            }
            return match command {
                6 | 8 => {
                    create(state)?;
                    if state.flags & MS_TMPFS != 0 {
                        crate::tmpfs::reference(&state.source, id)?;
                    }
                    Ok(())
                }
                7 => {
                    changes = Some(reconfigure(state)?);
                    Ok(())
                }
                _ => Err(EOPNOTSUPP),
            };
        }
        if !matches!(state.phase, PARAMS | RECONFIGURE) {
            return Err(EBUSY);
        }
        let key = key.ok_or(EINVAL)?;
        if key.is_empty() || key.len() > 255 {
            return Err(EINVAL);
        }
        let value = value.ok_or(EINVAL)?;
        if state.flags & (MS_MQUEUE | MS_SYSFS | MS_CGROUP) != 0 {
            let text = match value {
                Parameter::Flag => "",
                Parameter::String(v) => v,
                _ => return Err(EINVAL),
            };
            match key {
                "source" => {
                    state.options.insert(key.into(), text.into());
                }
                "ro" if text.is_empty() => state.flags |= 1,
                "rw" if text.is_empty() => state.flags &= !1,
                _ => return Err(EINVAL),
            }
            return Ok(());
        }
        if state.flags & MS_DEVPTS != 0 {
            let text = match value {
                Parameter::Flag => String::new(),
                Parameter::String(v) => v.to_owned(),
                _ => return Err(EINVAL),
            };
            match key {
                "source" | "uid" | "gid" | "mode" | "ptmxmode" | "max" | "newinstance" => {
                    state.options.insert(key.into(), text);
                }
                "ro" if text.is_empty() => state.flags |= 1,
                "rw" if text.is_empty() => state.flags &= !1,
                _ => return Err(EINVAL),
            }
            return Ok(());
        }
        if state.flags & MS_TMPFS != 0 {
            let text = match value {
                Parameter::Flag => String::new(),
                Parameter::String(v) => v.to_owned(),
                _ => return Err(EINVAL),
            };
            match key {
                "ro" if text.is_empty() => state.flags |= 1,
                "rw" if text.is_empty() => state.flags &= !1,
                "source" | "size" | "nr_blocks" | "nr_inodes" | "mode" | "uid" | "gid"
                | "inode64" | "inode32" | "huge" | "mpol" | "noswap" => {
                    state.options.insert(key.into(), text);
                }
                _ => return Err(EINVAL),
            }
            return Ok(());
        }
        if state.flags & MS_PROC != 0 {
            let text = match value {
                Parameter::Flag => "",
                Parameter::String(s) => s,
                _ => return Err(EINVAL),
            };
            match key {
                "source" => {
                    state.options.insert(key.into(), text.into());
                }
                "ro" | "rw" if text.is_empty() => {
                    state.flags = (state.flags & !1) | u64::from(key == "ro")
                }
                "subset" if matches!(text, "pid" | "all") => {
                    state.options.insert(key.into(), text.into());
                }
                "hidepid" if matches!(text, "0" | "off") => {
                    state.options.insert(key.into(), text.into());
                }
                "hidepid" | "gid" => return Err(EOPNOTSUPP),
                _ => return Err(EINVAL),
            }
            return Ok(());
        }
        if key == "lowerdir" {
            let Parameter::String(value) = value else {
                return Err(EINVAL);
            };
            if state
                .layers
                .iter()
                .any(|(k, _)| matches!(k.as_str(), "lowerdir+" | "datadir+"))
            {
                return Err(EINVAL);
            }
            let options = overlay::parse_options(&format!("lowerdir={value}"), 0)?;
            let split = options.lowerdirs.len() - overlay::features::data_count(options.flags);
            for (index, path) in options.lowerdirs.iter().enumerate() {
                add_layer(
                    state,
                    id,
                    if index < split {
                        "lowerdir+"
                    } else {
                        "datadir+"
                    },
                    Parameter::Path(fs::AT_FDCWD, path, false),
                )?;
            }
            return Ok(());
        }
        if matches!(key, "lowerdir+" | "datadir+" | "upperdir" | "workdir") {
            return add_layer(state, id, key, value);
        }
        let text = match value {
            Parameter::Flag => String::new(),
            Parameter::String(s) => s.into(),
            _ => return Err(EINVAL),
        };
        if key == "source" {
            state.options.insert(key.into(), text);
            return Ok(());
        }
        if matches!(key, "ro" | "rw") {
            if !text.is_empty() {
                return Err(EINVAL);
            }
            state.flags = (state.flags & !1) | u64::from(key == "ro");
            return Ok(());
        }
        // Validate this option's value while dependencies are resolved at CREATE.
        let known = matches!(
            key,
            "index"
                | "metacopy"
                | "redirect_dir"
                | "xino"
                | "uuid"
                | "nfs_export"
                | "verity"
                | "fsync"
                | "userxattr"
                | "volatile"
        );
        if !known {
            return Err(EINVAL);
        }
        let part = if text.is_empty() {
            key.into()
        } else {
            format!("{key}={text}")
        };
        overlay::parse_options(&format!("lowerdir=/l,upperdir=/u,workdir=/w,{part}"), 0)?;
        state.options.insert(key.into(), text);
        Ok(())
    });
    if result.is_ok() {
        if let Some(changes) = changes {
            changes.commit();
        }
    }
    result
}
fn add_layer(state: &mut State, id: u64, key: &str, value: Parameter<'_>) -> Result<(), i32> {
    if state.phase != PARAMS {
        return Err(EBUSY);
    }
    if key == "lowerdir+" && state.layers.iter().any(|(key, _)| key == "datadir+") {
        return Err(EINVAL);
    }
    if matches!(key, "upperdir" | "workdir") && state.layers.iter().any(|(k, _)| k == key) {
        return Err(EINVAL);
    }
    if state.layers.len() >= 502 {
        return Err(EINVAL);
    }
    let (object, guest) = match value {
        Parameter::Fd(fd) => {
            if overlay::descriptor_filesystem(fd)?.is_some() {
                return Err(EOPNOTSUPP);
            }
            let object = Object::from_fd(fd)?;
            let guest = crate::procfs::local_fd_link_target(fd)?;
            (object, guest)
        }
        Parameter::String(path) | Parameter::Path(fs::AT_FDCWD, path, false) => {
            if path.is_empty() {
                return Err(ENOENT);
            }
            let guest = fs::absolute_linux(path);
            if overlay::is_overlay_path(&guest, true, false)? {
                return Err(EOPNOTSUPP);
            }
            let native = crate::resolve_linux_path(&guest).map_err(|e| match e {
                crate::path::PathError::Filesystem(e) => e,
                _ => EINVAL,
            })?;
            (Object::open(&native, ACCESS)?, guest)
        }
        Parameter::Path(fd, path, empty) => {
            if path.is_empty() {
                if !empty {
                    return Err(ENOENT);
                }
                if fd == fs::AT_FDCWD {
                    return add_layer(state, id, key, Parameter::String(&fs::getcwd()));
                }
                if overlay::descriptor_filesystem(fd)?.is_some() {
                    return Err(EOPNOTSUPP);
                }
                (
                    Object::from_fd(fd)?,
                    crate::procfs::local_fd_link_target(fd)?,
                )
            } else {
                let file = fs::openat(fd, path, fs::O_PATH | fs::O_DIRECTORY, 0)?;
                let result = (|| {
                    if overlay::descriptor_filesystem(file)?.is_some() {
                        return Err(EOPNOTSUPP);
                    }
                    Ok((
                        Object::from_fd(file)?,
                        crate::procfs::local_fd_link_target(file)?,
                    ))
                })();
                let _ = crate::close(file);
                result?
            }
        }
        _ => return Err(EINVAL),
    };
    let (layer, pin) = Layer::pin(object, guest)?;
    pins()
        .lock()
        .map_err(|_| EIO)?
        .entry(id)
        .or_default()
        .push(pin);
    state.layers.push((key.into(), layer));
    Ok(())
}
fn create(state: &mut State) -> Result<(), i32> {
    if state.phase != PARAMS {
        return Err(EBUSY);
    }
    if state.flags & MS_TMPFS != 0 {
        let options = state
            .options
            .iter()
            .filter(|(k, _)| k.as_str() != "source")
            .map(|(k, v)| {
                if v.is_empty() {
                    k.clone()
                } else {
                    format!("{k}={v}")
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        state.source = if state.flags & MS_CGROUP != 0 {
            crate::tmpfs::cgroupfs::prepare(&options)?
        } else if state.flags & MS_SYSFS != 0 {
            crate::sysfs::prepare(&options)?
        } else if state.flags & MS_MQUEUE != 0 {
            crate::mqueue::prepare(&options)?
        } else if state.flags & MS_DEVPTS != 0 {
            crate::devpts::prepare(&options)?
        } else {
            crate::tmpfs::prepare(&options)?
        };
        state.phase = READY;
        return Ok(());
    }
    if state.flags & MS_PROC != 0 {
        let options = state
            .options
            .iter()
            .filter(|(k, _)| k.as_str() != "source")
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        state.source = crate::procfs::instance::prepare(&options)?;
        state.phase = READY;
        return Ok(());
    }
    let mut lowers = Vec::new();
    let mut data = Vec::new();
    let mut upper = None;
    let mut work = None;
    for (key, layer) in &state.layers {
        match key.as_str() {
            "lowerdir+" => lowers.push(layer),
            "datadir+" => data.push(layer),
            "upperdir" => upper = Some(layer),
            "workdir" => work = Some(layer),
            _ => return Err(EIO),
        }
    }
    let mut options = format!(
        "lowerdir={}",
        lowers
            .iter()
            .map(|l| escaped(&l.guest))
            .collect::<Vec<_>>()
            .join(":")
    );
    for layer in &data {
        options.push_str("::");
        options.push_str(&escaped(&layer.guest));
    }
    if let Some(layer) = upper {
        options.push_str(&format!(",upperdir={}", escaped(&layer.guest)));
    }
    if let Some(layer) = work {
        options.push_str(&format!(",workdir={}", escaped(&layer.guest)));
    }
    for (key, value) in &state.options {
        if key == "source" {
            continue;
        }
        options.push(',');
        options.push_str(key);
        if !value.is_empty() {
            options.push('=');
            options.push_str(value);
        }
    }
    let result = (|| {
        let parsed = overlay::parse_options(&options, state.flags)?;
        let objects = upper
            .into_iter()
            .chain(lowers.into_iter())
            .chain(data.into_iter())
            .chain(work)
            .map(Layer::open)
            .collect::<Result<Vec<_>, _>>()?;
        let source = overlay::prepare_mount_pinned(
            &parsed.lowerdirs,
            parsed.upperdir.as_deref(),
            parsed.workdir.as_deref(),
            parsed.flags,
            objects,
        )?;
        state.source = source;
        state.flags = parsed.flags | if upper.is_none() { 1 } else { 0 };
        Ok(())
    })();
    state.phase = if result.is_ok() { READY } else { FAILED };
    result
}
fn reconfigure(state: &mut State) -> Result<policy::Changes, i32> {
    if state.phase != RECONFIGURE {
        return Err(EBUSY);
    }
    // Layer sets are immutable after superblock creation.
    if state.flags & MS_TMPFS == 0 && state.options.keys().any(|key| key != "source") {
        return Err(EINVAL);
    }
    if state.flags & (MS_PROC | MS_TMPFS) != 0 {
        if state.attached_namespace != namespace_id()? || state.attached_id == 0 {
            return Err(EINVAL);
        }
        if !crate::user_namespace::capable(shared::get()?.owner(), 21) {
            return Err(crate::EPERM);
        }
        let mut pending = None;
        update(|table, _| {
            let point = table
                .iter_mut()
                .find(|p| p.id == state.attached_id && p.source == state.source)
                .ok_or(ENOENT)?;
            let reference = policy::get(state.attached_namespace, point.id, point.flags)?;
            if state.flags & MS_TMPFS != 0 {
                let options = state
                    .options
                    .iter()
                    .filter(|(k, _)| k.as_str() != "source")
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",");
                if state.flags & MS_CGROUP != 0 {
                    crate::tmpfs::cgroupfs::reconfigure(&state.source, &options)?;
                } else if state.flags & MS_SYSFS != 0 {
                    crate::sysfs::reconfigure(&state.source, &options)?;
                } else if state.flags & MS_MQUEUE != 0 {
                    crate::mqueue::reconfigure(&state.source, &options)?;
                } else if state.flags & MS_DEVPTS != 0 {
                    crate::devpts::reconfigure(&state.source, &options)?;
                } else {
                    crate::tmpfs::resize(&state.source, &options)?;
                }
            }
            point.flags = (point.flags & !1) | (state.flags & 1);
            pending = Some(policy::Changes::apply(vec![(reference, point.flags)])?);
            Ok(())
        })?;
        return pending.ok_or(EIO);
    }
    overlay::reconfigure(&state.source, state.flags & 1 != 0)
}
pub fn fsmount(fd: i32, flags: u32, attributes: u64) -> Result<i32, i32> {
    check_kind(fd, FdKind::FsContext)?;
    if flags & !1 != 0 {
        return Err(EINVAL);
    }
    let policy = attribute_flags(attributes)?;
    change(fd, |state, context_id| {
        if state.source.is_empty() {
            return Err(EINVAL);
        }
        if state.phase != READY {
            return Err(EBUSY);
        }
        let id = update(|_, next| allocate_id(next))?;
        let tree = MountPoint {
            meta: MountMetadata::default(),
            id,
            parent: ROOT_MOUNT_ID,
            source: state.source.clone(),
            target: "/".into(),
            flags: state.flags
                | if state.flags & (MS_PROC | MS_TMPFS) == 0 {
                    MS_OVERLAY
                } else {
                    0
                }
                | policy,
        };
        let result = descriptor(
            State {
                phase: TREE,
                tree: vec![tree],
                attached_namespace: namespace_id()?,
                ..State::default()
            },
            flags,
            FdKind::MountTree,
        )?;
        let tree_id = shared::object_fd(result)?.id();
        let mut registry = pins().lock().map_err(|_| EIO)?;
        let inherited = registry.get(&context_id).cloned().unwrap_or_default();
        registry.insert(tree_id, inherited);
        state.phase = RECONFIGURE;
        state.options.clear();
        Ok(result)
    })
}
fn attribute_flags(attributes: u64) -> Result<u64, i32> {
    if attributes & !(0xff | 0x200000) != 0 || !matches!(attributes & 0x70, 0 | 0x10 | 0x20) {
        return Err(EINVAL);
    }
    Ok((attributes & 15)
        | if attributes & 0x200000 != 0 { 256 } else { 0 }
        | match attributes & 0x70 {
            0x10 => 1024,
            0x20 => 1 << 24,
            _ => 1 << 21,
        }
        | if attributes & 0x80 != 0 { 2048 } else { 0 })
}
pub(crate) fn tree_policy(fd: i32, path: &str) -> Result<Arc<policy::Policy>, i32> {
    let state = read_tree_state(fd)?;
    let tree = tree_snapshot(&state)?;
    let point = visible_mount(&tree, &tree_absolute(fd, path)?)?.ok_or(ENOENT)?;
    policy::get_mapped(
        state.attached_namespace,
        point.id,
        point.flags,
        point.meta.idmap.as_ref(),
    )
}
pub fn tree_path(fd: i32) -> Result<String, i32> {
    if crate::get(fd)?.kind != FdKind::MountTree && overlay::directory_anchor(fd)?.is_none() {
        return Err(crate::EBADF);
    }
    Ok(format!("/proc/self/fd/{fd}"))
}
/// Recognize a live mount descriptor's magic link before procfs dispatch.
/// Keep the suffix unnormalized: a symlink must be expanded before `..`.
pub fn tree_reference(path: &str) -> Option<(i32, &str)> {
    let rest = path.strip_prefix("/proc/self/fd/")?;
    let (number, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let fd = number.parse::<i32>().ok()?;
    (crate::get(fd).ok()?.kind == FdKind::MountTree
        || overlay::directory_anchor(fd).ok()?.is_some())
    .then_some((fd, tail))
}
/// Directory descriptions own the mount section, independently of fd numbers.
#[derive(Clone)]
pub(crate) struct DirectoryAnchor {
    pub(crate) pin: Arc<Object>,
    pub(crate) prefix: String,
}
impl DirectoryAnchor {
    pub(crate) fn restore(raw: usize, prefix: String) -> Result<Self, i32> {
        let object = Object::duplicate(raw as _)?;
        crate::platform::try_set_inheritable(object.raw() as usize, true)?;
        Ok(Self {
            pin: Arc::new(object),
            prefix,
        })
    }
    fn store(&self) -> Result<shared::Store, i32> {
        let entry = crate::FdEntry {
            raw: self.pin.raw() as usize,
            kind: FdKind::MountTree,
            flags: FdFlags::PATH_ONLY,
            generation: 0,
            description_id: 0,
            offset: 0,
        };
        shared::object_entry(entry)
    }
    fn state(&self) -> Result<State, i32> {
        State::decode(&self.store()?.read()?.1)
    }
}
pub(crate) fn pin_tree(path: &str) -> Result<Option<DirectoryAnchor>, i32> {
    let Some((fd, tail)) = tree_reference(path) else {
        return Ok(None);
    };
    let prefix = tree_absolute(fd, tail)?;
    if let Some(anchor) = overlay::directory_anchor(fd)? {
        return Ok(Some(DirectoryAnchor {
            pin: anchor.pin,
            prefix,
        }));
    }
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|e| *e)
        .ok_or(crate::EBADF)?;
    if entry.kind != FdKind::MountTree {
        return Err(crate::EBADF);
    }
    Ok(Some(DirectoryAnchor::restore(entry.raw, prefix)?))
}
fn read_tree_state(fd: i32) -> Result<State, i32> {
    if let Some(anchor) = overlay::directory_anchor(fd)? {
        anchor.state()
    } else {
        read_state(fd)
    }
}
pub(crate) fn tree_absolute(fd: i32, tail: &str) -> Result<String, i32> {
    let base = overlay::directory_anchor(fd)?.map_or_else(|| "/".into(), |a| a.prefix);
    normalize(&join(&base, tail.trim_start_matches('/')))
}
/// Spell a component-walk position relative to the directory magic link.
pub(crate) fn tree_relative(fd: i32, absolute: &str) -> Result<String, i32> {
    let base = tree_absolute(fd, "")?;
    let a: Vec<_> = base.split('/').filter(|s| !s.is_empty()).collect();
    let b: Vec<_> = absolute.split('/').filter(|s| !s.is_empty()).collect();
    let same = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    Ok(std::iter::repeat_n("..", a.len() - same)
        .chain(b[same..].iter().copied())
        .collect::<Vec<_>>()
        .join("/"))
}
pub(crate) fn tree_mount(fd: i32, path: &str) -> Result<(String, String, u64), i32> {
    let state = read_tree_state(fd)?;
    let tree = tree_snapshot(&state)?;
    let path = tree_absolute(fd, path)?;
    let point = visible_mount(&tree, &path)?.ok_or(EIO)?;
    let rest = suffix(&path, &point.target).ok_or(EIO)?;
    if point.flags & MS_TMPFS != 0 {
        return Ok((
            crate::tmpfs::subtree(&point.source, rest)?,
            String::new(),
            point.flags,
        ));
    }
    if point.flags & MS_OVERLAY != 0 {
        let (source, view) = overlay::split_view(&point.source)?;
        Ok((
            source.into(),
            join(&view, rest).trim_start_matches('/').into(),
            point.flags,
        ))
    } else {
        Ok((join(&point.source, rest), String::new(), point.flags))
    }
}
pub(crate) fn tree_id(fd: i32, path: &str) -> Result<u64, i32> {
    let state = read_tree_state(fd)?;
    let tree = tree_snapshot(&state)?;
    let point = visible_mount(&tree, &tree_absolute(fd, path)?)?.ok_or(ENOENT)?;
    super::query::unique_id(
        if state.attached_namespace == 0 {
            namespace_id()?
        } else {
            state.attached_namespace
        },
        point.id,
    )
}
pub(crate) fn tree_record(fd: i32) -> Result<crate::procfs::mounts::Record, i32> {
    let state = read_tree_state(fd)?;
    let tree = tree_snapshot(&state)?;
    let point = visible_mount(&tree, &tree_absolute(fd, "")?)?.ok_or(EIO)?;
    if point.flags & MS_TMPFS != 0 {
        let (volume, node) = crate::tmpfs::parse(&point.source)?;
        return Ok(crate::procfs::mounts::Record {
            idmap: None,
            peer: point.meta.peer,
            master: point.meta.master,
            id: point.id,
            parent: point.id,
            device: crate::tmpfs::device(volume),
            root: if node == 1 {
                "/".into()
            } else {
                format!("/inode/{node}")
            },
            target: String::new(),
            filesystem: crate::tmpfs::filesystem(volume)?.into(),
            source: crate::tmpfs::filesystem(volume)?.into(),
            options: if point.flags & 1 != 0 { "ro" } else { "rw" }.into(),
            super_options: crate::tmpfs::options(volume)?,
            flags: point.flags,
        });
    }
    if point.flags & MS_PROC != 0 {
        let (view, tail) = crate::procfs::instance::parse(&point.source)?;
        return Ok(crate::procfs::mounts::Record {
            idmap: None,
            peer: point.meta.peer,
            master: point.meta.master,
            id: point.id,
            parent: point.id,
            device: view.device(),
            root: format!("/{tail}"),
            target: String::new(),
            filesystem: "proc".into(),
            source: "proc".into(),
            options: if point.flags & 1 != 0 { "ro" } else { "rw" }.into(),
            super_options: if view.subset_pid {
                "rw,subset=pid"
            } else {
                "rw"
            }
            .into(),
            flags: point.flags,
        });
    }
    if point.flags & MS_OVERLAY == 0 {
        let native = crate::path::resolve_bound_mount_source(
            &crate::path::default_system_root(),
            &point.source,
        )
        .map_err(|_| EIO)?;
        let mut record = crate::procfs::mounts::host_record(
            &native,
            point.id,
            point.parent,
            String::new(),
            point.flags,
        )?;
        record.idmap = point.meta.idmap.clone();
        record.peer = point.meta.peer;
        record.master = point.meta.master;
        return Ok(record);
    }
    let (base, view) = overlay::split_view(&point.source)?;
    let (device, readonly) = overlay::mount_metadata(base)?;
    let options = if readonly || point.flags & 1 != 0 {
        "ro"
    } else {
        "rw"
    };
    Ok(crate::procfs::mounts::Record {
        idmap: point.meta.idmap.clone(),
        peer: point.meta.peer,
        master: point.meta.master,
        id: point.id,
        parent: point.id,
        device,
        root: format!("/{view}"),
        target: String::new(),
        filesystem: "overlay".into(),
        source: "overlay".into(),
        options: options.into(),
        super_options: format!(
            "{},{}",
            if readonly { "ro" } else { "rw" },
            overlay::feature_options(base)?
        ),
        flags: point.flags,
    })
}
fn tree_snapshot(state: &State) -> Result<Vec<MountPoint>, i32> {
    if state.attached_id != 0 {
        let table = decode_table(&shared::namespace(state.attached_namespace)?.read()?.1)?;
        if let Some(root) = table.iter().find(|p| p.id == state.attached_id) {
            let ids = descendants(&table, root.id);
            return table
                .iter()
                .filter(|p| ids.contains(&p.id))
                .map(|p| {
                    let mut point = p.clone();
                    point.target = join("/", suffix(&point.target, &root.target).ok_or(EIO)?);
                    if point.id == root.id {
                        point.parent = ROOT_MOUNT_ID;
                    }
                    Ok(point)
                })
                .collect();
        }
    }
    Ok(state.tree.clone())
}
pub fn stat(fd: i32) -> Result<fs::Stat, i32> {
    let entry = crate::get(fd)?;
    if entry.kind == FdKind::FsContext {
        return Ok(fs::Stat {
            st_mode: fs::S_IFREG | 0o600,
            st_ino: shared::object_fd(fd)?.id(),
            st_nlink: 1,
            ..fs::Stat::default()
        });
    }
    check_kind(fd, FdKind::MountTree)?;
    let (source, _, flags) = tree_mount(fd, "")?;
    if flags & (MS_OVERLAY | MS_TMPFS | MS_PROC) == 0 {
        let path =
            crate::path::resolve_bound_mount_source(&crate::path::default_system_root(), &source)
                .map_err(|_| EIO)?;
        let object = Object::open(&path, ACCESS)?;
        return fs::stat_handle(object.raw(), false);
    }
    fs::stat(&format!("/proc/self/fd/{fd}/."))
}
pub fn fspick(path: &str, flags: u32) -> Result<i32, i32> {
    if flags & !15 != 0 {
        return Err(EINVAL);
    }
    if flags & 2 != 0 && fs::lstat(path)?.st_mode & fs::S_IFMT == fs::S_IFLNK {
        return Err(crate::ELOOP);
    }
    fs::stat(path)?;
    let path = canonical_mount_path(path, true)?;
    if let Some((fd, tail)) = tree_reference(&path) {
        let (source, _, mount_flags) = tree_mount(fd, &format!("/{tail}"))?;
        if mount_flags & MS_OVERLAY == 0 {
            return Err(EOPNOTSUPP);
        }
        let readonly = overlay::mount_metadata(&source)?.1;
        return descriptor(
            State {
                phase: RECONFIGURE,
                flags: (mount_flags & !1) | u64::from(readonly),
                source,
                ..State::default()
            },
            flags & 1,
            FdKind::FsContext,
        );
    }
    let path = namespace_path(&path)?;
    let table = snapshot()?;
    let point = visible_mount(&table, &path)?.ok_or(EINVAL)?;
    if point.flags & (MS_PROC | MS_TMPFS) != 0 {
        return descriptor(
            State {
                phase: RECONFIGURE,
                flags: point.flags,
                source: point.source.clone(),
                attached_namespace: namespace_id()?,
                attached_id: point.id,
                ..State::default()
            },
            flags & 1,
            FdKind::FsContext,
        );
    }
    if point.flags & MS_OVERLAY == 0 {
        return Err(EOPNOTSUPP);
    }
    descriptor(
        State {
            phase: RECONFIGURE,
            flags: (point.flags & !1)
                | u64::from(overlay::mount_metadata(overlay::split_view(&point.source)?.0)?.1),
            source: point.source.clone(),
            attached_namespace: namespace_id()?,
            attached_id: point.id,
            ..State::default()
        },
        flags & 1,
        FdKind::FsContext,
    )
}
pub fn open_tree(path: &str, flags: u32) -> Result<i32, i32> {
    if flags & !(1 | 0x80000 | 0x1000 | 0x100 | 0x800 | 0x8000) != 0 {
        return Err(EINVAL);
    }
    if flags & 0x8001 == 0x8000 {
        return Err(EINVAL);
    }
    if flags & 1 == 0 {
        return fs::open(
            path,
            fs::O_PATH
                | if flags & 0x80000 != 0 {
                    fs::O_CLOEXEC
                } else {
                    0
                }
                | if flags & 0x100 != 0 {
                    fs::O_NOFOLLOW
                } else {
                    0
                },
            0,
        );
    }
    let path = canonical_mount_path(path, flags & 0x100 == 0)?;
    let (source, table) = if let Some((fd, tail)) = tree_reference(&path) {
        (
            tree_absolute(fd, tail)?,
            tree_snapshot(&read_tree_state(fd)?)?,
        )
    } else {
        (namespace_path(&path)?, snapshot()?.as_ref().clone())
    };
    let point = visible_mount(&table, &source)?;
    if point.is_some_and(|point| point.flags & MS_UNBINDABLE != 0) {
        return Err(EINVAL);
    }
    let (backing, policy) = if let Some(point) = point.filter(|p| p.flags & MS_TMPFS != 0) {
        (
            crate::tmpfs::subtree(&point.source, suffix(&source, &point.target).ok_or(EIO)?)?,
            point.flags,
        )
    } else if let Some(point) = point.filter(|p| p.flags & MS_PROC != 0) {
        (
            format!(
                "{}{}",
                point.source.trim_end_matches('/').to_owned() + "/",
                suffix(&source, &point.target).ok_or(EIO)?
            ),
            point.flags,
        )
    } else if let Some(point) = point.filter(|p| p.flags & MS_OVERLAY != 0) {
        let (base, view) = overlay::split_view(&point.source)?;
        (
            overlay::with_view(
                base,
                &join(&view, suffix(&source, &point.target).ok_or(EIO)?),
            ),
            point.flags,
        )
    } else {
        (translate_in(&table, &source)?, MS_BIND)
    };
    let stat = if flags & 0x100 != 0 {
        fs::lstat(&path)?
    } else {
        fs::stat(&path)?
    };
    if stat.st_mode & fs::S_IFMT == fs::S_IFLNK {
        return Err(EOPNOTSUPP);
    }
    let id = update(|_, next| allocate_id(next))?;
    let mut tree = vec![MountPoint {
        meta: point
            .map(|p| p.meta.cloned_for_attachment())
            .unwrap_or_default(),
        id,
        parent: ROOT_MOUNT_ID,
        source: backing,
        target: "/".into(),
        flags: policy,
    }];
    if flags & 0x8000 != 0 && stat.st_mode & fs::S_IFMT == fs::S_IFDIR {
        let source_id = point.map_or(ROOT_MOUNT_ID, |p| p.id);
        let mut cloned = HashMap::from([(source_id, id)]);
        for point in table.iter() {
            if point.flags & MS_UNBINDABLE != 0 {
                continue;
            }
            if let (Some(parent), Some(rest)) =
                (cloned.get(&point.parent), suffix(&point.target, &source))
            {
                let child = update(|_, next| allocate_id(next))?;
                let parent = *parent;
                cloned.insert(point.id, child);
                tree.push(MountPoint {
                    meta: point.meta.cloned_for_attachment(),
                    id: child,
                    parent,
                    source: point.source.clone(),
                    target: join("/", rest),
                    flags: point.flags,
                });
            }
        }
    }
    let mut objects = Vec::new();
    for point in &tree {
        if point.flags & MS_OVERLAY != 0 {
            objects.extend(overlay::mount_pins(&point.source)?);
        }
    }
    let fd = descriptor(
        State {
            phase: TREE,
            tree,
            attached_namespace: namespace_id()?,
            ..State::default()
        },
        u32::from(flags & 0x80000 != 0),
        FdKind::MountTree,
    )?;
    let object_id = shared::object_fd(fd)?.id();
    pins().lock().map_err(|_| EIO)?.insert(object_id, objects);
    Ok(fd)
}
pub fn move_tree(fd: i32, target: &str, flags: u32) -> Result<(), i32> {
    check_kind(fd, FdKind::MountTree)?;
    if flags & !0x77 != 0 {
        return Err(EINVAL);
    }
    let metadata = if flags & 0x10 == 0 {
        fs::lstat(target)?
    } else {
        fs::stat(target)?
    };
    let source_is_directory = fs::fstat(fd)?.st_mode & fs::S_IFMT == fs::S_IFDIR;
    let target = namespace_path(&canonical_mount_path(target, flags & 0x10 != 0)?)?;
    if has_fixed_virtual_dispatch(&target)
        && !read_tree_state(fd)?
            .tree
            .first()
            .is_some_and(|p| p.flags & (MS_PROC | MS_TMPFS) != 0)
    {
        return Err(EOPNOTSUPP);
    }
    change(fd, |state, _| {
        if state.phase != TREE || state.tree.is_empty() {
            return Err(EINVAL);
        }
        if (metadata.st_mode & fs::S_IFMT == fs::S_IFDIR) != source_is_directory {
            return Err(crate::ENOTDIR);
        }
        if state.attached_namespace != namespace_id()? {
            return Err(EXDEV);
        }
        state.tree = tree_snapshot(state)?;
        let root_id = state.tree[0].id;
        update(|table, _| {
            if state.attached_id != 0 {
                let original = table
                    .iter()
                    .find(|p| p.id == state.attached_id)
                    .ok_or(EINVAL)?
                    .target
                    .clone();
                if suffix(&target, &original).is_some() {
                    return Err(EINVAL);
                }
                let ids = descendants(table, state.attached_id);
                table.retain(|p| !ids.contains(&p.id));
            }
            let parent = visible_mount(table, &target)?.map_or(ROOT_MOUNT_ID, |p| p.id);
            for (index, point) in state.tree.iter().enumerate() {
                let mut point = point.clone();
                if index == 0 {
                    point.parent = parent;
                }
                point.target = join(&target, point.target.trim_start_matches('/'));
                table.push(point);
            }
            Ok(())
        })?;
        state.attached_namespace = namespace_id()?;
        state.attached_id = root_id;
        Ok(())
    })
}
pub(super) fn descendants(table: &[MountPoint], root: u64) -> std::collections::HashSet<u64> {
    let mut ids = std::collections::HashSet::from([root]);
    for p in table {
        if ids.contains(&p.parent) {
            ids.insert(p.id);
        }
    }
    ids
}

/// Atomically change attachment policies, including recursive writer exclusion.
pub fn set_attributes(
    path: &str,
    flags: u32,
    set: u64,
    clear: u64,
    propagation: u64,
    userns: u64,
) -> Result<(), i32> {
    if flags & !(0x1000 | 0x100 | 0x8000) != 0 {
        return Err(EINVAL);
    }
    if (set | clear) & !(0xff | 0x100000 | 0x200000) != 0 {
        return Err(EINVAL);
    }
    if clear & 0x70 != 0 && clear & 0x70 != 0x70 || set & 0x70 != 0 && clear & 0x70 != 0x70 {
        return Err(EINVAL);
    }
    if propagation & !MS_PROPAGATION != 0 || propagation.count_ones() > 1 {
        return Err(EINVAL);
    }
    if set & 0x100000 == 0 && userns != 0 {
        return Err(EINVAL);
    }
    if clear & 0x100000 != 0 {
        return Err(EINVAL);
    }
    let mapping = if set & 0x100000 != 0 {
        if crate::credentials::current().uid != 0
            || crate::user_namespace::id(crate::job::process_id())? != 1
        {
            return Err(crate::EPERM);
        }
        Some(crate::user_namespace::descriptor_map(
            i32::try_from(userns).map_err(|_| crate::EBADF)?,
        )?)
    } else {
        None
    };

    if flags & 0x100 != 0 {
        fs::lstat(path)?;
    } else {
        fs::stat(path)?;
    }
    let path = canonical_mount_path(path, flags & 0x100 == 0)?;
    // A proc fd names the descriptor's attachment, even when another mount
    // now covers the same pathname. Native bind descriptors retain that
    // policy identity; do not apply attributes to procfs or re-resolve a name.
    let native_attachment = if flags & 0x100 == 0 {
        match crate::procfs::fd_magic_link(&path) {
            Some((pid, fd)) if pid == crate::job::process_id() => {
                super::native::reference(crate::get(fd)?)?.map(|d| d.policy)
            }
            _ => None,
        }
    } else {
        None
    };
    let mut changes = None;
    let mut apply = |tree: &mut Vec<MountPoint>, root: u64, namespace: u64| -> Result<(), i32> {
        if !tree.iter().any(|point| point.id == root) {
            return Err(ENOENT);
        }
        let ids = if flags & 0x8000 != 0 {
            descendants(tree, root)
        } else {
            std::collections::HashSet::from([root])
        };
        if (set | clear) != 0
            && tree.iter().filter(|p| ids.contains(&p.id)).any(|p| {
                p.flags & (MS_OVERLAY | MS_PROC | MS_TMPFS | MS_BIND | MS_ROOT) == 0
                    || (mapping.is_some() && p.flags & MS_OVERLAY == 0)
            })
        {
            return Err(EOPNOTSUPP);
        }
        let set_flags = attribute_flags(set & !0x100000)?
            & if clear & 0x70 == 0 {
                !(1 << 21)
            } else {
                u64::MAX
            };
        let clear_flags = (clear & 15)
            | if clear & 0x200000 != 0 { 256 } else { 0 }
            | if clear & 0x70 != 0 {
                1024 | (1 << 21) | (1 << 24)
            } else {
                0
            }
            | if clear & 0x80 != 0 { 2048 } else { 0 };
        let mut pending = Vec::new();
        for point in tree.iter_mut().filter(|p| ids.contains(&p.id)) {
            let reference = policy::get(namespace, point.id, point.flags)?;
            point.flags = (reference.flags() & !(clear_flags | MS_PROPAGATION))
                | (point.flags & MS_PROPAGATION)
                | set_flags;
            if propagation != 0 {
                super::propagation::change(point, propagation)?;
            }
            if let Some(mapping) = &mapping {
                if point.meta.idmap.is_some() {
                    return Err(crate::EPERM);
                }
                point.meta.idmap = Some(mapping.clone());
                point.flags |= MS_IDMAPPED;
            }
            pending.push((reference, point.flags));
        }
        let pending = policy::Changes::apply(pending)?;
        if let Some(mapping) = &mapping {
            pending.set_mapping(mapping)?;
        }
        changes = Some(pending);
        Ok(())
    };
    let result = if let Some(attachment) = native_attachment {
        if mapping.is_some() {
            return Err(EINVAL);
        }
        if attachment.namespace != namespace_id()? {
            return Err(EXDEV);
        }
        update(|table, _| apply(table, attachment.id, attachment.namespace))
    } else if let Some((fd, tail)) = tree_reference(&path) {
        change(fd, |state, _| {
            if mapping.is_some() && state.attached_id != 0 {
                return Err(EINVAL);
            }
            let tree = tree_snapshot(state)?;
            let selected = visible_mount(&tree, &tree_absolute(fd, tail)?)?
                .ok_or(ENOENT)?
                .id;
            if state.attached_id != 0 {
                if state.attached_namespace != namespace_id()? {
                    return Err(EXDEV);
                }
                update(|table, _| apply(table, selected, state.attached_namespace))?;
            } else {
                apply(&mut state.tree, selected, state.attached_namespace)?;
            }
            Ok(())
        })
    } else {
        if mapping.is_some() {
            return Err(EINVAL);
        }
        let path = namespace_path(&path)?;
        update(|table, _| {
            let id = visible_mount(table, &path)?.ok_or(EOPNOTSUPP)?.id;
            apply(table, id, namespace_id()?)
        })
    };
    if result.is_ok() {
        if let Some(changes) = changes {
            changes.commit();
        }
    }
    result
}

pub fn move_path(source: &str, target: &str, flags: u32) -> Result<(), i32> {
    if flags & !0x77 != 0 {
        return Err(EINVAL);
    }
    let from_stat = if flags & 1 == 0 {
        fs::lstat(source)?
    } else {
        fs::stat(source)?
    };
    let to_stat = if flags & 0x10 == 0 {
        fs::lstat(target)?
    } else {
        fs::stat(target)?
    };
    if (from_stat.st_mode & fs::S_IFMT == fs::S_IFDIR)
        != (to_stat.st_mode & fs::S_IFMT == fs::S_IFDIR)
    {
        return Err(crate::ENOTDIR);
    }
    let source = namespace_path(&canonical_mount_path(source, flags & 1 != 0)?)?;
    let target = namespace_path(&canonical_mount_path(target, flags & 0x10 != 0)?)?;
    if has_fixed_virtual_dispatch(&target) {
        return Err(EOPNOTSUPP);
    }
    update(|table, _| {
        let point = visible_mount(table, &source)?
            .filter(|p| p.target == source)
            .ok_or(EINVAL)?;
        if suffix(&target, &source).is_some()
            || table
                .iter()
                .any(|parent| parent.id == point.parent && parent.meta.peer != 0)
        {
            return Err(EINVAL);
        }
        let root = point.id;
        let ids = descendants(table, root);
        let mut moving: Vec<_> = table
            .iter()
            .filter(|p| ids.contains(&p.id))
            .cloned()
            .collect();
        table.retain(|p| !ids.contains(&p.id));
        let parent = visible_mount(table, &target)?.map_or(ROOT_MOUNT_ID, |p| p.id);
        for point in &mut moving {
            let rest = suffix(&point.target, &source).ok_or(EIO)?;
            point.target = join(&target, rest);
            if point.id == root {
                point.parent = parent;
            }
        }
        table.extend(moving);
        Ok(())
    })
}

#[cfg(test)]
mod tests;

fn canonical_mount_path(path: &str, follow: bool) -> Result<String, i32> {
    if crate::procfs::owns(path) || crate::tmpfs::owns(path) {
        return normalize(path);
    }
    overlay::canonical_guest(path, follow)
}
