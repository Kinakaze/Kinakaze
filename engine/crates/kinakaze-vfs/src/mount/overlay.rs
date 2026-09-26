//! Component-level OverlayFS lookup and directory merging over real layers.
//!
//! The mounted backend connects these primitives to namespace lookup, inode
//! copy-up, whiteout transactions and inherited file descriptions. Layer roots
//! and opened metadata objects are native inode pins, not directory aliases.
//!
//! Semantics: Documentation/filesystems/overlayfs.rst and fs/overlayfs/namei.c,
//! util.c, readdir.c in the Linux source. Layer order is highest priority first.

use crate::fs::object::Object;
use std::collections::HashSet;
use std::ffi::OsStr;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_EA};

#[cfg(test)]
use crate::fs::S_IFLNK;
use crate::fs::{S_IFCHR, S_IFDIR, S_IFMT, S_IFREG, Stat};
use crate::xattr::Attributes;
use crate::{EINVAL, EIO, ENAMETOOLONG, ENOENT, ENOTDIR, EOPNOTSUPP};

pub(crate) mod copy_up;
mod directory;
pub mod features;
mod identity;
mod index;
mod lookup;
mod metacopy;
mod mounted;
mod options;
mod profile;
pub use directory::DirectoryRecord;
pub use mounted::check_execute;
pub use mounted::metadata_path;
pub use mounted::rename_with_flags;
pub use mounted::sync_descriptor;
pub use mounted::{MetadataHandle, chmod_descriptor, metadata_handle};
pub use mounted::{
    WritePath, descriptor_path, descriptor_write_path, hard_link, prepare_create, prepare_write,
};
pub use mounted::{descriptor_filesystem, filesystem_path, is_overlay_path};
pub use mounted::{directory_records, read_directory_bytes};
pub use mounted::{encode_handle, encode_handle_fd, open_handle};
pub use options::{Options, parse_options};
pub const USER_XATTR_FLAG: u64 = mounted::USER_XATTR;
pub(crate) use mounted::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum XattrNamespace {
    Trusted,
    User,
}

impl XattrNamespace {
    fn prefix(self) -> &'static str {
        match self {
            Self::Trusted => "trusted.overlay.",
            Self::User => "user.overlay.",
        }
    }
}

#[derive(Clone, Debug)]
struct Backing {
    layer: usize,
    object: Arc<Object>,
    metadata: Stat,
    whiteout: bool,
    opaque: bool,
    requires_redirect_or_metacopy: bool,
    redirect: Option<String>,
    metacopy: Option<Vec<u8>>,
    indexed: bool,
}

impl Backing {
    #[cfg(test)]
    fn open(path: PathBuf, namespace: XattrNamespace, layer: usize) -> Result<Self, i32> {
        Self::from_object(
            Object::open(&path, FILE_READ_ATTRIBUTES | FILE_READ_EA)?,
            namespace,
            layer,
        )
    }

    fn from_object(object: Object, namespace: XattrNamespace, layer: usize) -> Result<Self, i32> {
        // Open the entry itself, not a symlink's target. Metadata and EAs refer
        // to this same inode; symlink expansion belongs to the guest path walk.
        let handle = unsafe { Attributes::from_handle(object.raw(), false)? };
        let metadata = handle.metadata()?;
        let attributes = handle.snapshot()?;
        let attr = |suffix: &str| {
            attributes
                .get(format!("{}{suffix}", namespace.prefix()).as_bytes())
                .map(Vec::as_slice)
        };
        let kind = metadata.st_mode & S_IFMT;
        Ok(Self {
            layer,
            object: Arc::new(object),
            metadata,
            whiteout: (kind == S_IFCHR && metadata.st_rdev == 0)
                || (kind == S_IFREG && metadata.st_size == 0 && attr("whiteout").is_some()),
            opaque: kind == S_IFDIR && attr("opaque") == Some(b"y"),
            requires_redirect_or_metacopy: attr("redirect").is_some() || attr("metacopy").is_some(),
            redirect: attr("redirect")
                .map(|v| std::str::from_utf8(v).map(str::to_owned).map_err(|_| EIO))
                .transpose()?,
            metacopy: attr("metacopy").map(Vec::from),
            indexed: false,
        })
    }

    fn is_directory(&self) -> bool {
        self.metadata.st_mode & S_IFMT == S_IFDIR
    }
}

/// A non-directory has one backing object; a directory can merge several.
/// Stored metadata is underlying metadata, not yet an overlay stat result.
#[derive(Clone, Debug)]
pub(crate) struct Node {
    entries: Vec<Backing>,
    namespace: XattrNamespace,
    context: Option<Arc<LookupContext>>,
}

#[derive(Debug)]
struct LookupContext {
    roots: Vec<Backing>,
    flags: u64,
    writable: bool,
    volumes: Vec<[u8; 16]>,
    index: Option<Arc<index::Index>>,
}

#[derive(Clone, Debug)]
pub(crate) struct DirectoryItem {
    pub name: String,
    pub mode: u32,
}

impl Node {
    fn from_objects(objects: Vec<Object>, namespace: XattrNamespace) -> Result<Self, i32> {
        let entries = objects
            .into_iter()
            .enumerate()
            .map(|(layer, object)| {
                let entry = Backing::from_object(object, namespace, layer)?;
                if !entry.is_directory() {
                    return Err(ENOTDIR);
                }
                Ok(entry)
            })
            .collect::<Result<Vec<_>, i32>>()?;
        if entries.is_empty() {
            return Err(EINVAL);
        }
        Ok(Self {
            entries,
            namespace,
            context: None,
        })
    }
    #[cfg(test)]
    /// Assemble the mount root from already resolved, ordered layer roots.
    /// Unlike a looked-up subdirectory, the overlay root merges all configured
    /// roots even if a root itself carries an opaque xattr (ovl_get_root).
    pub(crate) fn root(
        paths: impl IntoIterator<Item = PathBuf>,
        namespace: XattrNamespace,
    ) -> Result<Self, i32> {
        let mut entries = Vec::new();
        for (layer, path) in paths.into_iter().enumerate() {
            let entry = Backing::open(path, namespace, layer)?;
            if !entry.is_directory() {
                return Err(ENOTDIR);
            }
            entries.push(entry);
        }
        if entries.is_empty() {
            return Err(EINVAL);
        }
        Ok(Self {
            entries,
            namespace,
            context: None,
        })
    }

    fn configure(mut self, flags: u64, work: Option<&Object>) -> Result<Self, i32> {
        let flags = features::validate(flags, work.is_some())?;
        let visible = self
            .entries
            .len()
            .checked_sub(features::data_count(flags))
            .ok_or(EINVAL)?;
        if visible <= usize::from(work.is_some()) {
            return Err(EINVAL);
        }
        let mut volumes = Vec::new();
        for root in &self.entries {
            let uuid = match identity::volume_uuid(&root.object) {
                Ok(uuid) => uuid,
                Err(e)
                    if flags & (features::INDEX | features::NFS_EXPORT | features::XINO) != 0 =>
                {
                    return Err(e);
                }
                Err(_) => [0; 16],
            };
            volumes.push(uuid);
        }
        self.context = Some(Arc::new(LookupContext {
            roots: self.entries.clone(),
            flags,
            writable: work.is_some(),
            volumes,
            index: None,
        }));
        self.entries.truncate(visible);
        if flags & features::INDEX != 0
            && let Some(work) = work
        {
            let index = Arc::new(index::Index::open(&self, work)?);
            Arc::get_mut(self.context.as_mut().unwrap())
                .ok_or(EIO)?
                .index = Some(index);
        }
        Ok(self)
    }

    fn flags(&self) -> u64 {
        self.context.as_ref().map_or(0, |c| c.flags)
    }
    fn is_metacopy(&self) -> bool {
        self.entries[0].metacopy.is_some()
    }
    fn data_object(&self) -> &Object {
        if self.is_metacopy() {
            &self.entries.last().unwrap().object
        } else {
            self.backing_object()
        }
    }

    /// A different process can finish copying the data into our pinned inode.
    /// Keep the inode pin, but do not keep using its obsolete metacopy marker.
    fn refreshed(&self) -> Result<Self, i32> {
        if !self.is_metacopy() {
            return Ok(self.clone());
        }
        let mut entries = Vec::new();
        for entry in &self.entries {
            let object = Object::reopen(entry.object.raw(), FILE_READ_ATTRIBUTES | FILE_READ_EA)?;
            let mut fresh = Backing::from_object(object, self.namespace, entry.layer)?;
            fresh.indexed = entry.indexed;
            let last = fresh.metacopy.is_none();
            entries.push(fresh);
            if last {
                break;
            }
        }
        Ok(Self {
            entries,
            namespace: self.namespace,
            context: self.context.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn backing_path(&self) -> PathBuf {
        self.backing_object()
            .path()
            .expect("test fixture inode has a pathname")
    }

    pub(crate) fn backing_object(&self) -> &Object {
        &self.entries[0].object
    }

    pub(crate) fn backing_layer(&self) -> usize {
        self.entries[0].layer
    }

    pub(crate) fn backing_metadata(&self) -> &Stat {
        &self.entries[0].metadata
    }

    pub(crate) fn is_directory(&self) -> bool {
        self.entries[0].is_directory()
    }

    /// Look up exactly one guest component, retaining symlinks as objects.
    /// A whiteout or a non-directory at any level cuts off all deeper layers,
    /// including when an earlier level has already contributed a directory.
    pub(crate) fn child(&self, name: &str) -> Result<Self, i32> {
        if self.context.is_some() {
            return self.feature_child(name, 0);
        }
        validate_component(name)?;
        if !self.is_directory() {
            return Err(ENOTDIR);
        }
        let mut entries = Vec::new();
        let stored = crate::path::escape_component(name);
        for parent in &self.entries {
            let entry = match parent
                .object
                .child(
                    OsStr::new(stored.as_ref()),
                    FILE_READ_ATTRIBUTES | FILE_READ_EA,
                )
                .and_then(|object| Backing::from_object(object, self.namespace, parent.layer))
            {
                Ok(entry) => entry,
                Err(ENOENT) => continue,
                Err(error) => return Err(error),
            };
            if entry.whiteout {
                break;
            }
            if !entries.is_empty() && !entry.is_directory() {
                break;
            }
            if entry.requires_redirect_or_metacopy {
                // Interpreting a metadata-only file as data would silently
                // return wrong bytes. No such feature is enabled by this API.
                return Err(EOPNOTSUPP);
            }
            let stop = !entry.is_directory() || entry.opaque;
            entries.push(entry);
            if stop {
                break;
            }
        }
        if entries.is_empty() {
            return Err(ENOENT);
        }
        Ok(Self {
            entries,
            namespace: self.namespace,
            context: None,
        })
    }

    /// Build an owned listing for one open-directory description. Its caller
    /// retains the snapshot/cookies until rewind; another open builds its own.
    /// Dot entries and overlay inode numbers belong to that descriptor layer.
    pub(crate) fn directory_snapshot(&self) -> Result<Vec<DirectoryItem>, i32> {
        if !self.is_directory() {
            return Err(ENOTDIR);
        }
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for parent in &self.entries {
            for stored in parent.object.maintenance_entries()? {
                // Do not silently coalesce invalid UTF-16 host names through
                // lossy replacement. Byte-name support is a separate VFS gap.
                let stored_name = stored.to_str().ok_or(EIO)?;
                let name = crate::path::unescape_path(stored_name).into_owned();
                if !seen.insert(name.clone()) {
                    continue;
                }
                let entry = Backing::from_object(
                    parent
                        .object
                        .child(&stored, FILE_READ_ATTRIBUTES | FILE_READ_EA)?,
                    self.namespace,
                    parent.layer,
                )?;
                if !entry.whiteout {
                    result.push(DirectoryItem {
                        name,
                        mode: entry.metadata.st_mode,
                    });
                }
            }
        }
        Ok(result)
    }
}

fn validate_component(name: &str) -> Result<(), i32> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err(EINVAL);
    }
    if name.len() > 255 {
        return Err(ENAMETOOLONG);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .join(format!("kinakaze-overlay-{}-{stamp}", std::process::id()));
            std::fs::create_dir(&root).unwrap();
            for name in ["upper", "middle", "lower"] {
                std::fs::create_dir(root.join(name)).unwrap();
            }
            Self(root)
        }
        fn path(&self, relative: &str) -> PathBuf {
            self.0.join(relative)
        }
        fn dir(&self, relative: &str) {
            std::fs::create_dir_all(self.path(relative)).unwrap();
        }
        fn file(&self, relative: &str, bytes: &[u8]) {
            std::fs::write(self.path(relative), bytes).unwrap();
        }
        fn attr(&self, relative: &str, name: &str, bytes: &[u8]) {
            Attributes::open_host(&self.path(relative), true)
                .unwrap()
                .set(name.as_bytes(), bytes, 0)
                .unwrap();
        }
        #[cfg(test)]
        fn root(&self, namespace: XattrNamespace) -> Node {
            Node::root(
                [self.path("upper"), self.path("middle"), self.path("lower")],
                namespace,
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // The absolute target is the one unique directory created above.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn names(node: &Node) -> BTreeSet<String> {
        node.directory_snapshot()
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect()
    }
    fn expected(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn ordered_layers_merge_names_but_not_directory_metadata() {
        let f = Fixture::new();
        for layer in ["upper", "middle", "lower"] {
            f.dir(&format!("{layer}/dir"));
        }
        f.file("upper/dir/duplicate", b"upper");
        f.file("middle/dir/duplicate", b"middle");
        f.file("lower/dir/duplicate", b"lower");
        f.file("middle/dir/middle", b"m");
        f.file("lower/dir/lower", b"l");
        crate::fs::set_mode_host_path(&f.path("upper/dir"), 0o751).unwrap();
        let dir = f.root(XattrNamespace::Trusted).child("dir").unwrap();
        assert_eq!(dir.entries.len(), 3);
        assert_eq!(dir.backing_metadata().st_mode & 0o777, 0o751);
        assert_eq!(names(&dir), expected(&["duplicate", "middle", "lower"]));
        assert_eq!(
            std::fs::read(dir.child("duplicate").unwrap().backing_path()).unwrap(),
            b"upper"
        );
        assert_eq!(
            std::fs::read(f.path("lower/dir/duplicate")).unwrap(),
            b"lower"
        );
    }

    #[test]
    fn intermediate_type_conflicts_cut_off_deeper_directories() {
        let f = Fixture::new();
        f.dir("upper/dir");
        f.file("upper/dir/top", b"top");
        f.file("middle/dir", b"barrier");
        f.dir("lower/dir");
        f.file("lower/dir/hidden", b"hidden");
        let root = f.root(XattrNamespace::Trusted);
        let dir = root.child("dir").unwrap();
        assert_eq!(names(&dir), expected(&["top"]));
        assert_eq!(dir.child("hidden").unwrap_err(), ENOENT);
        f.file("upper/file", b"top");
        f.dir("lower/file");
        f.file("lower/file/hidden", b"hidden");
        assert_eq!(
            root.child("file").unwrap().child("hidden").unwrap_err(),
            ENOTDIR
        );
    }

    #[test]
    fn xattr_whiteouts_hide_lower_names_but_oci_archive_names_are_ordinary() {
        let f = Fixture::new();
        f.file("upper/deleted", b"");
        f.attr("upper/deleted", "trusted.overlay.whiteout", b"");
        f.file("lower/deleted", b"hidden");
        f.file("upper/nonempty", b"not a whiteout");
        f.attr("upper/nonempty", "trusted.overlay.whiteout", b"");
        f.file("upper/.wh.filename", b"");
        f.file("upper/.wh..wh..opq", b"");
        let root = f.root(XattrNamespace::Trusted);
        assert_eq!(root.child("deleted").unwrap_err(), ENOENT);
        assert_eq!(
            names(&root),
            expected(&["nonempty", ".wh.filename", ".wh..wh..opq"])
        );
        assert_eq!(
            root.child("nonempty").unwrap().backing_metadata().st_size,
            14
        );
    }

    #[test]
    fn device_whiteouts_and_middle_layer_whiteouts_stop_lookup() {
        let f = Fixture::new();
        crate::fs::create_device(
            &crate::path::to_guest_path(&f.path("upper/deleted")),
            S_IFCHR | 0o600,
            0,
        )
        .unwrap();
        f.file("lower/deleted", b"hidden");
        f.dir("upper/dir");
        f.file("upper/dir/top", b"top");
        f.file("middle/dir", b"");
        f.attr("middle/dir", "trusted.overlay.whiteout", b"");
        f.dir("lower/dir");
        f.file("lower/dir/hidden", b"hidden");
        let root = f.root(XattrNamespace::Trusted);
        assert_eq!(root.child("deleted").unwrap_err(), ENOENT);
        assert_eq!(names(&root), expected(&["dir"]));
        assert_eq!(names(&root.child("dir").unwrap()), expected(&["top"]));
    }

    #[test]
    fn opaque_y_stops_merging_but_x_is_not_an_opaque_directory() {
        let f = Fixture::new();
        for layer in ["upper", "middle", "lower"] {
            f.dir(&format!("{layer}/dir"));
        }
        f.file("upper/dir/top", b"top");
        f.file("middle/dir/middle", b"middle");
        f.file("lower/dir/bottom", b"bottom");
        f.attr("middle/dir", "trusted.overlay.opaque", b"y");
        assert_eq!(
            names(&f.root(XattrNamespace::Trusted).child("dir").unwrap()),
            expected(&["top", "middle"])
        );
        f.attr("middle/dir", "trusted.overlay.opaque", b"x");
        f.file("middle/dir/gone", b"");
        f.attr("middle/dir/gone", "trusted.overlay.whiteout", b"");
        f.file("lower/dir/gone", b"hidden");
        assert_eq!(
            names(&f.root(XattrNamespace::Trusted).child("dir").unwrap()),
            expected(&["top", "middle", "bottom"])
        );
        f.attr("upper/dir", "trusted.overlay.opaque", b"y");
        assert_eq!(
            names(&f.root(XattrNamespace::Trusted).child("dir").unwrap()),
            expected(&["top"])
        );
    }

    #[test]
    fn root_always_merges_and_xattr_namespace_is_explicit() {
        let f = Fixture::new();
        f.attr("upper", "trusted.overlay.opaque", b"y");
        f.file("lower/visible", b"visible");
        f.file("upper/deleted", b"");
        f.attr("upper/deleted", "user.overlay.whiteout", b"");
        f.file("upper/escaped", b"");
        f.attr("upper/escaped", "trusted.overlay.overlay.whiteout", b"");
        assert_eq!(
            names(&f.root(XattrNamespace::Trusted)),
            expected(&["visible", "deleted", "escaped"])
        );
        assert_eq!(
            names(&f.root(XattrNamespace::User)),
            expected(&["visible", "escaped"])
        );
    }

    #[test]
    fn symlinks_are_returned_without_host_or_lower_target_traversal() {
        let f = Fixture::new();
        crate::path::create_emulated_symlink(&f.path("upper/link"), "../../outside").unwrap();
        f.dir("lower/link");
        f.file("lower/link/hidden", b"hidden");
        let root = f.root(XattrNamespace::Trusted);
        let link = root.child("link").unwrap();
        assert_eq!(link.backing_metadata().st_mode & S_IFMT, S_IFLNK);
        assert_eq!(link.child("hidden").unwrap_err(), ENOTDIR);
        assert_eq!(root.directory_snapshot().unwrap()[0].mode & S_IFMT, S_IFLNK);
    }

    #[test]
    fn snapshots_are_owned_and_rebuilding_observes_directory_changes() {
        let f = Fixture::new();
        f.file("upper/first", b"first");
        let root = f.root(XattrNamespace::Trusted);
        let old = root.directory_snapshot().unwrap();
        f.file("lower/new", b"new");
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].name, "first");
        assert_eq!(names(&root), expected(&["first", "new"]));
    }

    #[test]
    fn lookup_validates_one_component_and_round_trips_escaped_names() {
        let f = Fixture::new();
        f.file("lower/package\u{f03a}arch\u{f02e}", b"content");
        let root = f.root(XattrNamespace::Trusted);
        assert_eq!(names(&root), expected(&["package:arch."]));
        assert_eq!(
            std::fs::read(root.child("package:arch.").unwrap().backing_path()).unwrap(),
            b"content"
        );
        for name in ["", ".", "..", "/absolute", "a/b", "a\\b", "nul\0"] {
            assert_eq!(root.child(name).unwrap_err(), EINVAL);
        }
        assert_eq!(root.child(&"a".repeat(256)).unwrap_err(), ENAMETOOLONG);
    }

    #[test]
    fn unsupported_redirects_and_metacopy_never_masquerade_as_data() {
        let f = Fixture::new();
        f.file("upper/file", b"");
        f.attr("upper/file", "trusted.overlay.metacopy", b"");
        f.file("lower/file", b"real data");
        f.dir("upper/dir");
        f.attr("upper/dir", "trusted.overlay.redirect", b"/elsewhere");
        let root = f.root(XattrNamespace::Trusted);
        assert_eq!(root.child("file").unwrap_err(), EOPNOTSUPP);
        assert_eq!(root.child("dir").unwrap_err(), EOPNOTSUPP);
        f.file("upper/ordinary", b"visible");
        f.dir("lower/ordinary");
        f.attr("lower/ordinary", "trusted.overlay.redirect", b"/irrelevant");
        assert_eq!(
            std::fs::read(root.child("ordinary").unwrap().backing_path()).unwrap(),
            b"visible"
        );
    }
}

mod volatile;
pub(crate) use volatile::record as record_io_error;
pub fn check_volatile_epoch(volume: u64, since: u64) -> Result<(), i32> {
    volatile::check(volume, since)
}
