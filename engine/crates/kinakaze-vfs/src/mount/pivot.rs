//! Stacked pivot used by runc: retain stable namespace coordinates while the
//! old-root attachment is detached independently of the new-root subtree.
use super::*;
use std::collections::HashSet;

pub fn pivot_root(new_root: &str, put_old: &str) -> Result<(), i32> {
    if !crate::user_namespace::capable(shared::get()?.owner(), 21) {
        return Err(crate::EPERM);
    }
    if new_root.is_empty() || put_old.is_empty() {
        return Err(crate::ENOENT);
    }
    let new_guest = crate::fs::absolute_linux(new_root);
    let old_guest = crate::fs::absolute_linux(put_old);
    for path in [&new_guest, &old_guest] {
        if crate::fs::stat(path)?.st_mode & crate::fs::S_IFMT != crate::fs::S_IFDIR {
            return Err(ENOTDIR);
        }
    }
    let new_path = namespace_path(&new_guest)?;
    let old_path = namespace_path(&old_guest)?;
    if suffix(&old_path, &new_path).is_none() {
        return Err(EINVAL);
    }
    // Separate put_old needs relocation of the old tree. Do not report success
    // until that topology is implemented; the stacked Linux form is supported.
    if old_path != new_path {
        return Err(EOPNOTSUPP);
    }
    if crate::path::namespace_root_path()?.is_some_and(|p| p != "/") {
        return Err(EINVAL);
    }
    let check = |table: &[MountPoint]| -> Result<u64, i32> {
        let point = visible_mount(table, &new_path)?
            .filter(|p| p.target == new_path)
            .ok_or(EINVAL)?;
        if new_path == "/" || point.flags & MS_SHARED != 0 {
            return Err(EINVAL);
        }
        if point.flags & (MS_PROC | MS_TMPFS) != 0 {
            return Err(EOPNOTSUPP);
        }
        if table.iter().any(|p| {
            (p.id == point.parent || p.id == ROOT_MOUNT_ID)
                && (p.flags & MS_SHARED != 0 || p.meta.pivot != 0)
        }) {
            return Err(EINVAL);
        }
        Ok(point.id)
    };
    let id = check(&snapshot()?)?;
    let saved = crate::fs_context::read(Clone::clone);
    let result = (|| {
        // Include both directory changes in rollback: a signal may interrupt
        // the chroot lookup after chdir has already retained a new cwd object.
        crate::fs::chdir(&crate::fs::getcwd())?;
        crate::fs::chroot(&new_guest)?;
        update(|table, _| {
            if check(table)? != id {
                return Err(crate::EAGAIN);
            }
            if let Some(root) = table.iter_mut().find(|p| p.id == ROOT_MOUNT_ID) {
                root.meta.pivot = id;
            } else {
                table.insert(
                    0,
                    MountPoint {
                        meta: MountMetadata {
                            pivot: id,
                            ..Default::default()
                        },
                        id: ROOT_MOUNT_ID,
                        parent: 0,
                        source: "/".into(),
                        target: "/".into(),
                        flags: MS_ROOT,
                    },
                );
            }
            table
                .iter_mut()
                .find(|point| point.id == id)
                .ok_or(EIO)?
                .meta
                .pivot = id;
            Ok(())
        })
    })();
    if result.is_err() {
        crate::fs_context::update(|s| *s = saved);
    }
    result
}

/// Finds the unique mount that permanently serves as `/` after pivot_root.
/// Before old-root detach both the root declaration and the new-root mount are
/// present; after detach the self marker on the surviving mount is authoritative.
pub(super) fn namespace_root(table: &[MountPoint]) -> Result<Option<&MountPoint>, i32> {
    let root = table.iter().find(|point| point.id == ROOT_MOUNT_ID);
    let declared = root.map(|point| point.meta.pivot).filter(|id| *id != 0);
    let mut marked = table
        .iter()
        .filter(|point| point.id != ROOT_MOUNT_ID && point.meta.pivot == point.id);
    let marker = marked.next();
    if marked.next().is_some() {
        return Err(EIO);
    }
    if table.iter().any(|point| {
        point.meta.pivot != 0 && point.id != ROOT_MOUNT_ID && point.meta.pivot != point.id
    }) {
        return Err(EIO);
    }
    match (root, declared, marker) {
        (Some(_), None, None) | (None, None, None) => Ok(None),
        (Some(_), Some(id), Some(point)) if id == point.id => Ok(Some(point)),
        (None, None, Some(point)) => Ok(Some(point)),
        _ => Err(EIO),
    }
}

pub(super) fn new_root_subtree(table: &[MountPoint]) -> Result<HashSet<u64>, i32> {
    Ok(namespace_root(table)?
        .map(|point| api::descendants(table, point.id))
        .unwrap_or_default())
}

pub(super) fn detach_old_root(
    table: &mut Vec<MountPoint>,
    target: &str,
    flags: i32,
) -> Result<bool, i32> {
    if target != "/" {
        return Ok(false);
    }
    let keep = new_root_subtree(table)?;
    if keep.is_empty() {
        return Ok(false);
    }
    if flags & MNT_DETACH == 0 {
        return Err(crate::EBUSY);
    }
    let root_id = namespace_root(table)?.ok_or(EIO)?.id;
    table.retain(|p| keep.contains(&p.id));
    table
        .iter_mut()
        .find(|p| p.id == root_id)
        .ok_or(EIO)?
        .parent = ROOT_MOUNT_ID;
    Ok(true)
}
