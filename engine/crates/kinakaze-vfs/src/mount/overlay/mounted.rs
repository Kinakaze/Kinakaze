//! Mounted overlay operations. Layer objects are pinned; mutations of a shared
//! upper serialize on an inode-keyed native mutex in every hosted process.
use super::{
    Backing, Node, XattrNamespace, copy_up,
    directory::{Directory, DirectoryRecord},
};
use crate::fs::{self, S_IFDIR, S_IFLNK, S_IFMT, Stat, object::Object};
use crate::xattr::{Attributes, InodeLock};
use crate::{EBUSY, EEXIST, EINVAL, EIO, ENOENT, ENOTDIR, EROFS, EXDEV};
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_READ_ATTRIBUTES, FILE_READ_EA};

#[path = "export.rs"]
mod export;
#[path = "rights.rs"]
mod rights;
pub(crate) use rights::{export_rights, import_rights};
#[cfg(test)]
#[path = "mounted_tests.rs"]
mod tests;
pub use export::{encode_handle, encode_handle_fd, open_handle};

pub(crate) const USER_XATTR: u64 = 1 << 41;
const ACCESS: u32 = FILE_READ_ATTRIBUTES | FILE_READ_EA;

struct Instance {
    source: String,
    root: Node,
    work: Option<Object>,
    flags: u64,
    device: u64,
    fsid: u64,
    error_epoch: u64,
    policy: Arc<crate::mount::policy::Policy>,
}
fn super_policy(source: &str, flags: u64) -> Result<Arc<crate::mount::policy::Policy>, i32> {
    let id = source.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    crate::mount::policy::get(0, id, flags & 1)
}
#[derive(Clone)]
struct Writer {
    mount: Option<Arc<Object>>,
    superblock: Arc<Object>,
}

fn instances() -> &'static Mutex<HashMap<String, Arc<Instance>>> {
    static INSTANCES: OnceLock<Mutex<HashMap<String, Arc<Instance>>>> = OnceLock::new();
    INSTANCES.get_or_init(Default::default)
}
pub(crate) fn mount_pins(source: &str) -> Result<Vec<Arc<Object>>, i32> {
    let (source, _) = super::split_view(source)?;
    let instance = instance(source)?;
    let mut objects = Vec::new();
    for root in &instance.root.context.as_ref().ok_or(EIO)?.roots {
        let object = Object::reopen(root.object.raw(), ACCESS)?;
        crate::platform::try_set_inheritable(object.raw() as usize, true)?;
        objects.push(Arc::new(object));
    }
    if let Some(work) = &instance.work {
        let object = Object::reopen(work.raw(), ACCESS)?;
        crate::platform::try_set_inheritable(object.raw() as usize, true)?;
        objects.push(Arc::new(object));
    }
    Ok(objects)
}

fn volume_hint(path: &Path) -> Result<PathBuf, i32> {
    let mut hint = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => hint.push(component),
            _ => break,
        }
    }
    if !hint.is_absolute() {
        return Err(EINVAL);
    }
    Ok(hint)
}

fn put_path(bytes: &mut Vec<u8>, object: &Object) -> Result<(), i32> {
    let stat = fs::stat_handle(object.raw(), false)?;
    let path = object.path()?;
    let words: Vec<_> = path.as_os_str().encode_wide().collect();
    bytes.extend_from_slice(&(words.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&stat.st_dev.to_le_bytes());
    bytes.extend_from_slice(&stat.st_ino.to_le_bytes());
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    Ok(())
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], i32> {
        if length > self.0.len() {
            return Err(EIO);
        }
        let (value, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, i32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, i32> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn object(&mut self) -> Result<Object, i32> {
        let count = self.u32()? as usize;
        let dev = self.u64()?;
        let ino = self.u64()?;
        let words: Vec<u16> = self
            .take(count.checked_mul(2).ok_or(EIO)?)?
            .chunks_exact(2)
            .map(|v| u16::from_le_bytes([v[0], v[1]]))
            .collect();
        if words.contains(&0) {
            return Err(EIO);
        }
        let path = PathBuf::from(OsString::from_wide(&words));
        if !path.is_absolute() {
            return Err(EIO);
        }
        if let Ok(object) = Object::open(&path, ACCESS) {
            let stat = fs::stat_handle(object.raw(), false)?;
            if stat.st_dev == dev && stat.st_ino == ino {
                return Ok(object);
            }
        }
        let object = Object::by_id(&volume_hint(&path)?, ino, ACCESS)?;
        let stat = fs::stat_handle(object.raw(), false)?;
        if stat.st_dev != dev || stat.st_ino != ino {
            return Err(crate::ESTALE);
        }
        Ok(object)
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| {
            [
                DIGITS[(b >> 4) as usize] as char,
                DIGITS[(b & 15) as usize] as char,
            ]
        })
        .collect()
}
fn unhex(value: &str) -> Result<Vec<u8>, i32> {
    if value.len() % 2 != 0 {
        return Err(EIO);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let a = (pair[0] as char).to_digit(16).ok_or(EIO)?;
            let b = (pair[1] as char).to_digit(16).ok_or(EIO)?;
            Ok((a * 16 + b) as u8)
        })
        .collect()
}
pub(crate) fn decode_option(value: &str) -> Option<String> {
    String::from_utf8(unhex(value).ok()?).ok()
}

fn device(source: &str) -> u64 {
    // A stable anonymous device for this superblock, including namespace clones.
    let hash = source.bytes().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    });
    let minor = (hash as u32) | 0x8000_0000;
    (minor as u64 & 0xff) | ((minor as u64 & !0xff) << 12)
}

fn instance(source: &str) -> Result<Arc<Instance>, i32> {
    let (source, _) = split_view(source)?;
    if let Some(found) = instances().lock().map_err(|_| EIO)?.get(source).cloned() {
        return Ok(found);
    }
    let encoded = source
        .split(';')
        .find_map(|p| p.strip_prefix("native="))
        .ok_or(crate::EOPNOTSUPP)?;
    let bytes = unhex(encoded)?;
    let mut input = Reader(&bytes);
    let version = input.u32()?;
    if !matches!(version, 1 | 2 | 3 | 4) {
        return Err(EIO);
    }
    let flags = input.u64()?;
    let error_epoch = if version >= 4 { input.u64()? } else { 0 };
    let count = input.u32()? as usize;
    if count == 0 || count > 501 {
        return Err(EIO);
    }
    let mut objects = Vec::with_capacity(count);
    for _ in 0..count {
        objects.push(input.object()?);
    }
    let work = match input.u32()? {
        0 => None,
        1 => Some(input.object()?),
        _ => return Err(EIO),
    };
    if !input.0.is_empty() {
        return Err(EIO);
    }
    let namespace = if flags & USER_XATTR != 0 {
        XattrNamespace::User
    } else {
        XattrNamespace::Trusted
    };
    let root = Node::from_objects(objects, namespace)?.configure(flags, work.as_ref())?;
    let fsid = root.filesystem_id()?;
    let found = Arc::new(Instance {
        source: source.into(),
        policy: super_policy(source, flags)?,
        root,
        work,
        flags,
        device: device(source),
        fsid,
        error_epoch,
    });
    Ok(instances()
        .lock()
        .map_err(|_| EIO)?
        .entry(source.into())
        .or_insert(found)
        .clone())
}
pub(crate) fn split_view(source: &str) -> Result<(&str, String), i32> {
    if let Some((source, view)) = source.split_once(";view=") {
        Ok((source, String::from_utf8(unhex(view)?).map_err(|_| EIO)?))
    } else {
        Ok((source, String::new()))
    }
}
pub(crate) fn with_view(source: &str, view: &str) -> String {
    format!("{source};view={}", hex(view.as_bytes()))
}

/// Validate all layer inodes before the mount table is changed. Stored native
/// IDs keep a renamed root from being mistaken for a replacement pathname.
pub(crate) fn prepare_mount(
    lowerdirs: &[String],
    upper: Option<&str>,
    work: Option<&str>,
    flags: u64,
) -> Result<String, i32> {
    prepare_mount_inner(lowerdirs, upper, work, flags, None)
}

pub(crate) fn prepare_mount_pinned(
    lowerdirs: &[String],
    upper: Option<&str>,
    work: Option<&str>,
    flags: u64,
    pins: Vec<Object>,
) -> Result<String, i32> {
    prepare_mount_inner(lowerdirs, upper, work, flags, Some(pins))
}

fn prepare_mount_inner(
    lowerdirs: &[String],
    upper: Option<&str>,
    work: Option<&str>,
    flags: u64,
    pins: Option<Vec<Object>>,
) -> Result<String, i32> {
    let work_name = work.unwrap_or("").to_owned();
    if lowerdirs.is_empty()
        || lowerdirs.len() > 500
        || super::features::data_count(flags) >= lowerdirs.len()
        || upper.is_some() != work.is_some()
    {
        return Err(EINVAL);
    }
    if flags
        & !(1
            | 2
            | 4
            | 8
            | 16
            | 256
            | 1024
            | 2048
            | (1 << 21)
            | (1 << 24)
            | USER_XATTR
            | super::features::ALL)
        != 0
    {
        return Err(EINVAL);
    }
    // The native backend currently enforces read-only at every mutation entry.
    // Other VFS policy bits require execution/device/atime support outside this
    // module; never publish a mount claiming policies that are not enforced.
    if flags & !(15 | 256 | 1024 | 2048 | (1 << 21) | (1 << 24) | USER_XATTR | super::features::ALL)
        != 0
    {
        return Err(crate::EOPNOTSUPP);
    }
    let mut roots = pins.unwrap_or_default();
    if roots.is_empty() {
        for path in upper
            .into_iter()
            .chain(lowerdirs.iter().map(String::as_str))
            .chain(work)
        {
            let absolute = fs::absolute_linux(path);
            if resolve(&absolute, true, false)?.is_some_and(|resolved| resolved.location.is_some())
            {
                // A nested layer needs a recursive overlay node, not whichever
                // physical directory happened to win this pathname lookup.
                return Err(crate::EOPNOTSUPP);
            }
            let native = crate::resolve_linux_path(&absolute).map_err(path_error)?;
            let object = Object::open(&native, ACCESS)?;
            if fs::stat_handle(object.raw(), false)?.st_mode & S_IFMT != S_IFDIR {
                return Err(ENOTDIR);
            }
            roots.push(object);
        }
    }
    if roots.len() != lowerdirs.len() + usize::from(upper.is_some()) + usize::from(work.is_some()) {
        return Err(EINVAL);
    }
    for object in &roots {
        if fs::stat_handle(object.raw(), false)?.st_mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
    }
    let work = if work.is_some() { roots.pop() } else { None };
    if let Some(work) = &work {
        super::volatile::check_marker(work)?;
        let work_stat = fs::stat_handle(work.raw(), false)?;
        let upper_stat = fs::stat_handle(roots[0].raw(), false)?;
        if work_stat.st_dev != upper_stat.st_dev {
            return Err(EXDEV);
        }
        let upper_path = roots[0].path()?;
        let work_path = work.path()?;
        if upper_path.starts_with(&work_path) || work_path.starts_with(&upper_path) {
            return Err(EINVAL);
        }
        if work
            .entries()?
            .iter()
            .any(|name| flags & super::features::INDEX == 0 || name != "index")
        {
            return Err(crate::ENOTEMPTY);
        }
        // A writable layer must support real EAs; opening with write rights is
        // also checked before publication, rather than during the first write.
        let attributes = unsafe { Attributes::from_handle(roots[0].raw(), true)? };
        attributes.snapshot()?;
        for lower in roots.iter().skip(1) {
            let path = lower.path()?;
            if path.starts_with(&upper_path)
                || upper_path.starts_with(&path)
                || path.starts_with(&work_path)
                || work_path.starts_with(&path)
            {
                return Err(EINVAL);
            }
        }
    }
    let flags = flags | if upper.is_none() { 1 } else { 0 };
    let mut bytes = Vec::new();
    let error_epoch = if flags & super::features::VOLATILE != 0 {
        super::volatile::sample(&roots[0])?
    } else {
        0
    };
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&error_epoch.to_le_bytes());
    bytes.extend_from_slice(&(roots.len() as u32).to_le_bytes());
    for root in &roots {
        put_path(&mut bytes, root)?;
    }
    bytes.extend_from_slice(&(work.is_some() as u32).to_le_bytes());
    if let Some(work) = &work {
        put_path(&mut bytes, work)?;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| EIO)?
        .as_nanos();
    let source = format!(
        "overlay:lowerdir_hex={};upperdir_hex={};workdir_hex={};native={};instance={}-{}",
        lowerdirs
            .iter()
            .map(|s| hex(s.as_bytes()))
            .collect::<Vec<_>>()
            .join(":"),
        hex(upper.unwrap_or("").as_bytes()),
        hex(work_name.as_bytes()),
        hex(&bytes),
        std::process::id(),
        nonce
    );
    // The diagnostic workdir spelling is supplied below without using a native
    // path in the guest-visible mount options.
    let namespace = if flags & USER_XATTR != 0 {
        XattrNamespace::User
    } else {
        XattrNamespace::Trusted
    };
    let root = Node::from_objects(roots, namespace)?.configure(flags, work.as_ref())?;
    let fsid = root.filesystem_id()?;
    if flags & super::features::VOLATILE != 0 {
        super::volatile::create_marker(work.as_ref().ok_or(EINVAL)?)?;
    }
    let found = Arc::new(Instance {
        source: source.clone(),
        policy: super_policy(&source, flags)?,
        root,
        work,
        flags,
        device: device(&source),
        fsid,
        error_epoch,
    });
    instances()
        .lock()
        .map_err(|_| EIO)?
        .insert(source.clone(), found);
    Ok(source)
}

fn path_error(error: crate::path::PathError) -> i32 {
    match error {
        crate::path::PathError::Filesystem(e) => e,
        crate::path::PathError::TooManySymlinks => crate::ELOOP,
        _ => EINVAL,
    }
}

#[derive(Clone)]
pub(crate) struct Location {
    instance: Arc<Instance>,
    policy: Option<Arc<crate::mount::policy::Policy>>,
    pub(crate) guest: String,
    components: Vec<String>,
    node: Option<Node>,
}
impl Location {
    fn read_only(&self) -> Result<bool, i32> {
        Ok(self.instance.policy.flags() & 1 != 0
            || self.policy.as_ref().is_some_and(|p| p.flags() & 1 != 0))
    }
    fn writer(&self) -> Result<Option<Writer>, i32> {
        let superblock = self.instance.policy.writer()?;
        let mount = self.policy.as_ref().map(|p| p.writer()).transpose()?;
        Ok(Some(Writer { superblock, mount }))
    }
    fn lower_exists(&self) -> Result<bool, i32> {
        if self.instance.work.is_none() {
            return Ok(true);
        }
        let entries: Vec<_> = self.instance.root.entries.iter().skip(1).cloned().collect();
        if entries.is_empty() {
            return Ok(false);
        }
        let mut node = Node {
            entries,
            namespace: self.instance.root.namespace,
            context: self.instance.root.context.clone(),
        };
        for component in &self.components {
            node = match node.child(component) {
                Ok(node) => node,
                Err(ENOENT | ENOTDIR) => return Ok(false),
                Err(e) => return Err(e),
            };
        }
        Ok(true)
    }
    fn lookup(&self) -> Result<Node, i32> {
        let mut node = self.instance.root.clone();
        for part in &self.components {
            node = node.child(part)?;
        }
        Ok(node)
    }
    fn upper(&self, truncate: bool) -> Result<Node, i32> {
        self.upper_mode(truncate, false)
    }
    fn upper_mode(&self, truncate: bool, metadata_only: bool) -> Result<Node, i32> {
        if self.read_only()? {
            return Err(EROFS);
        }
        let parts: Vec<_> = self.components.iter().map(String::as_str).collect();
        let node = copy_up::ensure_upper_mode(
            &self.instance.root,
            self.instance.work.as_ref().ok_or(EROFS)?,
            &parts,
            truncate,
            metadata_only && self.instance.flags & super::features::METACOPY != 0,
        )?;
        self.remember_upper(&node)?;
        Ok(node)
    }
    fn remember_upper(&self, node: &Node) -> Result<(), i32> {
        let inode = self.metadata(node)?.st_ino;
        let pin = pin_node(node)?;
        let mut descriptions = descriptions().lock().map_err(|_| EIO)?;
        for description in descriptions.values_mut() {
            if description.inode == inode
                && description.location.instance.source == self.instance.source
                && (description.location.components == self.components
                    || self.instance.flags & super::features::INDEX != 0)
            {
                description.location.node = Some(pin.clone());
            }
        }
        Ok(())
    }
    fn parent(&self) -> Result<(Node, &str), i32> {
        let (name, ancestors) = self.components.split_last().ok_or(EBUSY)?;
        let parts: Vec<_> = ancestors.iter().map(String::as_str).collect();
        let node = copy_up::ensure_upper(
            &self.instance.root,
            self.instance.work.as_ref().ok_or(EROFS)?,
            &parts,
            false,
        )?;
        Ok((node, name))
    }
    fn metadata(&self, node: &Node) -> Result<Stat, i32> {
        let refreshed = node.refreshed()?;
        let node = &refreshed;
        if !node.is_directory()
            && node.backing_layer() != 0
            && !node.entries[0].indexed
            && let Some(index) = self
                .instance
                .root
                .context
                .as_ref()
                .and_then(|context| context.index.as_ref())
        {
            match index.lookup(&node.origin()?) {
                Ok(Some(object)) => {
                    let mut indexed = Backing::from_object(object, node.namespace, 0)?;
                    indexed.indexed = true;
                    let mut entries = vec![indexed];
                    if entries[0].metacopy.is_some() {
                        entries.extend(node.entries.clone());
                    }
                    return self.metadata(&Node {
                        entries,
                        namespace: node.namespace,
                        context: node.context.clone(),
                    });
                }
                Err(crate::ESTALE) => {
                    let mut stat = fs::stat_handle(node.backing_object().raw(), false)?;
                    stat.st_nlink = 0;
                    stat.st_dev = self.instance.device;
                    return Ok(stat);
                }
                Err(e) => return Err(e),
                _ => {}
            }
        }
        let mut stat = fs::stat_handle(node.backing_object().raw(), false)?;
        let mut origin_device = stat.st_dev;
        if node.is_metacopy() {
            let data = fs::stat_handle(node.data_object().raw(), false)?;
            stat.st_size = data.st_size;
            stat.st_blocks = data.st_blocks;
        }
        if self.instance.work.is_some() && (node.backing_layer() == 0 || node.entries[0].indexed) {
            let attrs = unsafe { Attributes::from_handle(node.backing_object().raw(), false)? };
            if let Some(origin) = attrs
                .snapshot()?
                .get(format!("{}kinakaze.origin", node.namespace.prefix()).as_bytes())
            {
                if origin.len() != 16 {
                    return Err(EIO);
                }
                stat.st_ino = u64::from_le_bytes(origin[8..].try_into().unwrap());
                origin_device = u64::from_le_bytes(origin[..8].try_into().unwrap());
            }
            if self.instance.flags & super::features::INDEX != 0
                && let Some(links) = attrs
                    .snapshot()?
                    .get(format!("{}kinakaze.nlink", node.namespace.prefix()).as_bytes())
            {
                stat.st_nlink = u64::from_le_bytes(links.as_slice().try_into().map_err(|_| EIO)?);
            }
        }
        (stat.st_dev, stat.st_ino) =
            node.map_inode(origin_device, stat.st_ino, self.instance.device);
        if let Some(policy) = &self.policy {
            if let Some(map) = policy.mapping()? {
                map.stat(&mut stat);
            }
        }
        Ok(stat)
    }
}

pub(crate) struct Resolved {
    pub(crate) path: PathBuf,
    pub(crate) location: Option<Location>,
    guest: String,
}
#[cfg(test)]
thread_local! { static RESOLUTION_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub(crate) fn canonical_guest(path: &str, follow: bool) -> Result<String, i32> {
    let resolved = resolve_inner(path, follow, false, true, None)?.ok_or(EIO)?;
    if let Some(root) = crate::path::namespace_root_path()?
        && let Some(rest) = resolved.guest.strip_prefix(&root)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return Ok(format!("/{}", rest.trim_start_matches('/')));
    }
    Ok(resolved.guest)
}

/// Walk guest components, expanding symlinks in the merged namespace, including
/// links which enter or leave a mount. Native layer paths never become guest /
/// when an absolute symlink is followed.
#[track_caller]
pub(crate) fn resolve(
    path: &str,
    follow: bool,
    allow_missing: bool,
) -> Result<Option<Resolved>, i32> {
    resolve_inner(path, follow, allow_missing, false, None)
}
#[track_caller]
fn resolve_inner(
    path: &str,
    follow: bool,
    allow_missing: bool,
    force: bool,
    virtual_crossing: Option<&mut bool>,
) -> Result<Option<Resolved>, i32> {
    let _profile = super::profile::resolution(path);
    let overlay_root = crate::path::overlay_root();
    let namespace_root = crate::path::namespace_root_path()?;
    if namespace_root.is_some() && (crate::tmpfs::owns(path) || crate::procfs::owns(path)) {
        return Ok(None);
    }
    let mut tree = crate::mount::api::tree_reference(path);
    if tree.is_some_and(|(_, tail)| tail.is_empty()) && !follow {
        return Ok(None);
    }
    if !force && !crate::mount::has_overlay()? && namespace_root.is_none() && tree.is_none() {
        return Ok(None);
    }
    #[cfg(test)]
    RESOLUTION_WALKS.set(RESOLUTION_WALKS.get() + 1);
    if path.contains('\0') || path.contains('\\') {
        return Err(EINVAL);
    }
    let root = if let Some(root) = &overlay_root {
        root.native_base.clone()
    } else {
        if namespace_root.is_some() {
            crate::path::default_system_root().to_path_buf()
        } else {
            crate::system_root().map_err(path_error)?
        }
    };
    let absolute = if let Some((fd, tail)) = tree {
        format!("{}/{tail}", crate::mount::api::tree_absolute(fd, "")?)
    } else if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("{}/{path}", fs::getcwd())
    };
    let mut pending: VecDeque<String> = absolute
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let root_base: Vec<String> = namespace_root
        .as_ref()
        .map(|r| {
            r.split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let mut base = if tree.is_some() {
        Vec::new()
    } else {
        root_base.clone()
    };
    let mut names = base.clone();
    let mut links = 0;
    let mut result: Option<Resolved> = None;
    // Reuse only the parent resolved in this walk. Rewalking every native
    // prefix from the root makes a depth-n path perform O(n^2) inode queries
    // whenever any overlay exists in the namespace (including unrelated ones).
    // A changed mount translation, overlay crossing or symlink restarts lookup.
    // Nothing survives this operation, so external rename/replacement remains
    // visible to the next lookup without a timer or an invalidation protocol.
    let mut native_parent: Option<(String, PathBuf)> = None;
    loop {
        let next = pending.pop_front();
        match next.as_deref() {
            Some(".") => continue,
            Some("..") => {
                if names.len() > base.len() {
                    names.pop();
                }
            }
            Some(name) => names.push(name.into()),
            None if result.is_some() => break,
            None => {}
        }
        let within = format!("/{}", names.join("/"));
        let relative = tree
            .map(|(fd, _)| crate::mount::api::tree_relative(fd, &within))
            .transpose()?;
        let guest = if let Some((fd, _)) = tree {
            format!(
                "{}/{}",
                crate::mount::api::tree_path(fd)?,
                relative.as_deref().unwrap_or("")
            )
        } else {
            within.clone()
        };
        let final_component = pending.is_empty();
        let detached = tree
            .map(|(fd, _)| crate::mount::api::tree_mount(fd, relative.as_deref().unwrap_or("")))
            .transpose()?;
        let mounted = if let Some((source, relative, flags)) = &detached {
            (flags & crate::mount::MS_OVERLAY != 0).then(|| (source.clone(), relative.clone()))
        } else {
            crate::mount::overlay_location(&guest)?
        };
        let mounted = match mounted {
            Some(mounted) => Some(mounted),
            None if detached.is_none() && !crate::mount::has_attachment(&guest)? => {
                overlay_root.as_ref().map(|r| {
                    let suffix = guest
                        .strip_prefix(&r.namespace_path)
                        .unwrap_or("")
                        .trim_start_matches('/');
                    (
                        r.source.clone(),
                        [r.relative.as_str(), suffix]
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>()
                            .join("/"),
                    )
                })
            }
            None => None,
        };
        let (native, location, symlink) = if let Some((source, relative)) = mounted {
            native_parent = None;
            let instance = instance(&source)?;
            let _guard = InodeLock::acquire(instance.root.backing_object().raw())?;
            let components: Vec<String> = relative
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            let mut location = Location {
                instance: instance.clone(),
                policy: crate::mount::attachment_reference(&guest, &source)?,
                guest: guest.clone(),
                components,
                node: None,
            };
            // Continue this one path walk from the directory inode just
            // resolved, like a component-relative lookup. Rewalking every
            // prefix from the layer root makes depth N require O(N^2) opens.
            // The pin is local to this operation; symlink expansion clears it,
            // and a mount/source or prefix change falls back to a fresh lookup.
            let parent = result
                .as_ref()
                .and_then(|previous| previous.location.as_ref())
                .filter(|previous| {
                    Arc::ptr_eq(&previous.instance, &instance)
                        && location.components.len() == previous.components.len() + 1
                        && location.components.starts_with(&previous.components)
                })
                .and_then(|previous| previous.node.as_ref());
            let looked_up = if let Some(parent) = parent {
                parent.child(location.components.last().ok_or(EIO)?)
            } else {
                location.lookup()
            };
            match looked_up {
                Ok(node) => {
                    let link = if node.backing_metadata().st_mode & S_IFMT == S_IFLNK {
                        Some(fs::readlink_handle(node.backing_object().raw())?)
                    } else {
                        None
                    };
                    if !final_component && link.is_none() && !node.is_directory() {
                        return Err(ENOTDIR);
                    }
                    let native = node.data_object().path()?;
                    location.node = Some(node);
                    (native, Some(location), link)
                }
                Err(ENOENT) if final_component && allow_missing => {
                    (PathBuf::new(), Some(location), None)
                }
                Err(error) => return Err(error),
            }
        } else {
            let translated = if let Some((source, _, _)) = detached {
                source
            } else {
                match crate::mount::native_translation(&guest)? {
                    Some(path) => path,
                    None => {
                        if let Some(crossing) = virtual_crossing {
                            *crossing = true;
                            return Ok(None);
                        }
                        return Err(crate::EOPNOTSUPP);
                    }
                }
            };
            let continued = translated.rsplit_once('/').and_then(|(parent, name)| {
                let (previous, native) = native_parent.as_ref()?;
                (parent == previous && !name.is_empty() && name != "." && name != "..")
                    .then(|| native.join(crate::path::escape_component(name).as_ref()))
            });
            let native = match continued {
                Some(native) => native,
                None => {
                    crate::path::resolve_unmounted(&root, &translated, false).map_err(path_error)?
                }
            };
            let link = crate::path::emulated_symlink_target(&native)?;
            native_parent = Some((translated, native.clone()));
            (native, None, link)
        };
        if let Some(target) = symlink
            && (!final_component || follow)
        {
            native_parent = None;
            if let Some(location) = &location
                && crate::mount::attachment_policy(&location.guest, &location.instance.source)?
                    .is_some_and(|flags| flags & 256 != 0)
            {
                return Err(crate::ELOOP);
            }
            links += 1;
            if links > 40 {
                return Err(crate::ELOOP);
            }
            names.pop();
            if target.starts_with('/') {
                tree = None;
                base = root_base.clone();
                names = base.clone();
            }
            let parts: Vec<_> = target
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            for part in parts.into_iter().rev() {
                pending.push_front(part);
            }
            result = None;
            continue;
        }
        result = Some(Resolved {
            path: native,
            location,
            guest,
        });
        if final_component {
            break;
        }
    }
    Ok(result)
}

/// An operation-scoped writable path. The guard must live through the native
/// mutation; dropping it before opening the returned path is not sufficient.
pub struct WritePath {
    path: PathBuf,
    location: Option<Location>,
    publication: Option<(Node, String)>,
    anonymous: bool,
    _guard: Option<InodeLock>,
    writer: Option<Writer>,
    _native_writer: Option<Arc<Object>>,
}
impl std::ops::Deref for WritePath {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}
impl AsRef<Path> for WritePath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

fn temporary(work: &Object) -> Result<PathBuf, i32> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut core::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    let mut random = [0u8; 16];
    if unsafe { BCryptGenRandom(std::ptr::null_mut(), random.as_mut_ptr(), 16, 2) } < 0 {
        return Err(EIO);
    }
    Ok(work.path()?.join(format!("create-{}", hex(&random))))
}

impl WritePath {
    pub fn initialize_created_handle(
        &self,
        handle: windows_sys::Win32::Foundation::HANDLE,
        mode: u32,
    ) -> Result<(), i32> {
        let Some(location) = &self.location else {
            return fs::initialize_created_handle(handle, &self.path, mode);
        };
        let parent = if self.anonymous {
            location.lookup()?
        } else {
            location.parent()?.0
        };
        let parent = location.metadata(&parent)?;
        fs::initialize_inode(handle, &parent, mode)?;
        if let Some(policy) = &location.policy {
            if let Some(map) = policy.mapping()? {
                crate::fs::inode::update(handle, |inode| {
                    inode.uid = Some(
                        map.up(inode.uid.ok_or(EIO)?, false)
                            .ok_or(crate::EOVERFLOW)?,
                    );
                    inode.gid = Some(
                        map.up(inode.gid.ok_or(EIO)?, true)
                            .ok_or(crate::EOVERFLOW)?,
                    );
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    pub fn initialize_created(&self, mode: u32) -> Result<(), i32> {
        let object = Object::open(&self.path, ACCESS)?;
        self.initialize_created_handle(object.raw(), mode)
    }

    pub(crate) fn description(
        &self,
        handle: windows_sys::Win32::Foundation::HANDLE,
        writable: bool,
    ) -> Result<Option<Description>, i32> {
        let Some(mut location) = self.location.clone() else {
            return Ok(None);
        };
        let node = if self.anonymous {
            let object = Object::reopen(handle, ACCESS)?;
            let entry = Backing::from_object(object, location.instance.root.namespace, 0)?;
            let name = format!("#{}", entry.metadata.st_ino);
            location.guest.push('/');
            location.guest.push_str(&name);
            location.components.push(name);
            Node {
                entries: vec![entry],
                namespace: location.instance.root.namespace,
                context: location.instance.root.context.clone(),
            }
        } else {
            location.lookup()?
        };
        let inode = location.metadata(&node)?.st_ino;
        let directory = if node.is_directory() {
            Some(Arc::new(Directory::create()?))
        } else {
            None
        };
        location.node = Some(pin_node(&node)?);
        let anchor = if directory.is_some() {
            crate::mount::api::pin_tree(&location.guest)?
        } else {
            None
        };
        Ok(Some(Description {
            location,
            inode,
            directory,
            writer: if writable { self.writer.clone() } else { None },
            anchor,
        }))
    }
    /// Publish a fully created inode. Until this succeeds the old whiteout is
    /// still visible, and a failed creator leaves both layers unchanged.
    pub fn finish(&mut self) -> Result<(), i32> {
        let Some((parent, name)) = self.publication.as_ref() else {
            return Ok(());
        };
        let object = Object::open(&self.path, ACCESS | DELETE)?;
        let directory = fs::stat_handle(object.raw(), false)?.st_mode & S_IFMT == S_IFDIR;
        let location = self.location.as_ref().ok_or(EIO)?;
        let stored = crate::path::escape_component(name);
        let target = match parent
            .backing_object()
            .child(OsStr::new(stored.as_ref()), ACCESS | DELETE)
        {
            Ok(object) => {
                let entry = Backing::from_object(object, parent.namespace, 0)?;
                if !entry.whiteout {
                    return Err(EEXIST);
                }
                Some(entry)
            }
            Err(ENOENT) => None,
            Err(error) => return Err(error),
        };
        if directory && (target.is_some() || location.lower_exists()?) {
            unsafe { Attributes::from_handle(object.raw(), true)? }.set(
                format!("{}opaque", parent.namespace.prefix()).as_bytes(),
                b"y",
                0,
            )?;
        }
        // NTFS cannot replace a non-directory whiteout with a directory. Move
        // that inode aside while holding the upper lock and restore on failure.
        let mut saved = if directory {
            target
                .map(|entry| {
                    SavedEntry::new(
                        entry.object.as_ref(),
                        parent.backing_object(),
                        name,
                        location.instance.work.as_ref().ok_or(EROFS)?,
                        parent.namespace,
                    )
                })
                .transpose()?
        } else {
            None
        };
        unsafe {
            fs::rename_host_handle_relative(
                object.raw(),
                parent.backing_object().raw(),
                OsStr::new(stored.as_ref()),
                !directory,
            )?;
        }
        if let Some(saved) = &mut saved {
            saved.discard()?;
        }
        self.path = object.path()?;
        self.publication = None;
        Ok(())
    }
}

pub(crate) enum OpenPath {
    Native(Option<WritePath>),
    /// The full descriptor walker owns cross-backend links and their shared
    /// forty-link limit; never turn procfs or tmpfs into a native layer alias.
    Virtual,
}

pub(crate) fn open_path(path: &str, flags: i32) -> Result<OpenPath, i32> {
    let is_path = flags & fs::O_PATH != 0;
    let exclusive = !is_path && flags & (fs::O_CREAT | fs::O_EXCL) == fs::O_CREAT | fs::O_EXCL;
    let follow = flags & fs::O_NOFOLLOW == 0 && !exclusive;
    let create = !is_path && flags & fs::O_CREAT != 0;
    let mut virtual_crossing = false;
    let resolved = resolve_inner(path, follow, create, false, Some(&mut virtual_crossing))?;
    if virtual_crossing {
        return Ok(OpenPath::Virtual);
    }
    open_native_path(path, flags, resolved, is_path, exclusive, follow, create)
        .map(OpenPath::Native)
}

fn open_native_path(
    path: &str,
    flags: i32,
    resolved: Option<Resolved>,
    is_path: bool,
    exclusive: bool,
    follow: bool,
    create: bool,
) -> Result<Option<WritePath>, i32> {
    let Some(Resolved {
        location: Some(location),
        ..
    }) = resolved
    else {
        return Ok(None);
    };
    let guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let node = match location.lookup() {
        Ok(node) => Some(node),
        Err(ENOENT) if create => None,
        Err(e) => return Err(e),
    };
    if !is_path && flags & 0o20000000 != 0 {
        if flags & fs::O_DIRECTORY == 0 || flags & fs::O_ACCMODE == fs::O_RDONLY {
            return Err(EINVAL);
        }
        if !node.as_ref().is_some_and(Node::is_directory) {
            return Err(ENOTDIR);
        }
        let writer = location.writer()?;
        let upper = location.upper(false)?;
        return Ok(Some(WritePath {
            path: upper.backing_object().path()?,
            location: Some(location),
            publication: None,
            anonymous: true,
            _native_writer: None,
            writer,
            _guard: Some(guard),
        }));
    }
    if exclusive && node.is_some() {
        return Err(EEXIST);
    }
    if !is_path && flags & fs::O_ACCMODE == 3 {
        return Err(EINVAL);
    }
    if let Some(node) = &node {
        if !is_path
            && location.policy.as_ref().is_some_and(|p| p.flags() & 4 != 0)
            && matches!(
                node.backing_metadata().st_mode & S_IFMT,
                fs::S_IFCHR | fs::S_IFBLK
            )
        {
            return Err(crate::EACCES);
        }
        if flags & fs::O_DIRECTORY != 0 && !node.is_directory() {
            return Err(ENOTDIR);
        }
        if !is_path && node.is_directory() && flags & fs::O_ACCMODE != fs::O_RDONLY {
            return Err(crate::EISDIR);
        }
        if !is_path && node.backing_metadata().st_mode & S_IFMT == S_IFLNK && !follow {
            return Err(crate::ELOOP);
        }
    }
    let write = !is_path
        && (node.is_none() || flags & fs::O_ACCMODE != fs::O_RDONLY || flags & fs::O_TRUNC != 0);
    if write {
        // The recursive native mutex keeps the initial existence/type check
        // valid until prepare has obtained its own operation guard.
        return prepare(path, follow, create, flags & fs::O_TRUNC != 0).map(Some);
    }
    let node = node.ok_or(ENOENT)?;
    Ok(Some(WritePath {
        path: if is_path {
            node.backing_object()
        } else {
            node.data_object()
        }
        .path()?,
        location: Some(location),
        publication: None,
        anonymous: false,
        _guard: Some(guard),
        writer: None,
        _native_writer: None,
    }))
}
impl Drop for WritePath {
    fn drop(&mut self) {
        if self.publication.is_some() {
            if let Ok(object) = Object::open(&self.path, DELETE) {
                let _ = fs::unlink_inode(object.raw());
            }
        }
    }
}

struct SavedEntry {
    object: Object,
    parent: Object,
    name: String,
    namespace: XattrNamespace,
    active: bool,
}
impl SavedEntry {
    fn new(
        object: &Object,
        parent: &Object,
        name: &str,
        work: &Object,
        namespace: XattrNamespace,
    ) -> Result<Self, i32> {
        let object = Object::reopen(object.raw(), ACCESS | DELETE)?;
        let parent = Object::reopen(parent.raw(), ACCESS)?;
        let backup = temporary(work)?;
        unsafe {
            fs::rename_host_handle(object.raw(), &backup, false)?;
        }
        Ok(Self {
            object,
            parent,
            name: name.into(),
            namespace,
            active: true,
        })
    }
    fn discard(&mut self) -> Result<(), i32> {
        if fs::stat_handle(self.object.raw(), false)?.st_mode & S_IFMT == S_IFDIR {
            // Only logically empty directories can be saved for replacement.
            // Their remaining native children are private whiteout inodes.
            for name in self.object.entries()? {
                let child = self.object.child(&name, ACCESS | DELETE)?;
                let entry = Backing::from_object(child, self.namespace, 0)?;
                if !entry.whiteout {
                    return Err(crate::ENOTEMPTY);
                }
                fs::unlink_inode(entry.object.raw())?;
            }
        }
        fs::unlink_inode(self.object.raw())?;
        self.active = false;
        Ok(())
    }
}
impl Drop for SavedEntry {
    fn drop(&mut self) {
        if self.active {
            if let Err(error) = unsafe {
                fs::rename_host_handle_relative(
                    self.object.raw(),
                    self.parent.raw(),
                    OsStr::new(crate::path::escape_component(&self.name).as_ref()),
                    false,
                )
            } {
                eprintln!(
                    "kinakaze: overlay rollback of {} failed: errno={error}",
                    self.name
                );
            }
        }
    }
}

pub fn prepare_write(path: &str, follow: bool, create: bool) -> Result<WritePath, i32> {
    prepare_mode(path, follow, create, false, true)
}
/// Resolve metadata independently of the lower data backing a metacopy file.
/// The guard keeps namespace mutations out until the caller pins the inode.
pub fn metadata_path(path: &str, follow: bool) -> Result<WritePath, i32> {
    let resolved = resolve(path, follow, false)?;
    let Some(Resolved {
        location: Some(location),
        ..
    }) = resolved
    else {
        let path = if follow {
            crate::resolve_linux_path(&fs::absolute_linux(path))
        } else {
            crate::resolve_linux_path_no_follow(&fs::absolute_linux(path))
        }
        .map_err(path_error)?;
        return Ok(WritePath {
            path,
            location: None,
            publication: None,
            anonymous: false,
            _guard: None,
            writer: None,
            _native_writer: None,
        });
    };
    let guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let path = location.lookup()?.backing_object().path()?;
    Ok(WritePath {
        path,
        location: Some(location),
        publication: None,
        anonymous: false,
        _guard: Some(guard),
        writer: None,
        _native_writer: None,
    })
}
pub fn prepare_create(path: &str) -> Result<WritePath, i32> {
    match open_path(path, fs::O_CREAT | fs::O_EXCL | fs::O_WRONLY)? {
        OpenPath::Native(Some(path)) => return Ok(path),
        OpenPath::Native(None) => {}
        OpenPath::Virtual => return Err(crate::EOPNOTSUPP),
    }
    prepare_write(path, false, true)
}
fn prepare(path: &str, follow: bool, create: bool, truncate: bool) -> Result<WritePath, i32> {
    prepare_mode(path, follow, create, truncate, false)
}
fn prepare_mode(
    path: &str,
    follow: bool,
    create: bool,
    truncate: bool,
    metadata_only: bool,
) -> Result<WritePath, i32> {
    let resolved = resolve(path, follow, create)?;
    let Some(Resolved {
        location: Some(location),
        ..
    }) = resolved
    else {
        let native_writer = crate::mount::native::write_path(path, follow)?;
        let native = if follow {
            crate::resolve_linux_path(&fs::absolute_linux(path))
        } else {
            crate::resolve_linux_path_no_follow(&fs::absolute_linux(path))
        }
        .map_err(path_error)?;
        return Ok(WritePath {
            path: native,
            location: None,
            publication: None,
            anonymous: false,
            _guard: None,
            writer: None,
            _native_writer: native_writer,
        });
    };
    let writer = location.writer()?;
    let guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let mut publication = None;
    let native = match location.lookup() {
        Ok(_) => location
            .upper_mode(truncate, metadata_only)?
            .backing_object()
            .path()?,
        Err(ENOENT) if create => {
            let (parent, name) = location.parent()?;
            publication = Some((parent, name.to_owned()));
            temporary(location.instance.work.as_ref().ok_or(EROFS)?)?
        }
        Err(error) => return Err(error),
    };
    Ok(WritePath {
        path: native,
        location: Some(location),
        publication,
        anonymous: false,
        _guard: Some(guard),
        writer,
        _native_writer: None,
    })
}

pub(crate) enum StatResolution {
    Overlay(Stat),
    Native(PathBuf),
}

/// Preserve a completed native walk for the caller's metadata query. Returning
/// only "not overlay" made stat repeat every ancestor's inode/EA lookup.
pub(crate) fn stat_resolution(path: &str, follow: bool) -> Result<Option<StatResolution>, i32> {
    let Some(resolved) = resolve(path, follow, false)? else {
        return Ok(None);
    };
    let Some(location) = resolved.location else {
        return Ok(Some(StatResolution::Native(resolved.path)));
    };
    let _guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    Ok(Some(StatResolution::Overlay(
        location.metadata(&location.lookup()?)?,
    )))
}

pub(crate) fn read_directory(path: &str) -> Result<Option<Vec<fs::DirectoryEntry>>, i32> {
    let Some(resolved) = resolve(path, true, false)? else {
        return Ok(None);
    };
    let Some(location) = resolved.location else {
        return Ok(None);
    };
    let _guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let node = location.lookup()?;
    let mut entries = vec![
        fs::DirectoryEntry {
            name: ".".into(),
            is_directory: true,
            is_symlink: false,
        },
        fs::DirectoryEntry {
            name: "..".into(),
            is_directory: true,
            is_symlink: false,
        },
    ];
    for entry in node.directory_snapshot()? {
        entries.push(fs::DirectoryEntry {
            name: entry.name,
            is_directory: entry.mode & S_IFMT == S_IFDIR,
            is_symlink: entry.mode & S_IFMT == S_IFLNK,
        });
    }
    Ok(Some(entries))
}

// Descriptor state is keyed by the shared open-description identity, so dup
// has no new pathname state and cannot detach a descriptor from its overlay.
#[derive(Clone)]
pub(crate) struct Description {
    location: Location,
    inode: u64,
    directory: Option<Arc<Directory>>,
    writer: Option<Writer>,
    anchor: Option<crate::mount::api::DirectoryAnchor>,
}
pub(crate) fn directory_anchor(fd: i32) -> Result<Option<crate::mount::api::DirectoryAnchor>, i32> {
    let entry = crate::get(fd)?;
    if entry.kind != crate::FdKind::Directory {
        return Ok(None);
    }
    Ok(reference(entry)?.and_then(|d| d.anchor))
}
impl std::fmt::Debug for Description {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayDescription")
            .field("inode", &self.inode)
            .finish_non_exhaustive()
    }
}
impl Description {
    /// Reading a lower inode must never copy it up or modify its timestamps.
    /// Upper atime is best effort, as in touch_atime(), after the data lock has
    /// been released. Resolve an old lower fd by inode before choosing an upper.
    pub(crate) fn accessed(&self) {
        let touch = || -> Result<(), i32> {
            let flags = self.mount_flags();
            if flags & (1 | 1024) != 0 || self.location.instance.work.is_none() {
                return Ok(());
            }
            let original = self.location.node.as_ref().ok_or(EIO)?;
            if original.is_directory() && flags & 2048 != 0 {
                return Ok(());
            }
            let _writer = self.location.writer()?;
            let _root = InodeLock::acquire(self.location.instance.root.backing_object().raw())?;
            let (object, description) =
                reopen_object(self, original.backing_object().raw(), false, true)?;
            let node = description.location.node.as_ref().ok_or(EIO)?;
            if node.backing_layer() != 0 && !node.entries[0].indexed {
                return Ok(());
            }
            let _inode = InodeLock::acquire(object.raw())?;
            let stat = fs::stat_handle(object.raw(), false)?;
            let mut now = windows_sys::Win32::Foundation::FILETIME::default();
            unsafe {
                windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime(
                    &mut now,
                );
            }
            let ticks = ((now.dwHighDateTime as u64) << 32) | now.dwLowDateTime as u64;
            let current = ticks as i128 * 100 - 11_644_473_600i128 * 1_000_000_000;
            let stamp = |sec: i64, ns: i64| sec as i128 * 1_000_000_000 + ns as i128;
            let access = stamp(stat.st_atime, stat.st_atime_nsec);
            if flags & (1 << 24) == 0
                && access > stamp(stat.st_mtime, stat.st_mtime_nsec)
                && access > stamp(stat.st_ctime, stat.st_ctime_nsec)
                && current - access < 86_400_000_000_000
            {
                return Ok(());
            }
            let writable = Object::reopen(
                object.raw(),
                windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES,
            )?;
            if unsafe {
                windows_sys::Win32::Storage::FileSystem::SetFileTime(
                    writable.raw(),
                    std::ptr::null(),
                    &now,
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(crate::errno_from_win32(unsafe {
                    windows_sys::Win32::Foundation::GetLastError()
                }));
            }
            Ok(())
        };
        let _ = touch();
    }
    pub(crate) fn volatile_state(&self) -> Result<Option<(u64, u64)>, i32> {
        if self.location.instance.flags & super::features::VOLATILE == 0 {
            return Ok(None);
        }
        Ok(Some((
            fs::stat_handle(self.location.instance.root.backing_object().raw(), false)?.st_dev,
            self.location.instance.error_epoch,
        )))
    }
    pub(crate) fn mount_flags(&self) -> u64 {
        self.location.policy.as_ref().map_or(0, |p| p.flags())
    }
    pub(crate) fn mapping_writer(&self) -> Result<Vec<std::os::windows::io::OwnedHandle>, i32> {
        use std::os::windows::io::FromRawHandle;
        self.writer
            .iter()
            .flat_map(|w| w.mount.iter().chain(Some(&w.superblock)))
            .map(|w| {
                Object::duplicate(w.raw()).map(|o| unsafe {
                    std::os::windows::io::OwnedHandle::from_raw_handle(o.into_raw())
                })
            })
            .collect()
    }
}

pub fn check_execute(path: &str) -> Result<(), i32> {
    let absolute = fs::absolute_linux(path);
    if let Some(fd) = absolute
        .strip_prefix("/proc/self/fd/")
        .or_else(|| absolute.strip_prefix("/dev/fd/"))
        .and_then(|s| s.parse::<i32>().ok())
    {
        if description(fd)?.is_some_and(|d| d.mount_flags() & 8 != 0)
            || crate::mount::native::reference(crate::get(fd)?)?
                .is_some_and(|d| d.policy.flags() & 8 != 0)
        {
            return Err(crate::EACCES);
        }
        return Ok(());
    }
    if crate::mount::native::path_policy(&absolute, true)?.is_some_and(|p| p.flags() & 8 != 0) {
        return Err(crate::EACCES);
    }
    if let Some(location) = resolve(&absolute, true, false)?.and_then(|r| r.location) {
        if location.policy.as_ref().is_some_and(|p| p.flags() & 8 != 0) {
            return Err(crate::EACCES);
        }
    }
    Ok(())
}
fn pin_node(node: &Node) -> Result<Node, i32> {
    let mut entries = Vec::with_capacity(node.entries.len());
    for entry in &node.entries {
        let object = Object::reopen(entry.object.raw(), ACCESS)?;
        crate::platform::try_set_inheritable(object.raw() as usize, true)?;
        let mut entry = entry.clone();
        entry.object = Arc::new(object);
        entries.push(entry);
    }
    Ok(Node {
        entries,
        namespace: node.namespace,
        context: node.context.clone(),
    })
}
pub(crate) fn auxiliary_handles() -> Result<Vec<(u64, Arc<Object>)>, i32> {
    let descriptions = descriptions().lock().map_err(|_| EIO)?;
    let mut handles: Vec<_> = descriptions
        .iter()
        .flat_map(|(id, description)| {
            description.location.node.iter().flat_map(move |node| {
                node.entries
                    .iter()
                    .map(move |entry| (*id, entry.object.clone()))
            })
        })
        .collect();
    handles.extend(descriptions.iter().flat_map(|(id, d)| {
        d.writer.iter().flat_map(move |w| {
            w.mount
                .iter()
                .chain(Some(&w.superblock))
                .map(move |o| (*id, o.clone()))
        })
    }));
    handles.extend(
        descriptions
            .iter()
            .filter_map(|(id, d)| d.anchor.as_ref().map(|a| (*id, a.pin.clone()))),
    );
    handles.extend(crate::mount::native::auxiliary_handles()?);
    Ok(handles)
}
fn descriptions() -> &'static Mutex<HashMap<u64, Description>> {
    static DESCRIPTIONS: OnceLock<Mutex<HashMap<u64, Description>>> = OnceLock::new();
    DESCRIPTIONS.get_or_init(Default::default)
}
pub(crate) fn register(entry: crate::FdEntry, mut description: Description) -> Result<(), i32> {
    if !entry.flags.contains(crate::FdFlags::WRITE_ACCESS) {
        description.writer = None;
    }
    for writer in description
        .writer
        .iter()
        .flat_map(|w| w.mount.iter().chain(Some(&w.superblock)))
    {
        crate::platform::try_set_inheritable(writer.raw() as usize, true)?;
    }
    descriptions()
        .lock()
        .map_err(|_| EIO)?
        .insert(entry.description_id, description);
    Ok(())
}
pub(crate) fn reference(entry: crate::FdEntry) -> Result<Option<Description>, i32> {
    // Overlay descriptions can only belong to native file objects.  A restored
    // descriptor table may reuse an open-description id that was present in
    // this process before fork/exec state was imported.  Do not let that stale
    // map entry turn pipes, sockets, or other kernel objects into overlay files.
    if !matches!(entry.kind, crate::FdKind::File | crate::FdKind::Directory) {
        return Ok(None);
    }
    Ok(descriptions()
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned())
}

/// Open one literal component from a retained merged directory. Neither the
/// backing layer's native pathname nor a symlink target is traversed here.
/// The caller owns symlink expansion and keeps the ancestor descriptor stack.
pub(crate) fn confined_child(fd: i32, name: &str, no_xdev: bool) -> Result<Option<i32>, i32> {
    let Some(parent) = reference(crate::get(fd)?)? else {
        return Ok(None);
    };
    let _guard = InodeLock::acquire(parent.location.instance.root.backing_object().raw())?;
    // Rename and copy-up update the description table under the same mutex.
    let parent = reference(crate::get(fd)?)?.ok_or(crate::EBADF)?;
    let original = parent.location.node.as_ref().ok_or(EIO)?.refreshed()?;
    if !original.is_directory() {
        return Err(ENOTDIR);
    }
    let guest = format!("{}/{}", parent.location.guest.trim_end_matches('/'), name);
    let crossing = if parent.anchor.is_some() {
        crate::mount::api::tree_id(fd, "")? != crate::mount::api::tree_id(fd, name)?
    } else {
        crate::mount::visible_mount_id(&parent.location.guest)?
            != crate::mount::visible_mount_id(&guest)?
    };
    if crossing {
        if no_xdev {
            return Err(EXDEV);
        }
        return fs::openat(fd, name, fs::O_PATH | fs::O_NOFOLLOW | fs::O_CLOEXEC, 0).map(Some);
    }
    let node = original.child(name)?;
    let mut location = parent.location.clone();
    location.guest = guest;
    location.components.push(name.into());
    let inode = location.metadata(&node)?.st_ino;
    let is_directory = node.is_directory();
    location.node = Some(pin_node(&node)?);
    let anchor = parent.anchor.map(|a| crate::mount::api::DirectoryAnchor {
        prefix: format!("{}/{}", a.prefix.trim_end_matches('/'), name),
        pin: a.pin,
    });
    let description = Description {
        location,
        inode,
        writer: None,
        directory: if is_directory {
            Some(Arc::new(Directory::create()?))
        } else {
            None
        },
        anchor: if is_directory { anchor } else { None },
    };
    let object = Object::duplicate(node.backing_object().raw())?;
    let fd = crate::install_with(
        object.raw() as usize,
        if is_directory {
            crate::FdKind::Directory
        } else {
            crate::FdKind::File
        },
        crate::FdFlags::PATH_ONLY
            .union(crate::FdFlags::OVERLAPPED)
            .union(crate::FdFlags::CLOSE_ON_EXEC),
        |_, entry| register(entry, description),
    )?;
    object.into_raw();
    Ok(Some(fd))
}

/// Publish a new inode from the work directory using a validated, pinned parent.
/// A renamed parent is refreshed under the mutation lock; a replaced parent is
/// never silently resolved through its old pathname.
pub(crate) fn confined_create(fd: i32, name: Option<&str>, mode: u32) -> Result<Option<i32>, i32> {
    let Some(parent) = reference(crate::get(fd)?)? else {
        return Ok(None);
    };
    let guard = InodeLock::acquire(parent.location.instance.root.backing_object().raw())?;
    let parent = reference(crate::get(fd)?)?.ok_or(crate::EBADF)?;
    let mut location = parent.location.clone();
    let current = location
        .lookup()
        .map_err(|e| if e == ENOENT { crate::EAGAIN } else { e })?;
    if location.metadata(&current)?.st_ino != parent.inode {
        return Err(crate::EAGAIN);
    }
    if !current.is_directory() {
        return Err(ENOTDIR);
    }
    let writer = location.writer()?;
    let publication = if let Some(name) = name {
        match current.child(name) {
            Ok(_) => return Err(EEXIST),
            Err(ENOENT) => {}
            Err(e) => return Err(e),
        }
        location.guest = format!("{}/{}", location.guest.trim_end_matches('/'), name);
        location.components.push(name.into());
        let (parent, name) = location.parent()?;
        Some((parent, name.into()))
    } else {
        None
    };
    let work = location.instance.work.as_ref().ok_or(EROFS)?;
    let native = temporary(work)?;
    let object = work.create_regular_child(
        native.file_name().ok_or(EIO)?,
        windows_sys::Win32::Foundation::GENERIC_READ
            | windows_sys::Win32::Foundation::GENERIC_WRITE
            | ACCESS
            | DELETE,
    )?;
    let mut staged = WritePath {
        path: native,
        location: Some(location),
        publication,
        anonymous: name.is_none(),
        _guard: Some(guard),
        writer,
        _native_writer: None,
    };
    let initialized = staged.initialize_created_handle(object.raw(), fs::S_IFREG | mode & 0o7777);
    if let Err(error) = initialized {
        let _ = fs::unlink_inode(object.raw());
        return Err(error);
    }
    staged.finish()?;
    let description = staged.description(object.raw(), false)?.ok_or(EIO)?;
    if name.is_none() {
        fs::unlink_inode(object.raw())?;
    }
    let fd = crate::install_with(
        object.raw() as usize,
        crate::FdKind::File,
        crate::FdFlags::PATH_ONLY
            .union(crate::FdFlags::OVERLAPPED)
            .union(crate::FdFlags::CLOSE_ON_EXEC),
        |_, entry| register(entry, description),
    )?;
    object.into_raw();
    Ok(Some(fd))
}

pub(crate) fn reopen_object(
    description: &Description,
    pinned: windows_sys::Win32::Foundation::HANDLE,
    write: bool,
    metadata_only: bool,
) -> Result<(Object, Description), i32> {
    let mut refreshed = description.clone();
    refreshed.writer = if write {
        refreshed.location.writer()?
    } else {
        None
    };
    let _refresh_guard =
        InodeLock::acquire(refreshed.location.instance.root.backing_object().raw())?;
    if let Some(node) = &refreshed.location.node {
        refreshed.location.node = Some(pin_node(&node.refreshed()?)?);
    }
    let description = &refreshed;
    let location = &description.location;
    if write && location.read_only()? {
        return Err(EROFS);
    }
    let _guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let mut description = description.clone();
    let original_upper = location
        .node
        .as_ref()
        .is_some_and(|node| node.backing_layer() == 0 || node.entries[0].indexed)
        && location.instance.work.is_some();
    let node = if original_upper {
        let original = location.node.as_ref().ok_or(EIO)?;
        if write && !metadata_only && original.is_metacopy() {
            copy_up::complete_metacopy(
                original,
                location.instance.work.as_ref().ok_or(EROFS)?,
                false,
            )?;
            let object = Object::reopen(original.backing_object().raw(), ACCESS)?;
            Some(Node {
                entries: vec![Backing::from_object(object, original.namespace, 0)?],
                namespace: original.namespace,
                context: original.context.clone(),
            })
        } else {
            None
        }
    } else {
        match location.lookup() {
            Ok(node) if location.metadata(&node)?.st_ino == description.inode => Some(if write {
                location.upper_mode(false, metadata_only)?
            } else {
                node
            }),
            _ if !write => None,
            _ => {
                let source = location.node.as_ref().ok_or(EIO)?;
                let object = copy_up::Staged::anonymous(
                    source,
                    location.instance.work.as_ref().ok_or(EROFS)?,
                )?;
                let entry = Backing::from_object(object, source.namespace, 0)?;
                let node = Node {
                    entries: vec![entry],
                    namespace: source.namespace,
                    context: source.context.clone(),
                };
                location.remember_upper(&node)?;
                Some(node)
            }
        }
    };
    let backing = |node: &Node| {
        if metadata_only {
            node.backing_object().raw()
        } else {
            node.data_object().raw()
        }
    };
    if !metadata_only {
        if let Some(node) = node.as_ref().or(description.location.node.as_ref()) {
            node.verify_metacopy()?;
        }
    }
    let source = node
        .as_ref()
        .map(backing)
        .or_else(|| description.location.node.as_ref().map(backing))
        .unwrap_or(pinned);
    let object = Object::reopen(source, ACCESS)?;
    if let Some(node) = node {
        description.location.node = Some(pin_node(&node)?);
    }
    description.directory = if description.directory.is_some() {
        Some(Arc::new(Directory::create()?))
    } else {
        None
    };
    Ok((object, description))
}

pub struct MetadataHandle {
    handle: std::os::windows::io::OwnedHandle,
    _writer: Option<Writer>,
    _native_writer: Option<Arc<Object>>,
}
impl std::os::windows::io::AsRawHandle for MetadataHandle {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        std::os::windows::io::AsRawHandle::as_raw_handle(&self.handle)
    }
}
pub fn metadata_handle(
    fd: i32,
    write: bool,
    allow_path: bool,
) -> Result<Option<MetadataHandle>, i32> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    let mut description = None;
    let mut native = None;
    let pinned = crate::native_pin::pin_native_fd(fd, |entry| {
        description = reference(entry)?;
        native = crate::mount::native::reference(entry)?;
        if (description.is_some() || native.is_some())
            && !allow_path
            && entry.flags.contains(crate::FdFlags::PATH_ONLY)
        {
            return Err(crate::EBADF);
        }
        Ok(())
    });
    let (_, pinned) = match pinned {
        Ok(value) => value,
        Err(crate::EOPNOTSUPP) => return Ok(None),
        Err(e) => return Err(e),
    };
    let Some(description) = description else {
        let Some(native) = native else {
            return Ok(None);
        };
        let writer = if write {
            Some(native.policy.writer()?)
        } else {
            None
        };
        let access = ACCESS
            | if write {
                windows_sys::Win32::Storage::FileSystem::FILE_WRITE_EA
                    | windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES
            } else {
                0
            };
        let object = Object::reopen(pinned.as_raw_handle(), access)?;
        return Ok(Some(MetadataHandle {
            handle: unsafe {
                std::os::windows::io::OwnedHandle::from_raw_handle(object.into_raw())
            },
            _writer: None,
            _native_writer: writer,
        }));
    };
    let (object, refreshed) = reopen_object(&description, pinned.as_raw_handle(), write, true)?;
    let access = ACCESS
        | if write {
            windows_sys::Win32::Storage::FileSystem::FILE_WRITE_EA
                | windows_sys::Win32::Storage::FileSystem::FILE_WRITE_ATTRIBUTES
        } else {
            0
        };
    let object = Object::reopen(object.raw(), access)?;
    Ok(Some(MetadataHandle {
        handle: unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(object.into_raw()) },
        _writer: refreshed.writer,
        _native_writer: None,
    }))
}

pub fn chmod_descriptor(fd: i32, mode: u32) -> Result<bool, i32> {
    use std::os::windows::io::AsRawHandle;
    let Some(handle) = metadata_handle(fd, true, false)? else {
        return Ok(false);
    };
    fs::set_mode_handle(handle.as_raw_handle(), mode)?;
    Ok(true)
}
pub(crate) fn closed(description: u64) {
    if let Ok(mut descriptions) = descriptions().lock() {
        descriptions.remove(&description);
    }
}
pub fn descriptor_path(fd: i32) -> Result<Option<String>, i32> {
    let entry = crate::get(fd)?;
    if entry.kind == crate::FdKind::Directory
        && reference(entry)?.is_some_and(|d| d.anchor.is_some())
    {
        return Ok(Some(format!("/proc/self/fd/{fd}")));
    }
    Ok(descriptions()
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .map(|d| guest_visible(&d.location.guest)))
}
pub(crate) fn descriptor_namespace(entry: crate::FdEntry) -> Result<Option<String>, i32> {
    Ok(reference(entry)?.map(|d| d.location.guest))
}
pub(crate) fn descriptor_display(fd: i32) -> Result<Option<String>, i32> {
    let entry = crate::get(fd)?;
    let description = reference(entry)?;
    let Some(description) = description else {
        return Ok(None);
    };
    let location = &description.location;
    let visible = location
        .lookup()
        .and_then(|node| location.metadata(&node))
        .is_ok_and(|stat| stat.st_ino == description.inode);
    Ok(Some(format!(
        "{}{}",
        guest_visible(&location.guest),
        if visible { "" } else { " (deleted)" }
    )))
}
fn guest_visible(global: &str) -> String {
    if let Some(root) = crate::path::overlay_root() {
        if global == root.namespace_path {
            return "/".into();
        }
        if let Some(rest) = global
            .strip_prefix(&root.namespace_path)
            .filter(|s| s.starts_with('/'))
        {
            return rest.into();
        }
    }
    global.into()
}

pub(crate) fn chroot(path: &str) -> Result<bool, i32> {
    let Some(Resolved {
        path: native,
        location: Some(location),
        ..
    }) = resolve(path, true, false)?
    else {
        return Ok(false);
    };
    if !location.lookup()?.is_directory() {
        return Err(ENOTDIR);
    }
    let native_base = crate::path::overlay_root()
        .map(|r| r.native_base)
        .unwrap_or(crate::system_root().map_err(path_error)?);
    let root = crate::path::OverlayRoot {
        namespace_path: location.guest,
        source: location.instance.source.clone(),
        relative: location.components.join("/"),
        native_base,
    };
    crate::path::set_overlay_root(native, root);
    Ok(true)
}
pub(crate) fn ownership_mapping(
    path: &str,
    follow: bool,
) -> Result<Option<crate::user_namespace::Mapping>, i32> {
    if let Some(location) = resolve(path, follow, false)?.and_then(|r| r.location) {
        if let Some(policy) = location.policy {
            return policy.mapping();
        }
    }
    Ok(None)
}
pub(crate) fn descriptor_mapping(fd: i32) -> Result<Option<crate::user_namespace::Mapping>, i32> {
    if let Some(description) = description(fd)? {
        if let Some(policy) = description.location.policy {
            return policy.mapping();
        }
    }
    Ok(None)
}

pub fn descriptor_write_path(fd: i32) -> Result<Option<WritePath>, i32> {
    let entry = crate::get(fd)?;
    let description = descriptions()
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned();
    let Some(description) = description else {
        return Ok(None);
    };
    if entry.flags.contains(crate::FdFlags::PATH_ONLY) {
        return Err(crate::EBADF);
    }
    let location = description.location;
    let writer = location.writer()?;
    let guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let node = location.lookup()?;
    if location.metadata(&node)?.st_ino != description.inode {
        return Err(crate::ESTALE);
    }
    let upper = location.upper(false)?;
    Ok(Some(WritePath {
        path: upper.backing_object().path()?,
        location: Some(location),
        publication: None,
        anonymous: false,
        _guard: Some(guard),
        writer,
        _native_writer: None,
    }))
}

pub(crate) fn fstat(entry: crate::FdEntry) -> Result<Option<Stat>, i32> {
    let Some(description) = reference(entry)? else {
        return Ok(None);
    };
    let location = &description.location;
    let node = match location.lookup() {
        Ok(node) if location.metadata(&node)?.st_ino == description.inode => Some(node),
        _ => location.node.clone(),
    };
    let mut stat = match node.as_ref() {
        Some(node) => location.metadata(node)?,
        None => fs::stat_handle(entry.raw as _, false)?,
    };
    stat.st_ino = description.inode;
    Ok(Some(stat))
}

pub(crate) fn mount_metadata(source: &str) -> Result<(u64, bool), i32> {
    let instance = instance(source)?;
    Ok((instance.device, instance.policy.flags() & 1 != 0))
}
pub(crate) fn reconfigure(
    source: &str,
    readonly: bool,
) -> Result<crate::mount::policy::Changes, i32> {
    let instance = instance(source)?;
    if !readonly && instance.work.is_none() {
        return Err(EROFS);
    }
    crate::mount::policy::Changes::apply(vec![(instance.policy.clone(), u64::from(readonly))])
}
pub(crate) fn feature_options(source: &str) -> Result<String, i32> {
    Ok(super::features::display(instance(source)?.flags))
}
pub fn is_overlay_path(path: &str, follow: bool, missing: bool) -> Result<bool, i32> {
    Ok(resolve(path, follow, missing)?.is_some_and(|resolved| resolved.location.is_some()))
}

pub fn filesystem_path(path: &str) -> Result<Option<(PathBuf, u64, bool)>, i32> {
    let Some(Resolved {
        location: Some(location),
        ..
    }) = resolve(path, true, false)?
    else {
        return Ok(None);
    };
    let readonly = location.read_only()?;
    let instance = location.instance;
    Ok(Some((
        instance.root.backing_object().path()?,
        instance.fsid,
        readonly,
    )))
}
pub fn descriptor_filesystem(fd: i32) -> Result<Option<(PathBuf, u64, bool)>, i32> {
    let entry = crate::get(fd)?;
    if entry.kind == crate::FdKind::MountTree {
        return filesystem_path(&format!("{}/.", crate::mount::api::tree_path(fd)?));
    }
    let location = descriptions()
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .map(|d| d.location.clone());
    location
        .map(|l| {
            Ok((
                l.instance.root.backing_object().path()?,
                l.instance.fsid,
                l.read_only()?,
            ))
        })
        .transpose()
}

/// Flush the writable metadata inode even when the data FD still pins lower.
pub fn sync_descriptor(fd: i32) -> Result<Option<()>, i32> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    let mut description = None;
    let pinned = crate::native_pin::pin_native_fd(fd, |entry| {
        description = reference(entry)?;
        if description.is_some() && entry.flags.contains(crate::FdFlags::PATH_ONLY) {
            return Err(crate::EBADF);
        }
        Ok(())
    })?;
    let Some(description) = description else {
        return Ok(None);
    };
    if let Some((volume, since)) = description.volatile_state()? {
        super::volatile::check(volume, since)?;
        return Ok(Some(()));
    }
    let _guard = InodeLock::acquire(description.location.instance.root.backing_object().raw())?;
    let (object, description) = reopen_object(&description, pinned.1.as_raw_handle(), false, true)?;
    if let Some(node) = &description.location.node
        && description.location.instance.work.is_some()
        && (node.backing_layer() == 0 || node.entries[0].indexed)
    {
        Object::reopen(object.raw(), GENERIC_READ | GENERIC_WRITE)?.flush()?;
    }
    Ok(Some(()))
}

pub(crate) fn serialize(keep: impl Fn(i32) -> bool) -> Result<Vec<u8>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let descriptions = descriptions().lock().map_err(|_| EIO)?;
    let mut selected = HashMap::new();
    for (fd, entry) in table.slots.enumerated() {
        if keep(fd as i32)
            && let Some(entry) = entry
            && let Some(description) = descriptions.get(&entry.description_id)
        {
            selected.insert(entry.description_id, description);
        }
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(selected.len() as u32).to_le_bytes());
    for (id, description) in selected {
        bytes.extend_from_slice(&id.to_le_bytes());
        bytes.extend_from_slice(&description.inode.to_le_bytes());
        let policy = description.location.policy.as_ref();
        for word in [
            policy.map_or(0, |p| p.namespace),
            policy.map_or(0, |p| p.id),
            policy.map_or(0, |p| p.flags()),
            description
                .writer
                .as_ref()
                .and_then(|w| w.mount.as_ref())
                .map_or(0, |w| w.raw() as u64),
            description
                .writer
                .as_ref()
                .map_or(0, |w| w.superblock.raw() as u64),
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(
            &description
                .anchor
                .as_ref()
                .map_or(0, |a| a.pin.raw() as u64)
                .to_le_bytes(),
        );
        let prefix = description
            .anchor
            .as_ref()
            .map_or("", |a| a.prefix.as_str());
        bytes.extend_from_slice(&(prefix.len() as u32).to_le_bytes());
        bytes.extend_from_slice(prefix.as_bytes());
        for text in [
            &description.location.instance.source,
            &description.location.guest,
            &description.location.components.join("/"),
        ] {
            bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
            bytes.extend_from_slice(text.as_bytes());
        }
        let name = description
            .directory
            .as_ref()
            .map_or("", |d| d.name.as_str());
        bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        let node = description.location.node.as_ref().ok_or(EIO)?;
        bytes.extend_from_slice(&(node.entries.len() as u32).to_le_bytes());
        for entry in &node.entries {
            bytes.extend_from_slice(
                &((entry.layer as u32) | (u32::from(entry.indexed) << 31)).to_le_bytes(),
            );
            bytes.extend_from_slice(&(entry.object.raw() as u64).to_le_bytes());
        }
    }
    Ok(bytes)
}

pub(crate) fn restore(bytes: &[u8]) -> bool {
    fn restore_inner(bytes: &[u8]) -> Result<(), i32> {
        let mut input = Reader(bytes);
        let count = input.u32()? as usize;
        if count > bytes.len() / 28 {
            return Err(EIO);
        }
        let entries: HashMap<_, _> = crate::table()
            .read()
            .map_err(|_| EIO)?
            .slots
            .iter()
            .flatten()
            .map(|entry| (entry.description_id, *entry))
            .collect();
        let mut restored = HashMap::new();
        let mut inherited = std::collections::HashSet::new();
        for _ in 0..count {
            let id = input.u64()?;
            let inode = input.u64()?;
            let namespace = input.u64()?;
            let mount_id = input.u64()?;
            let flags = input.u64()?;
            let writer_raw = input.u64()? as usize;
            let super_raw = input.u64()? as usize;
            let anchor_raw = input.u64()? as usize;
            let prefix_length = input.u32()? as usize;
            let prefix = std::str::from_utf8(input.take(prefix_length)?)
                .map_err(|_| EIO)?
                .to_owned();
            let anchor = if anchor_raw == 0 {
                None
            } else {
                inherited.insert(anchor_raw);
                Some(crate::mount::api::DirectoryAnchor::restore(
                    anchor_raw, prefix,
                )?)
            };
            let policy = if mount_id == 0 {
                None
            } else {
                Some(crate::mount::policy::get(namespace, mount_id, flags)?)
            };
            let writer = if super_raw == 0 {
                None
            } else {
                let mut duplicate = |raw: usize| -> Result<Arc<Object>, i32> {
                    let object = Object::duplicate(raw as _)?;
                    crate::platform::try_set_inheritable(object.raw() as usize, true)?;
                    inherited.insert(raw);
                    Ok(Arc::new(object))
                };
                Some(Writer {
                    superblock: duplicate(super_raw)?,
                    mount: if writer_raw == 0 {
                        None
                    } else {
                        Some(duplicate(writer_raw)?)
                    },
                })
            };
            let mut strings = Vec::new();
            for _ in 0..4 {
                let len = input.u32()? as usize;
                strings.push(
                    std::str::from_utf8(input.take(len)?)
                        .map_err(|_| EIO)?
                        .to_owned(),
                );
            }
            let instance = instance(&strings[0])?;
            let guest = strings[1].clone();
            let components: Vec<_> = strings[2]
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            for component in &components {
                super::validate_component(component)?;
            }
            entries.get(&id).ok_or(EIO)?;
            let mut location = Location {
                instance: instance.clone(),
                policy,
                guest,
                components,
                node: None,
            };
            let node_count = input.u32()? as usize;
            if node_count == 0 || node_count > 501 {
                return Err(EIO);
            }
            let mut backing = Vec::with_capacity(node_count);
            for _ in 0..node_count {
                let layer_flags = input.u32()?;
                let layer = (layer_flags & 0x7fff_ffff) as usize;
                let raw = input.u64()? as usize;
                if raw == 0 || raw == usize::MAX {
                    return Err(EIO);
                }
                if layer >= instance.root.context.as_ref().ok_or(EIO)?.roots.len() {
                    return Err(EIO);
                }
                let object = Object::reopen(raw as _, ACCESS)?;
                crate::platform::try_set_inheritable(object.raw() as usize, true)?;
                let mut entry = Backing::from_object(object, instance.root.namespace, layer)?;
                entry.indexed = layer_flags & 0x8000_0000 != 0;
                backing.push(entry);
                inherited.insert(raw);
            }
            let node = Node {
                entries: backing,
                namespace: instance.root.namespace,
                context: instance.root.context.clone(),
            };
            location.node = Some(node);
            let directory = if strings[3].is_empty() {
                None
            } else {
                Some(Arc::new(Directory::open(&strings[3], false)?))
            };
            if restored
                .insert(
                    id,
                    Description {
                        location,
                        inode,
                        directory,
                        writer,
                        anchor,
                    },
                )
                .is_some()
            {
                return Err(EIO);
            }
        }
        if !input.0.is_empty() {
            return Err(EIO);
        }
        *descriptions().lock().map_err(|_| EIO)? = restored;
        for raw in inherited {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(raw as _);
            }
        }
        Ok(())
    }
    restore_inner(bytes).is_ok()
}

fn description(fd: i32) -> Result<Option<Description>, i32> {
    let entry = crate::get(fd)?;
    if entry.flags.contains(crate::FdFlags::PATH_ONLY) {
        return Err(crate::EBADF);
    }
    Ok(descriptions()
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned())
}
fn records(description: &Description) -> Result<Vec<DirectoryRecord>, i32> {
    let location = &description.location;
    let _guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let node = match location.lookup() {
        Ok(node) if location.metadata(&node)?.st_ino == description.inode => node,
        _ => location.node.clone().ok_or(ENOENT)?,
    };
    if !node.is_directory() {
        return Err(ENOTDIR);
    }
    let mut result = vec![DirectoryRecord {
        name: ".".into(),
        inode: description.inode,
        kind: 4,
    }];
    let parent_ino = if location.components.is_empty() {
        description.inode
    } else {
        let mut parent = location.clone();
        parent.components.pop();
        parent
            .lookup()
            .and_then(|n| parent.metadata(&n))
            .map_or(description.inode, |s| s.st_ino)
    };
    result.push(DirectoryRecord {
        name: "..".into(),
        inode: parent_ino,
        kind: 4,
    });
    for entry in node.directory_snapshot()? {
        let child = node.child(&entry.name)?;
        let stat = location.metadata(&child)?;
        result.push(DirectoryRecord {
            name: entry.name,
            inode: stat.st_ino,
            kind: ((stat.st_mode & S_IFMT) >> 12) as u8,
        });
    }
    Ok(result)
}
pub fn directory_records(fd: i32) -> Result<Option<Vec<DirectoryRecord>>, i32> {
    description(fd)?
        .map(|d| {
            let records = records(&d)?;
            if !crate::get(fd)?.flags.contains(crate::FdFlags::NOATIME) {
                d.accessed();
            }
            Ok(records)
        })
        .transpose()
}
pub fn read_directory_bytes(fd: i32, bytes: &mut [u8], wide: bool) -> Result<Option<usize>, i32> {
    let Some(description) = description(fd)? else {
        return Ok(None);
    };
    let directory = description.directory.as_ref().ok_or(ENOTDIR)?;
    let count = directory.read(bytes, wide, || records(&description))?;
    if !crate::get(fd)?.flags.contains(crate::FdFlags::NOATIME) {
        description.accessed();
    }
    Ok(Some(count))
}
pub(crate) fn seek_directory(fd: i32, offset: i64, whence: i32) -> Result<Option<u64>, i32> {
    let Some(description) = description(fd)? else {
        return Ok(None);
    };
    description
        .directory
        .as_ref()
        .map(|d| d.seek(offset, whence))
        .transpose()
}

pub(crate) fn remove(path: &str, directory: bool) -> Result<Option<()>, i32> {
    let Some(Resolved {
        location: Some(location),
        ..
    }) = resolve(path, false, false)?
    else {
        return Ok(None);
    };
    if location.components.is_empty() {
        return Err(EBUSY);
    }
    let _writer = location.writer()?;
    let _guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let node = location.lookup()?;
    if directory && !node.is_directory() {
        return Err(ENOTDIR);
    }
    if !directory && node.is_directory() {
        return Err(crate::EISDIR);
    }
    if directory && !node.directory_snapshot()?.is_empty() {
        return Err(crate::ENOTEMPTY);
    }
    // Preserve the union link count before hiding the first lower alias.
    let node = if !directory
        && node.backing_layer() != 0
        && !node.entries[0].indexed
        && node.backing_metadata().st_nlink > 1
        && location.instance.flags & super::features::INDEX != 0
    {
        location.upper_mode(false, true)?
    } else {
        node
    };
    let lower = location.lower_exists()?;
    let (parent, name) = location.parent()?;
    let work = location.instance.work.as_ref().ok_or(EROFS)?;
    // Prepare the replacement before removing anything, including on ENOSPC.
    let whiteout = if lower {
        Some(copy_up::Staged::whiteout(work)?)
    } else {
        None
    };
    if node.backing_layer() == 0 {
        if !directory {
            if let Some(whiteout) = whiteout {
                whiteout.publish_replacing(parent.backing_object(), name)?;
            } else {
                let object = Object::reopen(node.backing_object().raw(), DELETE)?;
                fs::unlink_inode(object.raw())?;
            }
        } else {
            let mut saved = SavedEntry::new(
                node.backing_object(),
                parent.backing_object(),
                name,
                work,
                parent.namespace,
            )?;
            if let Some(whiteout) = whiteout {
                whiteout.publish(parent.backing_object(), name)?;
            }
            saved.discard()?;
        }
    } else if let Some(whiteout) = whiteout {
        whiteout.publish(parent.backing_object(), name)?;
    }
    if let Some(index) = location
        .instance
        .root
        .context
        .as_ref()
        .and_then(|context| context.index.as_ref())
    {
        index.adjust_links(&node, -1, work)?;
    }
    Ok(Some(()))
}

pub fn rename_with_flags(from: &str, to: &str, flags: u32) -> Result<Option<()>, i32> {
    if flags & !7 != 0 || flags & 2 != 0 && flags & 5 != 0 {
        return Err(EINVAL);
    }
    let from = resolve(from, false, false)?.and_then(|r| r.location);
    let to = resolve(to, false, true)?.and_then(|r| r.location);
    let (from, to) = match (from, to) {
        (None, None) => return Ok(None),
        (Some(from), Some(to)) if from.instance.source == to.instance.source => (from, to),
        _ => return Err(EXDEV),
    };
    if from.components.is_empty() || to.components.is_empty() {
        return Err(EBUSY);
    }
    let _source_writer = from.writer()?;
    let _target_writer = to.writer()?;
    let _guard = InodeLock::acquire(from.instance.root.backing_object().raw())?;
    let source = from.lookup()?;
    let target = match to.lookup() {
        Ok(node) => Some(node),
        Err(ENOENT) => None,
        Err(e) => return Err(e),
    };
    if flags & 1 != 0 && target.is_some() {
        return Err(EEXIST);
    }
    if flags & 2 != 0 && target.is_none() {
        return Err(ENOENT);
    }
    if from.components == to.components {
        return Ok(Some(()));
    }
    if let Some(target) = &target {
        let left = source.backing_metadata();
        let right = target.backing_metadata();
        if left.st_dev == right.st_dev && left.st_ino == right.st_ino {
            return Ok(Some(()));
        }
        if flags & 2 == 0 {
            if source.is_directory() != target.is_directory() {
                return Err(if source.is_directory() {
                    ENOTDIR
                } else {
                    crate::EISDIR
                });
            }
            if target.is_directory() && !target.directory_snapshot()?.is_empty() {
                return Err(crate::ENOTEMPTY);
            }
        }
    }
    if flags & 2 != 0 {
        if source.is_directory() {
            if to.components.starts_with(&from.components) {
                return Err(EINVAL);
            }
            if source.entries.iter().any(|entry| entry.layer != 0)
                && from.instance.flags & super::features::REDIRECT == 0
            {
                return Err(EXDEV);
            }
        }
        let target = target.ok_or(ENOENT)?;
        if target.is_directory() {
            if from.components.starts_with(&to.components) {
                return Err(EINVAL);
            }
            if target.entries.iter().any(|entry| entry.layer != 0)
                && from.instance.flags & super::features::REDIRECT == 0
            {
                return Err(EXDEV);
            }
        }
        return exchange(&from, &to);
    }
    if source.is_directory() {
        if to.components.starts_with(&from.components) {
            return Err(EINVAL);
        }
        // Linux redirect_dir=nofollow: rename of a lower or merged directory
        // is EXDEV, so callers can perform their normal recursive move.
        if source.entries.iter().any(|entry| entry.layer != 0)
            && from.instance.flags & super::features::REDIRECT == 0
        {
            return Err(EXDEV);
        }
    }
    let work = from.instance.work.as_ref().ok_or(EROFS)?;
    let target = if target.as_ref().is_some_and(|node| {
        !node.is_directory()
            && node.backing_layer() != 0
            && !node.entries[0].indexed
            && node.backing_metadata().st_nlink > 1
    }) && from.instance.flags & super::features::INDEX != 0
    {
        Some(to.upper_mode(false, true)?)
    } else {
        target
    };
    let whiteout = if flags & 4 != 0 || from.lower_exists()? {
        Some(copy_up::Staged::whiteout(work)?)
    } else {
        None
    };
    let source = rename_upper(&from)?;
    let object = Object::reopen(source.backing_object().raw(), ACCESS | DELETE)?;
    let (from_parent, from_name) = from.parent()?;
    let (to_parent, to_name) = to.parent()?;
    let stored = crate::path::escape_component(to_name);
    let existing = match to_parent
        .backing_object()
        .child(OsStr::new(stored.as_ref()), ACCESS | DELETE)
    {
        Ok(object) => Some(object),
        Err(ENOENT) => None,
        Err(e) => return Err(e),
    };
    let mut saved = existing
        .as_ref()
        .map(|entry| {
            SavedEntry::new(
                entry,
                to_parent.backing_object(),
                to_name,
                work,
                to_parent.namespace,
            )
        })
        .transpose()?;
    if source.is_directory() && to.lower_exists()? {
        unsafe { Attributes::from_handle(object.raw(), true)? }.set(
            format!("{}opaque", source.namespace.prefix()).as_bytes(),
            b"y",
            0,
        )?;
    }
    unsafe {
        fs::rename_host_handle_relative(
            object.raw(),
            to_parent.backing_object().raw(),
            OsStr::new(stored.as_ref()),
            false,
        )?;
    }
    if let Some(whiteout) = whiteout {
        if let Err(error) = whiteout.publish(from_parent.backing_object(), from_name) {
            unsafe {
                fs::rename_host_handle_relative(
                    object.raw(),
                    from_parent.backing_object().raw(),
                    OsStr::new(crate::path::escape_component(from_name).as_ref()),
                    false,
                )?;
            }
            return Err(error);
        }
    }
    if let Some(saved) = &mut saved {
        saved.discard()?;
    }
    if let Some(target) = &target
        && let Some(index) = from
            .instance
            .root
            .context
            .as_ref()
            .and_then(|context| context.index.as_ref())
    {
        index.adjust_links(target, -1, work)?;
    }
    // Descriptors still name their original inode. Rewrite only references
    // below the moved source, never an open descriptor of the replaced target.
    let source_inode = from.metadata(&source)?.st_ino;
    if let Ok(mut descriptions) = descriptions().lock() {
        for description in descriptions.values_mut() {
            let location = &mut description.location;
            if location.instance.source == from.instance.source
                && location.components.starts_with(&from.components)
                && (location.components.len() != from.components.len()
                    || description.inode == source_inode)
            {
                let mut components = to.components.clone();
                components.extend_from_slice(&location.components[from.components.len()..]);
                let tail = &location.guest[from.guest.len()..];
                location.guest = format!("{}{tail}", to.guest);
                location.components = components;
            }
        }
    }
    Ok(Some(()))
}

fn exchange(from: &Location, to: &Location) -> Result<Option<()>, i32> {
    let left = rename_upper(from)?;
    let right = rename_upper(to)?;
    let left_inode = from.metadata(&left)?.st_ino;
    let right_inode = to.metadata(&right)?.st_ino;
    let (left_parent, left_name) = from.parent()?;
    let (right_parent, right_name) = to.parent()?;
    let left_object = Object::reopen(left.backing_object().raw(), ACCESS | DELETE)?;
    for (node, destination) in [(&left, to), (&right, from)] {
        if node.is_directory() && destination.lower_exists()? {
            unsafe { Attributes::from_handle(node.backing_object().raw(), true)? }.set(
                format!("{}opaque", node.namespace.prefix()).as_bytes(),
                b"y",
                0,
            )?;
        }
    }
    let mut saved = SavedEntry::new(
        right.backing_object(),
        right_parent.backing_object(),
        right_name,
        from.instance.work.as_ref().ok_or(EROFS)?,
        right.namespace,
    )?;
    let left_name = crate::path::escape_component(left_name);
    let right_name = crate::path::escape_component(right_name);
    unsafe {
        fs::rename_host_handle_relative(
            left_object.raw(),
            right_parent.backing_object().raw(),
            OsStr::new(right_name.as_ref()),
            false,
        )?;
    }
    if let Err(error) = unsafe {
        fs::rename_host_handle_relative(
            saved.object.raw(),
            left_parent.backing_object().raw(),
            OsStr::new(left_name.as_ref()),
            false,
        )
    } {
        unsafe {
            fs::rename_host_handle_relative(
                left_object.raw(),
                left_parent.backing_object().raw(),
                OsStr::new(left_name.as_ref()),
                false,
            )?;
        }
        return Err(error);
    }
    saved.active = false;
    // Readers hold the same upper mutex, making all three native renames one
    // observable namespace transaction. Each descriptor is relocated once.
    for description in descriptions().lock().map_err(|_| EIO)?.values_mut() {
        let location = &mut description.location;
        if location.instance.source != from.instance.source {
            continue;
        }
        for (old, new, inode) in [(from, to, left_inode), (to, from, right_inode)] {
            if location.components.starts_with(&old.components)
                && (location.components.len() != old.components.len() || description.inode == inode)
            {
                let mut components = new.components.clone();
                components.extend_from_slice(&location.components[old.components.len()..]);
                location.guest = format!("{}{}", new.guest, &location.guest[old.guest.len()..]);
                location.components = components;
                break;
            }
        }
    }
    Ok(Some(()))
}

fn rename_upper(location: &Location) -> Result<Node, i32> {
    let node = location.lookup()?;
    let redirect = if (node.is_directory() || node.is_metacopy())
        && location.instance.flags & super::features::REDIRECT != 0
    {
        if let Some(existing) = &node.entries[0].redirect {
            Some(existing.clone())
        } else if let Some(lower) = node.entries.iter().find(|entry| entry.layer != 0) {
            let root = location
                .instance
                .root
                .entries
                .iter()
                .find(|entry| entry.layer == lower.layer)
                .ok_or(EIO)?;
            let root_path = root.object.path()?;
            let path = lower.object.path()?;
            let relative = path.strip_prefix(&root_path).map_err(|_| EIO)?;
            let mut redirect = String::new();
            for component in relative.components() {
                let std::path::Component::Normal(name) = component else {
                    return Err(EIO);
                };
                redirect.push('/');
                redirect.push_str(&crate::path::unescape_path(name.to_str().ok_or(EIO)?));
            }
            if redirect.len() > 256 {
                return Err(EXDEV);
            }
            Some(redirect)
        } else {
            None
        }
    } else {
        None
    };
    let upper = location.upper_mode(false, true)?;
    if let Some(redirect) = redirect {
        unsafe { Attributes::from_handle(upper.backing_object().raw(), true)? }.set(
            format!("{}redirect", upper.namespace.prefix()).as_bytes(),
            redirect.as_bytes(),
            0,
        )?;
        return location.lookup();
    }
    Ok(upper)
}

pub fn hard_link(existing: &str, new: &str) -> Result<bool, i32> {
    let from = resolve(existing, false, false)?.and_then(|r| r.location);
    let to = resolve(new, false, true)?.and_then(|r| r.location);
    let (from, to) = match (from, to) {
        (None, None) => return Ok(false),
        (Some(from), Some(to)) if from.instance.source == to.instance.source => (from, to),
        _ => return Err(EXDEV),
    };
    let _source_writer = from.writer()?;
    let _target_writer = to.writer()?;
    let _guard = InodeLock::acquire(from.instance.root.backing_object().raw())?;
    let node = from.lookup()?;
    if node.is_directory() {
        return Err(crate::EPERM);
    }
    match to.lookup() {
        Ok(_) => return Err(EEXIST),
        Err(ENOENT) => {}
        Err(e) => return Err(e),
    }
    let source = from.upper(false)?;
    let mut target = prepare_create(new)?;
    std::fs::hard_link(source.backing_object().path()?, &target.path).map_err(|e| {
        e.raw_os_error()
            .map_or(EIO, |e| crate::errno_from_win32(e as u32))
    })?;
    target.finish()?;
    if let Some(index) = from
        .instance
        .root
        .context
        .as_ref()
        .and_then(|context| context.index.as_ref())
    {
        index.adjust_links(&source, 1, from.instance.work.as_ref().ok_or(EROFS)?)?;
    }
    Ok(true)
}
