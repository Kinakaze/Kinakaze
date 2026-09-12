use super::super::{IpAddress, Stack};
use super::*;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;

fn wait_child(mut child: std::process::Child) -> std::process::Output {
    if unsafe { WaitForSingleObject(child.as_raw_handle(), 15000) } != WAIT_OBJECT_0 {
        let _ = child.kill();
        let _ = child.wait();
        panic!("shared endpoint child timed out");
    }
    child.wait_with_output().unwrap()
}
fn ret(value: u32) -> Vec<u8> {
    [&[6, 0, 0, 0][..], &value.to_le_bytes()].concat()
}

#[test]
fn detached_section_pin_preserves_state_until_the_last_reference_closes() {
    let domain = std::process::id() as u64;
    let id = now_ms() as u64 + (3u64 << 48);
    let local = IpEndpoint::new(IpAddress::v4(10, 93, 1, 1), 8080);
    let endpoint =
        SharedEndpoint::create(domain, id, &[IpCidr::new(local.addr, 24)], 2, Kind::Udp).unwrap();
    endpoint.bind_udp(local).unwrap();
    endpoint.attach_filter(&ret(0)).unwrap();
    endpoint.lock_filter(true).unwrap();
    let pin = endpoint.pin().unwrap();
    drop(endpoint);
    let reopened = SharedEndpoint::open(domain, id).unwrap();
    drop(pin);
    assert_eq!(reopened.filter_program(), Ok(ret(0)));
    assert_eq!(reopened.detach_filter(), Err(EPERM));
    drop(reopened);
    assert!(
        SharedEndpoint::open(domain, id).is_err(),
        "no permanent owner may keep the section alive"
    );
}

#[test]
fn occupied_child_address_is_preserved_when_mapping_is_refused() {
    let domain = std::process::id() as u64;
    let id = now_ms() as u64 + (2u64 << 48);
    let local = IpEndpoint::new(IpAddress::v4(10, 93, 0, 1), 8080);
    let endpoint =
        SharedEndpoint::create(domain, id, &[IpCidr::new(local.addr, 24)], 2, Kind::Tcp).unwrap();
    endpoint.listen(local).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "usernet::stack::shared::tests::child_reopens_live_protocol_state",
            "--test-threads=1",
        ])
        .env("KINAKAZE_PACKET_SHARED_CHILD", format!("{domain}:{id}"))
        .env(
            "KINAKAZE_PACKET_OCCUPIED_BASE",
            (endpoint.view.0.Value as usize).to_string(),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000)
        .spawn()
        .unwrap();
    let output = wait_child(child);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(endpoint.tcp_state(), Ok(TcpState::Listen));
}

#[test]
fn child_reopens_live_protocol_state() {
    let Ok(identity) = std::env::var("KINAKAZE_PACKET_SHARED_CHILD") else {
        return;
    };
    let (domain, id) = identity.split_once(':').unwrap();
    if let Ok(base) = std::env::var("KINAKAZE_PACKET_OCCUPIED_BASE") {
        let base = base.parse::<usize>().unwrap();
        let allocated = unsafe {
            VirtualAlloc(
                base as _,
                size_of::<Layout>(),
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        assert_eq!(
            allocated as usize, base,
            "probe could not own the target address"
        );
        unsafe {
            allocated.cast::<u64>().write(0x71a85ef03082449a);
        }
        let result = SharedEndpoint::open(domain.parse().unwrap(), id.parse().unwrap());
        assert!(
            result.is_err(),
            "an occupied mapping must never be replaced"
        );
        assert_eq!(
            unsafe { allocated.cast::<u64>().read() },
            0x71a85ef03082449a
        );
        assert_ne!(unsafe { VirtualFree(allocated, 0, MEM_RELEASE) }, 0);
        return;
    }
    let endpoint = SharedEndpoint::open(domain.parse().unwrap(), id.parse().unwrap()).unwrap();
    if std::env::var_os("KINAKAZE_PACKET_ABANDONED").is_some() {
        let guard = endpoint.acquire().unwrap();
        std::mem::forget(guard);
        std::process::exit(0);
    }
    if std::env::var_os("KINAKAZE_PACKET_SHARED_UDP").is_some() {
        let mut bytes = [0; 32];
        let (length, peer, full) = endpoint.recv_udp(&mut bytes, true).unwrap();
        assert_eq!((length, full), (3, 3));
        assert_eq!(&bytes[..3], b"abc");
        assert!(endpoint.readiness().unwrap().0);
        endpoint.recv_udp(&mut bytes, false).unwrap();
        assert!(!endpoint.readiness().unwrap().0);
        endpoint.send_udp(b"fromchild", peer).unwrap();
        endpoint.attach_filter(&ret(0)).unwrap();
        endpoint.lock_filter(true).unwrap();
        return;
    }
    assert_eq!(endpoint.tcp_state(), Ok(TcpState::Established));
    let mut bytes = [0; 32];
    assert_eq!(endpoint.recv_tcp(&mut bytes), Ok(10));
    assert_eq!(&bytes[..10], b"beforefork");
    assert_eq!(endpoint.send_tcp(b"fromchild"), Ok(9));
    endpoint.attach_filter(&ret(0)).unwrap();
}

#[test]
fn waiter_cleanup_cannot_hide_an_abandoned_protocol_mutation() {
    let domain = std::process::id() as u64;
    let id = now_ms() as u64 + (4u64 << 48);
    let local = IpEndpoint::new(IpAddress::v4(10, 93, 2, 1), 8080);
    let endpoint =
        SharedEndpoint::create(domain, id, &[IpCidr::new(local.addr, 24)], 2, Kind::Udp).unwrap();
    let waiter = endpoint.subscribe().unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "usernet::stack::shared::tests::child_reopens_live_protocol_state",
            "--test-threads=1",
        ])
        .env("KINAKAZE_PACKET_SHARED_CHILD", format!("{domain}:{id}"))
        .env("KINAKAZE_PACKET_ABANDONED", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000)
        .spawn()
        .unwrap();
    let result = wait_child(child);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    // Drop observes WAIT_ABANDONED first. A later call must still reject the
    // potentially interrupted Rust mutation even though the mutex is now free.
    drop(waiter);
    assert_eq!(endpoint.readiness(), Err(EIO));
    assert!(matches!(endpoint.poll(None), Err(EIO)));
}

#[test]
fn independent_process_shares_established_tcp_buffers_and_filter() {
    let domain = std::process::id() as u64;
    let id = now_ms() as u64;
    let local = IpEndpoint::new(IpAddress::v4(10, 91, 0, 1), 40000);
    let peer = IpEndpoint::new(IpAddress::v4(10, 91, 0, 2), 8080);
    let endpoint =
        SharedEndpoint::create(domain, id, &[IpCidr::new(peer.addr, 24)], 2, Kind::Tcp).unwrap();
    endpoint.listen(peer).unwrap();
    let alias = SharedEndpoint::open(domain, id).unwrap();
    assert!(Arc::ptr_eq(&endpoint, &alias));
    drop(alias);
    let mut client = Stack::new(&[IpCidr::new(local.addr, 24)], 1, 5, now_ms()).unwrap();
    let connection = client.connect(local, peer).unwrap();
    fn exchange(client: &mut Stack, endpoint: &SharedEndpoint) {
        for _ in 0..64 {
            let mut progress = false;
            client.poll(now_ms());
            while let Some(frame) = client.take_packet() {
                let (ingress, output, _) = endpoint.poll(Some(frame)).unwrap();
                assert_eq!(ingress, Some(Ingress::Delivered));
                for frame in output {
                    client.receive_packet(frame, now_ms());
                }
                progress = true;
            }
            for frame in endpoint.poll(None).unwrap().1 {
                client.receive_packet(frame, now_ms());
                progress = true;
            }
            if !progress {
                return;
            }
        }
        panic!("shared protocol did not become idle");
    }
    exchange(&mut client, &endpoint);
    assert_eq!(endpoint.tcp_state(), Ok(TcpState::Established));
    client.send_tcp(connection, b"beforefork").unwrap();
    exchange(&mut client, &endpoint);
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "usernet::stack::shared::tests::child_reopens_live_protocol_state",
            "--test-threads=1",
        ])
        .env("KINAKAZE_PACKET_SHARED_CHILD", format!("{domain}:{id}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000)
        .spawn()
        .unwrap();
    let output = wait_child(child);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        endpoint.recv_tcp(&mut [0; 32]),
        Err(EAGAIN),
        "child consumed the same receive buffer"
    );
    let (_, output, _) = endpoint.poll(None).unwrap();
    assert!(!output.is_empty());
    for frame in output {
        client.receive_packet(frame, now_ms());
    }
    let mut bytes = [0; 32];
    assert_eq!(client.recv_tcp(connection, &mut bytes, false), Ok(9));
    assert_eq!(&bytes[..9], b"fromchild");
    client
        .send_tcp(connection, b"blocked after child exited")
        .unwrap();
    client.poll(now_ms());
    let mut filtered = 0;
    while let Some(frame) = client.take_packet() {
        let (ingress, _, _) = endpoint.poll(Some(frame)).unwrap();
        assert_eq!(ingress, Some(Ingress::Filtered));
        filtered += 1;
    }
    assert!(filtered > 0);
    endpoint.detach_filter().unwrap();
}

#[test]
fn independent_process_shares_udp_queue_readiness_and_permanent_lock() {
    let domain = std::process::id() as u64;
    let id = now_ms() as u64 + (1u64 << 48);
    let local = IpEndpoint::new(IpAddress::v4(10, 92, 0, 1), 40000);
    let peer = IpEndpoint::new(IpAddress::v4(10, 92, 0, 2), 8080);
    let endpoint =
        SharedEndpoint::create(domain, id, &[IpCidr::new(peer.addr, 24)], 2, Kind::Udp).unwrap();
    endpoint.bind_udp(peer).unwrap();
    endpoint.attach_filter(&ret(11)).unwrap();
    let mut client = Stack::new(&[IpCidr::new(local.addr, 24)], 1, 5, now_ms()).unwrap();
    let connection = client.bind_udp(local).unwrap();
    client.send_udp(connection, b"abcdef", peer).unwrap();
    client.poll(now_ms());
    assert_eq!(
        endpoint
            .poll(Some(client.take_packet().unwrap()))
            .unwrap()
            .0,
        Some(Ingress::Delivered)
    );
    assert!(endpoint.readiness().unwrap().0);
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "usernet::stack::shared::tests::child_reopens_live_protocol_state",
            "--test-threads=1",
        ])
        .env("KINAKAZE_PACKET_SHARED_CHILD", format!("{domain}:{id}"))
        .env("KINAKAZE_PACKET_SHARED_UDP", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000)
        .spawn()
        .unwrap();
    let output = wait_child(child);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!endpoint.readiness().unwrap().0);
    assert_eq!(endpoint.filter_program().unwrap(), ret(0));
    assert_eq!(endpoint.detach_filter(), Err(EPERM));
    assert_eq!(endpoint.attach_filter(&[]), Err(EPERM));
    for packet in endpoint.poll(None).unwrap().1 {
        client.receive_packet(packet, now_ms());
    }
    let mut data = [0; 20];
    assert_eq!(
        client.recv_udp(connection, &mut data, false),
        Ok((9, peer, 9))
    );
    assert_eq!(&data[..9], b"fromchild");
    client.send_udp(connection, b"drop", peer).unwrap();
    client.poll(now_ms());
    assert_eq!(
        endpoint
            .poll(Some(client.take_packet().unwrap()))
            .unwrap()
            .0,
        Some(Ingress::Filtered)
    );
}
