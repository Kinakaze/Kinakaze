use super::*;
use crate::socket::filter::classic::Packet;

fn instruction(code: u16, jt: u8, jf: u8, k: u32) -> Vec<u8> {
    [code.to_le_bytes().as_slice(), &[jt, jf], &k.to_le_bytes()].concat()
}
fn ret(value: u32) -> Vec<u8> {
    instruction(6, 0, 0, value)
}
fn peers(v6: bool) -> (Stack, Stack, IpEndpoint, IpEndpoint) {
    let (a, b, prefix) = if v6 {
        ("fd80::1".parse().unwrap(), "fd80::2".parse().unwrap(), 64)
    } else {
        (IpAddress::v4(10, 80, 0, 1), IpAddress::v4(10, 80, 0, 2), 24)
    };
    (
        Stack::new(&[IpCidr::new(a, prefix)], 7, 1, 0).unwrap(),
        Stack::new(&[IpCidr::new(b, prefix)], 8, 2, 0).unwrap(),
        IpEndpoint::new(a, 41000),
        IpEndpoint::new(b, 8080),
    )
}
fn exchange(a: &mut Stack, b: &mut Stack, now: i64) -> Vec<Ingress> {
    let mut outcomes = vec![];
    for _ in 0..100 {
        a.poll(now);
        b.poll(now);
        let mut progress = false;
        while let Some(bytes) = a.take_packet() {
            outcomes.push(b.receive_packet(bytes, now));
            progress = true;
        }
        while let Some(bytes) = b.take_packet() {
            outcomes.push(a.receive_packet(bytes, now));
            progress = true;
        }
        if !progress {
            return outcomes;
        }
    }
    panic!("protocol failed to reach quiescence at fixed time");
}

#[test]
fn dropped_syn_has_no_syn_ack_and_detach_requires_sender_retransmission() {
    for v6 in [false, true] {
        let (mut a, mut b, local, peer) = peers(v6);
        let server = b.listen(peer).unwrap();
        b.attach_filter(server, &ret(0)).unwrap();
        let client = a.connect(local, peer).unwrap();
        assert_eq!(exchange(&mut a, &mut b, 0), [Ingress::Filtered]);
        assert_eq!(a.tcp_state(client), Ok(TcpState::SynSent));
        assert_eq!(b.tcp_state(server), Ok(TcpState::Listen));
        b.detach_filter(server).unwrap();
        assert!(
            exchange(&mut a, &mut b, 0).is_empty(),
            "discarded SYN must not be retained"
        );
        assert_eq!(b.tcp_state(server), Ok(TcpState::Listen));
        let due = a.poll_delay(0).unwrap() as i64;
        assert!(due > 0);
        exchange(&mut a, &mut b, due);
        assert_eq!(a.tcp_state(client), Ok(TcpState::Established));
        assert_eq!(b.tcp_state(server), Ok(TcpState::Established));
        assert_eq!(
            b.poll_delay(due),
            None,
            "idle receiver needs no polling timer"
        );
    }
}

#[test]
fn dropped_established_tcp_data_is_not_acked_and_recovers_after_detach() {
    for v6 in [false, true] {
        let (mut a, mut b, local, peer) = peers(v6);
        let server = b.listen(peer).unwrap();
        let client = a.connect(local, peer).unwrap();
        exchange(&mut a, &mut b, 0);
        b.attach_filter(server, &ret(0)).unwrap();
        assert_eq!(a.send_tcp(client, b"unacknowledged"), Ok(14));
        assert_eq!(exchange(&mut a, &mut b, 1), [Ingress::Filtered]);
        assert_eq!(a.sockets.get::<tcp::Socket>(client).send_queue(), 14);
        assert_eq!(b.recv_tcp(server, &mut [0; 20], false), Err(EAGAIN));
        b.detach_filter(server).unwrap();
        assert!(exchange(&mut a, &mut b, 1).is_empty());
        let due = 1 + a.poll_delay(1).unwrap() as i64;
        exchange(&mut a, &mut b, due);
        let mut buffer = [0; 20];
        assert_eq!(b.recv_tcp(server, &mut buffer, true), Ok(14));
        assert_eq!(&buffer[..14], b"unacknowledged");
        assert_eq!(b.recv_tcp(server, &mut buffer, false), Ok(14));
        exchange(&mut a, &mut b, due + 100);
        assert_eq!(a.sockets.get::<tcp::Socket>(client).send_queue(), 0);
    }
}

#[test]
fn udp_filter_controls_enqueue_and_peek_uses_the_committed_truncation() {
    for v6 in [false, true] {
        let (mut a, mut b, local, peer) = peers(v6);
        let tx = a.bind_udp(local).unwrap();
        let rx = b.bind_udp(peer).unwrap();
        b.attach_filter(rx, &ret(0)).unwrap();
        a.send_udp(tx, b"dropped", peer).unwrap();
        assert_eq!(exchange(&mut a, &mut b, 0), [Ingress::Filtered]);
        assert_eq!(b.recv_udp(rx, &mut [0; 20], false), Err(EAGAIN));
        b.attach_filter(rx, &ret(11)).unwrap(); // UDP header + 3 payload bytes.
        a.send_udp(tx, b"abcdef", peer).unwrap();
        exchange(&mut a, &mut b, 1);
        b.attach_filter(rx, &ret(0)).unwrap(); // Do not re-filter already queued data.
        let mut buffer = [0; 20];
        for peek in [true, true, false] {
            assert_eq!(b.recv_udp(rx, &mut buffer, peek), Ok((3, local, 3)));
            assert_eq!(&buffer[..3], b"abc");
        }
        b.attach_filter(rx, &ret(1)).unwrap(); // Header cap gives a real empty datagram.
        a.send_udp(tx, b"payload", peer).unwrap();
        exchange(&mut a, &mut b, 2);
        assert_eq!(b.recv_udp(rx, &mut buffer, false), Ok((0, local, 0)));
    }
}

#[test]
fn payload_and_network_header_filters_execute_on_real_stack_packets() {
    for v6 in [false, true] {
        let (mut a, mut b, local, peer) = peers(v6);
        let tx = a.bind_udp(local).unwrap();
        let rx = b.bind_udp(peer).unwrap();
        let filter = [
            instruction(0x30, 0, 0, 8),
            instruction(0x15, 0, 1, b'A' as u32),
            ret(u32::MAX),
            ret(0),
        ]
        .concat();
        b.attach_filter(rx, &filter).unwrap();
        a.send_udp(tx, b"Allow", peer).unwrap();
        a.poll(0);
        let frame = a.take_packet().unwrap();
        let parsed = IpPacket::parse(&frame, 8).unwrap().unwrap();
        assert_eq!(parsed.load(0, 2), Some(local.port as u32));
        assert_eq!(parsed.load(2, 2), Some(peer.port as u32));
        assert_eq!(
            parsed.load(-0x100000, 1).unwrap() >> 4,
            if v6 { 6 } else { 4 }
        );
        assert_eq!(parsed.load(-0x200000, 1), None);
        assert_eq!(parsed.ancillary(8, 0, 0), Some(8));
        assert_eq!(parsed.ancillary(60, 0, 0), Some(8));
        assert_eq!(b.receive_packet(frame, 0), Ingress::Delivered);
        a.send_udp(tx, b"Block", peer).unwrap();
        assert_eq!(exchange(&mut a, &mut b, 1), [Ingress::Filtered]);
        let mut buffer = [0; 20];
        assert_eq!(b.recv_udp(rx, &mut buffer, false), Ok((5, local, 5)));
        assert_eq!(&buffer[..5], b"Allow");
        assert_eq!(b.recv_udp(rx, &mut buffer, false), Err(EAGAIN));
    }
}

#[test]
fn malformed_and_corrupt_packets_cannot_be_repaired_into_valid_input() {
    let (mut a, mut b, local, peer) = peers(false);
    let tx = a.bind_udp(local).unwrap();
    let rx = b.bind_udp(peer).unwrap();
    b.attach_filter(rx, &ret(9)).unwrap();
    a.send_udp(tx, b"abcdef", peer).unwrap();
    a.poll(0);
    let original = a.take_packet().unwrap();
    for len in 0..original.len() {
        assert!(matches!(
            b.receive_packet(original[..len].to_vec(), 0),
            Ingress::Invalid(_)
        ));
    }
    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert_eq!(
        b.receive_packet(corrupt, 0),
        Ingress::Invalid(PacketError::Checksum)
    );
    let mut fragmented = original;
    let mut ip = smoltcp::wire::Ipv4Packet::new_unchecked(&mut fragmented[..]);
    ip.set_more_frags(true);
    ip.fill_checksum();
    assert_eq!(
        b.receive_packet(fragmented, 0),
        Ingress::Invalid(PacketError::NeedsReassembly)
    );
    assert_eq!(b.recv_udp(rx, &mut [0; 20], false), Err(EAGAIN));
}

#[test]
fn udp_small_user_buffer_does_not_change_the_filter_snaplen() {
    let (mut a, mut b, local, peer) = peers(false);
    let tx = a.bind_udp(local).unwrap();
    let rx = b.bind_udp(peer).unwrap();
    b.attach_filter(rx, &ret(12)).unwrap();
    a.send_udp(tx, b"abcdef", peer).unwrap();
    exchange(&mut a, &mut b, 0);
    let mut short = [0; 2];
    assert_eq!(b.recv_udp(rx, &mut short, true), Ok((2, local, 4)));
    assert_eq!(&short, b"ab");
    let mut full = [0; 8];
    assert_eq!(b.recv_udp(rx, &mut full, false), Ok((4, local, 4)));
    assert_eq!(&full[..4], b"abcd");
}

#[test]
fn filter_locks_and_invalid_replacement_do_not_change_the_active_rule() {
    let (mut a, mut b, local, peer) = peers(false);
    let tx = a.bind_udp(local).unwrap();
    let rx = b.bind_udp(peer).unwrap();
    b.attach_filter(rx, &ret(0)).unwrap();
    assert_eq!(b.attach_filter(rx, &[0; 3]), Err(EINVAL));
    b.lock_filter(rx).unwrap();
    assert_eq!(b.detach_filter(rx), Err(EPERM));
    assert_eq!(b.attach_filter(rx, &ret(u32::MAX)), Err(EPERM));
    a.send_udp(tx, b"still dropped", peer).unwrap();
    assert_eq!(exchange(&mut a, &mut b, 0), [Ingress::Filtered]);
}

#[test]
fn tcp_snaplen_keeps_header_and_only_acknowledges_retained_payload() {
    for v6 in [false, true] {
        let (mut a, mut b, local, peer) = peers(v6);
        let server = b.listen(peer).unwrap();
        let client = a.connect(local, peer).unwrap();
        exchange(&mut a, &mut b, 0);
        b.attach_filter(server, &ret(23)).unwrap(); // Data header 20 + 3 bytes.
        a.send_tcp(client, b"abcdef").unwrap();
        exchange(&mut a, &mut b, 1);
        exchange(&mut a, &mut b, 101);
        let mut buffer = [0; 20];
        assert_eq!(b.recv_tcp(server, &mut buffer, false), Ok(3));
        assert_eq!(&buffer[..3], b"abc");
        assert_eq!(a.sockets.get::<tcp::Socket>(client).send_queue(), 3);
        b.detach_filter(server).unwrap();
        let due = 101 + a.poll_delay(101).unwrap() as i64;
        exchange(&mut a, &mut b, due);
        assert_eq!(b.recv_tcp(server, &mut buffer, false), Ok(3));
        assert_eq!(&buffer[..3], b"def");
    }
}

#[test]
fn ipv6_requires_udp_checksum_even_when_ipv4_allows_omission() {
    for v6 in [false, true] {
        let (mut a, mut b, local, peer) = peers(v6);
        let tx = a.bind_udp(local).unwrap();
        let rx = b.bind_udp(peer).unwrap();
        b.attach_filter(rx, &ret(9)).unwrap();
        a.send_udp(tx, b"hello", peer).unwrap();
        a.poll(0);
        let mut frame = a.take_packet().unwrap();
        let start = IpPacket::parse(&frame, 1).unwrap().unwrap().start;
        frame[start + 6..start + 8].fill(0);
        assert_eq!(
            b.receive_packet(frame, 0),
            if v6 {
                Ingress::Invalid(PacketError::Checksum)
            } else {
                Ingress::Delivered
            }
        );
        assert_eq!(
            b.recv_udp(rx, &mut [0; 10], false),
            if v6 { Err(EAGAIN) } else { Ok((1, local, 1)) }
        );
    }
}

#[test]
fn oversized_udp_fails_without_creating_an_unsendable_busy_queue() {
    for v6 in [false, true] {
        let (mut a, _, local, peer) = peers(v6);
        let tx = a.bind_udp(local).unwrap();
        assert_eq!(a.send_udp(tx, &[0; 2000], peer), Err(EMSGSIZE));
        a.poll(0);
        assert!(a.take_packet().is_none());
        assert_eq!(a.poll_delay(0), None);
    }
}
