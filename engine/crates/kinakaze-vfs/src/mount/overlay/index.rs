//! Durable copy-up index: regular entries are real hard links, directories retain
//! native upper handles. Lower identity is verified before reusing an index.
use super::identity::Identity;
use super::*;
use crate::fs::S_IFMT;

#[derive(Debug)]
pub(super) struct Index {
    pub directory: Object,
    namespace: XattrNamespace,
}

pub(super) struct Installation {
    entry: Object,
    committed: bool,
}
impl Installation {
    pub(super) fn commit(mut self) {
        self.committed = true;
    }
    pub(super) fn flush(&self) -> Result<(), i32> {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        unsafe { Object::reopen(self.entry.raw(), GENERIC_READ | GENERIC_WRITE)? }.flush()
    }
}
impl Drop for Installation {
    fn drop(&mut self) {
        if !self.committed {
            let _ = crate::fs::unlink_inode(self.entry.raw());
        }
    }
}

impl Index {
    pub(super) fn flush(&self) -> Result<(), i32> {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        unsafe { Object::reopen(self.directory.raw(), GENERIC_READ | GENERIC_WRITE)? }.flush()
    }
    pub(super) fn adjust_links(&self, node: &Node, delta: i64, work: &Object) -> Result<(), i32> {
        let bytes =
            unsafe { Attributes::from_handle(node.backing_object().raw(), false)? }.snapshot()?;
        let Some(origin) = bytes.get(format!("{}origin", self.namespace.prefix()).as_bytes())
        else {
            return Ok(());
        };
        let origin = Identity::decode(origin)?;
        let name = format!("{}kinakaze.nlink", self.namespace.prefix());
        let remaining = if node.is_directory() {
            0
        } else {
            let Some(bytes) = bytes.get(name.as_bytes()) else {
                return Ok(());
            };
            let links = u64::from_le_bytes(bytes.as_slice().try_into().map_err(|_| EIO)?);
            let remaining = links.checked_add_signed(delta).ok_or(EIO)?;
            unsafe { Attributes::from_handle(node.backing_object().raw(), true)? }.set(
                name.as_bytes(),
                &remaining.to_le_bytes(),
                0,
            )?;
            remaining
        };
        if remaining == 0 {
            copy_up::Staged::whiteout(work)?.publish_replacing(&self.directory, &origin.key())?;
        }
        Ok(())
    }
    pub(super) fn open(root: &Node, work: &Object) -> Result<Self, i32> {
        let context = root.context.as_ref().ok_or(EIO)?;
        let lower = root.entries.get(1).ok_or(EINVAL)?;
        let origin = Node {
            entries: vec![lower.clone()],
            namespace: root.namespace,
            context: root.context.clone(),
        }
        .origin()?
        .encode();
        let attrs = unsafe { Attributes::from_handle(root.backing_object().raw(), true)? };
        let name = format!("{}origin", root.namespace.prefix());
        if let Some(previous) = attrs.snapshot()?.get(name.as_bytes()) {
            if previous != &origin {
                return Err(crate::ESTALE);
            }
        } else {
            attrs.set(name.as_bytes(), &origin, crate::xattr::XATTR_CREATE)?;
        }
        let path = work.path()?.join("index");
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(e
                    .raw_os_error()
                    .map_or(EIO, |v| crate::errno_from_win32(v as u32)));
            }
        }
        let directory = Object::open(&path, FILE_READ_ATTRIBUTES | FILE_READ_EA)?;
        if crate::fs::stat_handle(directory.raw(), false)?.st_mode & S_IFMT != S_IFDIR {
            return Err(ENOTDIR);
        }
        let index = Self {
            directory,
            namespace: root.namespace,
        };
        if context.flags & features::NFS_EXPORT != 0 {
            index.verify(root)?;
        }
        Ok(index)
    }

    pub(super) fn directory_upper(
        &self,
        origin: &Identity,
        root: &Node,
    ) -> Result<Option<Object>, i32> {
        let object = match self.directory.child(
            OsStr::new(&origin.key()),
            FILE_READ_ATTRIBUTES | FILE_READ_EA,
        ) {
            Ok(object) => object,
            Err(ENOENT) => return Ok(None),
            Err(e) => return Err(e),
        };
        let entry = Backing::from_object(object, self.namespace, 0)?;
        if entry.whiteout {
            return Err(crate::ESTALE);
        }
        let attrs = unsafe { Attributes::from_handle(entry.object.raw(), false)? }.snapshot()?;
        let Some(bytes) = attrs.get(format!("{}upper", self.namespace.prefix()).as_bytes()) else {
            return Ok(None);
        };
        let handle = Identity::decode(bytes)?;
        let context = root.context.as_ref().ok_or(EIO)?;
        if !handle.upper || handle.uuid != context.volumes[0] {
            return Err(crate::ESTALE);
        }
        let object = Object::by_id(
            &root.backing_object().path()?,
            handle.inode,
            FILE_READ_ATTRIBUTES | FILE_READ_EA,
        )
        .map_err(|_| crate::ESTALE)?;
        let stat = crate::fs::stat_handle(object.raw(), false)?;
        if !object.path()?.starts_with(root.backing_object().path()?)
            || stat.st_mode & S_IFMT != S_IFDIR
            || stat.st_dev != handle.device
        {
            return Err(crate::ESTALE);
        }
        Ok(Some(object))
    }

    fn verify(&self, root: &Node) -> Result<(), i32> {
        for name in self.directory.entries()? {
            let text = name.to_str().ok_or(EIO)?;
            if text.len() != 82 {
                return Err(crate::ESTALE);
            }
            let bytes = text
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    Ok(((pair[0] as char).to_digit(16).ok_or(EIO)? * 16
                        + (pair[1] as char).to_digit(16).ok_or(EIO)?) as u8)
                })
                .collect::<Result<Vec<_>, i32>>()?;
            let origin = Identity::decode(&bytes)?;
            let object = self
                .directory
                .child(&name, FILE_READ_ATTRIBUTES | FILE_READ_EA)?;
            if Backing::from_object(object, self.namespace, 0)?.whiteout {
                continue;
            }
            if self.directory_upper(&origin, root)?.is_none() {
                self.lookup(&origin)?;
            }
        }
        Ok(())
    }

    pub(super) fn lookup(&self, origin: &Identity) -> Result<Option<Object>, i32> {
        let object = match self.directory.child(
            OsStr::new(&origin.key()),
            FILE_READ_ATTRIBUTES | FILE_READ_EA,
        ) {
            Ok(object) => object,
            Err(ENOENT) => return Ok(None),
            Err(e) => return Err(e),
        };
        let entry = Backing::from_object(object, self.namespace, 0)?;
        if entry.whiteout {
            return Err(crate::ESTALE);
        }
        if unsafe { Attributes::from_handle(entry.object.raw(), false)? }
            .snapshot()?
            .contains_key(format!("{}upper", self.namespace.prefix()).as_bytes())
        {
            return Ok(None); // Directory origin verification is separate from file alias lookup.
        }
        let bytes = unsafe { Attributes::from_handle(entry.object.raw(), false)? }
            .get(format!("{}origin", self.namespace.prefix()).as_bytes())?;
        if Identity::decode(&bytes)? != *origin {
            return Err(crate::ESTALE);
        }
        Ok(Some(unsafe {
            Object::reopen(entry.object.raw(), FILE_READ_ATTRIBUTES | FILE_READ_EA)?
        }))
    }

    pub(super) fn install(
        &self,
        origin: &Identity,
        upper: &Object,
        links: u64,
    ) -> Result<Installation, i32> {
        let path = self.directory.path()?.join(origin.key());
        let attrs = unsafe { Attributes::from_handle(upper.raw(), true)? };
        attrs.set(
            format!("{}origin", self.namespace.prefix()).as_bytes(),
            &origin.encode(),
            0,
        )?;
        if crate::fs::stat_handle(upper.raw(), false)?.st_mode & S_IFMT == S_IFDIR {
            // A directory cannot be hard-linked; its index entry stores an upper handle.
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| {
                    e.raw_os_error()
                        .map_or(EIO, |v| crate::errno_from_win32(v as u32))
                })?;
            let pending = Installation {
                entry: Object::open(&path, windows_sys::Win32::Storage::FileSystem::DELETE)?,
                committed: false,
            };
            drop(file);
            let upper_stat = crate::fs::stat_handle(upper.raw(), false)?;
            let handle = Identity {
                upper: true,
                uuid: identity::volume_uuid(upper)?,
                device: upper_stat.st_dev,
                inode: upper_stat.st_ino,
            };
            Attributes::open_host(&path, true)?.set(
                format!("{}upper", self.namespace.prefix()).as_bytes(),
                &handle.encode(),
                0,
            )?;
            Ok(pending)
        } else {
            attrs.set(
                format!("{}kinakaze.nlink", self.namespace.prefix()).as_bytes(),
                &links.to_le_bytes(),
                0,
            )?;
            std::fs::hard_link(upper.path()?, &path).map_err(|e| {
                e.raw_os_error()
                    .map_or(EIO, |v| crate::errno_from_win32(v as u32))
            })?;
            Ok(Installation {
                entry: Object::open(&path, windows_sys::Win32::Storage::FileSystem::DELETE)?,
                committed: false,
            })
        }
    }
}
