//! Batched native directory identities and validated inode-type hints.
use super::*;
use std::cell::RefCell;
use std::collections::HashMap;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx,
};

pub struct NativeDirectoryEntry {
    pub name: String,
    pub inode: u64,
    pub kind: u8,
}

type TypeCache = HashMap<(u32, object::DirectoryIdentity), u8>;
thread_local! {
    // Hints are thread-local: native fork must never inherit a cache mutex
    // held by another, non-surviving thread. No borrow crosses filesystem I/O.
    static TYPES: RefCell<TypeCache> = RefCell::new(HashMap::new());
}

/// Read Linux directory records using the open description's shared cursor.
pub fn read_directory_bytes(fd: i32, output: &mut [u8], wide: bool) -> Result<usize, i32> {
    crate::ofd::with(fd, || {
        let entry = get(fd)?;
        if entry.flags.contains(FdFlags::PATH_ONLY) {
            return Err(EBADF);
        }
        if entry.kind == FdKind::TmpfsDirectory {
            return crate::tmpfs::read_directory_bytes(fd, output, wide);
        }
        if let Some(count) = crate::mount::overlay::read_directory_bytes(fd, output, wide)? {
            return Ok(count);
        }
        if !matches!(entry.kind, FdKind::Directory | FdKind::SyntheticDirectory) {
            return Err(ENOTDIR);
        }
        let entries = crate::ofd::directory_snapshot(entry, || {
            if let Some(entries) = read_native_directory_fd(fd)? {
                return Ok(entries);
            }
            Ok(read_directory_fd(fd)?
                .into_iter()
                .enumerate()
                .map(|(index, entry)| NativeDirectoryEntry {
                    name: entry.name,
                    inode: (index + 1) as u64,
                    kind: if entry.is_directory {
                        4
                    } else if entry.is_symlink {
                        10
                    } else {
                        8
                    },
                })
                .collect())
        })?;
        let mut position = usize::try_from(entry.offset).map_err(|_| EINVAL)?;
        let mut written = 0;
        while let Some(item) = entries.get(position) {
            let name = item.name.as_bytes();
            let length = (20 + name.len() + 7) & !7;
            if length > u16::MAX as usize || length > output.len() - written {
                if written == 0 {
                    return Err(EINVAL);
                }
                break;
            }
            let record = &mut output[written..written + length];
            record.fill(0);
            record[..8].copy_from_slice(&item.inode.to_ne_bytes());
            record[8..16].copy_from_slice(&((position + 1) as i64).to_ne_bytes());
            record[16..18].copy_from_slice(&(length as u16).to_ne_bytes());
            if wide {
                record[18] = item.kind;
                record[19..19 + name.len()].copy_from_slice(name);
            } else {
                record[18..18 + name.len()].copy_from_slice(name);
                record[length - 1] = item.kind;
            }
            position += 1;
            written += length;
        }
        let mut table = table().write().map_err(|_| EIO)?;
        let stored = table
            .slots
            .get_mut(fd as usize)
            .and_then(Option::as_mut)
            .filter(|stored| stored.generation == entry.generation)
            .ok_or(EBADF)?;
        stored.offset = position as u64;
        Ok(written)
    })
}

pub fn read_native_directory_fd(fd: i32) -> Result<Option<Vec<NativeDirectoryEntry>>, i32> {
    if get(fd)?.kind != FdKind::Directory {
        return Ok(None);
    }
    let (directory, entry) = object::Object::from_fd_with_entry(fd)?;
    if entry.flags.contains(FdFlags::PATH_ONLY) {
        return Err(EBADF);
    }
    if entry.kind != FdKind::Directory {
        return Err(ENOTDIR);
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        trace_native!(
            "native.GetFileInformationByHandle",
            GetFileInformationByHandle(directory.raw(), &mut info)
        )
    } == 0
    {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let volume = info.dwVolumeSerialNumber;
    let mut result = vec![
        NativeDirectoryEntry {
            name: ".".into(),
            inode: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            kind: 4,
        },
        NativeDirectoryEntry {
            name: "..".into(),
            inode: 2,
            kind: 4,
        },
    ];
    for item in directory.directory_items()? {
        let name = crate::path::unescape_path(&item.name.to_string_lossy()).into_owned();
        let identity = item.identity;
        let cached =
            identity.and_then(|id| TYPES.with(|cache| cache.borrow().get(&(volume, id)).copied()));
        if let Some(kind) = cached {
            result.push(NativeDirectoryEntry {
                name,
                inode: identity.unwrap().inode,
                kind,
            });
            continue;
        }
        // Enumeration reports directories directly. Linux symlink markers are
        // regular files with an EA; native reparse points still require lookup.
        if let Some(id) = identity.filter(|id| {
            id.attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
                && (id.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 || id.ea_size == 0)
        }) {
            result.push(NativeDirectoryEntry {
                name,
                inode: id.inode,
                kind: if id.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                    4
                } else {
                    8
                },
            });
            continue;
        }
        let child = match directory.child(&item.name, FILE_READ_ATTRIBUTES | FILE_READ_EA) {
            Ok(child) => child,
            Err(ENOENT) => continue,
            Err(EACCES) => {
                // Listing a directory does not require metadata access on its
                // children. Keep the name and let a caller resolve DT_UNKNOWN.
                result.push(NativeDirectoryEntry {
                    name,
                    inode: identity.map_or(0, |id| id.inode),
                    kind: 0,
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        if unsafe {
            trace_native!(
                "native.GetFileInformationByHandle",
                GetFileInformationByHandle(child.raw(), &mut info)
            )
        } == 0
        {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let inode = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        let record = inode::read_object(&child)?;
        let mode = if record.symlink.is_some()
            || (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                && native_symlink_target_handle(child.raw())?.is_some())
        {
            S_IFLNK
        } else if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            S_IFDIR
        } else {
            record
                .mode
                .filter(|mode| mode & S_IFMT != 0)
                .unwrap_or(S_IFREG)
                & S_IFMT
        };
        let kind = (mode >> 12) as u8;
        if let Some(id) = identity.filter(|id| id.inode == inode && id.inode != 0) {
            let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
            // A fresh native scan supplies the ID, creation/change timestamps,
            // attributes and EA size every time. This bounded cache owns no
            // handles or mutable filesystem state. Validate a miss against the
            // pinned object so replacement/EA changes cannot poison an old key.
            if unsafe {
                trace_native!(
                    "native.GetFileInformationByHandleEx",
                    GetFileInformationByHandleEx(
                        child.raw(),
                        FileBasicInfo,
                        (&mut basic as *mut FILE_BASIC_INFO).cast(),
                        size_of_val(&basic) as u32,
                    )
                )
            } != 0
                && basic.CreationTime as u64 == id.created
                && basic.ChangeTime as u64 == id.changed
                && basic.FileAttributes == id.attributes
            {
                TYPES.with(|cache| {
                    let mut cache = cache.borrow_mut();
                    if cache.len() >= 8192 {
                        cache.clear();
                    }
                    cache.insert((volume, id), kind);
                });
            }
        }
        result.push(NativeDirectoryEntry { name, inode, kind });
    }
    Ok(Some(result))
}
