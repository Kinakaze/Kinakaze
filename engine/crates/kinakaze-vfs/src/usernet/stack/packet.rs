//! The socket-filter view of a real, checksum-verified IP packet.
//! Offset zero is the transport header, as in Linux tcp_filter/udp_queue_rcv_skb.
use crate::socket::filter::{LocalPacket, classic::Packet};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Packet, Ipv6Packet, TcpPacket, UdpPacket};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketError {
    Malformed,
    Checksum,
    NeedsReassembly,
    UnsupportedHeader,
}

pub(super) struct IpPacket<'a> {
    pub bytes: &'a [u8],
    pub start: usize,
    pub cap: usize,
    pub protocol: u8,
    pub source: IpEndpoint,
    pub destination: IpEndpoint,
    pub ifindex: u32,
}

impl<'a> IpPacket<'a> {
    pub fn parse(bytes: &'a [u8], ifindex: u32) -> Result<Option<Self>, PacketError> {
        use PacketError::*;
        let (start, end, protocol, source, destination) = match bytes.first().map(|b| b >> 4) {
            Some(4) => {
                let ip = Ipv4Packet::new_checked(bytes).map_err(|_| Malformed)?;
                if !ip.verify_checksum() {
                    return Err(Checksum);
                }
                if ip.more_frags() || ip.frag_offset() != 0 {
                    return Err(NeedsReassembly);
                }
                (
                    ip.header_len() as usize,
                    ip.total_len() as usize,
                    u8::from(ip.next_header()),
                    IpAddress::Ipv4(ip.src_addr()),
                    IpAddress::Ipv4(ip.dst_addr()),
                )
            }
            Some(6) => {
                let ip = Ipv6Packet::new_checked(bytes).map_err(|_| Malformed)?;
                let end = 40 + ip.payload_len() as usize;
                let mut start = 40;
                let mut next = u8::from(ip.next_header());
                // Extension headers precede the transport header. The stack's
                // IP layer still validates their semantics after filtering.
                while matches!(next, 0 | 43 | 60 | 44 | 51 | 50) {
                    if next == 44 {
                        return Err(NeedsReassembly);
                    }
                    if matches!(next, 50 | 51) {
                        return Err(UnsupportedHeader);
                    }
                    let header = bytes.get(start..start + 2).ok_or(Malformed)?;
                    let size = (header[1] as usize + 1) * 8;
                    next = header[0];
                    start += size;
                    if start > end {
                        return Err(Malformed);
                    }
                }
                (
                    start,
                    end,
                    next,
                    IpAddress::Ipv6(ip.src_addr()),
                    IpAddress::Ipv6(ip.dst_addr()),
                )
            }
            _ => return Err(Malformed),
        };
        let mut bytes = bytes.get(..end).ok_or(Malformed)?;
        let transport = bytes.get(start..).ok_or(Malformed)?;
        let (cap, source_port, destination_port) = match protocol {
            6 => {
                let tcp = TcpPacket::new_checked(transport).map_err(|_| Malformed)?;
                if !tcp.verify_checksum(&source, &destination) {
                    return Err(Checksum);
                }
                (tcp.header_len() as usize, tcp.src_port(), tcp.dst_port())
            }
            17 => {
                let udp = UdpPacket::new_checked(transport).map_err(|_| Malformed)?;
                // An omitted UDP checksum is allowed on IPv4 only.
                if matches!(source, IpAddress::Ipv6(_)) && udp.checksum() == 0 {
                    return Err(Checksum);
                }
                if !(matches!(source, IpAddress::Ipv4(_)) && udp.checksum() == 0)
                    && !udp.verify_checksum(&source, &destination)
                {
                    return Err(Checksum);
                }
                bytes = &bytes[..start + udp.len() as usize];
                (8, udp.src_port(), udp.dst_port())
            }
            _ => return Ok(None),
        };
        Ok(Some(Self {
            bytes,
            start,
            cap,
            protocol,
            source: IpEndpoint::new(source, source_port),
            destination: IpEndpoint::new(destination, destination_port),
            ifindex,
        }))
    }

    pub fn retained(&self, length: usize) -> Vec<u8> {
        let mut bytes = self.bytes[..self.start + length].to_vec();
        if length == self.bytes.len() - self.start {
            return bytes;
        }
        // Linux trims the skb after checksum validation. Here the IP stack is
        // a separate consumer, so repair lengths/checksums on its private input.
        // BPF has already seen the original, unmodified bytes and metadata.
        let total = bytes.len();
        if matches!(self.source.addr, IpAddress::Ipv4(_)) {
            let mut ip = Ipv4Packet::new_unchecked(&mut bytes[..]);
            ip.set_total_len(total as u16);
            ip.fill_checksum();
        } else {
            Ipv6Packet::new_unchecked(&mut bytes[..]).set_payload_len((total - 40) as u16);
        }
        if self.protocol == 6 {
            TcpPacket::new_unchecked(&mut bytes[self.start..])
                .fill_checksum(&self.source.addr, &self.destination.addr);
        } else {
            let mut udp = UdpPacket::new_unchecked(&mut bytes[self.start..]);
            udp.set_len(length as u16);
            udp.fill_checksum(&self.source.addr, &self.destination.addr);
        }
        bytes
    }
}

impl Packet for IpPacket<'_> {
    fn len(&self) -> u32 {
        (self.bytes.len() - self.start) as u32
    }
    fn load(&self, offset: i32, size: usize) -> Option<u32> {
        let at = if offset >= 0 {
            self.start.checked_add(offset as usize)?
        } else if offset >= -0x100000 {
            (offset + 0x100000) as usize // SKF_NET_OFF
        } else {
            return None; // No link-layer header exists on this IP device.
        };
        LocalPacket(self.bytes.get(at..)?).load(0, size)
    }
    fn ancillary(&self, offset: u32, a: u32, x: u32) -> Option<u32> {
        match offset {
            0 => Some(if matches!(self.source.addr, IpAddress::Ipv4(_)) {
                0x0800
            } else {
                0x86dd
            }),
            8 => Some(self.ifindex),
            28 => Some(0xfffe), // ARPHRD_NONE: an IP device, no invented Ethernet header.
            60 => Some(self.cap as u32),
            _ => LocalPacket(&self.bytes[self.start..]).ancillary(offset, a, x),
        }
    }
}
