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
}

pub(super) fn resolve(
    root: &Path,
    namespace_root: &str,
    absolute: &str,
    follow: bool,
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
        if follow
            && attributes & FILE_ATTRIBUTE_DIRECTORY == 0
            && crate::fs::inode::read_path(&native)?.symlink.is_some()
        {
            return Ok(None);
        }
    }
    Ok(Some(Lookup {
        path: native,
        guest,
        missing,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
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
