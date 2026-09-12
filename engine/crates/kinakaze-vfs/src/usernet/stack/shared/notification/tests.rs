use super::*;
use crate::usernet::stack::{IpAddress, Stack};
use std::os::windows::process::CommandExt;

fn fixture(
    suffix: u64,
) -> (
    Arc<SharedEndpoint>,
    Stack,
    super::super::super::SocketHandle,
    IpEndpoint,
) {
    let local = IpEndpoint::new(IpAddress::v4(10, 94, 0, 1), 40000);
    let peer = IpEndpoint::new(IpAddress::v4(10, 94, 0, 2), 8080);
    let endpoint = SharedEndpoint::create(
        std::process::id() as u64,
        now_ms() as u64 + (suffix << 48),
        &[IpCidr::new(peer.addr, 24)],
        2,
        Kind::Udp,
    )
    .unwrap();
    endpoint.bind_udp(peer).unwrap();
    let mut client = Stack::new(&[IpCidr::new(local.addr, 24)], 1, 15, now_ms()).unwrap();
    let handle = client.bind_udp(local).unwrap();
    (endpoint, client, handle, peer)
}

#[test]
fn one_waiter_cannot_clear_another_waiters_wakeup() {
    let (endpoint, mut client, handle, peer) = fixture(10);
    let first = endpoint.subscribe().unwrap();
    let second = endpoint.subscribe().unwrap();
    for _ in 0..64 {
        first.arm().unwrap();
        second.arm().unwrap();
        assert!(!endpoint.readiness().unwrap().0);
        client.send_udp(handle, b"event", peer).unwrap();
        client.poll(now_ms());
        endpoint.poll(Some(client.take_packet().unwrap())).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(first.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert_eq!(
            unsafe { WaitForSingleObject(second.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        // The second waiter observes the new state and prepares another wait.
        // This must not steal the first waiter's already delivered wakeup.
        second.arm().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(first.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert!(endpoint.readiness().unwrap().0);
        endpoint.recv_udp(&mut [0; 8], false).unwrap();
    }
    first.arm().unwrap();
    second.arm().unwrap();
    for _ in 0..64 {
        assert_eq!(endpoint.poll(None).unwrap().2, None);
    }
    assert_eq!(
        unsafe { WaitForSingleObject(first.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
    assert_eq!(
        unsafe { WaitForSingleObject(second.as_raw_handle(), 0) },
        WAIT_TIMEOUT
    );
}

#[test]
fn subscriber_capacity_is_reusable_and_observations_do_not_wake() {
    let (endpoint, _, _, _) = fixture(11);
    let mut subscriptions: Vec<_> = (0..MAX_WAITERS)
        .map(|_| endpoint.subscribe().unwrap())
        .collect();
    assert!(matches!(endpoint.subscribe(), Err(crate::ENOBUFS)));
    subscriptions.pop();
    subscriptions.push(endpoint.subscribe().unwrap());
    for subscription in &subscriptions {
        subscription.arm().unwrap();
    }
    endpoint.readiness().unwrap();
    assert_eq!(endpoint.recv_udp(&mut [0; 8], true), Err(EAGAIN));
    assert_eq!(endpoint.recv_udp(&mut [0; 8], false), Err(EAGAIN));
    for subscription in &subscriptions {
        assert_eq!(
            unsafe { WaitForSingleObject(subscription.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
    }
}

#[test]
fn child_waits_for_shared_io() {
    let Ok(identity) = std::env::var("KINAKAZE_PACKET_EVENT_CHILD") else {
        return;
    };
    let (domain, id) = identity.split_once(':').unwrap();
    let endpoint = SharedEndpoint::open(domain.parse().unwrap(), id.parse().unwrap()).unwrap();
    let notification = endpoint.subscribe().unwrap();
    notification.arm().unwrap();
    assert!(!endpoint.readiness().unwrap().0);
    let name: Vec<u16> = format!("{}.TestReady", endpoint.name)
        .encode_utf16()
        .chain([0])
        .collect();
    let ready = Handle::new(unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) }).unwrap();
    assert_ne!(unsafe { SetEvent(ready.0) }, 0);
    assert_eq!(
        unsafe { WaitForSingleObject(notification.as_raw_handle(), 5000) },
        WAIT_OBJECT_0
    );
    assert!(endpoint.readiness().unwrap().0);
    // Exercise on-demand cleanup after process exit without Rust destructors.
    std::process::exit(0);
}

#[test]
fn cross_process_io_wakes_native_event_and_dead_waiter_is_reclaimed() {
    let (endpoint, mut client, handle, peer) = fixture(12);
    let fields: Vec<_> = endpoint.name.split('.').collect();
    let domain = u64::from_str_radix(fields[2], 16).unwrap();
    let id = u64::from_str_radix(fields[3], 16).unwrap();
    let name: Vec<u16> = format!("{}.TestReady", endpoint.name)
        .encode_utf16()
        .chain([0])
        .collect();
    let ready = Handle::new(unsafe { CreateEventW(ptr::null(), 1, 0, name.as_ptr()) }).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "usernet::stack::shared::notification::tests::child_waits_for_shared_io",
            "--test-threads=1",
        ])
        .env("KINAKAZE_PACKET_EVENT_CHILD", format!("{domain}:{id}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(0x08000000)
        .spawn()
        .unwrap();
    if unsafe { WaitForSingleObject(ready.0, 5000) } != WAIT_OBJECT_0 {
        let _ = child.kill();
        let output = child.wait_with_output().unwrap();
        panic!(
            "child registration failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    client.send_udp(handle, b"wake", peer).unwrap();
    client.poll(now_ms());
    endpoint.poll(Some(client.take_packet().unwrap())).unwrap();
    if unsafe { WaitForSingleObject(child.as_raw_handle(), 5000) } != WAIT_OBJECT_0 {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child did not wake");
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        unsafe {
            (&*endpoint.directory())
                .records
                .iter()
                .filter(|r| r.id != 0)
                .count()
        },
        1
    );
    let subscriptions: Vec<_> = (0..MAX_WAITERS)
        .map(|_| endpoint.subscribe().unwrap())
        .collect();
    assert_eq!(subscriptions.len(), MAX_WAITERS);
    assert_eq!(
        unsafe {
            (&*endpoint.directory())
                .records
                .iter()
                .filter(|r| r.id != 0)
                .count()
        },
        MAX_WAITERS
    );
}
