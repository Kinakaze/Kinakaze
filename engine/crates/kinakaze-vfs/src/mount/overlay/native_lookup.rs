//! Optimistic lookup of a path that crosses no mount attachments. A successful
//! native lookup already proves that every ancestor is a native directory:
//! hosted Linux symlinks are regular-file placeholders, so Windows cannot
//! traverse one. Such paths fall back to the full Linux component walker.
use std::path::{Path, PathBuf};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
};
mod attributes;

pub(super) struct Lookup {
    pub path: PathBuf,
    pub guest: String,
    // An observation from this lookup only; never retained across syscalls.
    pub missing: bool,
    // A completed stat in this operation, never a pathname metadata cache.
    pub stat: Option<crate::fs::Stat>,
}

pub(super) fn resolve(
    root: &Path,
    namespace_root: &str,
    absolute: &str,
    follow: bool,
) -> Result<Option<Lookup>, i32> {
    resolve_inner(root, namespace_root, absolute, follow, false)
}

pub(super) fn resolve_stat(
    root: &Path,
    namespace_root: &str,
    absolute: &str,
    follow: bool,
) -> Result<Option<Lookup>, i32> {
    resolve_inner(root, namespace_root, absolute, follow, true)
}

fn resolve_inner(
    root: &Path,
    namespace_root: &str,
    absolute: &str,
    follow: bool,
    observe_stat: bool,
) -> Result<Option<Lookup>, i32> {
    // `..` is evaluated after links, and a trailing slash changes leaf rules.
    // Leave both to the existing walker, including its shared forty-link limit.
    if !absolute.starts_with('/')
        || absolute.ends_with('/')
        || absolute.split('/').any(|p| p == "." || p == "..")
    {
        return Ok(None);
    }
    let mut guest = namespace_root.trim_end_matches('/').to_owned();
    let mut native = root.to_path_buf();
    for part in namespace_root.split('/').filter(|s| !s.is_empty()) {
        native.push(crate::path::escape_component(part).as_ref());
    }
    for part in absolute.split('/').filter(|s| !s.is_empty()) {
        guest.push('/');
        guest.push_str(part);
        native.push(crate::path::escape_component(part).as_ref());
    }
    if crate::mount::has_attachment(&guest)? {
        return Ok(None);
    }
    let mut stat = None;
    let attributes = attributes::query(&native);
    let missing = attributes.is_err();
    if let Err(status) = attributes {
        if status != 0xc000_0034 {
            // STATUS_OBJECT_NAME_NOT_FOUND
            return Ok(None);
        }
        // Validate the existing parent too: never treat a missing name below
        // a hosted link as a new file in the unresolved native path.
        let Some(parent) = native.parent() else {
            return Ok(None);
        };
        if !attributes::query(parent).is_ok_and(|a| {
            a & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                == FILE_ATTRIBUTE_DIRECTORY
        }) {
            return Ok(None);
        }
        // A missing leaf below a verified native directory is resolved too.
        // dpkg probes several not-yet-existing destination/backup names per
        // file; falling back here walked every ancestor for each ENOENT.
        // As in the full native walker, the actual operation decides whether
        // absence is allowed (including synthetic configuration-file opens).
    } else {
        let attributes = attributes.unwrap();
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Ok(None);
        }
        if follow && attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            if observe_stat {
                // Combine hosted-link detection and stat instead of opening
                // and querying the same inode twice. Both the native reparse
                // case and the persistent hosted-link EA are checked from
                // this owned inode, even if the name was replaced.
                match crate::fs::stat_native_query(&native) {
                    Ok(observed) if observed.st_mode & crate::fs::S_IFMT != crate::fs::S_IFLNK => {
                        stat = Some(observed);
                    }
                    Ok(_) => return Ok(None),
                    Err(crate::ENOENT) => {} // Preserve the ordinary final-open race.
                    Err(error) => return Err(error),
                }
            } else if crate::fs::inode::read_path(&native)?.symlink.is_some() {
                return Ok(None);
            }
        }
    }
    Ok(Some(Lookup {
        path: native,
        guest,
        missing,
        stat,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stat_observations_expire_and_follow_replaced_links() {
        use crate::fs::{S_IFREG, inode, object::Object};
        use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_WRITE_EA};
        let root = std::env::temp_dir().join(format!(
            "kinakaze-native-stat-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("jail")).unwrap();
        let leaf = root.join("jail/file");
        std::fs::write(&leaf, b"original").unwrap();
        let object = Object::open(&leaf, FILE_READ_ATTRIBUTES | FILE_WRITE_EA).unwrap();
        let mut record = inode::Record {
            mode: Some(S_IFREG | 0o600),
            uid: Some(123),
            ..Default::default()
        };
        inode::replace(object.raw(), &record).unwrap();
        let first = resolve_stat(&root, "/jail", "/file", true)
            .unwrap()
            .unwrap()
            .stat
            .unwrap();
        assert_eq!(
            (first.st_size, first.st_mode, first.st_uid),
            (8, S_IFREG | 0o600, 123)
        );
        record.mode = Some(S_IFREG | 0o644);
        record.uid = Some(456);
        inode::replace(object.raw(), &record).unwrap();
        std::fs::write(&leaf, b"later content").unwrap();
        let later = resolve_stat(&root, "/jail", "/file", true)
            .unwrap()
            .unwrap()
            .stat
            .unwrap();
        assert_eq!(
            (later.st_size, later.st_mode, later.st_uid),
            (13, S_IFREG | 0o644, 456)
        );
        assert_eq!(first.st_size, 8);
        std::fs::rename(&leaf, leaf.with_extension("old")).unwrap();
        crate::create_emulated_symlink(&leaf, "file.old").unwrap();
        assert!(
            resolve_stat(&root, "/jail", "/file", true)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve_stat(&root, "/jail", "/file", false)
                .unwrap()
                .unwrap()
                .stat
                .is_none()
        );
        std::fs::remove_file(&leaf).unwrap();
        std::fs::write(&leaf, b"replacement").unwrap();
        let replacement = resolve_stat(&root, "/jail", "/file", true)
            .unwrap()
            .unwrap()
            .stat
            .unwrap();
        assert_ne!(replacement.st_ino, first.st_ino);
        assert_eq!(replacement.st_size, 11);
        assert!(
            resolve_stat(&root, "/jail", "/missing", true)
                .unwrap()
                .unwrap()
                .missing
        );
        drop(object);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lookup_observes_links_missing_parents_chroot_and_replacement() {
        let root = std::env::temp_dir().join(format!(
            "kinakaze-native-lookup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("jail/deep/child")).unwrap();
        let original = root.join("jail/deep/child/file");
        std::fs::write(&original, b"original").unwrap();
        let found = resolve(&root, "/jail", "/deep/child/file", true)
            .unwrap()
            .unwrap();
        assert_eq!(found.path, original);
        assert_eq!(found.guest, "/jail/deep/child/file");
        assert!(!found.missing);
        crate::create_emulated_symlink(&root.join("jail/link"), "deep/child").unwrap();
        assert!(
            resolve(&root, "/jail", "/link/file", true)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve(&root, "/jail", "/link/new", true)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve(&root, "/jail", "/missing/new", true)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve(&root, "/jail", "/deep/child/new", true)
                .unwrap()
                .is_some()
        );
        assert!(
            resolve(&root, "/jail", "/deep/child/new", false)
                .unwrap()
                .is_some()
        );
        assert!(
            resolve(&root, "/jail", "/deep/../child/file", true)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve(&root, "/jail", "/deep/child/file/", true)
                .unwrap()
                .is_none()
        );
        std::fs::rename(&original, original.with_extension("old")).unwrap();
        crate::create_emulated_symlink(&original, "file.old").unwrap();
        assert!(
            resolve(&root, "/jail", "/deep/child/file", true)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve(&root, "/jail", "/deep/child/file", false)
                .unwrap()
                .is_some()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
