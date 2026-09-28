//! Duplication publishes the descriptor and its side tables under one lock.
//! Native retirement follows publication, outside that lock.
use super::*;
use std::collections::HashMap;
use std::sync::{MutexGuard, RwLockWriteGuard};

#[derive(Clone, Copy)]
pub enum DuplicateTarget {
    Lowest,
    AtLeast(i32),
    Exactly(i32),
}

enum NativeCopy {
    Handle(fs::object::Object),
    Socket(socket::RightsSocket),
    None,
}
impl NativeCopy {
    fn prepare(source: FdEntry) -> Result<Self, i32> {
        if source.raw == 0 && source.kind.allows_missing_host_handle() {
            return Ok(Self::None);
        }
        if source.kind == FdKind::Socket {
            let protocol = socket::export_rights(source.raw, std::process::id())?;
            return socket::import_rights(&protocol).map(Self::Socket);
        }
        fs::object::Object::duplicate(source.raw as _).map(Self::Handle)
    }
    fn raw(&self) -> usize {
        match self {
            Self::Handle(handle) => handle.raw() as usize,
            Self::Socket(socket) => socket.0,
            Self::None => 0,
        }
    }
    fn commit(self) {
        match self {
            Self::Handle(handle) => {
                handle.into_raw();
            }
            Self::Socket(socket) => {
                socket.into_raw();
            }
            Self::None => {}
        }
    }
}

fn synthetic_kind(kind: FdKind) -> bool {
    matches!(
        kind,
        FdKind::Synthetic | FdKind::SyntheticDirectory | FdKind::CgroupFile | FdKind::ProcSysctl
    )
}
struct SyntheticPublication {
    descriptions: RwLockWriteGuard<'static, SyntheticTable>,
    cgroups: MutexGuard<'static, HashMap<i32, String>>,
    sysctls: MutexGuard<'static, HashMap<i32, String>>,
    contents: Option<Vec<u8>>,
    path: Option<String>,
}
impl SyntheticPublication {
    fn prepare(fd: i32, source: FdEntry) -> Result<Self, i32> {
        let descriptions = synthetic().write().map_err(|_| EIO)?;
        let cgroups = cgroup_paths().lock().map_err(|_| EIO)?;
        let sysctls = proc_sysctl_paths().lock().map_err(|_| EIO)?;
        let contents = if synthetic_kind(source.kind) {
            Some(
                descriptions
                    .get(&fd)
                    .filter(|(generation, _)| *generation == source.generation)
                    .ok_or(EBADF)?
                    .1
                    .clone(),
            )
        } else {
            None
        };
        let path = match source.kind {
            FdKind::CgroupFile => Some(cgroups.get(&fd).ok_or(EBADF)?.clone()),
            FdKind::Synthetic | FdKind::ProcSysctl => Some(sysctls.get(&fd).ok_or(EBADF)?.clone()),
            _ => None,
        };
        Ok(Self {
            descriptions,
            cgroups,
            sysctls,
            contents,
            path,
        })
    }
    fn commit(mut self, fd: i32, next: FdEntry) {
        self.descriptions.remove(&fd);
        self.cgroups.remove(&fd);
        self.sysctls.remove(&fd);
        if let Some(contents) = self.contents.take() {
            self.descriptions.insert(fd, (next.generation, contents));
        }
        if let Some(path) = self.path.take() {
            if next.kind == FdKind::CgroupFile {
                self.cgroups.insert(fd, path);
            } else {
                self.sysctls.insert(fd, path);
            }
        }
    }
}

pub fn duplicate_descriptor(
    oldfd: i32,
    target: DuplicateTarget,
    cloexec: bool,
) -> Result<i32, i32> {
    let limit = job::current_nofile_limit()?;
    let mut table = table().write().map_err(|_| EIO)?;
    let source = table
        .slots
        .get(usize::try_from(oldfd).map_err(|_| EBADF)?)
        .and_then(|entry| *entry)
        .ok_or(EBADF)?;
    let fd = match target {
        DuplicateTarget::Lowest => table.first_free_between(0, limit).ok_or(EMFILE)? as i32,
        DuplicateTarget::AtLeast(floor) => {
            if floor < 0 || floor as usize >= limit {
                return Err(EINVAL);
            }
            table
                .first_free_between(floor as usize, limit)
                .ok_or(EMFILE)? as i32
        }
        DuplicateTarget::Exactly(fd) => {
            if fd == oldfd {
                return Ok(fd);
            }
            if fd < 0 || fd as usize >= limit {
                return Err(EBADF);
            }
            if table.slots.is_reserved(fd as usize) {
                return Err(EBUSY);
            }
            fd
        }
    };
    // A raw IoRing duplicate cannot retain its completion queues yet. Refuse
    // before touching the destination, including when it is already open.
    if source.kind == FdKind::IoRing {
        return Err(EOPNOTSUPP);
    }
    let copy = NativeCopy::prepare(source)?;
    let flags = FdFlags(
        (source.flags.0 & !(FdFlags::CLOSE_ON_EXEC.0 | FdFlags::BORROWED.0))
            | if cloexec { FdFlags::CLOSE_ON_EXEC.0 } else { 0 },
    );
    validate_descriptor_flags(source.kind, flags)?;
    let inheritance =
        exec_inheritance::DescriptorInheritance::prepare(copy.raw(), source.kind, flags)?;
    let previous = table.slots.get(fd as usize).and_then(|entry| *entry);
    let exclusion = if let Some(old) = previous.filter(|old| {
        old.raw != 0
            && !matches!(old.kind, FdKind::Socket | FdKind::Fifo)
            && (!old.flags.contains(FdFlags::BORROWED)
                || (kinakaze_runtime::authority::get().is_some()
                    && !table.slots.enumerated().any(|(other, entry)| {
                        other != fd as usize && entry.is_some_and(|e| e.raw == old.raw)
                    })))
    }) {
        Some(exec_inheritance::DescriptorInheritance::exclude(old.raw)?)
    } else {
        None
    };
    let next = FdEntry {
        raw: copy.raw(),
        flags,
        generation: table.next_generation,
        ..source
    };
    let synthetic =
        if synthetic_kind(source.kind) || previous.is_some_and(|e| synthetic_kind(e.kind)) {
            Some(SyntheticPublication::prepare(oldfd, source)?)
        } else {
            None
        };
    let unix = if source.kind == FdKind::UnixSocket
        || previous.is_some_and(|e| e.kind == FdKind::UnixSocket)
    {
        Some(unix::prepare_duplicate(
            (source.kind == FdKind::UnixSocket).then_some(oldfd),
            fd,
            next.raw,
        )?)
    } else {
        None
    };
    let fifo = if source.kind == FdKind::Fifo || previous.is_some_and(|e| e.kind == FdKind::Fifo) {
        Some(fifo::prepare_duplicate(
            (source.kind == FdKind::Fifo).then_some(source),
            fd,
            previous,
            next,
        )?)
    } else {
        None
    };
    // Publish the shared pipe/socket identity before detaching the old entry.
    // Failure here leaves its descriptor and all local side tables intact.
    if previous.is_some() {
        pipe_inode::replace_entry(fd, next)?;
    } else {
        pipe_inode::publish_entry(fd, next)?;
    }
    let mut retired = previous
        .map(|_| RetiredDescriptor::detach(&mut table, fd, true))
        .transpose()?;
    table.insert_at_description(
        fd,
        next.raw,
        next.kind,
        next.flags,
        next.offset,
        next.description_id,
    );
    debug_assert_eq!(
        table.slots[fd as usize].unwrap().generation,
        next.generation
    );
    if let Some(synthetic) = synthetic {
        synthetic.commit(fd, next);
    }
    if let Some(unix) = unix {
        let detached = unix.commit();
        if let Some(retired) = &mut retired {
            retired.detached = detached;
        } else {
            debug_assert!(detached.is_none());
        }
    }
    let fifo_retired = fifo.and_then(|fifo| fifo.commit());
    if let Some(exclusion) = exclusion {
        exclusion.commit();
    }
    inheritance.commit();
    copy.commit();
    platform::sync_standard(fd, next.raw);
    drop(table);
    if let Some(retired) = retired {
        let _ = retired.release(fd);
    }
    drop(fifo_retired);
    Ok(fd)
}
