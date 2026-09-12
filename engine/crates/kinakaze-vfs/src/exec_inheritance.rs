//! Checked, table-locked inheritance changes for one process creation.

use super::{FdFlags, FdKind, FdTable, fifo};
use std::io;
use windows_sys::Win32::Foundation::{
    GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation,
};

#[cfg(test)]
mod tests;

/// Prepare native ownership before publishing a descriptor. A kind that permits
/// raw=0 can still carry a real handle (for example a connected Unix socket).
/// Linux CLOEXEC never removes that handle from a fork child. On publication
/// failure the caller still owns the handle with its original native flags.
pub(super) struct DescriptorInheritance {
    raw: usize,
    previous: u32,
}

impl DescriptorInheritance {
    pub(super) fn prepare(raw: usize, kind: FdKind, flags: FdFlags) -> Result<Self, i32> {
        if raw == 0 || flags.contains(FdFlags::BORROWED) {
            return Ok(Self {
                raw: 0,
                previous: 0,
            });
        }
        let mut previous = 0;
        if unsafe { GetHandleInformation(raw as HANDLE, &mut previous) } == 0 {
            let error = Restore::native_error("prepare-query", raw);
            return Err(super::errno_from_win32(
                error.raw_os_error().unwrap_or(0) as u32
            ));
        }
        let previous = previous & HANDLE_FLAG_INHERIT;
        let guard = Self { raw, previous };
        // Winsock objects are restored through the process-specific socket
        // protocol; other owned handles use the native fork inheritance set.
        super::platform::try_set_inheritable(raw, kind != FdKind::Socket)?;
        Ok(guard)
    }

    pub(super) fn commit(mut self) {
        self.raw = 0;
    }
}

impl Drop for DescriptorInheritance {
    fn drop(&mut self) {
        if self.raw != 0 {
            if let Err(error) = super::platform::try_set_inheritable(self.raw, self.previous != 0) {
                eprintln!(
                    "kinakaze: failed to roll back descriptor inheritance: Linux errno {error}"
                );
            }
        }
    }
}

struct Restore<'a> {
    table: std::sync::RwLockWriteGuard<'a, FdTable>,
    fifo_pins: Vec<fifo::AuxiliaryHandle>,
    unix_pins: Vec<(i32, std::sync::Arc<super::unix::SocketInode>)>,
    unix_state_pins: Vec<(i32, std::sync::Arc<super::fs::object::Object>)>,
    overlay_pins: Vec<(u64, std::sync::Arc<super::fs::object::Object>)>,
    handles: Vec<(usize, u32)>,
}

impl Restore<'_> {
    fn native_error(operation: &str, raw: usize) -> io::Error {
        let error = io::Error::last_os_error();
        if super::fd_trace_enabled() {
            eprintln!(
                "[fd] inheritance-error pid={} operation={operation} raw={raw:#x} error={error}",
                std::process::id()
            );
        }
        error
    }

    fn set(&mut self, raw: usize, inherit: bool, restore_override: Option<u32>) -> io::Result<()> {
        if self.handles.iter().any(|&(handle, _)| handle == raw) {
            return Ok(());
        }
        let mut previous = 0;
        if unsafe { GetHandleInformation(raw as HANDLE, &mut previous) } == 0 {
            return Err(Self::native_error("query", raw));
        }
        self.handles.try_reserve(1).map_err(io::Error::other)?;
        // Record before mutation. A later error must restore earlier handles,
        // and no error may invoke the caller with an incomplete exclusion set.
        self.handles.push((
            raw,
            restore_override.unwrap_or(previous & HANDLE_FLAG_INHERIT),
        ));
        if unsafe {
            SetHandleInformation(
                raw as HANDLE,
                HANDLE_FLAG_INHERIT,
                if inherit { HANDLE_FLAG_INHERIT } else { 0 },
            )
        } == 0
        {
            return Err(Self::native_error("set", raw));
        }
        Ok(())
    }
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        for &(raw, previous) in self.handles.iter().rev() {
            // The table lock and auxiliary Arc pins still own every handle.
            if unsafe { SetHandleInformation(raw as HANDLE, HANDLE_FLAG_INHERIT, previous) } == 0 {
                eprintln!(
                    "kinakaze: failed to restore handle inheritance: {}",
                    io::Error::last_os_error()
                );
            }
        }
    }
}

pub(super) fn with_filter<T>(
    exec: bool,
    snapshot: Option<&super::ExecFdSnapshot>,
    operation: impl FnOnce() -> T,
) -> io::Result<T> {
    let table = super::table()
        .write()
        .map_err(|_| io::Error::other("fd table poisoned"))?;
    if let Some(snapshot) = snapshot {
        let current: Vec<_> = table
            .slots
            .enumerated()
            .filter_map(|(fd, entry)| entry.map(|entry| (fd, entry)))
            .collect();
        let same = snapshot.0.len() == current.len()
            && snapshot
                .0
                .iter()
                .zip(&current)
                .all(|((a_fd, a), (b_fd, b))| {
                    a_fd == b_fd
                        && a.generation == b.generation
                        && a.raw == b.raw
                        && a.kind == b.kind
                        && a.flags.0 == b.flags.0
                        && a.description_id == b.description_id
                        && a.offset == b.offset
                });
        if !same {
            return Err(io::Error::from_raw_os_error(1237)); // ERROR_RETRY; no native child created
        }
    }
    let fifo_pins = fifo::auxiliary_handles().map_err(|error| {
        io::Error::other(format!("FIFO descriptor registry: Linux errno {error}"))
    })?;
    let mut overlay_pins = super::mount::overlay::auxiliary_handles()
        .map_err(|error| io::Error::other(format!("Overlay inode registry: {error}")))?;
    overlay_pins.extend(
        super::pipe_inode::auxiliary_handles()
            .map_err(|error| io::Error::other(format!("Pipe inode registry: {error}")))?,
    );
    overlay_pins.extend(
        super::ofd::auxiliary_handles()
            .map_err(|error| io::Error::other(format!("Open description registry: {error}")))?,
    );
    overlay_pins.extend(
        super::usernet::auxiliary_handles()
            .map_err(|error| io::Error::other(format!("Network description registry: {error}")))?,
    );
    overlay_pins.extend(
        super::mount::api::auxiliary_handles(table.slots.iter().flatten().copied())
            .map_err(|error| io::Error::other(format!("Mount context inode registry: {error}")))?,
    );
    let mut restore = Restore {
        table,
        fifo_pins,
        unix_pins: super::unix::auxiliary_handles()
            .map_err(|error| io::Error::other(format!("Unix inode registry: {error}")))?,
        unix_state_pins: super::unix::auxiliary_state_handles()
            .map_err(|error| io::Error::other(format!("Unix ancillary registry: {error}")))?,
        overlay_pins,
        handles: Vec::new(),
    };
    let descriptors: Vec<_> = restore
        .table
        .slots
        .enumerated()
        .filter_map(|(fd, entry)| entry.map(|entry| (fd, entry)))
        .collect();
    for (index, entry) in descriptors {
        if entry.raw != 0 && !entry.flags.contains(FdFlags::BORROWED) {
            let inherit = exec
                && entry.kind != FdKind::Socket
                && !entry.flags.contains(FdFlags::CLOSE_ON_EXEC);
            if super::fd_trace_enabled() {
                eprintln!(
                    "[fd] inheritance pid={} exec={exec} fd={index} kind={:?} generation={} raw={:#x} inherit={inherit}",
                    std::process::id(),
                    entry.kind,
                    entry.generation,
                    entry.raw
                );
            }
            restore.set(entry.raw, inherit, None)?;
        }
    }
    for index in 0..restore.fifo_pins.len() {
        let auxiliary = &restore.fifo_pins[index];
        let attached = restore
            .table
            .slots
            .get(auxiliary.fd as usize)
            .and_then(|slot| *slot)
            .filter(|entry| {
                entry.kind == FdKind::Fifo
                    && entry.generation == auxiliary.entry.generation
                    && entry.description_id == auxiliary.entry.description_id
                    && entry.raw == auxiliary.entry.raw
            });
        let inherit =
            exec && attached.is_some_and(|entry| !entry.flags.contains(FdFlags::CLOSE_ON_EXEC));
        let raw = auxiliary.raw();
        // A marker detached before this snapshot must not be made inheritable
        // again merely because an operation-local Arc is still retiring it.
        restore.set(raw, inherit, attached.is_none().then_some(0))?;
    }
    // Duplicated descriptors may share a pin; retain it if any surviving fd
    // needs it, irrespective of the order CLOEXEC and ordinary aliases occur.
    let mut unix_handles = std::collections::HashMap::new();
    for (fd, pin) in &restore.unix_pins {
        let inherit = exec
            && restore
                .table
                .slots
                .get(*fd as usize)
                .and_then(|slot| *slot)
                .is_some_and(|entry| {
                    entry.kind == FdKind::UnixSocket
                        && !entry.flags.contains(FdFlags::CLOSE_ON_EXEC)
                });
        *unix_handles.entry(pin.raw()).or_insert(false) |= inherit;
    }
    for (fd, pin) in &restore.unix_state_pins {
        let inherit = exec
            && restore
                .table
                .slots
                .get(*fd as usize)
                .and_then(|slot| *slot)
                .is_some_and(|entry| {
                    entry.kind == FdKind::UnixSocket
                        && !entry.flags.contains(FdFlags::CLOSE_ON_EXEC)
                });
        *unix_handles.entry(pin.raw() as usize).or_insert(false) |= inherit;
    }
    for (raw, inherit) in unix_handles {
        restore.set(raw, inherit, None)?;
    }
    let mut overlay_handles = std::collections::HashMap::new();
    for (id, pin) in &restore.overlay_pins {
        let inherit = exec
            && restore.table.slots.iter().flatten().any(|entry| {
                entry.description_id == *id && !entry.flags.contains(FdFlags::CLOSE_ON_EXEC)
            });
        *overlay_handles.entry(pin.raw() as usize).or_insert(false) |= inherit;
    }
    for (raw, inherit) in overlay_handles {
        restore.set(raw, inherit, None)?;
    }
    Ok(operation())
}
