//! Reuse the kernel-owned last-duplicate-close tokens used by FIFOs. These
//! private files carry ownership only; application data remains in shared RAM.
use super::*;
use crate::fifo::lifecycle;
use crate::fs::object::Object;
pub(super) fn directory(arena: u64) -> Result<std::path::PathBuf, i32> {
    let fifo = lifecycle::storage_root()?;
    let root = fifo.parent().ok_or(EIO)?.join("packet-v1");
    Ok(root.join(format!(
        "{:016x}-{arena:016x}",
        kinakaze_runtime::authority::domain_id()
    )))
}
pub(super) fn create(arena: u64, description: u64) -> Result<Arc<Object>, i32> {
    let directory = directory(arena)?;
    std::fs::create_dir_all(&directory).map_err(|error| {
        error
            .raw_os_error()
            .map(|error| crate::errno_from_win32(error as u32))
            .unwrap_or(EIO)
    })?;
    let token = lifecycle::token(&directory.join(format!("{description:016x}")), true)?;
    Ok(Arc::new(Object::owned(token.into_raw())?))
}
pub(super) fn present(arena: u64) -> Result<Vec<u64>, i32> {
    let mut result = Vec::new();
    let entries = match std::fs::read_dir(directory(arena)?) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Err(_) => return Err(EIO),
    };
    for entry in entries {
        let entry = entry.map_err(|_| EIO)?;
        if let Some(name) = entry.file_name().to_str() {
            if name.len() == 16 {
                if let Ok(id) = u64::from_str_radix(name, 16) {
                    result.push(id);
                }
            }
        }
    }
    Ok(result)
}
pub(super) fn watch(arena: u64) -> Result<lifecycle::DirectoryWatch, i32> {
    lifecycle::DirectoryWatch::new(&directory(arena)?)
}
pub(super) fn reconcile(root: &Arc<SharedEndpoint>, arena: u64) -> Result<Vec<Vec<u8>>, i32> {
    let present = present(arena)?;
    if let Some(listener) = crate::usernet::stack::shared::Listener::reopen(root.clone())? {
        listener.reconcile_owners(arena, &present)
    } else {
        if !present.contains(&arena) && root.tcp_state().is_ok() {
            root.close_tcp()?;
        }
        Ok(Vec::new())
    }
}
