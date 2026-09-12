use super::*;
use crate::socket::{MSG_DONTWAIT, MSG_NOSIGNAL, MSG_PEEK};
use crate::usernet::stack::{
    Ingress, TcpState,
    shared::{Listener, Notification, now_ms},
    transport::{PacketEngine, Progress, Transport},
};
use std::net::UdpSocket;
use std::os::windows::io::{AsRawHandle, AsRawSocket, FromRawSocket};

enum Engine {
    Endpoint(Arc<SharedEndpoint>),
    Listener(Arc<Listener>),
}
impl PacketEngine for Engine {
    fn subscribe(self: &Arc<Self>) -> Result<Notification, i32> {
        match &**self {
            Self::Endpoint(endpoint) => endpoint.subscribe(),
            Self::Listener(listener) => listener.endpoint().subscribe(),
        }
    }
    fn poll_for(
        &self,
        incoming: Option<Vec<u8>>,
        waiter: &Notification,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        match self {
            Self::Endpoint(endpoint) => PacketEngine::poll_for(&**endpoint, incoming, waiter),
            Self::Listener(listener) => listener.poll_for(incoming, waiter),
        }
    }
    fn readiness(&self) -> Result<(bool, bool, bool), i32> {
        match self {
            Self::Endpoint(endpoint) => endpoint.readiness(),
            Self::Listener(listener) => listener.readiness(),
        }
    }
    fn tcp_state(&self) -> Result<TcpState, i32> {
        match self {
            Self::Endpoint(endpoint) => endpoint.tcp_state(),
            Self::Listener(_) => Err(crate::EOPNOTSUPP),
        }
    }
}
pub(super) struct Driver(Transport<Engine>);

fn trace_error(fd: i32, stage: &str, error: &i32) {
    if let Some(path) = std::env::var_os("KINAKAZE_GATEWAY_TRACE_DIR") {
        use std::io::Write;
        let path = std::path::Path::new(&path).join(format!("packet-{}.log", std::process::id()));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{} fd={fd} {stage}: {error}", now_ms());
        }
    }
}

pub(super) fn clone_link(fd: i32, owner: &Endpoint) -> Result<UdpSocket, i32> {
    let table = crate::table().read().map_err(|_| EIO)?;
    let entry = table
        .slots
        .get(fd as usize)
        .and_then(|entry| *entry)
        .filter(|entry| entry.kind == crate::FdKind::Socket)
        .ok_or(crate::EBADF)?;
    if !ENDPOINTS
        .lock()
        .map_err(|_| EIO)?
        .get(&entry.description_id)
        .is_some_and(|current| std::ptr::eq(Arc::as_ptr(current), owner))
    {
        return Err(crate::EBADF);
    }
    let mut kind = 0i32;
    let mut size = 4;
    if unsafe {
        windows_sys::Win32::Networking::WinSock::getsockopt(
            entry.raw,
            0xffff,
            0x1008,
            (&mut kind as *mut i32).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(crate::socket::last_wsa_errno());
    }
    if kind != 2 {
        return Err(crate::EPROTOTYPE);
    }
    // The table pins the native socket through WSADuplicateSocket's provider
    // handoff. An unrelated close/reuse cannot substitute a different link.
    let link = crate::socket::import_rights(&crate::socket::export_rights(
        entry.raw,
        std::process::id(),
    )?)?;
    Ok(unsafe { UdpSocket::from_raw_socket(link.into_raw() as _) })
}
impl Binding {
    fn with_driver<T>(
        &self,
        fd: i32,
        owner: &Endpoint,
        action: impl FnOnce(&mut Driver) -> Result<T, i32>,
    ) -> Result<T, i32> {
        let mut driver = self.driver.lock().map_err(|_| EIO)?;
        if driver.is_none() {
            let state = read(owner)?;
            if !state.bound {
                return Err(crate::EDESTADDRREQ);
            }
            let engine = match Listener::reopen(self.root.clone())? {
                Some(listener) => Engine::Listener(listener),
                None => Engine::Endpoint(self.root.clone()),
            };
            let link = clone_link(fd, owner)?;
            *driver = Some(Driver(Transport::routed(
                Arc::new(engine),
                link,
                Arc::new(route::Router::new(state.ns, Some(owner.store.id()))),
            )?));
        }
        action(driver.as_mut().unwrap())
    }
    pub(super) fn drive(&self, fd: i32, owner: &Endpoint) -> Result<Progress, i32> {
        let state = read(owner)?;
        let output = if let Some(reference) = state.packet.filter(|reference| reference.managed) {
            lifetime::reconcile(&self.root, reference.arena)?
        } else {
            Vec::new()
        };
        let progress = self
            .with_driver(fd, owner, |driver| {
                driver.0.enqueue_packets(output)?;
                driver.0.drive()
            })
            .inspect_err(|e| trace_error(fd, "transport drive", e));
        let progress = match progress {
            Ok(progress) => progress,
            Err(
                error @ (crate::ECONNRESET
                | crate::ECONNREFUSED
                | crate::ENETUNREACH
                | crate::EHOSTUNREACH
                | crate::ETIMEDOUT
                | crate::EPIPE),
            ) => {
                edit(owner, |state| {
                    state.error = error;
                    Ok(())
                })?;
                Progress::default()
            }
            Err(error) => return Err(error),
        };
        tcp::refresh(owner)?;
        Ok(progress)
    }
    fn finish_io(&self, fd: i32, owner: &Endpoint) {
        // Bytes already committed to/consumed from the shared queue cannot be
        // reported as untransferred if a later packet transmission fails.
        if let Err(error) = self.drive(fd, owner) {
            let _ = edit(owner, |state| {
                state.error = error;
                Ok(())
            });
        }
    }
}

struct Wait {
    notification: Notification,
    link: UdpSocket,
    native: crate::epoll::packet_wait::PacketWait,
    lifetime: Vec<crate::fifo::lifecycle::DirectoryWatch>,
}
pub(crate) struct Prepared {
    _owner: Arc<Endpoint>,
    notification: Notification,
    link: UdpSocket,
    lifetime: Vec<crate::fifo::lifecycle::DirectoryWatch>,
    pub ready: u32,
    pub deadline: Option<i64>,
    pub output_blocked: bool,
}
impl Prepared {
    pub(crate) fn events(
        &self,
    ) -> impl Iterator<Item = windows_sys::Win32::Foundation::HANDLE> + '_ {
        std::iter::once(self.notification.as_raw_handle())
            .chain(self.lifetime.iter().map(|watch| watch.event()))
    }
    pub(crate) fn socket(&self) -> usize {
        self.link.as_raw_socket() as usize
    }
}
pub(crate) fn prepare(fd: i32) -> Result<Prepared, i32> {
    use crate::epoll::{EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, EPOLLRDHUP};
    let owner = description(fd)?.ok_or(crate::ENOTSOCK)?;
    let binding = owner.packet.as_ref().ok_or(EIO)?;
    let notification = binding
        .root
        .subscribe()
        .inspect_err(|e| trace_error(fd, "subscribe", e))?;
    notification
        .arm()
        .inspect_err(|e| trace_error(fd, "arm", e))?;
    let state = read(&owner)?;
    let (deadline, output_blocked, link, lifetime) = if state.bound {
        binding
            .drive(fd, &owner)
            .inspect_err(|e| trace_error(fd, "first drive", e))?;
        let watches = binding
            .with_driver(fd, &owner, |driver| {
                driver
                    .0
                    .lifetime_arenas()?
                    .into_iter()
                    .map(lifetime::watch)
                    .collect::<Result<Vec<_>, _>>()
            })
            .inspect_err(|e| trace_error(fd, "lifetime watches", e))?;
        // Each waiter has its own directory request. Arm before rechecking so
        // another process's final token close cannot disappear before sleep.
        let progress = binding
            .drive(fd, &owner)
            .inspect_err(|e| trace_error(fd, "second drive", e))?;
        binding.with_driver(fd, &owner, |driver| {
            Ok((
                progress.next_deadline_ms,
                driver.0.output_blocked(),
                driver.0.clone_link()?,
                watches,
            ))
        })?
    } else {
        (None, false, clone_link(fd, &owner)?, Vec::new())
    };
    let state = read(&owner)?;
    let (readable, writable, hangup) = if state.listening {
        Listener::reopen(binding.root.clone())?
            .ok_or(EIO)?
            .readiness()?
    } else {
        binding.endpoint.readiness()?
    };
    let ready = if readable || state.read_shut {
        EPOLLIN
    } else {
        0
    } | if writable || state.error != 0 {
        EPOLLOUT
    } else {
        0
    } | if hangup { EPOLLHUP | EPOLLRDHUP } else { 0 }
        | if state.error != 0 { EPOLLERR } else { 0 };
    Ok(Prepared {
        _owner: owner,
        notification,
        link,
        lifetime,
        ready,
        deadline,
        output_blocked,
    })
}
pub(crate) fn queued_bytes(fd: i32, owner: &Endpoint) -> Result<usize, i32> {
    let binding = owner.packet.as_ref().ok_or(EIO)?;
    if read(owner)?.bound {
        binding.drive(fd, owner)?;
    }
    binding.endpoint.queued_bytes()
}
pub(super) fn retry<T>(
    fd: i32,
    owner: &Endpoint,
    flags: i32,
    mut action: impl FnMut(&SharedEndpoint) -> Result<T, i32>,
) -> Result<T, i32> {
    let binding = owner.packet.as_ref().ok_or(crate::ENOTSOCK)?;
    let nonblocking =
        crate::get(fd)?.flags.contains(crate::FdFlags::NONBLOCK) || flags & MSG_DONTWAIT != 0;
    let mut wait: Option<Wait> = None;
    loop {
        if let Some(wait) = &mut wait {
            wait.notification.arm()?;
            for watch in &mut wait.lifetime {
                watch.rearm()?;
            }
        }
        let progress = if read(owner)?.bound {
            binding.drive(fd, owner)?
        } else {
            Progress::default()
        };
        if read(owner)?.error != 0 {
            let error = edit(owner, |state| {
                let error = state.error;
                state.error = 0;
                Ok(error)
            })?;
            if error != 0 {
                return Err(error);
            }
        }
        match action(&binding.endpoint) {
            Err(crate::EAGAIN) if !nonblocking => {}
            value => {
                if value.is_ok() {
                    binding.finish_io(fd, owner);
                }
                return value;
            }
        }
        let Some(waiter) = &wait else {
            wait = Some(Wait {
                notification: binding.root.subscribe()?,
                link: clone_link(fd, owner)?,
                lifetime: if read(owner)?.bound {
                    binding.with_driver(fd, owner, |driver| {
                        driver
                            .0
                            .lifetime_arenas()?
                            .into_iter()
                            .map(lifetime::watch)
                            .collect::<Result<Vec<_>, _>>()
                    })?
                } else {
                    Vec::new()
                },
                native: crate::epoll::packet_wait::PacketWait::new()?,
            });
            continue; // enroll, then inspect again before sleeping
        };
        let blocked = if read(owner)?.bound {
            binding.with_driver(fd, owner, |driver| Ok(driver.0.output_blocked()))?
        } else {
            false
        };
        let timeout = progress.next_deadline_ms.map(|deadline| {
            deadline
                .saturating_sub(now_ms())
                .clamp(0, (u32::MAX - 1) as i64) as u32
        });
        let mut lifetime = crate::epoll::native_wait::FanIn::new()?;
        for watch in &waiter.lifetime {
            unsafe {
                lifetime.add(watch.event())?;
            }
        }
        unsafe {
            waiter.native.wait_extra(
                &waiter.link,
                waiter.notification.as_raw_handle(),
                lifetime.raw(),
                blocked,
                timeout,
            )?;
        }
    }
}

pub(crate) fn send(
    fd: i32,
    owner: &Endpoint,
    bytes: &[u8],
    flags: i32,
    target: Option<Address>,
) -> Result<usize, i32> {
    if flags & !(MSG_DONTWAIT | MSG_NOSIGNAL) != 0 {
        return Err(crate::EOPNOTSUPP);
    }
    let state = read(owner)?;
    if state.write_shut {
        return Err(crate::EPIPE);
    }
    let result = if state.kind == 1 {
        if target.is_some() {
            return Err(crate::EISCONN);
        }
        retry(fd, owner, flags, |endpoint| endpoint.send_tcp(bytes))
    } else {
        let target = target
            .or((state.peer.port != 0).then_some(state.peer))
            .ok_or(crate::EDESTADDRREQ)?;
        if target.loopback() && !loopback_up(state.ns)? {
            return Err(ENETUNREACH);
        }
        socket::autobind(fd, owner, target)?;
        let state = read(owner)?;
        let local = if state.local.any() {
            if target.loopback() {
                target.local_transport()
            } else {
                primary(state.ns, target.family)?
            }
        } else {
            state.local
        };
        let target = route::endpoint(target)?;
        let local = route::endpoint(local)?.addr;
        retry(fd, owner, flags, |endpoint| {
            endpoint
                .send_udp_from(bytes, target, Some(local))
                .map(|_| bytes.len())
        })
    };
    crate::epoll::notify_write(fd);
    result
}
pub(crate) fn receive(
    fd: i32,
    owner: &Endpoint,
    bytes: &mut [u8],
    flags: i32,
) -> Result<(usize, Option<Address>, usize), i32> {
    const MSG_TRUNC: i32 = 0x20;
    if flags & !(MSG_DONTWAIT | MSG_PEEK | MSG_TRUNC) != 0 {
        return Err(crate::EOPNOTSUPP);
    }
    let state = read(owner)?;
    if state.read_shut {
        return Ok((0, None, 0));
    }
    if state.kind == 1 && flags & MSG_TRUNC != 0 {
        return Err(crate::EOPNOTSUPP);
    }
    let result = if state.kind == 1 {
        retry(fd, owner, flags, |endpoint| {
            endpoint
                .recv_tcp_flags(bytes, flags & MSG_PEEK != 0)
                .map(|n| (n, Some(state.peer), n))
        })
    } else {
        retry(fd, owner, flags, |endpoint| {
            endpoint
                .recv_udp(bytes, flags & MSG_PEEK != 0)
                .map(|(n, peer, full)| (n, Some(route::address(peer)), full))
        })
    };
    if flags & MSG_PEEK == 0 {
        crate::epoll::notify_read(fd);
    }
    result
}
