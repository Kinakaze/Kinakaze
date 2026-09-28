//! Weak references to shared readiness state, independent of a worker's FD table.
use super::*;
use crate::mount::shared::Store;

pub(super) fn capture(fd: i32, expected: crate::FdEntry) -> Result<u64, i32> {
    if !matches!(
        expected.kind,
        FdKind::EventFd | FdKind::Inotify | FdKind::NetlinkSocket
    ) {
        return Ok(0);
    }
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|entry| *entry)
        .ok_or(EBADF)?;
    if entry.generation != expected.generation {
        return Err(EBADF);
    }
    Ok(crate::ofd::object_store(entry)?.id())
}

pub(super) fn alive(registration: &Registration) -> Result<bool, i32> {
    match Store::user_object(registration.lifetime, false) {
        Ok(_) => Ok(true),
        Err(crate::ENOENT) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Neither the description token nor its state is retained across a wait.
/// In particular, watching an object must not prevent its final close.
pub(super) fn poll(registration: &Registration) -> Result<Option<(u32, [u64; 2])>, i32> {
    let lifetime = match Store::user_object(registration.lifetime, false) {
        Ok(value) => value,
        Err(crate::ENOENT) => return Ok(None),
        Err(error) => return Err(error),
    };
    let store = match Store::user_object(registration.object, false) {
        Ok(value) => value,
        // Last close can retire the state after we observed its OFD token.
        Err(crate::ENOENT) => return Ok(None),
        Err(error) => return Err(error),
    };
    let (readable, writable, overflow, edges) = match registration.kind {
        FdKind::EventFd => crate::eventfd::poll_store(&store)?,
        FdKind::Inotify => (crate::inotify::poll_store(&store)?, false, false, [0; 2]),
        FdKind::NetlinkSocket => {
            let (readable, writable) = crate::netlink::poll_store(store, &lifetime)?;
            (readable, writable, false, [0; 2])
        }
        _ => return Err(EIO),
    };
    drop(lifetime);
    Ok(Some((
        (if readable {
            registration.interest & (EPOLLIN | EPOLLRDNORM)
        } else {
            0
        }) | (if writable {
            registration.interest & (EPOLLOUT | EPOLLWRNORM)
        } else {
            0
        }) | (if overflow { EPOLLERR } else { 0 }),
        edges,
    )))
}
