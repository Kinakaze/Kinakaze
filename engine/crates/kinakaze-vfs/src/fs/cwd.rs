//! fs_struct owns its directory reference independently of the guest fd table.
use super::*;
use std::sync::Arc;

pub(crate) struct Directory {
    object: object::Object,
    namespace: String,
    original: std::path::PathBuf,
    native: Option<crate::mount::native::Description>,
}

impl Directory {
    fn namespace(&self) -> Result<String, i32> {
        let path = self.object.path()?;
        if let Some(description) = &self.native {
            return crate::mount::native::namespace_for_description(description, &path);
        }
        Ok(if path == self.original {
            self.namespace.clone()
        } else {
            crate::path::to_namespace_path(&path).ok_or(ENOENT)?
        })
    }
    fn display(&self) -> Result<String, i32> {
        let namespace = self.namespace()?;
        if let Some(root) = crate::path::namespace_root_path()? {
            if root == "/" {
                return Ok(namespace);
            }
            if namespace == root {
                return Ok("/".into());
            }
            if let Some(tail) = namespace.strip_prefix(&root).filter(|s| s.starts_with('/')) {
                return Ok(tail.into());
            }
            return Ok(format!("(unreachable){namespace}"));
        }
        Ok(namespace)
    }
}

pub(crate) fn display() -> Option<String> {
    crate::fs_context::read(|s| s.cwd_object.clone())
        .map(|d| d.display().unwrap_or_else(|_| "(unreachable)".into()))
}
pub(crate) fn namespace() -> Result<Option<String>, i32> {
    crate::fs_context::read(|s| s.cwd_object.clone())
        .map(|d| d.namespace())
        .transpose()
}
pub(crate) fn stat_dot(path: &str) -> Option<Result<Stat, i32>> {
    if path.is_empty()
        || path.split('/').any(|s| !s.is_empty() && s != ".")
        || path.starts_with('/')
    {
        return None;
    }
    crate::fs_context::read(|s| s.cwd_object.clone()).map(|d| stat_handle(d.object.raw(), false))
}

pub fn fchdir(fd: i32) -> Result<(), i32> {
    let entry = crate::get(fd)?;
    if entry.kind != FdKind::Directory {
        let path = match entry.kind {
            FdKind::TmpfsDirectory => crate::tmpfs::descriptor_path(fd)?,
            FdKind::SyntheticDirectory => crate::synthetic_directory_path(fd)?,
            _ => return Err(ENOTDIR),
        };
        return super::chdir(&path);
    }
    let (object, entry) = object::Object::from_fd_checked(fd, |e| {
        if e.kind == FdKind::Directory {
            Ok(())
        } else {
            Err(ENOTDIR)
        }
    })?;
    let stat = stat_handle(object.raw(), false)?;
    if stat.st_mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    let credentials = crate::credentials::filesystem();
    let bits = if credentials.uid == stat.st_uid {
        stat.st_mode >> 6
    } else if crate::credentials::group_member(stat.st_gid) {
        stat.st_mode >> 3
    } else {
        stat.st_mode
    };
    if credentials.uid != 0 && bits & 1 == 0 {
        return Err(EACCES);
    }
    let original = object.path()?;
    let namespace = crate::mount::overlay::descriptor_namespace(entry)?
        .or(crate::mount::native::namespace_for_native(
            entry, &original,
        )?)
        .or_else(|| crate::path::to_namespace_path(&original))
        .unwrap_or_else(|| crate::to_guest_path(&original));
    let directory = Arc::new(Directory {
        object,
        namespace,
        original,
        native: crate::mount::native::reference(entry)?,
    });
    let display = directory.display()?;
    crate::fs_context::update(|s| {
        s.cwd = Some(display);
        s.cwd_object = Some(directory);
    });
    crate::procfs::publish_fs_paths();
    Ok(())
}

pub(crate) fn serialize() -> Result<Option<Vec<u8>>, i32> {
    let Some(directory) = crate::fs_context::read(|s| s.cwd_object.clone()) else {
        return Ok(None);
    };
    let stat = stat_handle(directory.object.raw(), false)?;
    let path = directory.object.path()?;
    // A volume root remains a valid OpenFileById hint after cwd is renamed.
    let hint = path.ancestors().last().ok_or(EIO)?.to_string_lossy();
    let mut bytes = b"CYCWD002".to_vec();
    crate::state_codec::word(&mut bytes, stat.st_ino);
    crate::state_codec::bytes(&mut bytes, hint.as_bytes());
    crate::state_codec::bytes(&mut bytes, directory.namespace()?.as_bytes());
    crate::state_codec::word(&mut bytes, u64::from(directory.native.is_some()));
    if let Some(description) = &directory.native {
        for value in [
            description.policy.namespace,
            description.policy.id,
            description.policy.flags(),
        ] {
            crate::state_codec::word(&mut bytes, value);
        }
        crate::state_codec::bytes(&mut bytes, description.path.as_bytes());
    }
    Ok(Some(bytes))
}
pub(crate) fn restore(bytes: &[u8]) -> Result<bool, i32> {
    let version_two = bytes.starts_with(b"CYCWD002");
    let Some(bytes) = bytes.strip_prefix(if version_two {
        b"CYCWD002"
    } else {
        b"CYCWD001"
    }) else {
        return Ok(false);
    };
    let mut reader = crate::state_codec::Reader(bytes);
    let inode = reader.word()?;
    let hint = reader.text()?;
    let namespace = reader.text()?;
    let native = if version_two && reader.word()? != 0 {
        let policy = crate::mount::policy::get(reader.word()?, reader.word()?, reader.word()?)?;
        let path = reader.text()?;
        if !path.starts_with('/') {
            return Err(EINVAL);
        }
        Some(crate::mount::native::Description {
            policy,
            path,
            writer: None,
        })
    } else {
        None
    };
    reader.end()?;
    if !namespace.starts_with('/') {
        return Err(EINVAL);
    }
    let object = object::Object::by_id(Path::new(&hint), inode, FILE_READ_ATTRIBUTES)?;
    if stat_handle(object.raw(), false)?.st_mode & S_IFMT != S_IFDIR {
        return Err(ENOTDIR);
    }
    let original = object.path()?;
    let directory = Arc::new(Directory {
        object,
        namespace,
        original,
        native,
    });
    let display = directory.display()?;
    crate::fs_context::update(|s| {
        s.cwd = Some(display);
        s.cwd_object = Some(directory);
    });
    crate::procfs::publish_fs_paths();
    Ok(true)
}
