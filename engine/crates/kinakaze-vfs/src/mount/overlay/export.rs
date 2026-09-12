//! Stable opaque export handles and namespace-constrained native decode.
use super::super::{features, identity::Identity};
use super::*;
use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileNameW, FindNextFileNameW};

pub fn encode_handle(path: &str, follow: bool) -> Result<(Vec<u8>, u64), i32> {
    let location = resolve(path, follow, false)?
        .and_then(|r| r.location)
        .ok_or(crate::EOPNOTSUPP)?;
    if location.instance.flags & features::NFS_EXPORT == 0 {
        return Err(crate::EOPNOTSUPP);
    }
    let _guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let node = location.lookup()?;
    let identity = if node.is_directory() && location.instance.work.is_some() {
        let node = location.upper(false)?;
        let stat = fs::stat_handle(node.backing_object().raw(), false)?;
        let context = location.instance.root.context.as_ref().ok_or(EIO)?;
        Identity {
            upper: true,
            uuid: if context.ignore_uuid() {
                [0; 16]
            } else {
                context.volumes[0]
            },
            device: if context.ignore_uuid() {
                0
            } else {
                stat.st_dev
            },
            inode: stat.st_ino,
        }
    } else {
        node.origin()?
    };
    let mount = crate::mount::visible_mount_id(&location.guest)?.ok_or(crate::ESTALE)?;
    Ok((identity.encode(), mount))
}

pub fn encode_handle_fd(fd: i32) -> Result<(Vec<u8>, u64), i32> {
    let entry = crate::get(fd)?;
    let description = reference(entry)?.ok_or(crate::EOPNOTSUPP)?;
    let location = &description.location;
    if location.instance.flags & features::NFS_EXPORT == 0 {
        return Err(crate::EOPNOTSUPP);
    }
    let _guard = InodeLock::acquire(location.instance.root.backing_object().raw())?;
    let node = location.node.as_ref().ok_or(EIO)?.refreshed()?;
    if node.is_directory() {
        return encode_handle(&location.guest, true);
    }
    let mount = crate::mount::visible_mount_id(&location.guest)?.ok_or(crate::ESTALE)?;
    Ok((node.origin()?.encode(), mount))
}

/// Internal export entry point. The syscall adapter must enforce
/// CAP_DAC_READ_SEARCH before accepting a guest-supplied handle.
pub fn open_handle(mount_fd: i32, bytes: &[u8], flags: i32) -> Result<i32, i32> {
    if flags & (fs::O_CREAT | fs::O_EXCL | 0o20000000) != 0 {
        return Err(EINVAL);
    }
    let entry = crate::get(mount_fd)?;
    let description = reference(entry)?.ok_or(crate::EOPNOTSUPP)?;
    let instance = &description.location.instance;
    if instance.flags & features::NFS_EXPORT == 0 {
        return Err(crate::EOPNOTSUPP);
    }
    let _guard = InodeLock::acquire(instance.root.backing_object().raw())?;
    let identity = Identity::decode(bytes)?;
    let context = instance.root.context.as_ref().ok_or(EIO)?;
    let mut objects = Vec::new();
    if !identity.upper
        && let Some(index) = &context.index
    {
        if let Some(object) = index.directory_upper(&identity, &instance.root)? {
            objects.push((0, object));
        } else if let Some(object) = index.lookup(&identity)? {
            objects.push((0, object));
        }
    }
    if objects.is_empty() {
        for root in &context.roots {
            if identity.upper != (root.layer == 0 && context.writable) {
                continue;
            }
            let uuid_matches = identity.uuid == context.volumes[root.layer]
                || identity.uuid == [0; 16] && identity.device == 0 && context.ignore_uuid();
            if !uuid_matches {
                continue;
            }
            if let Ok(object) = Object::by_id(&root.object.path()?, identity.inode, ACCESS) {
                let stat = fs::stat_handle(object.raw(), false)?;
                if identity.device != 0 && identity.device != stat.st_dev {
                    continue;
                }
                objects.push((root.layer, object));
            }
        }
    }
    for (layer, object) in objects {
        let Some(root) = context.roots.iter().find(|root| root.layer == layer) else {
            continue;
        };
        let root_path = root.object.path()?;
        for path in aliases(&object)? {
            let Ok(relative) = path.strip_prefix(&root_path) else {
                continue;
            };
            let mut components = Vec::new();
            for component in relative.components() {
                let std::path::Component::Normal(name) = component else {
                    return Err(crate::ESTALE);
                };
                components.push(crate::path::unescape_path(name.to_str().ok_or(EIO)?).into_owned());
            }
            for mount in crate::mount::snapshot_list()? {
                if mount.flags & crate::mount::MS_OVERLAY == 0 {
                    continue;
                }
                let (source, view) = split_view(&mount.source)?;
                if source != instance.source {
                    continue;
                }
                let view: Vec<_> = view.split('/').filter(|s| !s.is_empty()).collect();
                if components.len() < view.len()
                    || !components.iter().zip(&view).all(|(a, b)| a == b)
                {
                    continue;
                }
                let global = format!(
                    "{}/{}",
                    mount.target.trim_end_matches('/'),
                    components[view.len()..].join("/")
                );
                if let Some(root) = crate::path::overlay_root()
                    && global != root.namespace_path
                    && !global
                        .starts_with(&format!("{}/", root.namespace_path.trim_end_matches('/')))
                {
                    continue;
                }
                let guest = guest_visible(&global);
                let Some(location) = resolve(&guest, false, false)
                    .ok()
                    .flatten()
                    .and_then(|r| r.location)
                else {
                    continue;
                };
                let node = location.lookup()?;
                let stat = fs::stat_handle(node.backing_object().raw(), false)?;
                let matches = if identity.upper {
                    stat.st_ino == identity.inode
                        && (identity.device == 0 || identity.device == stat.st_dev)
                } else {
                    node.origin()? == identity
                };
                if matches {
                    if stat.st_mode & S_IFMT == S_IFLNK && flags & fs::O_PATH == 0 {
                        return Err(crate::ELOOP);
                    }
                    return fs::open(&guest, flags | fs::O_NOFOLLOW, 0);
                }
            }
        }
    }
    Err(crate::ESTALE)
}

fn aliases(object: &Object) -> Result<Vec<PathBuf>, i32> {
    let path = object.path()?;
    let mut paths = vec![path.clone()];
    if fs::stat_handle(object.raw(), false)?.st_mode & S_IFMT == S_IFDIR {
        return Ok(paths);
    }
    let volume = volume_hint(&path)?;
    let path = crate::path::wide_path(&path)?;
    let mut name = vec![0u16; 32768];
    let mut size = name.len() as u32;
    let find = unsafe { FindFirstFileNameW(path.as_ptr(), 0, &mut size, name.as_mut_ptr()) };
    if find == INVALID_HANDLE_VALUE {
        return Ok(paths);
    }
    struct Search(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Search {
        fn drop(&mut self) {
            unsafe {
                FindClose(self.0);
            }
        }
    }
    let _search = Search(find);
    loop {
        let end = name.iter().position(|v| *v == 0).ok_or(EIO)?;
        let skip = usize::from(name.first() == Some(&(b'\\' as u16)));
        paths.push(volume.join(OsString::from_wide(&name[skip..end])));
        size = name.len() as u32;
        if unsafe { FindNextFileNameW(find, &mut size, name.as_mut_ptr()) } == 0 {
            let error = unsafe { GetLastError() };
            if error != 38 {
                return Err(crate::errno_from_win32(error));
            }
            break;
        }
    }
    Ok(paths)
}
