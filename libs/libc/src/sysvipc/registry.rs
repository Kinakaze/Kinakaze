//! Namespace-owned key directory. Sections are named by immutable IDs, so
//! IPC_RMID frees a key immediately while existing attachments keep their data.
use super::*;

pub(super) fn name(kind: IpcKind, namespace: u64, id: i32) -> String {
    kinakaze_v2_protocol::kernel::ObjectKey::Sysv {
        kind,
        namespace,
        id,
    }
    .name(kinakaze_runtime::authority::domain_id())
}

pub(super) fn open(kind: IpcKind, id: i32) -> Result<(*mut c_void, *mut c_void), i32> {
    if id <= 0 {
        return Err(EINVAL);
    }
    let name = wide(&name(kind, ipc_namespace(), id));
    // Fork queries the section extent before restoring retained views. A handle
    // opened with only FILE_MAP_READ/WRITE lacks SECTION_QUERY and fails there.
    let mapping = unsafe {
        OpenFileMappingW(
            windows_sys::Win32::System::Memory::FILE_MAP_ALL_ACCESS,
            0,
            name.as_ptr(),
        )
    };
    if mapping.is_null() {
        return Err(if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
            EINVAL
        } else {
            last_errno()
        });
    }
    let control = match map_control(mapping) {
        Ok(control) => control,
        Err(error) => {
            unsafe { CloseHandle(mapping) };
            return Err(error);
        }
    };
    let valid = unsafe {
        match kind {
            IpcKind::Memory => {
                let h = &*control.cast::<ShmHeader>();
                h.magic == SHM_MAGIC && h.id == id as u32
            }
            IpcKind::Semaphore => {
                let h = &*control.cast::<SemHeader>();
                h.magic == SEM_MAGIC && h.id == id as u32
            }
        }
    };
    if !valid {
        release_section(mapping, control);
        return Err(EINVAL);
    }
    Ok((mapping, control))
}

fn live(kind: IpcKind, id: i32) -> Result<bool, i32> {
    let (mapping, control) = match open(kind, id) {
        Ok(pair) => pair,
        Err(EINVAL) => return Ok(false),
        Err(error) => return Err(error),
    };
    let removed = unsafe {
        match kind {
            IpcKind::Memory => &(*control.cast::<ShmHeader>()).removed,
            IpcKind::Semaphore => &(*control.cast::<SemHeader>()).removed,
        }
    }
    .load(Ordering::Acquire);
    release_section(mapping, control);
    Ok(removed == 0)
}

type Entry = (u32, i32, i32);
fn decode(bytes: &[u8]) -> Result<Vec<Entry>, i32> {
    if bytes.len() % 12 != 0 {
        return Err(EIO);
    }
    bytes
        .chunks_exact(12)
        .map(|row| {
            let kind = u32::from_le_bytes(row[..4].try_into().unwrap());
            let key = i32::from_le_bytes(row[4..8].try_into().unwrap());
            let id = i32::from_le_bytes(row[8..].try_into().unwrap());
            if kind > 1 || key == IPC_PRIVATE || id <= 0 {
                return Err(EIO);
            }
            Ok((kind, key, id))
        })
        .collect()
}
fn encode(entries: &[Entry]) -> Vec<u8> {
    entries
        .iter()
        .flat_map(|(kind, key, id)| {
            [kind.to_le_bytes(), key.to_le_bytes(), id.to_le_bytes()].concat()
        })
        .collect()
}

pub(super) fn get<T>(
    kind: IpcKind,
    key: i32,
    flags: i32,
    action: impl FnOnce(i32, bool) -> Result<T, i32>,
) -> Result<T, i32> {
    // Private means a new unkeyed object; its ID is still usable by other tasks.
    if key == IPC_PRIVATE {
        return action(kinakaze_vfs::namespaces::allocate_ipc_id()?, true);
    }
    kinakaze_vfs::namespaces::ipc_registry(|bytes| {
        let mut entries = decode(bytes)?;
        if let Some(index) = entries
            .iter()
            .position(|row| row.0 == kind as u32 && row.1 == key)
        {
            let id = entries[index].2;
            if live(kind, id)? {
                if flags & (IPC_CREAT | IPC_EXCL) == (IPC_CREAT | IPC_EXCL) {
                    return Err(EEXIST);
                }
                return Ok((bytes.to_vec(), action(id, false)?));
            }
            entries.remove(index);
        }
        if flags & IPC_CREAT == 0 {
            return Err(ENOENT);
        }
        let id = kinakaze_vfs::namespaces::allocate_ipc_id()?;
        let result = action(id, true)?;
        entries.push((kind as u32, key, id));
        Ok((encode(&entries), result))
    })
}

pub(super) fn forget(kind: IpcKind, id: i32) {
    let _ = kinakaze_vfs::namespaces::ipc_registry(|bytes| {
        let mut entries = decode(bytes)?;
        entries.retain(|row| row.0 != kind as u32 || row.2 != id);
        Ok((encode(&entries), ()))
    });
}
