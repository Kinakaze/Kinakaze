use super::*;
use crate::usernet::stack::{IpAddress, Stack};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;

fn ret(value: u32) -> Vec<u8> {
    [&[6, 0, 0, 0][..], &value.to_le_bytes()].concat()
}
struct Fixture {
    listener: Arc<Listener>,
    client: Stack,
    local: IpEndpoint,
    peer: IpEndpoint,
    domain: u64,
    id: u64,
    clock: i64,
}
impl Fixture {
    fn new(backlog: usize, ipv6: bool, suffix: u64) -> Self {
        let local = IpEndpoint::new(
            if ipv6 {
                "fd97::1".parse().unwrap()
            } else {
                IpAddress::v4(10, 97, 0, 1)
            },
            40000,
        );
        let peer = IpEndpoint::new(
            if ipv6 {
                "fd97::2".parse().unwrap()
            } else {
                IpAddress::v4(10, 97, 0, 2)
            },
            8080,
        );
        let domain = std::process::id() as u64;
        let id = now_ms() as u64 + (suffix << 48);
        let clock = now_ms();
        let root = SharedEndpoint::create(
            domain,
            id,
            &[IpCidr::new(peer.addr, if ipv6 { 64 } else { 24 })],
            2,
            Kind::Tcp,
        )
        .unwrap();
        Self {
            listener: Listener::new(root, peer, backlog).unwrap(),
            client: Stack::new(
                &[IpCidr::new(local.addr, if ipv6 { 64 } else { 24 })],
                1,
                id,
                clock,
            )
            .unwrap(),
            local,
            peer,
            domain,
            id,
            clock,
        }
    }
    fn connect(&mut self, port: u16) -> SocketHandle {
        self.client
            .connect(IpEndpoint::new(self.local.addr, port), self.peer)
            .unwrap()
    }
    fn exchange(&mut self) {
        for _ in 0..64 {
            let mut progress = false;
            self.client.poll(self.clock);
            while let Some(packet) = self.client.take_packet() {
                let (_, output, _) = self.listener.poll(Some(packet)).unwrap();
                for packet in output {
                    self.client.receive_packet(packet, self.clock);
                }
                progress = true;
            }
            for packet in self.listener.poll(None).unwrap().1 {
                self.client.receive_packet(packet, self.clock);
                progress = true;
            }
            if !progress {
                return;
            }
        }
        panic!("listener failed to become idle");
    }
}

#[test]
fn backlog_accepts_multiple_connections_and_overflow_requires_retransmission() {
    for ipv6 in [false, true] {
        let mut f = Fixture::new(2, ipv6, 40 + u64::from(ipv6));
        let a = f.connect(40000);
        let b = f.connect(40001);
        let c = f.connect(40002);
        f.exchange();
        assert_eq!(f.client.tcp_state(a), Ok(TcpState::Established));
        assert_eq!(f.client.tcp_state(b), Ok(TcpState::Established));
        assert_eq!(f.client.tcp_state(c), Ok(TcpState::SynSent));
        assert_eq!(f.listener.endpoint().tcp_state(), Ok(TcpState::Listen));
        let first = f.listener.accept().unwrap();
        let second = f.listener.accept().unwrap();
        assert_eq!(
            first.endpoint.tcp_endpoints().unwrap().1.unwrap().port,
            40000
        );
        assert_eq!(
            second.endpoint.tcp_endpoints().unwrap().1.unwrap().port,
            40001
        );
        assert!(matches!(f.listener.accept(), Err(EAGAIN)));
        assert!(!f.listener.readiness().unwrap().0);
        f.clock += 2000;
        f.exchange();
        assert_eq!(f.client.tcp_state(c), Ok(TcpState::Established));
        let third = f.listener.accept().unwrap();
        assert_eq!(
            third.endpoint.tcp_endpoints().unwrap().1.unwrap().port,
            40002
        );
        f.client.send_tcp(a, b"one").unwrap();
        f.client.send_tcp(b, b"two").unwrap();
        f.client.send_tcp(c, b"three").unwrap();
        f.exchange();
        for (connection, expected) in [
            (&first, &b"one"[..]),
            (&second, &b"two"[..]),
            (&third, &b"three"[..]),
        ] {
            let mut data = [0; 16];
            let n = connection.endpoint.recv_tcp(&mut data).unwrap();
            assert_eq!(&data[..n], expected);
        }
    }
}

#[test]
fn handshake_uses_listener_filter_and_accepted_rule_is_independent() {
    let mut f = Fixture::new(2, false, 42);
    let client = f.connect(40000);
    f.client.poll(f.clock);
    let syn = f.client.take_packet().unwrap();
    let output = f.listener.poll(Some(syn)).unwrap().1;
    assert_eq!(output.len(), 1);
    f.listener.endpoint().attach_filter(&ret(0)).unwrap();
    for packet in output {
        f.client.receive_packet(packet, f.clock);
    }
    f.client.poll(f.clock);
    let ack = f.client.take_packet().unwrap();
    assert_eq!(
        f.listener.poll(Some(ack.clone())).unwrap().0,
        Some(Ingress::Filtered)
    );
    assert!(matches!(f.listener.accept(), Err(EAGAIN)));
    f.listener.endpoint().detach_filter().unwrap();
    assert_eq!(
        f.listener.poll(Some(ack)).unwrap().0,
        Some(Ingress::Delivered)
    );
    assert!(f.listener.readiness().unwrap().0);
    f.listener.endpoint().attach_filter(&ret(0)).unwrap();
    let accepted = f.listener.accept().unwrap();
    assert!(accepted.endpoint.filter_program().unwrap().is_empty());
    f.client.send_tcp(client, b"independent").unwrap();
    f.exchange();
    let mut bytes = [0; 20];
    assert_eq!(accepted.endpoint.recv_tcp(&mut bytes), Ok(11));
    assert_eq!(&bytes[..11], b"independent");
}

#[test]
fn accepted_view_survives_listener_drop_and_reopens_same_arena() {
    let mut f = Fixture::new(1, false, 43);
    f.connect(40000);
    f.exchange();
    let accepted = f.listener.accept().unwrap();
    let (domain, id) = (f.domain, f.id);
    drop(f.listener);
    let reopened =
        SharedEndpoint::open_connection(domain, id, accepted.slot, accepted.generation).unwrap();
    assert_eq!(reopened.tcp_state(), Ok(TcpState::Established));
    assert_eq!(reopened.tcp_endpoints(), accepted.endpoint.tcp_endpoints());
}

#[test]
fn retired_connection_generation_cannot_modify_reused_slot() {
    let mut f = Fixture::new(1, false, 44);
    let first_client = f.connect(40000);
    f.exchange();
    let old = f.listener.accept().unwrap();
    let old_waiter = old.endpoint.subscribe().unwrap();
    for packet in f.listener.abort(&old).unwrap() {
        f.client.receive_packet(packet, f.clock);
    }
    assert_eq!(f.client.tcp_state(first_client), Ok(TcpState::Closed));
    f.connect(40001);
    f.exchange();
    let new = f.listener.accept().unwrap();
    assert_eq!(old.slot, new.slot);
    assert_ne!(old.generation, new.generation);
    assert_eq!(old.endpoint.send_tcp(b"stale"), Err(crate::EBADF));
    assert_eq!(old_waiter.arm(), Err(crate::EBADF));
    drop(old_waiter);
    assert_eq!(new.endpoint.tcp_state(), Ok(TcpState::Established));
    assert!(SharedEndpoint::open_connection(f.domain, f.id, old.slot, old.generation).is_err());
}

#[test]
fn child_accepts_and_writes_the_shared_connection() {
    let Ok(identity) = std::env::var("KINAKAZE_PACKET_LISTENER_CHILD") else {
        return;
    };
    let (domain, id) = identity.split_once(':').unwrap();
    let listener = Listener::open(domain.parse().unwrap(), id.parse().unwrap()).unwrap();
    let accepted = listener.accept().unwrap();
    let mut data = [0; 32];
    assert_eq!(accepted.endpoint.recv_tcp(&mut data), Ok(10));
    assert_eq!(&data[..10], b"beforefork");
    assert_eq!(accepted.endpoint.send_tcp(b"fromchild"), Ok(9));
}

#[test]
fn independent_process_accepts_pending_connection_without_owning_its_creation() {
    let mut f = Fixture::new(2, false, 45);
    let client = f.connect(40000);
    f.exchange();
    f.client.send_tcp(client, b"beforefork").unwrap();
    f.exchange();
    let notification = f.listener.endpoint().subscribe().unwrap();
    notification.arm().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "usernet::stack::shared::listener::tests::child_accepts_and_writes_the_shared_connection", "--test-threads=1"])
        .env("KINAKAZE_PACKET_LISTENER_CHILD", format!("{}:{}", f.domain, f.id))
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped()).creation_flags(0x08000000).spawn().unwrap();
    if unsafe { WaitForSingleObject(child.as_raw_handle(), 10000) } != WAIT_OBJECT_0 {
        let _ = child.kill();
        let _ = child.wait();
        panic!("listener child timed out");
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        unsafe { WaitForSingleObject(notification.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
    assert!(matches!(f.listener.accept(), Err(EAGAIN)));
    f.exchange();
    let mut data = [0; 32];
    assert_eq!(f.client.recv_tcp(client, &mut data, false), Ok(9));
    assert_eq!(&data[..9], b"fromchild");
}

#[test]
fn closing_listener_resets_pending_but_keeps_accepted_connection() {
    let mut f = Fixture::new(2, false, 46);
    let first = f.connect(40000);
    let second = f.connect(40001);
    f.exchange();
    let accepted = f.listener.accept().unwrap();
    for packet in f.listener.close().unwrap() {
        f.client.receive_packet(packet, f.clock);
    }
    assert_eq!(f.client.tcp_state(first), Ok(TcpState::Established));
    assert_eq!(f.client.tcp_state(second), Ok(TcpState::Closed));
    assert_eq!(f.listener.readiness(), Ok((false, false, true)));
    assert!(matches!(f.listener.accept(), Err(EINVAL)));
    let third = f.connect(40002);
    f.exchange();
    assert_eq!(f.client.tcp_state(third), Ok(TcpState::Closed));
    f.client.send_tcp(first, b"still open").unwrap();
    f.exchange();
    let mut data = [0; 16];
    assert_eq!(accepted.endpoint.recv_tcp(&mut data), Ok(10));
    assert_eq!(&data[..10], b"still open");
}

#[test]
fn rejected_packet_does_not_clear_other_connections_protocol_deadline() {
    let mut f = Fixture::new(2, false, 47);
    f.connect(40000);
    f.exchange();
    let accepted = f.listener.accept().unwrap();
    accepted
        .endpoint
        .send_tcp(b"awaiting acknowledgment")
        .unwrap();
    let (_, output, deadline) = f.listener.poll(None).unwrap();
    assert!(!output.is_empty());
    assert!(deadline.is_some());
    // Leave the emitted data unacknowledged. Its retransmission timer must
    // survive unrelated malformed input and a filtered new SYN.
    let (verdict, _, deadline) = f.listener.poll(Some(vec![0])).unwrap();
    assert!(matches!(verdict, Some(Ingress::Invalid(_))));
    assert!(deadline.is_some());
    f.listener.endpoint().attach_filter(&ret(0)).unwrap();
    f.connect(40001);
    f.client.poll(f.clock);
    let (verdict, _, deadline) = f
        .listener
        .poll(Some(f.client.take_packet().unwrap()))
        .unwrap();
    assert_eq!(verdict, Some(Ingress::Filtered));
    assert!(deadline.is_some());
}

#[test]
fn unused_connection_slots_reserve_address_space_without_committing_buffers() {
    let mut f = Fixture::new(2, false, 49);
    let state = |index: usize| {
        let address = unsafe {
            f.listener
                .root
                .view
                .0
                .Value
                .cast::<u8>()
                .add(index * SLOT_STRIDE)
        };
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe {
                VirtualQuery(
                    address.cast(),
                    &mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            },
            0
        );
        info.State
    };
    assert_eq!(state(1), MEM_RESERVE);
    assert_eq!(state(CAPACITY), MEM_RESERVE);
    f.connect(40000);
    f.exchange();
    let address = unsafe { f.listener.root.view.0.Value.cast::<u8>().add(SLOT_STRIDE) };
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe {
            VirtualQuery(
                address.cast(),
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        },
        0
    );
    assert_eq!(info.State, MEM_COMMIT);
}

#[test]
fn connection_write_notifies_arena_but_its_own_pump_does_not_wake_itself() {
    let mut f = Fixture::new(2, false, 50);
    f.connect(40000);
    f.exchange();
    let accepted = f.listener.accept().unwrap();
    let waiter = f.listener.endpoint().subscribe().unwrap();
    waiter.arm().unwrap();
    accepted.endpoint.send_tcp(b"command").unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(waiter.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
    waiter.arm().unwrap();
    let (_, output, _) = f.listener.poll_for(None, &waiter).unwrap();
    assert!(!output.is_empty());
    assert_eq!(
        unsafe { WaitForSingleObject(waiter.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
    accepted.endpoint.send_tcp(b"another command").unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(waiter.as_raw_handle(), 0) },
        WAIT_OBJECT_0
    );
}

#[test]
fn listener_uses_afd_readiness_and_drives_accepted_socket_io() {
    use crate::usernet::stack::transport::{Interest, Transport};
    use std::net::UdpSocket;
    let f = Fixture::new(2, false, 51);
    let endpoint = SharedEndpoint::create(
        f.domain,
        f.id + (1 << 40),
        &[IpCidr::new(f.local.addr, 24)],
        1,
        Kind::Tcp,
    )
    .unwrap();
    endpoint.connect(f.local, f.peer).unwrap();
    let a = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let b = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    a.connect(b.local_addr().unwrap()).unwrap();
    b.connect(a.local_addr().unwrap()).unwrap();
    let mut client = Transport::new(endpoint, a).unwrap();
    let mut server = Transport::new(f.listener, b).unwrap();
    let (done, completed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        server.wait_ready(Interest::Readable, Some(3000)).unwrap();
        let accepted = server.endpoint().accept().unwrap();
        server
            .wait_until(Some(3000), |_| Ok(accepted.endpoint.readiness()?.0))
            .unwrap();
        let mut data = [0; 8];
        assert_eq!(accepted.endpoint.recv_tcp(&mut data), Ok(4));
        assert_eq!(&data[..4], b"wire");
        assert_eq!(accepted.endpoint.send_tcp(&data[..4]), Ok(4));
        server.drive().unwrap();
        completed
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
    });
    client.wait_ready(Interest::Connected, Some(3000)).unwrap();
    client.endpoint().send_tcp(b"wire").unwrap();
    client.wait_ready(Interest::Readable, Some(3000)).unwrap();
    let mut data = [0; 8];
    assert_eq!(client.endpoint().recv_tcp(&mut data), Ok(4));
    assert_eq!(&data[..4], b"wire");
    done.send(()).unwrap();
    worker.join().unwrap();
}

#[test]
fn malformed_syn_options_do_not_consume_backlog_forever() {
    let mut f = Fixture::new(1, false, 52);
    f.connect(40000);
    f.client.poll(f.clock);
    let original = f.client.take_packet().unwrap();
    let packet = IpPacket::parse(&original, 2).unwrap().unwrap();
    assert!(packet.cap >= 24);
    let mut malformed = original.clone();
    malformed[packet.start + 20] = 2; // MSS option
    malformed[packet.start + 21] = 1; // illegal option length
    smoltcp::wire::TcpPacket::new_unchecked(&mut malformed[packet.start..])
        .fill_checksum(&packet.source.addr, &packet.destination.addr);
    for _ in 0..16 {
        assert!(
            f.listener
                .poll(Some(malformed.clone()))
                .unwrap()
                .1
                .is_empty()
        );
    }
    let (_, output, _) = f.listener.poll(Some(original)).unwrap();
    assert_eq!(
        output.len(),
        1,
        "a valid SYN must still obtain a backlog slot"
    );
    let reply = IpPacket::parse(&output[0], 1).unwrap().unwrap();
    assert_eq!(reply.bytes[reply.start + 13] & 0x16, 0x12);
}

#[test]
fn expired_handshake_releases_backlog_and_listener_lock_is_inherited() {
    let mut f = Fixture::new(1, false, 53);
    f.listener.endpoint().attach_filter(&ret(u32::MAX)).unwrap();
    f.listener.endpoint().lock_filter(true).unwrap();
    f.connect(40000);
    f.client.poll(f.clock);
    let syn = f.client.take_packet().unwrap();
    f.listener.poll(Some(syn.clone())).unwrap();
    let child = {
        let _guard = f.listener.root.acquire().unwrap();
        let slot = unsafe { (*Listener::control(&f.listener.root)).slots[0] };
        f.listener.child_locked(0, slot.generation).unwrap()
    };
    child
        .with(|c| {
            c.sockets
                .get_mut::<tcp::Socket>(c.socket)
                .set_timeout(Some(smoltcp::time::Duration::from_millis(1)));
            Ok(())
        })
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    f.listener.poll(None).unwrap();
    assert_eq!(child.tcp_state(), Err(crate::EBADF));
    let (_, output, _) = f.listener.poll(Some(syn)).unwrap();
    assert!(!output.is_empty());
    for packet in output {
        f.client.receive_packet(packet, f.clock);
    }
    f.exchange();
    let accepted = f.listener.accept().unwrap();
    assert_eq!(accepted.endpoint.filter_program(), Ok(ret(u32::MAX)));
    assert_eq!(accepted.endpoint.lock_filter(false), Err(EPERM));
}

#[test]
fn unwinding_a_protocol_mutation_marks_the_whole_arena_unusable() {
    let mut f = Fixture::new(1, false, 54);
    f.connect(40000);
    f.exchange();
    let accepted = f.listener.accept().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), i32> = accepted
            .endpoint
            .with(|_| panic!("simulated interrupted mutation"));
    }));
    assert!(result.is_err());
    assert_eq!(accepted.endpoint.readiness(), Err(EIO));
    assert_eq!(f.listener.readiness(), Err(EIO));
}
