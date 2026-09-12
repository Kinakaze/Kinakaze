//! Native bind descriptors retain their attachment policy across dup/fork/exec.
use super::*;
use crate::{FdEntry, FdFlags, fs::object::Object};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

#[derive(Clone)]
pub(crate) struct Description {
    pub policy: Arc<policy::Policy>,
    pub path: String,
    pub writer: Option<Arc<Object>>,
}
impl std::fmt::Debug for Description {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeMountDescription")
            .field("mount", &self.policy.id)
            .finish_non_exhaustive()
    }
}
fn descriptions() -> &'static Mutex<HashMap<u64, Description>> {
    static STATE: OnceLock<Mutex<HashMap<u64, Description>>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}
pub(crate) fn reference(entry: FdEntry) -> Result<Option<Description>, i32> {
    // Native mount descriptions are attached only to filesystem handles.  The
    // description id namespace is restored together with fork/exec fd state,
    // so reject unrelated descriptor kinds before consulting process-local
    // mount state and accidentally matching an older id.
    if !matches!(entry.kind, crate::FdKind::File | crate::FdKind::Directory) {
        return Ok(None);
    }
    Ok(descriptions()
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .cloned())
}

pub(crate) fn export_rights_reference(
    description: Option<Description>,
    mut duplicate: impl FnMut(u64) -> Result<u64, i32>,
) -> Result<Vec<u8>, i32> {
    let Some(d) = description else {
        return Ok(Vec::new());
    };
    let mut bytes = Vec::new();
    for value in [
        d.policy.namespace,
        d.policy.id,
        d.policy.flags(),
        match &d.writer {
            Some(w) => duplicate(w.raw() as u64)?,
            None => 0,
        },
    ] {
        crate::state_codec::word(&mut bytes, value);
    }
    crate::state_codec::bytes(&mut bytes, d.path.as_bytes());
    Ok(bytes)
}
pub(crate) fn import_rights(
    entry: FdEntry,
    bytes: &[u8],
    mut duplicate: impl FnMut(u64) -> Result<Object, i32>,
) -> Result<(), i32> {
    if bytes.is_empty() {
        return Ok(());
    }
    let mut r = crate::state_codec::Reader(bytes);
    let policy = policy::get(r.word()?, r.word()?, r.word()?)?;
    let raw = r.word()?;
    let writer = if raw == 0 {
        None
    } else {
        Some(Arc::new(duplicate(raw)?))
    };
    let path = r.text()?;
    r.end()?;
    if !path.starts_with('/') {
        return Err(EIO);
    }
    register(
        entry,
        Description {
            policy,
            path,
            writer,
        },
    )
}
pub(crate) fn path_policy(path: &str, follow: bool) -> Result<Option<Arc<policy::Policy>>, i32> {
    if crate::tmpfs::owns(path) || crate::procfs::owns(path) {
        return Ok(None);
    }
    let canonical = overlay::canonical_guest(path, follow)?;
    policy_at_canonical(&canonical)
}

fn policy_at_canonical(canonical: &str) -> Result<Option<Arc<policy::Policy>>, i32> {
    let path = namespace_path(canonical)?;
    let table = snapshot()?;
    let point = visible_mount(&table, &path)?;
    if point.is_some_and(|p| p.flags & (MS_OVERLAY | MS_TMPFS | MS_PROC) != 0) {
        return Ok(None);
    }
    let (id, flags) = point.map(|p| (p.id, p.flags)).unwrap_or_else(|| {
        (
            ROOT_MOUNT_ID,
            table
                .iter()
                .find(|p| p.id == ROOT_MOUNT_ID)
                .map_or(0, |p| p.flags),
        )
    });
    policy::get(namespace_id()?, id, flags).map(Some)
}
pub(crate) fn prepare_open(path: &str, flags: i32) -> Result<Option<Description>, i32> {
    if crate::tmpfs::owns(path) || crate::procfs::owns(path) {
        return Ok(None);
    }
    // Policy and descriptor identity describe the same resolved guest name.
    // Reuse it only in this operation; a later open resolves rename/symlink or
    // mount changes again rather than consulting a persistent path cache.
    let canonical = overlay::canonical_guest(path, flags & crate::fs::O_NOFOLLOW == 0)?;
    let Some(policy) = policy_at_canonical(&canonical)? else {
        return Ok(None);
    };
    let create = flags & crate::fs::O_CREAT != 0 && crate::fs::stat(path).is_err();
    let write = flags & crate::fs::O_PATH == 0
        && (flags & (crate::fs::O_ACCMODE | crate::fs::O_TRUNC) != 0 || create);
    let writer = if write { Some(policy.writer()?) } else { None };
    Ok(Some(Description {
        policy,
        writer,
        path: namespace_path(&canonical)?,
    }))
}
pub(crate) fn write_path(path: &str, follow: bool) -> Result<Option<Arc<Object>>, i32> {
    path_policy(path, follow)?.map(|p| p.writer()).transpose()
}
pub(crate) fn namespace_for_native(
    entry: FdEntry,
    native: &std::path::Path,
) -> Result<Option<String>, i32> {
    let Some(description) = reference(entry)? else {
        return Ok(None);
    };
    if description.policy.id == ROOT_MOUNT_ID {
        return Ok(Some(
            crate::path::to_namespace_path(native).unwrap_or_else(|| crate::to_guest_path(native)),
        ));
    }
    if description.policy.namespace == namespace_id()? {
        if let Some(point) = snapshot()?.iter().find(|p| p.id == description.policy.id) {
            if point.flags & MS_BIND != 0 && point.flags & (MS_OVERLAY | MS_TMPFS | MS_PROC) == 0 {
                let backing = crate::path::resolve_bound_mount_source(
                    &crate::path::default_system_root(),
                    &point.source,
                )
                .map_err(|_| EIO)?;
                let backing = backing.canonicalize().unwrap_or(backing);
                if let Ok(tail) = native.strip_prefix(&backing) {
                    return Ok(Some(join(
                        &point.target,
                        &crate::path::unescape_path(&tail.to_string_lossy().replace('\\', "/")),
                    )));
                }
            }
        }
    }
    Ok(Some(description.path))
}
pub fn descriptor_path(fd: i32) -> Result<Option<String>, i32> {
    let (object, entry) = match Object::from_fd_with_entry(fd) {
        Ok(value) => value,
        Err(crate::EOPNOTSUPP) => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(path) = namespace_for_native(entry, &object.path()?)? else {
        return Ok(None);
    };
    if let Some(root) = crate::path::namespace_root_path()? {
        if root == "/" {
            return Ok(Some(path));
        }
        if path == root {
            return Ok(Some("/".into()));
        }
        if let Some(tail) = path.strip_prefix(&root).filter(|s| s.starts_with('/')) {
            return Ok(Some(tail.into()));
        }
        return Ok(None);
    }
    Ok(Some(path))
}
pub fn flags_fd(fd: i32) -> Result<Option<u64>, i32> {
    Ok(reference(crate::get(fd)?)?.map(|d| d.policy.flags()))
}
pub fn flags_path(path: &str) -> Result<Option<u64>, i32> {
    Ok(path_policy(path, true)?.map(|p| p.flags()))
}
pub(crate) fn register(entry: FdEntry, mut description: Description) -> Result<(), i32> {
    if !entry.flags.contains(FdFlags::WRITE_ACCESS) {
        description.writer = None;
    }
    if let Some(writer) = &description.writer {
        crate::platform::try_set_inheritable(writer.raw() as usize, true)?;
    }
    descriptions()
        .lock()
        .map_err(|_| EIO)?
        .insert(entry.description_id, description);
    Ok(())
}
pub(crate) fn closed(id: u64) {
    if let Ok(mut state) = descriptions().lock() {
        state.remove(&id);
    }
}
pub(crate) fn auxiliary_handles() -> Result<Vec<(u64, Arc<Object>)>, i32> {
    Ok(descriptions()
        .lock()
        .map_err(|_| EIO)?
        .iter()
        .filter_map(|(id, d)| d.writer.clone().map(|w| (*id, w)))
        .collect())
}
pub(crate) fn serialize(keep: impl Fn(i32) -> bool) -> Result<Vec<u8>, i32> {
    let ids: HashSet<_> = crate::table()
        .read()
        .map_err(|_| EIO)?
        .slots
        .enumerated()
        .filter(|(fd, _)| keep(*fd as i32))
        .filter_map(|(_, e)| e.map(|e| e.description_id))
        .collect();
    let mut bytes = Vec::new();
    for (id, d) in descriptions()
        .lock()
        .map_err(|_| EIO)?
        .iter()
        .filter(|(id, _)| ids.contains(id))
    {
        for value in [
            *id,
            d.policy.namespace,
            d.policy.id,
            d.policy.flags(),
            d.writer.as_ref().map_or(0, |w| w.raw() as u64),
        ] {
            crate::state_codec::word(&mut bytes, value);
        }
        crate::state_codec::bytes(&mut bytes, d.path.as_bytes());
    }
    Ok(bytes)
}
pub(crate) fn restore(bytes: &[u8]) -> bool {
    let result = || -> Result<(), i32> {
        let mut input = crate::state_codec::Reader(bytes);
        let mut restored = HashMap::new();
        let mut inherited = HashSet::new();
        while !input.0.is_empty() {
            let id = input.word()?;
            let policy = policy::get(input.word()?, input.word()?, input.word()?)?;
            let raw = input.word()?;
            let writer = if raw == 0 {
                None
            } else {
                let object = Object::duplicate(raw as _)?;
                crate::platform::try_set_inheritable(object.raw() as usize, true)?;
                inherited.insert(raw);
                Some(Arc::new(object))
            };
            let path = input.text()?;
            if !path.starts_with('/') {
                return Err(EIO);
            }
            restored.insert(
                id,
                Description {
                    policy,
                    writer,
                    path,
                },
            );
        }
        let mut state = descriptions().lock().map_err(|_| EIO)?;
        let owned: HashSet<_> = state
            .values()
            .filter_map(|d| d.writer.as_ref().map(|w| w.raw() as u64))
            .collect();
        *state = restored;
        for raw in inherited.difference(&owned) {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(*raw as _);
            }
        }
        Ok(())
    };
    result().is_ok()
}
