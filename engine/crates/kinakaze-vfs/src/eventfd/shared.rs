//! One pagefile-backed counter for every dup/fork/SCM_RIGHTS reference.
use super::{EFD_CLOEXEC, EFD_NONBLOCK, EFD_SEMAPHORE};
use crate::mount::shared::{self, Store};
use crate::state_codec::{Reader, word};
use crate::{EAGAIN, EINTR, EINVAL, EIO, FdFlags, FdKind};

const MAGIC: u64 = u64::from_le_bytes(*b"CYEVFD02");
fn encode(counter: u64, semaphore: bool, edges: [u64; 2]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for n in [MAGIC, counter, u64::from(semaphore), edges[0], edges[1]] {
        word(&mut bytes, n);
    }
    bytes
}
fn decode(bytes: &[u8]) -> Result<(u64, bool, [u64; 2]), i32> {
    let mut r = Reader(bytes);
    if r.word()? != MAGIC {
        return Err(EIO);
    }
    let count = r.word()?;
    let semaphore = r.word()? != 0;
    let edges = [r.word()?, r.word()?];
    r.end()?;
    Ok((count, semaphore, edges))
}
fn description(fd: i32) -> Result<std::sync::Arc<Store>, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(usize::try_from(fd).map_err(|_| crate::EBADF)?)
        .and_then(|entry| *entry)
        .ok_or(crate::EBADF)?;
    if entry.kind != FdKind::EventFd {
        return Err(crate::EBADF);
    }
    crate::ofd::object_store(entry)
}
fn ready_events(store: &Store) -> Result<&super::ReadyEvents, i32> {
    store
        .eventfd_events
        .get_or_init(|| {
            // Serialize first creation with publishers in every process. Existing
            // named events keep their levels; a new pair starts at the live count.
            store
                .inspect_exclusive(false, |bytes| {
                    super::ReadyEvents::new(store.id(), decode(bytes)?.0)
                })?
                .ok_or(EIO)
        })
        .as_ref()
        .map_err(|error| *error)
}
fn update_counter<T>(
    store: &Store,
    change: impl FnOnce(&[u8]) -> Result<(Vec<u8>, T), i32>,
) -> Result<T, i32> {
    let events = ready_events(store)?;
    store.update_notified(change, |bytes| {
        // All successful changes below encode exactly this counter format.
        if let Ok((count, _, _)) = decode(bytes) {
            events.publish(count);
        }
    })
}

pub(crate) fn prepare_wait(
    fd: i32,
    readable: bool,
    writable: bool,
) -> Result<super::readiness::Wait, i32> {
    let store = description(fd)?;
    let events = ready_events(&store)?;
    let mut handles = Vec::with_capacity(2);
    if readable {
        handles.push(events.readable);
    }
    if writable {
        handles.push(events.writable);
    }
    Ok(super::readiness::Wait {
        _store: store,
        handles,
    })
}
pub fn create_eventfd(initval: u32, flags: i32) -> Result<i32, i32> {
    if flags & !(EFD_CLOEXEC | EFD_NONBLOCK | EFD_SEMAPHORE) != 0 {
        return Err(EINVAL);
    }
    let store = shared::new_object()?;
    store.update(|_| {
        Ok((
            encode(initval as u64, flags & EFD_SEMAPHORE != 0, [0; 2]),
            (),
        ))
    })?;
    let mut fd_flags = FdFlags::READ_ACCESS.union(FdFlags::WRITE_ACCESS);
    if flags & EFD_CLOEXEC != 0 {
        fd_flags = fd_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & EFD_NONBLOCK != 0 {
        fd_flags = fd_flags.union(FdFlags::NONBLOCK);
    }
    let fd = store.descriptor_kind(FdKind::EventFd, fd_flags)?;
    if let Err(e) = crate::ofd::promote(fd) {
        let _ = crate::close(fd);
        return Err(e);
    }
    Ok(fd)
}
fn pause() -> Result<(), i32> {
    if crate::signal::deliver_pending() == crate::signal::Delivery::Interrupted {
        return Err(EINTR);
    }
    let interrupt = crate::interrupt::current();
    if interrupt.is_null() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    } else {
        crate::signal::register_waiter();
        let result =
            unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(interrupt, 1) };
        crate::signal::unregister_waiter();
        if result == windows_sys::Win32::Foundation::WAIT_FAILED {
            return Err(EIO);
        }
    }
    Ok(())
}
pub fn read_eventfd(fd: i32, buffer: &mut [u8], nonblock: bool) -> Result<usize, i32> {
    if buffer.len() < 8 {
        return Err(EINVAL);
    }
    let store = description(fd)?;
    loop {
        let result = update_counter(&store, |bytes| {
            let (count, semaphore, mut edges) = decode(bytes)?;
            if count == 0 {
                return Err(EAGAIN);
            }
            let value = if semaphore { 1 } else { count };
            edges[1] = edges[1].wrapping_add(1);
            Ok((encode(count - value, semaphore, edges), value))
        });
        match result {
            Ok(value) => {
                buffer[..8].copy_from_slice(&value.to_ne_bytes());
                return Ok(8);
            }
            Err(EAGAIN) if !nonblock && !crate::get(fd)?.flags.contains(FdFlags::NONBLOCK) => {
                pause()?
            }
            Err(e) => return Err(e),
        }
    }
}
pub fn write_eventfd(fd: i32, buffer: &[u8], nonblock: bool) -> Result<usize, i32> {
    if buffer.len() < 8 {
        return Err(EINVAL);
    }
    let value = u64::from_ne_bytes(buffer[..8].try_into().unwrap());
    if value == u64::MAX {
        return Err(EINVAL);
    }
    let store = description(fd)?;
    loop {
        let result = update_counter(&store, |bytes| {
            let (count, semaphore, mut edges) = decode(bytes)?;
            if count == u64::MAX || value > u64::MAX - 1 - count {
                return Err(EAGAIN);
            }
            edges[0] = edges[0].wrapping_add(1);
            Ok((encode(count + value, semaphore, edges), ()))
        });
        match result {
            Ok(()) => return Ok(8),
            Err(EAGAIN) if !nonblock && !crate::get(fd)?.flags.contains(FdFlags::NONBLOCK) => {
                pause()?
            }
            Err(e) => return Err(e),
        }
    }
}
pub fn poll_eventfd(fd: i32) -> Result<(bool, bool), i32> {
    let (readable, writable, _) = poll_status(fd)?;
    Ok((readable, writable))
}
pub fn poll_status(fd: i32) -> Result<(bool, bool, bool), i32> {
    let (readable, writable, overflow, _) = poll_status_with_edges(fd)?;
    Ok((readable, writable, overflow))
}
/// Linux wakes EPOLLIN on every write and EPOLLOUT on every read, even when
/// the counter's readiness level does not change. Keep those generations in
/// the shared counter so dup, fork, and transferred descriptors see each edge.
pub(crate) fn poll_status_with_edges(fd: i32) -> Result<(bool, bool, bool, [u64; 2]), i32> {
    let (count, _, edges) = description(fd)?.read_with(decode)?;
    Ok((count != 0, count < u64::MAX - 1, count == u64::MAX, edges))
}
pub fn forget_eventfd(_fd: i32) {}

/// Own the counter section, so asynchronous completion cannot hit a reused fd.
pub(crate) struct CompletionSignal(std::sync::Arc<Store>);
pub(crate) fn completion_signal(fd: i32) -> Result<CompletionSignal, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|entry| *entry)
        .ok_or(crate::EBADF)?;
    if entry.kind != FdKind::EventFd {
        return Err(EINVAL);
    }
    Ok(CompletionSignal(crate::ofd::object_store(entry)?))
}
impl CompletionSignal {
    pub(crate) fn signal(&self) -> Result<(), i32> {
        update_counter(&self.0, |bytes| {
            let (count, semaphore, mut edges) = decode(bytes)?;
            edges[0] = edges[0].wrapping_add(1);
            Ok((encode(count.saturating_add(1), semaphore, edges), ()))
        })
    }
}
