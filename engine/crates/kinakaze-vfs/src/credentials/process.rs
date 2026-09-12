//! Process credentials for cross-process procfs readers. Values remain in the
//! initial user namespace until the reader translates them into its own view.
use crate::mount::shared::Store;
use crate::{EIO, ENOENT};
use std::sync::{Arc, OnceLock};

const MAGIC: &[u8; 8] = b"CRYCRED1";
const RECORD_SIZE: usize = 44;

fn store() -> Result<Arc<Store>, i32> {
    static STORE: OnceLock<Arc<Store>> = OnceLock::new();
    if let Some(store) = STORE.get() {
        return Ok(store.clone());
    }
    let opened = Arc::new(Store::user_object(u64::MAX - 44, true)?);
    let _ = STORE.set(opened);
    Ok(STORE.get().unwrap().clone())
}

fn current_ids() -> [u32; 8] {
    let identity = super::identity();
    let filesystem = super::filesystem();
    [
        identity.uids[0],
        identity.uids[1],
        identity.uids[2],
        filesystem.uid,
        identity.gids[0],
        identity.gids[1],
        identity.gids[2],
        filesystem.gid,
    ]
}

fn records(bytes: &[u8]) -> Result<&[u8], i32> {
    if bytes.is_empty() {
        return Ok(bytes);
    }
    if bytes.get(..8) != Some(MAGIC) || !(bytes.len() - 8).is_multiple_of(RECORD_SIZE) {
        return Err(EIO);
    }
    Ok(&bytes[8..])
}

/// Call after registration, identity restoration, and every successful ID change.
pub fn publish() -> Result<(), i32> {
    let pid = crate::job::process_id();
    let entry = kinakaze_runtime::job::lookup(pid).ok_or(ENOENT)?;
    let ids = current_ids();
    store()?.update(|old| {
        let mut bytes = MAGIC.to_vec();
        for row in records(old)?.chunks_exact(RECORD_SIZE) {
            let other = u32::from_le_bytes(row[..4].try_into().unwrap());
            let born = u64::from_le_bytes(row[4..12].try_into().unwrap());
            if other != pid
                && kinakaze_runtime::job::process_info(other)
                    .is_some_and(|info| info.entry.start_ticks == born)
            {
                bytes.extend_from_slice(row);
            }
        }
        bytes.extend_from_slice(&pid.to_le_bytes());
        bytes.extend_from_slice(&entry.start_ticks.to_le_bytes());
        for id in ids {
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        Ok((bytes, ()))
    })?;
    Ok(())
}

pub(crate) fn process_ids(pid: u32) -> Result<[u32; 8], i32> {
    if pid == crate::job::process_id() {
        return Ok(current_ids());
    }
    let entry = kinakaze_runtime::job::lookup(pid).ok_or(ENOENT)?;
    store()?.read_with(|bytes| {
        for row in records(bytes)?.chunks_exact(RECORD_SIZE) {
            if row[..4] == pid.to_le_bytes() && row[4..12] == entry.start_ticks.to_le_bytes() {
                return Ok(std::array::from_fn(|i| {
                    u32::from_le_bytes(row[12 + i * 4..16 + i * 4].try_into().unwrap())
                }));
            }
        }
        Err(ENOENT)
    })
}
