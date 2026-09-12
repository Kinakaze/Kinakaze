//! Mount-namespace path mappings shared by hosted processes.
//!
//! Windows has no kernel mount namespace that can be entered by an ELF guest,
//! so mounts are represented at the VFS boundary. Each attachment records its
//! actual parent and an already-resolved backing path. Lookup walks this tree,
//! including covered mounts, before translating to a host path. Backing paths
//! are not yet pinned inodes: renaming a bind source remains a backend limit,
//! not a property hidden by the mount report.

use std::sync::Arc;

use crate::{EINVAL, EIO, ENOTDIR, EOPNOTSUPP};

pub mod api;
mod namespace;
pub mod native;
pub mod overlay;
mod pivot;
pub(crate) mod policy;
mod propagation;
pub mod query;
pub(crate) mod shared;
pub use namespace::enter_namespace;
pub use pivot::pivot_root;
#[cfg(test)]
mod topology_tests;

// Stable mount IDs change the table ABI. Keep the same shared-object name so
// an old live table is rejected, never hidden behind a new empty namespace.
const STATE_MAGIC: u64 = u64::from_le_bytes(*b"CYMOUNT6");
const FORK_MAGIC: u64 = u64::from_le_bytes(*b"CYMNS004");
pub(crate) const ROOT_MOUNT_ID: u64 = 1;
const FIRST_DYNAMIC_MOUNT_ID: u64 = 64;
const MAX_MOUNT_ID: u64 = i32::MAX as u64;

/// Linux mount flags accepted by the namespace layer.
pub const MS_BIND: u64 = 4096;
const BIND_ATTRS: u64 = 1 | 2 | 4 | 8 | 1024 | 2048 | (1 << 21) | (1 << 24);
pub const MS_REC: u64 = 16384;
pub const MS_UNBINDABLE: u64 = 1 << 17;
pub const MS_PRIVATE: u64 = 1 << 18;
pub const MS_SLAVE: u64 = 1 << 19;
pub const MS_SHARED: u64 = 1 << 20;
pub const MS_PROPAGATION: u64 = MS_UNBINDABLE | MS_PRIVATE | MS_SLAVE | MS_SHARED;
pub const MS_OVERLAY: u64 = 1 << 40;
pub const MS_CGROUP: u64 = 1 << 31;
const _: () = assert!(
    MS_CGROUP
        & (MS_SYSFS
            | MS_TMPFS
            | MS_PROC
            | MS_DEVPTS
            | MS_MQUEUE
            | MS_OVERLAY
            | MS_ROOT
            | MS_IDMAPPED
            | overlay::USER_XATTR_FLAG
            | overlay::features::ALL)
        == 0
);
pub const MS_SYSFS: u64 = 1 << 32;
const _: () = assert!(
    MS_SYSFS
        & (MS_TMPFS
            | MS_PROC
            | MS_DEVPTS
            | MS_MQUEUE
            | MS_OVERLAY
            | MS_ROOT
            | MS_IDMAPPED
            | overlay::USER_XATTR_FLAG
            | overlay::features::ALL)
        == 0
);
pub const MS_MQUEUE: u64 = 1 << 33;
pub const MS_DEVPTS: u64 = 1 << 34;
const _: () = assert!(
    MS_DEVPTS
        & (MS_TMPFS
            | MS_PROC
            | MS_OVERLAY
            | MS_ROOT
            | MS_IDMAPPED
            | overlay::USER_XATTR_FLAG
            | overlay::features::ALL)
        == 0
);
pub const MS_PROC: u64 = 1 << 36;
pub const MS_TMPFS: u64 = 1 << 35;
const _: () = assert!(
    MS_TMPFS
        & (MS_PROC
            | MS_OVERLAY
            | MS_ROOT
            | MS_IDMAPPED
            | overlay::USER_XATTR_FLAG
            | overlay::features::ALL)
        == 0
);
const _: () = assert!(
    MS_PROC
        & (MS_OVERLAY | MS_ROOT | MS_IDMAPPED | overlay::USER_XATTR_FLAG | overlay::features::ALL)
        == 0
);
const MS_ROOT: u64 = 1 << 37;
pub const MS_IDMAPPED: u64 = 1 << 38;

/// Mount policy for an already namespace-rooted overlay location.
pub(crate) fn attachment_policy(path: &str, source: &str) -> Result<Option<u64>, i32> {
    Ok(attachment_reference(path, source)?.map(|p| p.flags()))
}
pub(crate) fn attachment_reference(
    path: &str,
    source: &str,
) -> Result<Option<Arc<policy::Policy>>, i32> {
    if let Some((fd, tail)) = api::tree_reference(path) {
        let (actual, _, flags) = api::tree_mount(fd, &format!("/{tail}"))?;
        let _ = flags;
        return if actual == source {
            api::tree_policy(fd, &format!("/{tail}")).map(Some)
        } else {
            Ok(None)
        };
    }
    let table = snapshot()?;
    visible_mount(&table, &normalize(path)?)?
        .filter(|p| p.flags & MS_OVERLAY != 0)
        .filter(|p| overlay::split_view(&p.source).is_ok_and(|(base, _)| base == source))
        .map(|p| policy::get_mapped(namespace_id()?, p.id, p.flags, p.meta.idmap.as_ref()))
        .transpose()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParsedOverlay {
    pub lowerdirs: Vec<String>,
    pub upperdir: Option<String>,
    pub workdir: Option<String>,
}

impl ParsedOverlay {
    pub(crate) fn parse(source: &str) -> Option<Self> {
        let options = source.strip_prefix("overlay:")?;
        let mut lowerdirs = Vec::new();
        let mut upperdir = None;
        let mut workdir = None;
        for part in options.split(';') {
            if let Some(lowers) = part.strip_prefix("lowerdir_hex=") {
                for dir in lowers.split(':') {
                    lowerdirs.push(overlay::decode_option(dir)?);
                }
            } else if let Some(upper) = part.strip_prefix("upperdir_hex=") {
                if !upper.is_empty() {
                    upperdir = Some(overlay::decode_option(upper)?);
                }
            } else if let Some(work) = part.strip_prefix("workdir_hex=") {
                if !work.is_empty() {
                    workdir = Some(overlay::decode_option(work)?);
                }
            } else if let Some(lowers) = part.strip_prefix("lowerdir=") {
                for dir in lowers.split(':').filter(|d| !d.is_empty()) {
                    lowerdirs.push(dir.to_string());
                }
            } else if let Some(upper) = part.strip_prefix("upperdir=") {
                if !upper.is_empty() {
                    upperdir = Some(upper.to_string());
                }
            } else if let Some(work) = part.strip_prefix("workdir=") {
                if !work.is_empty() {
                    workdir = Some(work.to_string());
                }
            }
        }
        if lowerdirs.is_empty() && upperdir.is_none() {
            return None;
        }
        Some(Self {
            lowerdirs,
            upperdir,
            workdir,
        })
    }
}

/// Linux unmount flags.
pub const MNT_FORCE: i32 = 1;
pub const MNT_DETACH: i32 = 2;
pub const MNT_EXPIRE: i32 = 4;
pub const UMOUNT_NOFOLLOW: i32 = 8;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct MountMetadata {
    pivot: u64,
    peer: u64,
    master: u64,
    idmap: Option<crate::user_namespace::Mapping>,
}
impl MountMetadata {
    fn cloned_for_attachment(&self) -> Self {
        let mut metadata = self.clone();
        // pivot identifies the one mount serving as this namespace's root. It
        // is topology state, not a superblock attribute inherited by bind,
        // open_tree or propagation copies.
        metadata.pivot = 0;
        metadata
    }

    fn encode(&self) -> Vec<u8> {
        let mut bytes = self.peer.to_le_bytes().to_vec();
        bytes.extend_from_slice(&self.master.to_le_bytes());
        bytes.extend_from_slice(&self.pivot.to_le_bytes());
        if let Some(map) = &self.idmap {
            bytes.extend_from_slice(&map.encode());
        }
        bytes
    }
    fn decode(bytes: &[u8]) -> Result<Self, i32> {
        if bytes.len() < 24 {
            return Err(EIO);
        }
        Ok(Self {
            peer: u64::from_le_bytes(bytes[..8].try_into().unwrap()),
            master: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            pivot: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            idmap: if bytes.len() == 24 {
                None
            } else {
                Some(crate::user_namespace::Mapping::decode(&bytes[24..])?)
            },
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MountPoint {
    meta: MountMetadata,
    id: u64,
    parent: u64,
    source: String,
    target: String,
    flags: u64,
}

/// One entry in the actual shared mount table, not a synthetic procfs row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountSnapshotEntry {
    pub source: String,
    pub target: String,
    pub flags: u64,
}

/// Reads one consistent snapshot of the current namespace's published mounts.
///
/// Entries retain publication order, including multiple mounts stacked at the
/// same target. This does not bind, unmount or change any stored entry. An empty
/// list means no additional mounts are published in this table; it does not
/// assert that procfs or the guest root are independent Linux mount instances.
/// The returned value cannot prevent another process from mounting afterwards.
pub fn snapshot_list() -> Result<Vec<MountSnapshotEntry>, i32> {
    Ok(snapshot()?
        .iter()
        .map(|point| MountSnapshotEntry {
            source: point.source.clone(),
            target: point.target.clone(),
            flags: point.flags,
        })
        .collect())
}

/// Mount membership belongs to the calling task; fs_struct is copied separately.
pub fn unshare_namespace() -> Result<(), i32> {
    let install = shared::prepare_unshare()?;
    crate::fs_context::unshare();
    install()
}
pub fn prepare_unshare_namespace() -> Result<impl FnOnce() -> Result<(), i32>, i32> {
    shared::prepare_unshare()
}

pub fn prepare_with_user(
    user: Option<&crate::user_namespace::Prepared>,
) -> Result<impl FnOnce() -> Result<(), i32> + use<>, i32> {
    if user.is_none() && !crate::namespaces::privileged() {
        return Err(crate::EPERM);
    }
    shared::prepare_owned(match user {
        Some(u) => u.id(),
        None => crate::user_namespace::id(crate::job::process_id())?,
    })
}
pub fn namespace_owner(fd: i32) -> Result<u64, i32> {
    Ok(shared::namespace(shared::descriptor_id(fd)?)?.owner())
}
pub fn namespace_id() -> Result<u64, i32> {
    Ok(shared::get()?.id())
}

pub fn open_namespace(flags: crate::FdFlags) -> Result<i32, i32> {
    shared::get()?.descriptor(flags)
}

pub fn open_process_namespace(pid: u32, flags: crate::FdFlags) -> Result<i32, i32> {
    if pid == crate::job::process_id() {
        return open_namespace(flags);
    }
    shared::for_process(pid)?.descriptor(flags)
}

pub fn namespace_descriptor_inode(fd: i32) -> Result<u64, i32> {
    namespace_inode(shared::descriptor_id(fd)?)
}
pub fn namespace_descriptor_id(fd: i32) -> Result<u64, i32> {
    shared::descriptor_id(fd)
}

pub(crate) fn namespace_inode(id: u64) -> Result<u64, i32> {
    if id == 1 {
        return Ok(4026531840);
    }
    id.checked_add(1u64 << 32).ok_or(crate::EOVERFLOW)
}

/// Returns the persistent root coordinate of the current mount namespace.
/// A process entering through setns has a fresh fs_struct, so this coordinate
/// must come from the shared mount topology rather than thread-local chroot
/// state.
pub(crate) fn namespace_root_path() -> Result<Option<String>, i32> {
    let table = snapshot()?;
    Ok(pivot::namespace_root(&table)?.map(|point| point.target.clone()))
}

/// The real shared attachment topology, with IDs retained through serialization and
/// unrelated updates. It does not invent Linux namespace/propagation state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MountReport {
    pub idmap: Option<crate::user_namespace::Mapping>,
    pub peer: u64,
    pub master: u64,
    pub id: u64,
    pub parent: u64,
    pub source: String,
    pub target: String,
    pub flags: u64,
}

pub(crate) fn report_snapshot() -> Result<Vec<MountReport>, i32> {
    let points = snapshot()?;
    Ok(points
        .iter()
        .map(|point| MountReport {
            idmap: point.meta.idmap.clone(),
            peer: point.meta.peer,
            master: point.meta.master,
            id: point.id,
            parent: point.parent,
            source: point.source.clone(),
            target: point.target.clone(),
            flags: point.flags,
        })
        .collect())
}

fn snapshot() -> Result<Arc<Vec<MountPoint>>, i32> {
    // Readers consume an atomic namespace publication without waiting on a
    // native mutex that a frozen parent thread might own during fork.
    let store = shared::get()?;
    let revision = store.revision();
    if let Some((cached_revision, points)) = store.cache.read().map_err(|_| EIO)?.as_ref()
        && *cached_revision == revision
    {
        return Ok(Arc::clone(points));
    }
    let (revision, payload) = store.read()?;
    let points = Arc::new(decode_table(&payload)?);
    let mut cache = store.cache.write().map_err(|_| EIO)?;
    if let Some((cached_revision, cached)) = cache.as_ref()
        && *cached_revision >= revision
    {
        return Ok(Arc::clone(cached));
    }
    *cache = Some((revision, Arc::clone(&points)));
    Ok(points)
}

fn update<T>(
    action: impl FnOnce(&mut Vec<MountPoint>, &mut u64) -> Result<T, i32>,
) -> Result<T, i32> {
    propagation::update(action)
}

/// Collapses a Linux pathname without ever allowing .. above /.
pub(crate) fn normalize(path: &str) -> Result<String, i32> {
    if !path.starts_with('/') || path.contains('\0') || path.contains('\\') {
        return Err(EINVAL);
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        match part {
            "." => {}
            ".." => {
                let at_drive_root = parts.len() == 1 && parts[0].len() == 1;
                if !at_drive_root {
                    parts.pop();
                }
            }
            value => parts.push(value),
        }
    }
    if parts.is_empty() {
        Ok(String::from("/"))
    } else {
        Ok(format!("/{}", parts.join("/")))
    }
}

fn suffix<'a>(path: &'a str, target: &str) -> Option<&'a str> {
    if target == "/" {
        return Some(path.trim_start_matches('/'));
    }
    if path == target {
        return Some("");
    }
    path.strip_prefix(target)?.strip_prefix('/')
}

fn join(source: &str, rest: &str) -> String {
    if rest.is_empty() {
        source.to_owned()
    } else if source == "/" {
        format!("/{rest}")
    } else {
        format!("{source}/{rest}")
    }
}

fn has_fixed_virtual_dispatch(path: &str) -> bool {
    // These builtin trees are selected by fs.rs before the native pathname
    // walker. An alias cannot copy or cover them. Root includes those trees as
    // well; do not publish a bind that only appears to cover its contents.
    path == "/"
        || ["/proc", "/sys", "/dev"]
            .iter()
            .any(|root| suffix(path, root).is_some())
}

/// Applies the current mount stack to an absolute Linux pathname.
///
/// A backing path is resolved at bind time and is never reinterpreted through
/// later mounts at the source pathname. Nonrecursive binds therefore do not
/// acquire source submounts; recursive binds clone the actual attachment tree.
pub(crate) fn translate(path: &str) -> Result<String, i32> {
    native_translation(path)?.ok_or(EOPNOTSUPP)
}

/// `None` selects a virtual filesystem backend, which cannot be represented
/// by a Windows pathname. Callers that support backend crossings dispatch it
/// explicitly rather than interpreting EOPNOTSUPP as a missing file.
pub(crate) fn native_translation(path: &str) -> Result<Option<String>, i32> {
    if !path.starts_with('/') {
        if path.contains('\0') || path.contains('\\') {
            return Err(EINVAL);
        }
        return Ok(Some(path.to_owned()));
    }
    native_translation_in(&snapshot()?, path)
}

fn translate_in(table: &[MountPoint], path: &str) -> Result<String, i32> {
    native_translation_in(table, path)?.ok_or(EOPNOTSUPP)
}

fn native_translation_in(table: &[MountPoint], path: &str) -> Result<Option<String>, i32> {
    let current = normalize(path)?;
    if let Some(mount) = visible_mount(table, &current)? {
        if mount.flags & (MS_OVERLAY | MS_PROC | MS_TMPFS) != 0 {
            // A pathname alias cannot represent overlay inode identity, merged
            // directories, whiteouts or copy-up. In particular, forwarding an
            // existing lower pathname would let a guest write the image layer.
            // Overlay callers use overlay::resolve and operation guards. The
            // bind-only string translation API cannot hand out a lower alias.
            return Ok(None);
        }
        return Ok(Some(join(
            &mount.source,
            suffix(&current, &mount.target).ok_or(EIO)?,
        )));
    }
    Ok(Some(current))
}

pub(crate) fn has_overlay() -> Result<bool, i32> {
    Ok(snapshot()?
        .iter()
        .any(|point| point.flags & MS_OVERLAY != 0))
}
pub(crate) fn has_attachment(path: &str) -> Result<bool, i32> {
    Ok(visible_mount(&snapshot()?, path)?.is_some())
}

pub(crate) fn overlay_location(path: &str) -> Result<Option<(String, String)>, i32> {
    let points = snapshot()?;
    let Some(point) = visible_mount(&points, path)? else {
        return Ok(None);
    };
    if point.flags & MS_OVERLAY == 0 {
        return Ok(None);
    }
    let (source, view) = overlay::split_view(&point.source)?;
    let rest = suffix(path, &point.target).ok_or(EIO)?;
    Ok(Some((
        source.into(),
        [view.as_str(), rest]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/"),
    )))
}

pub(crate) fn visible_mount_id(path: &str) -> Result<Option<u64>, i32> {
    Ok(visible_mount(&snapshot()?, path)?.map(|point| point.id))
}

fn visible_mount<'a>(table: &'a [MountPoint], path: &str) -> Result<Option<&'a MountPoint>, i32> {
    let mut parent = ROOT_MOUNT_ID;
    let mut selected = None;
    for _ in 0..=table.len() {
        // Walk the first attachment encountered while descending from this
        // parent. A later /a can hide an older /a/b sibling; longest-prefix
        // selection would incorrectly reach the covered child.
        let Some(child) = table
            .iter()
            .filter(|point| point.parent == parent && suffix(path, &point.target).is_some())
            .min_by_key(|point| point.target.len())
        else {
            return Ok(selected);
        };
        parent = child.id;
        selected = Some(child);
    }
    Err(EIO)
}

fn allocate_id(next_id: &mut u64) -> Result<u64, i32> {
    if !(FIRST_DYNAMIC_MOUNT_ID..=MAX_MOUNT_ID).contains(next_id) {
        return Err(crate::EOVERFLOW);
    }
    let id = *next_id;
    *next_id += 1;
    Ok(id)
}

fn bind_in(
    table: &mut Vec<MountPoint>,
    next_id: &mut u64,
    source: &str,
    target: &str,
    flags: u64,
) -> Result<(), i32> {
    let source = normalize(source)?;
    let target = normalize(target)?;
    let selected_source = visible_mount(table, &source)?;
    let builtin_proc = selected_source.is_none() && crate::procfs::owns(&source);
    if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
        use std::io::Write;
        let path = std::path::PathBuf::from(directory)
            .join(format!("mount-bind-{}.log", std::process::id()));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(
                file,
                "source={source:?} target={target:?} selected={:?} flags={flags:#x}",
                selected_source.map(|p| (&p.source, &p.target, p.flags)),
            );
        }
    }
    if (has_fixed_virtual_dispatch(&source) || has_fixed_virtual_dispatch(&target))
        && !builtin_proc
        && !selected_source.is_some_and(|p| p.flags & (MS_PROC | MS_TMPFS) != 0)
    {
        return Err(EOPNOTSUPP);
    }
    if selected_source.is_some_and(|p| p.flags & MS_UNBINDABLE != 0) {
        return Err(EINVAL);
    }
    let (backing, attachment_flags) =
        if let Some(point) = selected_source.filter(|p| p.flags & MS_TMPFS != 0) {
            (
                crate::tmpfs::subtree(&point.source, suffix(&source, &point.target).ok_or(EIO)?)?,
                point.flags & !MS_REC,
            )
        } else if let Some(point) = selected_source.filter(|p| p.flags & MS_PROC != 0) {
            (
                format!(
                    "{}/{}",
                    point.source.trim_end_matches('/'),
                    suffix(&source, &point.target).ok_or(EIO)?
                ),
                point.flags & !MS_REC,
            )
        } else if builtin_proc {
            let root = crate::procfs::instance::prepare("")?;
            let tail = source
                .strip_prefix("/proc")
                .ok_or(EIO)?
                .trim_start_matches('/');
            (
                format!("{root}{tail}"),
                (flags & !(MS_BIND | MS_REC)) | MS_PROC,
            )
        } else if let Some(point) = selected_source.filter(|p| p.flags & MS_OVERLAY != 0) {
            let (base, view) = overlay::split_view(&point.source)?;
            let rest = suffix(&source, &point.target).ok_or(EIO)?;
            let view = [view.as_str(), rest]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            (
                overlay::with_view(base, &view),
                (flags & !MS_BIND) | (point.flags & !MS_REC),
            )
        } else {
            (
                translate_in(table, &source)?,
                flags | selected_source.map_or(0, |p| p.flags & (MS_PROPAGATION | BIND_ATTRS)),
            )
        };
    // A proc/tmpfs attachment carries its own virtual dispatch and may itself
    // be bind-mounted elsewhere (Docker persists /proc/<pid>/ns/net this way).
    // Ordinary native aliases still cannot target the built-in trees.
    if has_fixed_virtual_dispatch(&backing) && attachment_flags & (MS_PROC | MS_TMPFS) == 0 {
        return Err(EOPNOTSUPP);
    }
    let source_parent = visible_mount(table, &source)?.map_or(ROOT_MOUNT_ID, |point| point.id);
    let parent = visible_mount(table, &target)?.map_or(ROOT_MOUNT_ID, |point| point.id);
    let id = allocate_id(next_id)?;
    let mut additions = vec![MountPoint {
        meta: selected_source
            .map(|p| p.meta.cloned_for_attachment())
            .unwrap_or_default(),
        id,
        parent,
        source: backing,
        target: target.clone(),
        flags: attachment_flags,
    }];
    if flags & MS_REC != 0 {
        let mut copied = std::collections::HashMap::new();
        copied.insert(source_parent, id);
        // Publication order is parent-before-child. Include only mounts below
        // the selected source directory, but retain covered descendants too.
        for point in table.iter() {
            if point.flags & MS_UNBINDABLE != 0 {
                continue;
            }
            let Some(&parent) = copied.get(&point.parent) else {
                continue;
            };
            let Some(rest) = suffix(&point.target, &source) else {
                continue;
            };
            let clone_id = allocate_id(next_id)?;
            copied.insert(point.id, clone_id);
            additions.push(MountPoint {
                meta: point.meta.cloned_for_attachment(),
                id: clone_id,
                parent,
                source: point.source.clone(),
                target: join(&target, rest),
                flags: point.flags,
            });
        }
    }
    table.extend(additions);
    Ok(())
}

/// Resolve a proc magic-link mount target to its open directory identity.
pub fn resolve_mount_target(path: &str) -> Result<String, i32> {
    let path = normalize(&crate::fs::absolute_linux(path))?;
    if let Some((pid, fd)) = crate::procfs::fd_magic_link(&path) {
        if pid != crate::job::process_id() {
            return Err(EOPNOTSUPP);
        }
        crate::fs::fstat(fd)?;
        return crate::procfs::local_fd_link_target(fd);
    }
    Ok(path)
}
/// Look up a proc attachment before any fixed synthetic-path dispatch.
pub(crate) fn proc_location(
    path: &str,
) -> Result<Option<(crate::procfs::instance::View, String)>, i32> {
    let (point, rest, ns) = if let Some((fd, tail)) = api::tree_reference(path) {
        let (source, _, flags) = api::tree_mount(fd, &format!("/{tail}"))?;
        if flags & MS_PROC == 0 {
            return Ok(None);
        }
        let (mut view, tail) = crate::procfs::instance::parse(&source)?;
        let policy = api::tree_policy(fd, &format!("/{tail}"))?;
        view.flags = policy.flags();
        view.namespace = namespace_id()?;
        view.mount = api::tree_record(fd)?.id;
        view.target = format!("/proc/self/fd/{fd}");
        return Ok(Some((view, format!("/proc/{tail}"))));
    } else {
        let table = snapshot()?;
        let path = namespace_path(path)?;
        let Some(point) = visible_mount(&table, &path)? else {
            return Ok(None);
        };
        if point.flags & MS_PROC == 0 {
            return Ok(None);
        }
        (
            point.clone(),
            suffix(&path, &point.target).ok_or(EIO)?.to_owned(),
            namespace_id()?,
        )
    };
    let (mut view, base) = crate::procfs::instance::parse(&point.source)?;
    view.namespace = ns;
    view.mount = point.id;
    view.flags = point.flags;
    view.target = point.target.clone();
    if let Some(root) = crate::path::namespace_root_path()? {
        if let Some(rest) = suffix(&view.target, &root) {
            view.target = format!("/{rest}");
        }
    }
    Ok(Some((
        view,
        format!(
            "/proc/{}",
            join(base.trim_end_matches('/'), &rest).trim_start_matches('/')
        ),
    )))
}
pub fn proc_mount(target: &str, flags: u64, options: &str) -> Result<(), i32> {
    const ATTRS: u64 = 1 | 2 | 4 | 8 | 16 | 64 | 128 | 1024 | 2048 | (1 << 21) | (1 << 24);
    if flags & !(ATTRS | 32768) != 0 {
        return Err(EINVAL);
    }
    let target = resolve_mount_target(target)?;
    if crate::fs::stat(&target)?.st_mode & crate::fs::S_IFMT != crate::fs::S_IFDIR {
        return Err(ENOTDIR);
    }
    let source = crate::procfs::instance::prepare(options)?;
    let target = namespace_path(&target)?;
    if !crate::user_namespace::capable(shared::get()?.owner(), 21) {
        return Err(crate::EPERM);
    }
    update(|table, next| {
        let parent = visible_mount(table, &target)?.map_or(ROOT_MOUNT_ID, |p| p.id);
        let id = allocate_id(next)?;
        let flags = (flags & ATTRS)
            | MS_PROC
            | if flags & (1024 | (1 << 24)) == 0 {
                1 << 21
            } else {
                0
            };
        table.push(MountPoint {
            meta: MountMetadata::default(),
            id,
            parent,
            source,
            target,
            flags,
        });
        Ok(())
    })
}
pub fn tmpfs_mount(target: &str, flags: u64, options: &str) -> Result<(), i32> {
    memory_mount(target, flags, options, "tmpfs")
}
pub fn devpts_mount(target: &str, flags: u64, options: &str) -> Result<(), i32> {
    memory_mount(target, flags, options, "devpts")
}
pub fn mqueue_mount(target: &str, flags: u64, options: &str) -> Result<(), i32> {
    memory_mount(target, flags, options, "mqueue")
}
pub fn cgroup_mount(target: &str, flags: u64, options: &str) -> Result<(), i32> {
    memory_mount(target, flags, options, "cgroup2")
}
pub fn sysfs_mount(target: &str, flags: u64, options: &str) -> Result<(), i32> {
    memory_mount(target, flags, options, "sysfs")
}
fn memory_mount(target: &str, flags: u64, options: &str, filesystem: &str) -> Result<(), i32> {
    const ATTRS: u64 = 1 | 2 | 4 | 8 | 16 | 64 | 128 | 1024 | 2048 | (1 << 21) | (1 << 24);
    if flags & !(ATTRS | 32768) != 0 {
        return Err(EINVAL);
    }
    if !crate::user_namespace::capable(shared::get()?.owner(), 21) {
        return Err(crate::EPERM);
    }
    let target = resolve_mount_target(target)?;
    if crate::fs::stat(&target)?.st_mode & crate::fs::S_IFMT != crate::fs::S_IFDIR {
        return Err(ENOTDIR);
    }
    let source = match filesystem {
        "cgroup2" => crate::tmpfs::cgroupfs::prepare(options)?,
        "devpts" => crate::devpts::prepare(options)?,
        "mqueue" => crate::mqueue::prepare(options)?,
        "sysfs" => crate::sysfs::prepare(options)?,
        _ => crate::tmpfs::prepare(options)?,
    };
    let target = namespace_path(&target)?;
    update(|table, next| {
        let parent = visible_mount(table, &target)?.map_or(ROOT_MOUNT_ID, |p| p.id);
        let id = allocate_id(next)?;
        let flags = (flags & ATTRS)
            | MS_TMPFS
            | match filesystem {
                "devpts" => MS_DEVPTS,
                "mqueue" => MS_MQUEUE | 4 | 8,
                "sysfs" => MS_SYSFS | 2 | 4 | 8,
                "cgroup2" => MS_CGROUP | 2 | 4 | 8,
                _ => 0,
            }
            | if flags & (1024 | (1 << 24)) == 0 {
                1 << 21
            } else {
                0
            };
        table.push(MountPoint {
            meta: MountMetadata::default(),
            id,
            parent,
            source,
            target,
            flags,
        });
        Ok(())
    })
}
pub(crate) fn tmpfs_location(path: &str) -> Result<Option<crate::tmpfs::Location>, i32> {
    if let Some((fd, tail)) = api::tree_reference(path) {
        let (source, _, flags) = api::tree_mount(fd, &format!("/{tail}"))?;
        if flags & MS_TMPFS == 0 {
            return Ok(None);
        }
        let (volume, node) = crate::tmpfs::parse(&source)?;
        let p = api::tree_policy(fd, &format!("/{tail}"))?;
        return Ok(Some(crate::tmpfs::Location {
            volume,
            node,
            tail: String::new(),
            namespace: namespace_id()?,
            mount: api::tree_record(fd)?.id,
            flags: p.flags(),
            path: path.into(),
        }));
    }
    let table = snapshot()?;
    let absolute = namespace_path(path)?;
    let Some(point) = visible_mount(&table, &absolute)?.filter(|p| p.flags & MS_TMPFS != 0) else {
        return Ok(None);
    };
    let (volume, node) = crate::tmpfs::parse(&point.source)?;
    Ok(Some(crate::tmpfs::Location {
        volume,
        node,
        tail: suffix(&absolute, &point.target).ok_or(EIO)?.into(),
        namespace: namespace_id()?,
        mount: point.id,
        flags: point.flags,
        path: crate::fs::absolute_linux(path),
    }))
}
pub fn proc_remount(target: &str, flags: u64, options: &str) -> Result<(), i32> {
    if !options.is_empty() {
        return Err(EOPNOTSUPP);
    }
    let target = namespace_path(&resolve_mount_target(target)?)?;
    let namespace = namespace_id()?;
    if !crate::user_namespace::capable(shared::get()?.owner(), 21) {
        return Err(crate::EPERM);
    }
    let attrs = 1 | 2 | 4 | 8 | 16 | 64 | 128 | 1024 | 2048 | (1 << 21) | (1 << 24);
    if flags & !(attrs | 32 | MS_BIND) != 0 {
        return Err(EINVAL);
    }
    let mut changes = None;
    update(|table, _| {
        let id = visible_mount(table, &target)?
            .filter(|p| p.target == target && p.flags & MS_PROC != 0)
            .ok_or(EINVAL)?
            .id;
        let point = table.iter_mut().find(|p| p.id == id).ok_or(EIO)?;
        let reference = policy::get(namespace, id, point.flags)?;
        point.flags = (point.flags & !attrs) | (flags & attrs);
        changes = Some(policy::Changes::apply(vec![(reference, point.flags)])?);
        Ok(())
    })?;
    if let Some(changes) = changes {
        changes.commit();
    }
    Ok(())
}
pub(crate) fn tmpfs_attached(id: u64, previous_namespace: &mut Option<u64>) -> Result<bool, i32> {
    let contains = |ns: &shared::Store| -> Result<bool, i32> {
        let (_, bytes) = ns.read()?;
        if bytes.is_empty() {
            return Ok(false);
        }
        Ok(decode_table(&bytes)?.iter().any(|p| {
            p.flags & MS_TMPFS != 0 && crate::tmpfs::parse(&p.source).is_ok_and(|(v, _)| v == id)
        }))
    };
    // A positive hint avoids reopening every historically allocated object ID
    // on each keeper tick. Reopen and validate it against current metadata:
    // retaining an Arc here would itself keep a dead namespace/mount alive.
    if let Some(previous) = *previous_namespace {
        match shared::namespace(previous) {
            Ok(ns) if contains(&ns)? => return Ok(true),
            Ok(_) | Err(crate::ENOENT) => (),
            Err(error) => return Err(error),
        }
    }
    *previous_namespace = None;
    for ns in shared::live_namespaces()? {
        if contains(&ns)? {
            *previous_namespace = Some(ns.id());
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn tmpfs_remount(target: &str, flags: u64, options: &str) -> Result<(), i32> {
    let target = namespace_path(&resolve_mount_target(target)?)?;
    let namespace = namespace_id()?;
    if !crate::user_namespace::capable(shared::get()?.owner(), 21) {
        return Err(crate::EPERM);
    }
    let attrs = 1 | 2 | 4 | 8 | 16 | 64 | 128 | 1024 | 2048 | (1 << 21) | (1 << 24);
    if flags & !(attrs | 32 | MS_BIND) != 0 {
        return Err(EINVAL);
    }
    let mut changes = None;
    update(|table, _| {
        let id = visible_mount(table, &target)?
            .filter(|p| p.target == target && p.flags & MS_TMPFS != 0)
            .ok_or(EINVAL)?
            .id;
        let p = table.iter_mut().find(|p| p.id == id).ok_or(EIO)?;
        let reference = policy::get(namespace, id, p.flags)?;
        if p.flags & MS_CGROUP != 0 {
            crate::tmpfs::cgroupfs::reconfigure(&p.source, options)?;
        } else if p.flags & MS_SYSFS != 0 {
            crate::sysfs::reconfigure(&p.source, options)?;
        } else if p.flags & MS_MQUEUE != 0 {
            crate::mqueue::reconfigure(&p.source, options)?;
        } else if p.flags & MS_DEVPTS != 0 {
            crate::devpts::reconfigure(&p.source, options)?;
        } else {
            crate::tmpfs::resize(&p.source, options)?;
        }
        p.flags = (p.flags & !attrs)
            | (flags & attrs)
            | if p.flags & (MS_SYSFS | MS_CGROUP) != 0 {
                2 | 4 | 8
            } else if p.flags & MS_MQUEUE != 0 {
                4 | 8
            } else {
                0
            };
        changes = Some(policy::Changes::apply(vec![(reference, p.flags)])?);
        Ok(())
    })?;
    if let Some(changes) = changes {
        changes.commit();
    }
    Ok(())
}

/// Installs one bind mount in the calling process's mount namespace.
pub fn bind(source: &str, target: &str, flags: u64) -> Result<(), i32> {
    if flags & MS_BIND == 0 || flags & !(MS_BIND | MS_REC) != 0 {
        return Err(EINVAL);
    }
    let source = resolve_mount_target(source)?;
    let target = resolve_mount_target(target)?;
    let source = namespace_path(&source)?;
    let target = namespace_path(&target)?;
    update(|table, next_id| bind_in(table, next_id, &source, &target, flags))
}

/// Mount topology stores namespace-absolute paths even after chroot. Resolve
/// the caller's spelling against its retained overlay root before publication.
fn namespace_path(path: &str) -> Result<String, i32> {
    if !path.starts_with('/') {
        if let Some(cwd) = crate::fs::cwd::namespace()? {
            return normalize(&join(&cwd, path));
        }
        return namespace_path(&crate::fs::absolute_linux(path));
    }
    let path = normalize(path)?;
    Ok(if let Some(root) = crate::path::namespace_root_path()? {
        join(&root, path.trim_start_matches('/'))
    } else {
        path
    })
}

/// Change this attachment only; preserve children and its superblock policy.
pub fn remount_bind(target: &str, flags: u64) -> Result<(), i32> {
    const REMOUNT: u64 = 32;
    // Linux path_mount dispatches MS_REMOUNT|MS_BIND before the recursive
    // bind operation. MS_REC is accepted here but does not update descendants.
    if flags & (MS_BIND | REMOUNT) != MS_BIND | REMOUNT
        || flags & !(MS_BIND | REMOUNT | MS_REC | BIND_ATTRS) != 0
    {
        return Err(EINVAL);
    }
    if !crate::user_namespace::capable(shared::get()?.owner(), 21) {
        return Err(crate::EPERM);
    }
    let target = resolve_mount_target(target)?;
    let target = namespace_path(&target)?;
    let namespace = namespace_id()?;
    let mut changes = None;
    update(|table, _| {
        if target == "/" && !table.iter().any(|p| p.id == ROOT_MOUNT_ID) {
            table.insert(
                0,
                MountPoint {
                    meta: Default::default(),
                    id: ROOT_MOUNT_ID,
                    parent: 0,
                    source: "/".into(),
                    target: "/".into(),
                    flags: MS_ROOT,
                },
            );
        }
        let id = visible_mount(table, &target)?
            .filter(|p| p.target == target)
            .map(|p| p.id)
            .or_else(|| (target == "/").then_some(ROOT_MOUNT_ID))
            .ok_or(EINVAL)?;
        let point = table.iter_mut().find(|p| p.id == id).ok_or(EIO)?;
        let policy = policy::get_mapped(namespace, id, point.flags, point.meta.idmap.as_ref())?;
        let mut attrs = flags & BIND_ATTRS;
        // Linux preserves atime policy if no atime flags were supplied.
        let atime = 1024 | 2048 | (1 << 21) | (1 << 24);
        if attrs & atime == 0 {
            attrs |= point.flags & atime;
        }
        let updated = (point.flags & !BIND_ATTRS) | attrs;
        changes = Some(policy::Changes::apply(vec![(policy, updated)])?);
        point.flags = updated;
        Ok(())
    })?;
    if let Some(changes) = changes {
        changes.commit();
    }
    Ok(())
}

/// Sets mount propagation flags (e.g. MS_SHARED, MS_PRIVATE) for the target mount.
pub fn set_propagation(target: &str, flags: u64) -> Result<(), i32> {
    if (flags & MS_PROPAGATION).count_ones() != 1 || flags & !(MS_PROPAGATION | MS_REC) != 0 {
        return Err(EINVAL);
    }
    // A retained cwd can be outside the process root after pivot_root. Keep
    // relative lookup in namespace coordinates; getcwd's `(unreachable)` is
    // display text and must never be resolved as a filename.
    let norm = if !target.starts_with('/') && crate::fs::cwd::namespace()?.is_some() {
        crate::fs::stat(target)?;
        namespace_path(target)?
    } else {
        let target = resolve_mount_target(target)?;
        crate::fs::stat(&target)?;
        namespace_path(&target)?
    };
    update(|table, _next_id| {
        if norm == "/" && !table.iter().any(|p| p.id == ROOT_MOUNT_ID) {
            table.push(MountPoint {
                meta: MountMetadata::default(),
                id: ROOT_MOUNT_ID,
                parent: 0,
                source: "/".into(),
                target: "/".into(),
                flags: MS_ROOT,
            });
        }
        let selected = if norm == "/" {
            ROOT_MOUNT_ID
        } else {
            visible_mount(table, &norm)?
                .filter(|p| p.target == norm)
                .ok_or(EINVAL)?
                .id
        };
        let selected = if flags & MS_REC != 0 {
            api::descendants(table, selected)
        } else {
            std::collections::HashSet::from([selected])
        };
        // After a stacked pivot the new-root subtree no longer belongs to the
        // old root. It must not receive old-root propagation changes.
        let excluded = if norm == "/" {
            pivot::new_root_subtree(table)?
        } else {
            Default::default()
        };
        for point in table
            .iter_mut()
            .filter(|p| selected.contains(&p.id) && !excluded.contains(&p.id))
        {
            propagation::change(point, flags & MS_PROPAGATION)?;
        }
        Ok(())
    })
}

/// Publish a validated overlay superblock in this mount namespace.
pub fn overlay(
    target: &str,
    lowerdirs: &[String],
    upperdir: Option<&str>,
    workdir: Option<&str>,
    flags: u64,
) -> Result<(), i32> {
    if lowerdirs.is_empty() && upperdir.is_none() {
        return Err(EINVAL);
    }
    let absolute = |path: &str| {
        if path.is_empty() {
            return Err(EINVAL);
        }
        normalize(&crate::fs::absolute_linux(path))
    };
    let target = absolute(target)?;
    let lowerdirs = lowerdirs
        .iter()
        .map(|path| absolute(path))
        .collect::<Result<Vec<_>, _>>()?;
    let upperdir = upperdir.map(absolute).transpose()?;
    let workdir = workdir.map(absolute).transpose()?;
    let flags = overlay::features::validate(flags, upperdir.is_some())?;
    if has_fixed_virtual_dispatch(&target) {
        return Err(EOPNOTSUPP);
    }
    if crate::fs::stat(&target)?.st_mode & crate::fs::S_IFMT != crate::fs::S_IFDIR {
        return Err(ENOTDIR);
    }
    let source =
        overlay::prepare_mount(&lowerdirs, upperdir.as_deref(), workdir.as_deref(), flags)?;
    let target = namespace_path(&target)?;
    update(|table, next_id| {
        let parent = visible_mount(table, &target)?.map_or(ROOT_MOUNT_ID, |point| point.id);
        table.push(MountPoint {
            meta: MountMetadata::default(),
            id: allocate_id(next_id)?,
            parent,
            source,
            target,
            flags: flags | MS_OVERLAY | if upperdir.is_none() { 1 } else { 0 },
        });
        Ok(())
    })
}

/// Removes the visible mount at target; hidden children remain unreachable.
///
/// Linux uses EINVAL to tell callers such as containerd that repeated umount2
/// has reached the underlying ordinary directory. Returning success when no
/// mount exists makes those callers loop forever.
pub fn unmount(target: &str, flags: i32) -> Result<(), i32> {
    if flags & !(MNT_FORCE | MNT_DETACH | MNT_EXPIRE | UMOUNT_NOFOLLOW) != 0
        || flags & MNT_EXPIRE != 0 && flags & (MNT_FORCE | MNT_DETACH) != 0
    {
        return Err(EINVAL);
    }
    if flags & MNT_EXPIRE != 0 {
        // Expiry requires the Linux two-call unused-mount state machine.
        return Err(EOPNOTSUPP);
    }
    let target = namespace_path(target)?;
    update(|table, _| unmount_in(table, &target, flags))
}

fn unmount_in(table: &mut Vec<MountPoint>, target: &str, flags: i32) -> Result<(), i32> {
    if pivot::detach_old_root(table, target, flags)? {
        return Ok(());
    }
    let selected = visible_mount(table, target)?
        .filter(|point| point.target == target)
        .ok_or(EINVAL)?
        .id;
    if flags & MNT_DETACH == 0 && table.iter().any(|point| point.parent == selected) {
        return Err(crate::EBUSY);
    }
    let mut removed = std::collections::HashSet::from([selected]);
    for point in table.iter() {
        if removed.contains(&point.parent) {
            removed.insert(point.id);
        }
    }
    table.retain(|point| !removed.contains(&point.id));
    Ok(())
}

pub(crate) fn serialize_fork_state() -> Result<Vec<u8>, i32> {
    // Pin the shared object before the child is created. The fork/exec
    // coordinator keeps the parent alive until child restoration completes.
    let store = shared::get()?;
    let mut payload = Vec::with_capacity(24);
    payload.extend_from_slice(&FORK_MAGIC.to_le_bytes());
    payload.extend_from_slice(&kinakaze_runtime::authority::domain_id().to_le_bytes());
    payload.extend_from_slice(&store.id().to_le_bytes());
    Ok(payload)
}

pub(crate) fn restore_fork_state(payload: &[u8]) -> bool {
    if payload.len() != 24
        || u64::from_le_bytes(payload[0..8].try_into().unwrap()) != FORK_MAGIC
        || u64::from_le_bytes(payload[8..16].try_into().unwrap())
            != kinakaze_runtime::authority::domain_id()
    {
        return false;
    }
    shared::restore_membership(u64::from_le_bytes(payload[16..24].try_into().unwrap())).is_ok()
}

#[cfg(test)]
fn encode_table(table: &[MountPoint]) -> Result<Vec<u8>, i32> {
    let next_id = table
        .iter()
        .map(|point| point.id + 1)
        .max()
        .unwrap_or(FIRST_DYNAMIC_MOUNT_ID);
    encode_table_with_next_id(table, next_id)
}

fn encode_table_with_next_id(table: &[MountPoint], next_id: u64) -> Result<Vec<u8>, i32> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&STATE_MAGIC.to_le_bytes());
    payload.extend_from_slice(
        &u32::try_from(table.len())
            .map_err(|_| EINVAL)?
            .to_le_bytes(),
    );
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&next_id.to_le_bytes());
    for mount in table.iter() {
        let source = mount.source.as_bytes();
        let target = mount.target.as_bytes();
        let source_len = u32::try_from(source.len()).map_err(|_| EINVAL)?;
        let target_len = u32::try_from(target.len()).map_err(|_| EINVAL)?;
        payload.extend_from_slice(&mount.id.to_le_bytes());
        payload.extend_from_slice(&mount.parent.to_le_bytes());
        payload.extend_from_slice(&mount.flags.to_le_bytes());
        payload.extend_from_slice(&source_len.to_le_bytes());
        payload.extend_from_slice(&target_len.to_le_bytes());
        payload.extend_from_slice(source);
        payload.extend_from_slice(target);
        let metadata = mount.meta.encode();
        payload.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        payload.extend_from_slice(&metadata);
    }
    Ok(payload)
}

fn decode_table(payload: &[u8]) -> Result<Vec<MountPoint>, i32> {
    decode_table_view(payload, false)
}

/// Detached trees use paths relative to their own root. Their `/` and `/dev`
/// are not attachments over the process's built-in virtual filesystems.
fn decode_table_view(payload: &[u8], detached: bool) -> Result<Vec<MountPoint>, i32> {
    if payload.is_empty() {
        return Ok(Vec::new());
    }
    if payload.len() < 24
        || u64::from_le_bytes(payload[0..8].try_into().unwrap_or_default()) != STATE_MAGIC
        || payload[12..16] != [0; 4]
    {
        return Err(EIO);
    }
    let count = u32::from_le_bytes(payload[8..12].try_into().unwrap_or_default()) as usize;
    let next_id = u64::from_le_bytes(payload[16..24].try_into().unwrap_or_default());
    if !(FIRST_DYNAMIC_MOUNT_ID..=MAX_MOUNT_ID + 1).contains(&next_id) {
        return Err(EIO);
    }
    let mut cursor = 24usize;
    if count > (payload.len() - 24) / 32 {
        return Err(EIO);
    }
    let mut identifiers = std::collections::HashSet::with_capacity(count);
    let mut restored: Vec<MountPoint> = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(header) = payload.get(cursor..cursor.saturating_add(32)) else {
            return Err(EIO);
        };
        let id = u64::from_le_bytes(header[0..8].try_into().unwrap_or_default());
        if (id != ROOT_MOUNT_ID && !(FIRST_DYNAMIC_MOUNT_ID..next_id).contains(&id))
            || !identifiers.insert(id)
        {
            return Err(EIO);
        }
        let parent = u64::from_le_bytes(header[8..16].try_into().unwrap_or_default());
        let flags = u64::from_le_bytes(header[16..24].try_into().unwrap_or_default());
        let source_len = u32::from_le_bytes(header[24..28].try_into().unwrap_or_default()) as usize;
        let target_len = u32::from_le_bytes(header[28..32].try_into().unwrap_or_default()) as usize;
        cursor += 32;
        let Some(source_end) = cursor.checked_add(source_len) else {
            return Err(EIO);
        };
        let Some(target_end) = source_end.checked_add(target_len) else {
            return Err(EIO);
        };
        let (Some(source), Some(target)) = (
            payload.get(cursor..source_end),
            payload.get(source_end..target_end),
        ) else {
            return Err(EIO);
        };
        let (Ok(source), Ok(target)) = (std::str::from_utf8(source), std::str::from_utf8(target))
        else {
            return Err(EIO);
        };
        let is_overlay = flags & MS_OVERLAY != 0;
        let is_proc = flags & MS_PROC != 0;
        let is_tmpfs = flags & MS_TMPFS != 0;
        let is_bind = flags & MS_BIND != 0;
        let is_root = id == ROOT_MOUNT_ID && flags & MS_ROOT != 0;
        if !is_overlay && !is_bind && !is_root && !is_proc && !is_tmpfs {
            return Err(EIO);
        }
        if is_root {
            if source != "/"
                || target != "/"
                || parent != 0
                || flags & !(MS_ROOT | MS_PROPAGATION | BIND_ATTRS) != 0
            {
                return Err(EIO);
            }
        } else if is_tmpfs {
            if is_overlay
                || is_proc
                || crate::tmpfs::parse(source).is_err()
                || normalize(target).ok().as_deref() != Some(target)
            {
                return Err(EIO);
            }
        } else if is_proc {
            if is_overlay
                || crate::procfs::instance::parse(source).is_err()
                || normalize(target).ok().as_deref() != Some(target)
            {
                return Err(EIO);
            }
        } else if is_bind {
            if normalize(source).ok().as_deref() != Some(source)
                || normalize(target).ok().as_deref() != Some(target)
                || flags & !(MS_BIND | MS_REC | MS_PROPAGATION | BIND_ATTRS) != 0
            {
                return Err(EIO);
            }
            if has_fixed_virtual_dispatch(source) || !detached && has_fixed_virtual_dispatch(target)
            {
                return Err(EOPNOTSUPP);
            }
        } else {
            if normalize(target).ok().as_deref() != Some(target)
                || ParsedOverlay::parse(source).is_none()
            {
                return Err(EIO);
            }
        }
        if !is_root
            && parent != ROOT_MOUNT_ID
            && !restored
                .iter()
                .any(|point| point.id == parent && suffix(target, &point.target).is_some())
        {
            return Err(EIO);
        }
        if restored
            .iter()
            .any(|point| point.parent == parent && point.target == target)
        {
            return Err(EIO);
        }
        let length_end = target_end.checked_add(4).ok_or(EIO)?;
        let length = u32::from_le_bytes(
            payload
                .get(target_end..length_end)
                .ok_or(EIO)?
                .try_into()
                .unwrap(),
        ) as usize;
        let meta_end = length_end.checked_add(length).ok_or(EIO)?;
        let meta = MountMetadata::decode(payload.get(length_end..meta_end).ok_or(EIO)?)?;
        restored.push(MountPoint {
            meta,
            id,
            parent,
            source: source.to_owned(),
            target: target.to_owned(),
            flags,
        });
        cursor = meta_end;
    }
    if cursor != payload.len() {
        return Err(EIO);
    }
    // Validate that a pivot declaration and its surviving mount marker agree.
    // A corrupt namespace must never degrade into the host root.
    pivot::namespace_root(&restored)?;
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn public_snapshot_tracks_owned_bind_stack_and_unmount() {
        let _lock = test_lock();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = format!("/kinakaze-snapshot-test-{}-{nonce}", std::process::id());
        let target = format!("{prefix}/target");
        let first = format!("{prefix}/first");
        let second = format!("{prefix}/second");

        struct OwnedMounts {
            target: String,
            sources: Vec<String>,
        }
        impl Drop for OwnedMounts {
            fn drop(&mut self) {
                // Never clear the shared table or pop a mount somebody else
                // placed above this fixture's unique target.
                for source in self.sources.iter().rev() {
                    let owned_top = snapshot_list().ok().is_some_and(|points| {
                        points
                            .iter()
                            .rev()
                            .find(|point| point.target == self.target)
                            .is_some_and(|point| point.source == *source)
                    });
                    if !owned_top || unmount(&self.target, 0).is_err() {
                        break;
                    }
                }
            }
        }
        let own = |points: Vec<MountSnapshotEntry>| {
            points
                .into_iter()
                .filter(|point| point.target == target)
                .collect::<Vec<_>>()
        };
        assert!(own(snapshot_list().unwrap()).is_empty());
        let mut mounts = OwnedMounts {
            target: target.clone(),
            sources: Vec::with_capacity(2),
        };
        bind(&first, &target, MS_BIND).unwrap();
        mounts.sources.push(first.clone());
        let first_snapshot = own(snapshot_list().unwrap());
        assert_eq!(
            first_snapshot,
            vec![MountSnapshotEntry {
                source: first.clone(),
                target: target.clone(),
                flags: MS_BIND,
            }]
        );

        bind(&second, &target, MS_BIND | MS_REC).unwrap();
        mounts.sources.push(second.clone());
        assert_eq!(
            own(snapshot_list().unwrap()),
            vec![
                first_snapshot[0].clone(),
                MountSnapshotEntry {
                    source: second,
                    target: target.clone(),
                    flags: MS_BIND | MS_REC,
                },
            ]
        );
        unmount(&target, 0).unwrap();
        mounts.sources.pop();
        assert_eq!(own(snapshot_list().unwrap()), first_snapshot);
        unmount(&target, 0).unwrap();
        mounts.sources.pop();
        assert!(own(snapshot_list().unwrap()).is_empty());
        // A retained snapshot is immutable even when its mounts are removed.
        assert_eq!(first_snapshot[0].source, first);
    }

    #[test]
    fn bind_uses_longest_prefix_and_unmount_terminates_with_einval() {
        let _lock = test_lock();
        bind("/store/base", "/kinakaze-test-mnt", MS_BIND).unwrap();
        bind("/store/special", "/kinakaze-test-mnt/sub", MS_BIND).unwrap();
        assert_eq!(
            translate("/kinakaze-test-mnt/file").unwrap(),
            "/store/base/file"
        );
        assert_eq!(
            translate("/kinakaze-test-mnt/sub/file").unwrap(),
            "/store/special/file"
        );
        assert_eq!(unmount("/kinakaze-test-mnt/sub", 0), Ok(()));
        assert_eq!(unmount("/kinakaze-test-mnt/sub", 0), Err(EINVAL));
        assert_eq!(
            translate("/kinakaze-test-mnt/sub/file").unwrap(),
            "/store/base/sub/file"
        );
        unmount("/kinakaze-test-mnt", 0).unwrap();
    }

    #[test]
    fn fork_payload_reconnects_without_rolling_back_live_mount_changes() {
        let _lock = test_lock();
        bind("/source", "/kinakaze-test-target", MS_BIND | MS_REC).unwrap();
        let payload = serialize_fork_state().unwrap();
        unmount("/kinakaze-test-target", 0).unwrap();
        assert!(restore_fork_state(&payload));
        assert_eq!(
            translate("/kinakaze-test-target/child").unwrap(),
            "/kinakaze-test-target/child"
        );
    }

    #[test]
    fn fork_payload_cannot_switch_existing_mount_membership() {
        let _lock = test_lock();
        let before = shared::get().unwrap().id();
        let mut payload = serialize_fork_state().unwrap();
        payload[16..24].copy_from_slice(&before.wrapping_add(1).to_le_bytes());
        assert!(!restore_fork_state(&payload));
        assert_eq!(shared::get().unwrap().id(), before);
        assert_eq!(
            kinakaze_runtime::job::mount_namespace(crate::job::process_id()),
            Some(before)
        );
    }

    #[test]
    fn independent_process_helper() {
        let Ok(target) = std::env::var("KINAKAZE_MOUNT_SHARED_TEST") else {
            return;
        };
        assert_eq!(
            translate(&format!("{target}/file")).unwrap(),
            "/published/file"
        );
        unmount(&target, 0).unwrap();
        bind("/child-published", &target, MS_BIND).unwrap();
    }

    #[test]
    fn independently_started_processes_share_mount_changes_and_refresh_caches() {
        use std::os::windows::process::CommandExt;
        let _lock = test_lock();
        let target = format!("/kinakaze-mount-process-test-{}", std::process::id());
        bind("/published", &target, MS_BIND).unwrap();
        assert_eq!(
            translate(&format!("{target}/file")).unwrap(),
            "/published/file"
        );
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mount::tests::independent_process_helper",
                "--nocapture",
            ])
            .env("KINAKAZE_MOUNT_SHARED_TEST", &target)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{} {}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert_eq!(
            translate(&format!("{target}/file")).unwrap(),
            "/child-published/file"
        );
        unmount(&target, 0).unwrap();
    }

    #[test]
    fn corrupt_mount_payloads_are_rejected_before_large_allocations() {
        assert!(decode_table(b"short").is_err());
        let mut bytes = encode_table(&[]).unwrap();
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode_table(&bytes).is_err());
        assert!(!restore_fork_state(&bytes));
        let mut fork = serialize_fork_state().unwrap();
        fork[12] = 1;
        assert!(!restore_fork_state(&fork));
    }

    #[test]
    fn persisted_pivot_root_requires_one_matching_mount_marker() {
        let pivot_id = FIRST_DYNAMIC_MOUNT_ID;
        let mut table = vec![
            MountPoint {
                meta: MountMetadata {
                    pivot: pivot_id,
                    ..Default::default()
                },
                id: ROOT_MOUNT_ID,
                parent: 0,
                source: "/".into(),
                target: "/".into(),
                flags: MS_ROOT,
            },
            MountPoint {
                meta: MountMetadata {
                    pivot: pivot_id,
                    ..Default::default()
                },
                id: pivot_id,
                parent: ROOT_MOUNT_ID,
                source: "overlay:lowerdir=/lower".into(),
                target: "/container-root".into(),
                flags: MS_OVERLAY | 1,
            },
        ];
        let restored = decode_table(&encode_table(&table).unwrap()).unwrap();
        assert_eq!(
            pivot::namespace_root(&restored)
                .unwrap()
                .map(|point| point.target.as_str()),
            Some("/container-root")
        );

        table[1].meta.pivot = 0;
        assert_eq!(decode_table(&encode_table(&table).unwrap()), Err(EIO));
        table[1].meta.pivot = pivot_id + 1;
        assert_eq!(decode_table(&encode_table(&table).unwrap()), Err(EIO));
    }

    #[test]
    fn invalid_overlay_mount_does_not_publish_an_alias() {
        let _lock = test_lock();
        let target = "/kinakaze-test-ovl-target";
        let lower = vec!["/store/lower1".to_string(), "/store/lower2".to_string()];
        let before = snapshot().unwrap();
        assert_eq!(
            overlay(target, &lower, Some("/store/upper"), Some("/store/work"), 0),
            Err(crate::ENOENT)
        );
        assert_eq!(snapshot().unwrap(), before);
    }

    #[test]
    fn restored_overlay_never_forwards_operations_to_a_lower_file() {
        let table = vec![MountPoint {
            meta: MountMetadata::default(),
            id: FIRST_DYNAMIC_MOUNT_ID,
            parent: ROOT_MOUNT_ID,
            source: "overlay:lowerdir=/store/lower;upperdir=/store/upper;workdir=/store/work"
                .into(),
            target: "/merged".into(),
            flags: MS_OVERLAY,
        }];
        let decoded = decode_table(&encode_table(&table).unwrap()).unwrap();
        for path in ["/merged", "/merged/file", "/merged/unseen/child"] {
            assert_eq!(translate_in(&decoded, path), Err(EOPNOTSUPP));
        }
        assert_eq!(
            translate_in(&decoded, "/elsewhere"),
            Ok("/elsewhere".into())
        );
    }
}
