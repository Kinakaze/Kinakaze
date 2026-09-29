//! Pollable mount tables with a shared read snapshot and acknowledgement.
use crate::{mount::shared, *};

const MAGIC: &[u8; 8] = b"KPMNT001";

pub(crate) fn supports(path: &str) -> bool {
    let Ok((path, _scope)) = super::instance::enter(path) else {
        return false;
    };
    matches!(super::classify(&path), Some(super::Node::Mountinfo(pid) | super::Node::Mounts(pid)) if pid == job::process_id())
}

fn data(fd: i32) -> Result<shared::Store, i32> {
    if get(fd)?.kind != FdKind::ProcMounts {
        return Err(EBADF);
    }
    shared::object_fd(fd)
}

fn decode(bytes: &[u8]) -> Result<(u64, u64, &str, &[u8]), i32> {
    if bytes.len() < 32 || &bytes[..8] != MAGIC {
        return Err(EIO);
    }
    let namespace = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let revision = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
    let length =
        usize::try_from(u64::from_le_bytes(bytes[24..32].try_into().unwrap())).map_err(|_| EIO)?;
    let end = 32usize
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or(EIO)?;
    let path = std::str::from_utf8(&bytes[32..end]).map_err(|_| EIO)?;
    Ok((namespace, revision, path, &bytes[end..]))
}

fn snapshot(path: &str, namespace: &shared::Store) -> Result<Vec<u8>, i32> {
    loop {
        let revision = namespace.revision();
        let records = super::mounts::records(job::process_id())?;
        let contents = if path.ends_with("/mountinfo") {
            super::mounts::mountinfo(&records)
        } else {
            super::mounts::mounts(&records)
        }
        .into_bytes();
        if revision != namespace.revision() {
            continue;
        }
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&namespace.id().to_le_bytes());
        bytes.extend_from_slice(&revision.to_le_bytes());
        bytes.extend_from_slice(&(path.len() as u64).to_le_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(&contents);
        return Ok(bytes);
    }
}

pub(crate) fn open(path: &str, flags: FdFlags) -> Result<i32, i32> {
    let object = shared::new_object()?;
    let namespace = shared::get()?;
    namespace.lease_kernel(object.kernel_key())?;
    object.replace(&snapshot(path, &namespace)?)?;
    object.descriptor_kind(FdKind::ProcMounts, flags.union(FdFlags::SEEKABLE))
}

pub fn poll(fd: i32) -> Result<u32, i32> {
    let bytes = data(fd)?.read()?.1;
    let (namespace, revision, _, _) = decode(&bytes)?;
    let changed = shared::namespace(namespace)?.revision() != revision;
    Ok(epoll::EPOLLIN
        | epoll::EPOLLRDNORM
        | if changed {
            epoll::EPOLLPRI | epoll::EPOLLERR
        } else {
            0
        })
}

/// A read view for one active epoll wait. Authoritative snapshots and mount
/// revisions remain in init-retained shared objects; this only pins mappings.
pub(crate) struct PollView {
    data: shared::Store,
    namespace: shared::Store,
    snapshot_revision: u64,
    acknowledged: u64,
}

impl PollView {
    pub(crate) fn new(fd: i32) -> Result<Self, i32> {
        let data = data(fd)?;
        let (snapshot_revision, (namespace, acknowledged)) = data.read_with_revision(|bytes| {
            let (namespace, revision, _, _) = decode(bytes)?;
            Ok((namespace, revision))
        })?;
        Ok(Self {
            data,
            namespace: shared::namespace(namespace)?,
            snapshot_revision,
            acknowledged,
        })
    }

    pub(crate) fn poll(&mut self) -> Result<u32, i32> {
        if self.data.revision() != self.snapshot_revision {
            let (version, (namespace, acknowledged)) = self.data.read_with_revision(|bytes| {
                let (namespace, revision, _, _) = decode(bytes)?;
                Ok((namespace, revision))
            })?;
            if namespace != self.namespace.id() {
                self.namespace = shared::namespace(namespace)?;
            }
            self.snapshot_revision = version;
            self.acknowledged = acknowledged;
        }
        Ok(epoll::EPOLLIN
            | epoll::EPOLLRDNORM
            | if self.namespace.revision() != self.acknowledged {
                epoll::EPOLLPRI | epoll::EPOLLERR
            } else {
                0
            })
    }
}

pub(crate) fn path(fd: i32) -> Result<String, i32> {
    let bytes = data(fd)?.read()?.1;
    Ok(decode(&bytes)?.2.to_owned())
}

pub fn read_at(fd: i32, buffer: &mut [u8], offset: u64) -> Result<usize, i32> {
    if buffer.is_empty() {
        return Ok(0);
    }
    let count = data(fd)?.update(|bytes| {
        let (namespace, _, path, _) = decode(bytes)?;
        let mut next = bytes.to_vec();
        if offset == 0 {
            let current = shared::get()?;
            if current.id() != namespace {
                return Err(EOPNOTSUPP);
            }
            next = snapshot(path, &current)?;
        }
        let contents = decode(&next)?.3;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(contents.len());
        let count = buffer.len().min(contents.len() - start);
        buffer[..count].copy_from_slice(&contents[start..start + count]);
        Ok((next, count))
    })?;
    if offset == 0 {
        epoll::readiness_consumed(get(fd)?.description_id, epoll::EPOLLPRI | epoll::EPOLLERR);
    }
    Ok(count)
}
