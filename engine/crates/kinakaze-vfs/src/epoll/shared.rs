//! Registration metadata shared by every alias of an epoll open description.
use super::*;
use crate::{
    fs::object::Object,
    mount::shared::Store,
    state_codec::{Reader, word},
};
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::WAIT_ABANDONED;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
};

const MAGIC: u64 = u64::from_le_bytes(*b"CYEPOLL2");
pub(super) struct Gate(Arc<Object>);
impl Gate {
    pub fn acquire() -> Result<Self, i32> {
        static MUTEX: OnceLock<Arc<Object>> = OnceLock::new();
        let mutex = match MUTEX.get() {
            Some(value) => value.clone(),
            None => {
                let name: Vec<u16> = format!(
                    "Local\\kinakaze.epoll.v1.{}.guard\0",
                    kinakaze_runtime::authority::domain_id()
                )
                .encode_utf16()
                .collect();
                let value = Arc::new(Object::owned(unsafe {
                    CreateMutexW(std::ptr::null(), 0, name.as_ptr())
                })?);
                let _ = MUTEX.set(value);
                MUTEX.get().unwrap().clone()
            }
        };
        match unsafe { WaitForSingleObject(mutex.raw(), INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self(mutex)),
            _ => Err(EIO),
        }
    }
}
impl Drop for Gate {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.raw());
        }
    }
}

pub(super) fn encode(items: &HashMap<RegistrationKey, Registration>) -> Vec<u8> {
    let mut keys: Vec<_> = items.keys().copied().collect();
    keys.sort_unstable_by_key(|key| (key.description_id, key.fd));
    let mut out = Vec::with_capacity(8 + keys.len() * 104);
    word(&mut out, MAGIC);
    for key in keys {
        let r = &items[&key];
        for value in [
            key.fd as u64,
            key.description_id,
            r.kind.fork_code() as u64,
            r.flags.0 as u64,
            r.lifetime,
            r.object,
            r.interest as u64,
            r.data,
            u64::from(r.disarmed),
            r.reported as u64,
            r.readiness_revision,
            r.eventfd_edges[0],
            r.eventfd_edges[1],
        ] {
            word(&mut out, value);
        }
    }
    out
}
pub(super) fn decode(
    bytes: &[u8],
    previous: &HashMap<RegistrationKey, Registration>,
) -> Result<HashMap<RegistrationKey, Registration>, i32> {
    let mut input = Reader(bytes);
    if input.word()? != MAGIC || input.0.len() % 104 != 0 {
        return Err(EIO);
    }
    let mut items = HashMap::new();
    while !input.0.is_empty() {
        let key = RegistrationKey {
            fd: input.word()? as i32,
            description_id: input.word()?,
        };
        let kind = FdKind::from_fork_code(input.word()? as u32);
        let flags = FdFlags(input.word()? as u32);
        let lifetime = input.word()?;
        let mut registration = registration(kind, flags);
        registration.lifetime = lifetime;
        registration.object = input.word()?;
        registration.interest = input.word()? as u32;
        registration.data = input.word()?;
        registration.disarmed = input.word()? != 0;
        registration.reported = input.word()? as u32;
        registration.readiness_revision = input.word()?;
        registration.eventfd_edges = [input.word()?, input.word()?];
        // Weak membership cannot keep a pipe end, socket or timer alive. The
        // normal OFD pin covers native fork, exec and queued SCM_RIGHTS.
        if lifetime != 0 {
            match Store::user_object(lifetime, false) {
                Ok(_) => {}
                Err(crate::ENOENT) => continue,
                Err(error) => return Err(error),
            }
        }
        if let Some(old) = previous.get(&key).filter(|old| old.poll_fd >= 0) {
            registration.poll_fd = old.poll_fd;
            registration.base_handle = old.base_handle;
        } else {
            resolve(key, &mut registration)?;
        }
        if items.insert(key, registration).is_some() {
            return Err(EIO);
        }
    }
    Ok(items)
}
pub(super) fn resolve(key: RegistrationKey, registration: &mut Registration) -> Result<(), i32> {
    if let Some((fd, entry)) = crate::get_by_description_id(key.description_id) {
        if entry.kind != registration.kind {
            return Err(EIO);
        }
        registration.poll_fd = fd;
        registration.base_handle = if entry.kind == FdKind::Socket {
            base_handle(entry.raw as SOCKET)?
        } else {
            entry.raw
        };
    }
    Ok(())
}
pub(super) fn registration(kind: FdKind, flags: FdFlags) -> Registration {
    Registration {
        kind,
        flags,
        lifetime: 0,
        object: 0,
        base_handle: 0,
        poll_fd: -1,
        interest: 0,
        data: 0,
        disarmed: false,
        reported: 0,
        readiness_revision: 0,
        eventfd_edges: [0; 2],
        is_socket: kind == FdKind::Socket,
        is_packet: flags.contains(FdFlags::PACKET_SOCKET),
        packet_output_blocked: false,
        is_unix: kind == FdKind::UnixSocket,
        is_eventfd: matches!(
            kind,
            FdKind::Event
                | FdKind::EventFd
                | FdKind::TimerFd
                | FdKind::SignalFd
                | FdKind::ProcMounts
                | FdKind::MessageQueue
                | FdKind::SysfsFile
        ),
        is_netlink: kind == FdKind::NetlinkSocket,
        is_inotify: kind == FdKind::Inotify,
        is_pipe: kind == FdKind::Pipe,
        is_fifo: kind == FdKind::Fifo,
        pipe_readable: flags.contains(FdFlags::PIPE_READ_END),
        pipe_writable: flags.contains(FdFlags::PIPE_WRITE_END),
        pipe_overlapped: flags.contains(FdFlags::OVERLAPPED),
        is_pty: matches!(kind, FdKind::PtyMaster | FdKind::PtySlave | FdKind::Console),
    }
}
pub(super) fn attach(fd: i32, entry: crate::FdEntry, store: Store) -> Result<EpollSet, i32> {
    let name: Vec<u16> = format!(
        "Local\\kinakaze.epoll-ready.v1.{}.{}\0",
        kinakaze_runtime::authority::domain_id(),
        store.id()
    )
    .encode_utf16()
    .collect();
    let event = Object::owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) })?;
    let (revision, registrations) =
        store.read_with_revision(|data| decode(data, &HashMap::new()))?;
    Ok(EpollSet {
        shared_revision: revision,
        description_id: entry.description_id,
        descriptor: fd,
        registrations,
        wake_handle: event.raw() as usize,
        shared: Some(Arc::new(store)),
        _wake_owner: Some(event),
    })
}
