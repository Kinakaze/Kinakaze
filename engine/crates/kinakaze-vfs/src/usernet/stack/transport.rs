//! Event-driven IP link for one shared protocol endpoint. It owns only a local
//! UDP transport reference and a waiter subscription; the protocol state has no
//! owner process. Fork/rights integration must explicitly duplicate the native
//! socket and pin the shared section before publishing the receiving fd.
use super::{
    Ingress, TcpState,
    shared::{Listener, Notification, SharedEndpoint, now_ms},
};
use crate::{EIO, ETIMEDOUT};
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::os::windows::io::AsRawHandle;
use std::sync::Arc;

const OUTPUT_LIMIT: usize = 1024 * 1024;
const BURST: usize = 64;

pub(crate) fn configure_link(raw: usize) -> Result<(), i32> {
    use windows_sys::Win32::Networking::WinSock::WSAIoctl;
    // A private IP link multiplexes guest TCP connections. An ICMP error for
    // a retired relay is not a reset of its listener or any other connection.
    // Real guest errors are carried by the inner protocol, never UDP recvfrom.
    let report = 0i32;
    let mut returned = 0u32;
    if unsafe {
        WSAIoctl(
            raw,
            0x9800000c,
            (&report as *const i32).cast(),
            4,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            None,
        )
    } != 0
    {
        return Err(crate::socket::last_wsa_errno());
    }
    Ok(())
}

fn trace(stage: &str, error: &i32) {
    if let Some(path) = std::env::var_os("KINAKAZE_GATEWAY_TRACE_DIR") {
        use std::io::Write;
        let path = std::path::Path::new(&path).join(format!("packet-{}.log", std::process::id()));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{} {stage}: {error}", now_ms());
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Interest {
    Readable,
    Writable,
    Connected,
}

#[derive(Default, Debug)]
pub struct Progress {
    pub received: usize,
    pub filtered: usize,
    pub invalid: usize,
    pub transmitted: usize,
    pub next_deadline_ms: Option<i64>,
}
impl Progress {
    fn merge(&mut self, incoming: Option<Ingress>) {
        if let Some(incoming) = incoming {
            self.received += 1;
            self.filtered += usize::from(incoming == Ingress::Filtered);
            self.invalid += usize::from(matches!(incoming, Ingress::Invalid(_)));
        }
    }
}

/// Protocol state used by the event-driven link. A listener bank and a single
/// endpoint use the same AFD transport and differ only in packet dispatch.
pub trait PacketEngine: Send + Sync {
    fn subscribe(self: &Arc<Self>) -> Result<Notification, i32>;
    fn poll_for(
        &self,
        incoming: Option<Vec<u8>>,
        waiter: &Notification,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32>;
    fn readiness(&self) -> Result<(bool, bool, bool), i32>;
    fn tcp_state(&self) -> Result<TcpState, i32>;
}
/// Namespace routing for an unconnected packet link. An absent destination is
/// a network drop. The input check runs before the protocol engine sees a frame.
pub(crate) trait PacketRouter: Send + Sync {
    fn deliver(&self, _frame: &[u8]) -> Result<bool, i32> {
        Ok(false)
    }
    fn poll_peers(&self) -> Result<Option<i64>, i32> {
        Ok(None)
    }
    fn lifetime_arenas(&self) -> Result<Vec<u64>, i32> {
        Ok(Vec::new())
    }
    fn route(&self, frame: &[u8]) -> Result<Option<SocketAddr>, i32>;
    fn accepts(&self, frame: &[u8], source: SocketAddr) -> Result<bool, i32>;
}
impl PacketEngine for SharedEndpoint {
    fn subscribe(self: &Arc<Self>) -> Result<Notification, i32> {
        SharedEndpoint::subscribe(self)
    }
    fn poll_for(
        &self,
        incoming: Option<Vec<u8>>,
        waiter: &Notification,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        SharedEndpoint::poll_for(self, incoming, waiter)
    }
    fn readiness(&self) -> Result<(bool, bool, bool), i32> {
        SharedEndpoint::readiness(self)
    }
    fn tcp_state(&self) -> Result<TcpState, i32> {
        SharedEndpoint::tcp_state(self)
    }
}
impl PacketEngine for Listener {
    fn subscribe(self: &Arc<Self>) -> Result<Notification, i32> {
        self.endpoint().subscribe()
    }
    fn poll_for(
        &self,
        incoming: Option<Vec<u8>>,
        waiter: &Notification,
    ) -> Result<(Option<Ingress>, Vec<Vec<u8>>, Option<u64>), i32> {
        Listener::poll_for(self, incoming, waiter)
    }
    fn readiness(&self) -> Result<(bool, bool, bool), i32> {
        Listener::readiness(self)
    }
    fn tcp_state(&self) -> Result<TcpState, i32> {
        Err(crate::EOPNOTSUPP)
    }
}
pub struct Transport<E: PacketEngine = SharedEndpoint> {
    endpoint: Arc<E>,
    socket: UdpSocket,
    notification: Notification,
    wait: crate::epoll::packet_wait::PacketWait,
    output: VecDeque<Vec<u8>>,
    output_bytes: usize,
    next_deadline: Option<i64>,
    router: Option<Arc<dyn PacketRouter>>,
}
fn error(error: std::io::Error) -> i32 {
    error
        .raw_os_error()
        .map(crate::socket::errno_from_wsa)
        .unwrap_or(EIO)
}
impl<E: PacketEngine> Transport<E> {
    /// The supplied datagram link must already be connected to its packet peer.
    /// No bind, host firewall change, worker thread or helper process is created.
    pub fn new(endpoint: Arc<E>, socket: UdpSocket) -> Result<Self, i32> {
        socket.peer_addr().map_err(error)?;
        Self::create(endpoint, socket, None)
    }
    pub(crate) fn routed(
        endpoint: Arc<E>,
        socket: UdpSocket,
        router: Arc<dyn PacketRouter>,
    ) -> Result<Self, i32> {
        Self::create(endpoint, socket, Some(router))
    }
    fn create(
        endpoint: Arc<E>,
        socket: UdpSocket,
        router: Option<Arc<dyn PacketRouter>>,
    ) -> Result<Self, i32> {
        use std::os::windows::io::AsRawSocket;
        configure_link(socket.as_raw_socket() as usize)?;
        socket.set_nonblocking(true).map_err(error)?;
        let notification = endpoint.subscribe()?;
        let wait = crate::epoll::packet_wait::PacketWait::new()?;
        Ok(Self {
            endpoint,
            socket,
            notification,
            wait,
            output: VecDeque::new(),
            output_bytes: 0,
            next_deadline: None,
            router,
        })
    }
    pub fn endpoint(&self) -> &Arc<E> {
        &self.endpoint
    }
    pub(crate) fn clone_link(&self) -> Result<UdpSocket, i32> {
        self.socket.try_clone().map_err(error)
    }
    pub(crate) fn output_blocked(&self) -> bool {
        !self.output.is_empty()
    }
    pub(crate) fn enqueue_packets(&mut self, frames: Vec<Vec<u8>>) -> Result<(), i32> {
        self.output(frames)
    }
    pub(crate) fn lifetime_arenas(&self) -> Result<Vec<u64>, i32> {
        self.router
            .as_ref()
            .map(|router| router.lifetime_arenas())
            .transpose()
            .map(Option::unwrap_or_default)
    }
    fn output(&mut self, frames: Vec<Vec<u8>>) -> Result<(), i32> {
        let size: usize = frames.iter().map(Vec::len).sum();
        if self.output_bytes + size > OUTPUT_LIMIT {
            return Err(crate::ENOBUFS);
        }
        self.output_bytes += size;
        self.output.extend(frames);
        Ok(())
    }
    fn flush(&mut self, progress: &mut Progress) -> Result<(), i32> {
        while let Some(frame) = self.output.front() {
            if self
                .router
                .as_ref()
                .map(|router| router.deliver(frame))
                .transpose()
                .inspect_err(|e| trace("route deliver", e))?
                .unwrap_or(false)
            {
                self.output_bytes -= frame.len();
                self.output.pop_front();
                progress.transmitted += 1;
                continue;
            }
            let sent = if let Some(router) = &self.router {
                if let Some(destination) = router
                    .route(frame)
                    .inspect_err(|e| trace("route resolve", e))?
                {
                    self.socket.send_to(frame, destination)
                } else {
                    self.output_bytes -= frame.len();
                    self.output.pop_front();
                    continue;
                }
            } else {
                self.socket.send(frame)
            };
            match sent {
                Ok(n) if n == frame.len() => {
                    self.output_bytes -= n;
                    self.output.pop_front();
                    progress.transmitted += 1;
                }
                Ok(_) => return Err(EIO),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(error(e)),
            }
        }
        Ok(())
    }
    fn poll(&mut self, input: Option<Vec<u8>>, progress: &mut Progress) -> Result<(), i32> {
        let (incoming, output, delay) = self
            .endpoint
            .poll_for(input, &self.notification)
            .inspect_err(|e| trace("engine poll", e))?;
        progress.merge(incoming);
        self.output(output)?;
        self.next_deadline = delay.map(|d| now_ms().saturating_add(d.min(i64::MAX as u64) as i64));
        self.flush(progress)
    }
    /// Bounded nonblocking work. Arming precedes every state inspection, so a
    /// concurrent write/rule change after this point cannot be lost before wait.
    pub fn drive(&mut self) -> Result<Progress, i32> {
        self.notification
            .arm()
            .inspect_err(|e| trace("driver arm", e))?;
        let mut progress = Progress::default();
        let peer_deadline = self
            .router
            .as_ref()
            .map(|router| router.poll_peers())
            .transpose()
            .inspect_err(|e| trace("poll peers", e))?
            .flatten();
        self.flush(&mut progress)?;
        if self.output_bytes >= OUTPUT_LIMIT / 2 {
            progress.next_deadline_ms = self.next_deadline;
            return Ok(progress);
        }
        self.poll(None, &mut progress)?;
        let mut input = [0; 65536];
        for _ in 0..BURST {
            if self.output_bytes >= OUTPUT_LIMIT / 2 {
                break;
            }
            match self.socket.recv_from(&mut input) {
                Ok((n, source)) => {
                    if let Some(router) = &self.router {
                        if !router
                            .accepts(&input[..n], source)
                            .inspect_err(|e| trace("route accepts", e))?
                        {
                            progress.received += 1;
                            progress.invalid += 1;
                            continue;
                        }
                    }
                    self.poll(Some(input[..n].to_vec()), &mut progress)?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(error(e)),
            }
        }
        self.next_deadline = self.next_deadline.into_iter().chain(peer_deadline).min();
        progress.next_deadline_ms = self.next_deadline;
        Ok(progress)
    }
    /// Sleep after `drive` and the caller's readiness check. An application
    /// timeout is an absolute QPC deadline, separate from protocol retransmits.
    /// EINTR is returned only after the outstanding AFD request is retired.
    pub fn wait(&mut self, application_deadline: Option<i64>) -> Result<(), i32> {
        // Queued egress already proved the link is backpressured. Immediate
        // protocol work cannot make it writable: wait for AFD instead of spinning.
        let protocol_deadline = self
            .next_deadline
            .filter(|d| self.output.is_empty() || *d > now_ms());
        let deadline = match (protocol_deadline, application_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if application_deadline.is_some_and(|d| d <= now_ms()) {
            return Err(ETIMEDOUT);
        }
        let timeout =
            deadline.map(|d| d.saturating_sub(now_ms()).clamp(0, (u32::MAX - 1) as i64) as u32);
        unsafe {
            self.wait.wait(
                &self.socket,
                self.notification.as_raw_handle(),
                !self.output.is_empty(),
                timeout,
            )
        }
    }
    pub fn wait_ready(&mut self, interest: Interest, timeout_ms: Option<u64>) -> Result<(), i32> {
        self.wait_until(timeout_ms, |endpoint| {
            let (read, write, hangup) = endpoint.readiness()?;
            let ready = match interest {
                Interest::Readable => read || hangup,
                Interest::Writable => write || hangup,
                Interest::Connected => match endpoint.tcp_state()? {
                    TcpState::Established | TcpState::CloseWait => true,
                    TcpState::Closed => return Err(crate::ECONNREFUSED),
                    _ => false,
                },
            };
            Ok(ready)
        })
    }
    /// Drive an arena while waiting for a particular accepted connection. Its
    /// shared commands notify the arena too, so another process can write to a
    /// connection while this transport sleeps without a periodic scan.
    pub fn wait_until(
        &mut self,
        timeout_ms: Option<u64>,
        mut ready: impl FnMut(&E) -> Result<bool, i32>,
    ) -> Result<(), i32> {
        let deadline = timeout_ms.map(|d| now_ms().saturating_add(d.min(i64::MAX as u64) as i64));
        loop {
            self.drive()?;
            if ready(&self.endpoint)? {
                return Ok(());
            }
            self.wait(deadline)?;
        }
    }
}

#[cfg(test)]
mod tests;
