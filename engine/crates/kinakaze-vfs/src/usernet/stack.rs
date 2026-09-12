//! Driverless IP transport engine. The caller owns both the engine and its
//! packet transport, and waits for ingress or `poll_delay`'s protocol deadline.
//! No worker, global service, host socket, or Windows network setting is created.
//!
//! This engine is not yet the guest descriptor backend: OFD/fork transport,
//! listener backlogs, namespace routing and the host gateway must be connected
//! before existing Winsock descriptors can accept arbitrary socket filters.
mod device;
mod packet;
pub mod shared;
pub mod transport;
pub use packet::PacketError;
pub use smoltcp::iface::SocketHandle;
pub use smoltcp::socket::tcp::State as TcpState;
pub use smoltcp::wire::{IpAddress, IpCidr, IpEndpoint};

use crate::socket::filter::{State, classic::Program};
use crate::{EADDRINUSE, EAGAIN, EBADF, EINVAL, EMSGSIZE, ENOENT, ENOTCONN, EOPNOTSUPP, EPERM};
use device::PacketDevice;
use packet::IpPacket;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::socket::{Socket, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::HardwareAddress;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ingress {
    Delivered,
    Filtered,
    Invalid(PacketError),
    Backpressure,
}

pub(crate) fn packet_endpoints(
    bytes: &[u8],
) -> Result<Option<(IpEndpoint, IpEndpoint, u8)>, PacketError> {
    Ok(IpPacket::parse(bytes, 1)?
        .map(|packet| (packet.source, packet.destination, packet.protocol)))
}

/// A self-contained IP interface. Socket handles are local to this engine;
/// guest descriptors must use a separate identity/generation mapping.
pub struct Stack {
    interface: Interface,
    device: PacketDevice,
    sockets: SocketSet<'static>,
    filters: BTreeMap<SocketHandle, Arc<State>>,
    ifindex: u32,
    addresses: Vec<IpCidr>,
}

impl Stack {
    pub fn new(addresses: &[IpCidr], ifindex: u32, seed: u64, now_ms: i64) -> Result<Self, i32> {
        if addresses.is_empty() || addresses.len() > 2 || ifindex == 0 {
            return Err(EINVAL);
        }
        let mut device = PacketDevice::default();
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = seed;
        let mut interface = Interface::new(config, &mut device, Instant::from_millis(now_ms));
        interface.update_ip_addrs(|ips| {
            for addr in addresses {
                ips.push(*addr).unwrap();
            }
        });
        Ok(Self {
            interface,
            device,
            sockets: SocketSet::new(vec![]),
            filters: BTreeMap::new(),
            ifindex,
            addresses: addresses.to_vec(),
        })
    }

    fn check(&self, handle: SocketHandle) -> Result<(), i32> {
        if self.filters.contains_key(&handle) {
            Ok(())
        } else {
            Err(EBADF)
        }
    }
    fn free_endpoint(&self, local: IpEndpoint, tcp: bool) -> Result<(), i32> {
        if local.port == 0 || !self.addresses.iter().any(|a| a.address() == local.addr) {
            return Err(EINVAL);
        }
        for (_, socket) in self.sockets.iter() {
            let busy = match socket {
                Socket::Tcp(s) if tcp => {
                    s.local_endpoint() == Some(local)
                        || (s.listen_endpoint().port == local.port
                            && s.listen_endpoint().addr == Some(local.addr))
                }
                Socket::Udp(s) if !tcp => {
                    s.endpoint().port == local.port && s.endpoint().addr == Some(local.addr)
                }
                _ => false,
            };
            if busy {
                return Err(EADDRINUSE);
            }
        }
        Ok(())
    }
    fn new_tcp() -> tcp::Socket<'static> {
        tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; 65536]),
            tcp::SocketBuffer::new(vec![0; 65536]),
        )
    }
    pub fn listen(&mut self, local: IpEndpoint) -> Result<SocketHandle, i32> {
        self.free_endpoint(local, true)?;
        let mut socket = Self::new_tcp();
        socket.listen(local).map_err(|_| EINVAL)?;
        let handle = self.sockets.add(socket);
        self.filters.insert(handle, Arc::default());
        Ok(handle)
    }
    pub fn connect(&mut self, local: IpEndpoint, peer: IpEndpoint) -> Result<SocketHandle, i32> {
        self.free_endpoint(local, true)?;
        let mut socket = Self::new_tcp();
        socket
            .connect(self.interface.context(), peer, local)
            .map_err(|_| EINVAL)?;
        let handle = self.sockets.add(socket);
        self.filters.insert(handle, Arc::default());
        Ok(handle)
    }
    pub fn bind_udp(&mut self, local: IpEndpoint) -> Result<SocketHandle, i32> {
        self.free_endpoint(local, false)?;
        let mut socket = udp::Socket::new(
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 65536]),
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 64], vec![0; 65536]),
        );
        socket.bind(local).map_err(|_| EINVAL)?;
        let handle = self.sockets.add(socket);
        self.filters.insert(handle, Arc::default());
        Ok(handle)
    }
    pub fn attach_filter(&mut self, handle: SocketHandle, code: &[u8]) -> Result<(), i32> {
        self.check(handle)?;
        if self.filters[&handle].locked {
            return Err(EPERM);
        }
        let program = Program::decode(code)?;
        self.filters.insert(
            handle,
            Arc::new(State {
                program: Some(program),
                locked: false,
            }),
        );
        Ok(())
    }
    pub fn detach_filter(&mut self, handle: SocketHandle) -> Result<(), i32> {
        self.check(handle)?;
        let state = &self.filters[&handle];
        if state.locked {
            return Err(EPERM);
        }
        if state.program.is_none() {
            return Err(ENOENT);
        }
        self.filters.insert(handle, Arc::default());
        Ok(())
    }
    pub fn lock_filter(&mut self, handle: SocketHandle) -> Result<(), i32> {
        self.check(handle)?;
        let mut state = (*self.filters[&handle]).clone();
        state.locked = true;
        self.filters.insert(handle, Arc::new(state));
        Ok(())
    }
    fn receiver(&self, packet: &IpPacket<'_>) -> Option<SocketHandle> {
        let mut listener = None;
        for (handle, socket) in self.sockets.iter() {
            match socket {
                Socket::Tcp(s) if packet.protocol == 6 => {
                    if s.local_endpoint() == Some(packet.destination)
                        && s.remote_endpoint() == Some(packet.source)
                    {
                        return Some(handle);
                    }
                    let local = s.listen_endpoint();
                    if s.state() == TcpState::Listen
                        && local.port == packet.destination.port
                        && local.addr == Some(packet.destination.addr)
                    {
                        listener = Some(handle);
                    }
                }
                Socket::Udp(s) if packet.protocol == 17 => {
                    let local = s.endpoint();
                    if local.port == packet.destination.port
                        && local.addr == Some(packet.destination.addr)
                    {
                        return Some(handle);
                    }
                }
                _ => {}
            }
        }
        listener
    }
    /// Filters one received IP frame BEFORE the TCP state machine can emit an
    /// ACK/SYN-ACK and BEFORE UDP can enqueue data. A discarded frame is never
    /// retained for reclassification when a filter is detached later.
    pub fn receive_packet(&mut self, bytes: Vec<u8>, now_ms: i64) -> Ingress {
        if self.device.input.is_some() {
            return Ingress::Backpressure;
        }
        let packet = match IpPacket::parse(&bytes, self.ifindex) {
            Ok(packet) => packet,
            Err(error) => return Ingress::Invalid(error),
        };
        let retained = if let Some(packet) = packet {
            if let Some(handle) = self.receiver(&packet) {
                match self.filters[&handle].run(&packet, packet.cap) {
                    None => return Ingress::Filtered,
                    Some(length) if length < packet.bytes.len() - packet.start => {
                        Some(packet.retained(length))
                    }
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };
        self.device.input = Some(retained.unwrap_or(bytes));
        self.interface.poll_ingress_single(
            Instant::from_millis(now_ms),
            &mut self.device,
            &mut self.sockets,
        );
        if self.device.input.is_some() {
            self.device.input = None;
            Ingress::Backpressure
        } else {
            Ingress::Delivered
        }
    }
    pub fn poll(&mut self, now_ms: i64) {
        self.interface.poll_egress(
            Instant::from_millis(now_ms),
            &mut self.device,
            &mut self.sockets,
        );
    }
    /// None means no timer is armed: the caller can block until an IO/event
    /// completion or socket command. Zero means queued output/immediate work.
    pub fn poll_delay(&mut self, now_ms: i64) -> Option<u64> {
        if self.device.pending() {
            return Some(0);
        }
        self.interface
            .poll_delay(Instant::from_millis(now_ms), &self.sockets)
            .map(|d| d.total_millis())
    }
    pub fn take_packet(&mut self) -> Option<Vec<u8>> {
        self.device.take()
    }
    pub fn tcp_state(&self, handle: SocketHandle) -> Result<TcpState, i32> {
        self.check(handle)?;
        match self.sockets.iter().find(|(h, _)| *h == handle).unwrap().1 {
            Socket::Tcp(s) => Ok(s.state()),
            _ => Err(EOPNOTSUPP),
        }
    }
    pub fn send_tcp(&mut self, handle: SocketHandle, bytes: &[u8]) -> Result<usize, i32> {
        self.tcp_state(handle)?;
        self.sockets
            .get_mut::<tcp::Socket>(handle)
            .send_slice(bytes)
            .map_err(|_| ENOTCONN)
            .and_then(|n| {
                if n == 0 && !bytes.is_empty() {
                    Err(EAGAIN)
                } else {
                    Ok(n)
                }
            })
    }
    pub(crate) fn close_tcp(&mut self, handle: SocketHandle) -> Result<(), i32> {
        self.tcp_state(handle)?;
        self.sockets.get_mut::<tcp::Socket>(handle).close();
        Ok(())
    }
    pub(crate) fn abort_tcp(&mut self, handle: SocketHandle) -> Result<(), i32> {
        self.tcp_state(handle)?;
        self.sockets.get_mut::<tcp::Socket>(handle).abort();
        Ok(())
    }
    pub fn recv_tcp(
        &mut self,
        handle: SocketHandle,
        bytes: &mut [u8],
        peek: bool,
    ) -> Result<usize, i32> {
        self.tcp_state(handle)?;
        let s = self.sockets.get_mut::<tcp::Socket>(handle);
        if !s.can_recv() {
            return if s.may_recv() { Err(EAGAIN) } else { Ok(0) };
        }
        if peek {
            s.peek_slice(bytes)
        } else {
            s.recv_slice(bytes)
        }
        .map_err(|_| ENOTCONN)
    }
    fn udp(&mut self, handle: SocketHandle) -> Result<&mut udp::Socket<'static>, i32> {
        self.check(handle)?;
        match self
            .sockets
            .iter_mut()
            .find(|(h, _)| *h == handle)
            .unwrap()
            .1
        {
            Socket::Udp(s) => Ok(s),
            _ => Err(EOPNOTSUPP),
        }
    }
    pub fn send_udp(
        &mut self,
        handle: SocketHandle,
        bytes: &[u8],
        peer: IpEndpoint,
    ) -> Result<(), i32> {
        // Fragmentation is not wired yet. Never queue an unsendable datagram:
        // it would leave poll_delay at zero and busy-loop forever.
        let limit = if matches!(peer.addr, IpAddress::Ipv4(_)) {
            1472
        } else {
            1452
        };
        if bytes.len() > limit {
            return Err(EMSGSIZE);
        }
        self.udp(handle)?
            .send_slice(bytes, peer)
            .map_err(|_| EAGAIN)
    }
    /// Return payload, peer and full datagram length. Peek never re-runs BPF.
    pub fn recv_udp(
        &mut self,
        handle: SocketHandle,
        bytes: &mut [u8],
        peek: bool,
    ) -> Result<(usize, IpEndpoint, usize), i32> {
        let s = self.udp(handle)?;
        let (data, meta) = if peek {
            let (d, m) = s.peek().map_err(|_| EAGAIN)?;
            (d, *m)
        } else {
            s.recv().map_err(|_| EAGAIN)?
        };
        let length = bytes.len().min(data.len());
        bytes[..length].copy_from_slice(&data[..length]);
        Ok((length, meta.endpoint, data.len()))
    }
}

#[cfg(test)]
mod tests;
