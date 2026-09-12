use super::*;
use crate::usernet::stack::{IpAddress, IpCidr, IpEndpoint, shared::Kind};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::time::Instant;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
};

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
fn dropped_name(id: u64) -> Vec<u16> {
    format!("Local\\Kinakaze.Packet.TransportTest.{id:016x}")
        .encode_utf16()
        .chain([0])
        .collect()
}
fn completed_name(id: u64) -> Vec<u16> {
    format!("Local\\Kinakaze.Packet.TransportTest.{id:016x}.Done")
        .encode_utf16()
        .chain([0])
        .collect()
}
fn process_endpoint(server: bool, ipv6: bool) -> IpEndpoint {
    let last = if server { 2 } else { 1 };
    IpEndpoint::new(
        if ipv6 {
            format!("fd96::{last}").parse().unwrap()
        } else {
            IpAddress::v4(10, 96, 0, last)
        },
        if server { 8080 } else { 40000 },
    )
}

#[test]
fn child_runs_independent_event_transport() {
    let Ok(config) = std::env::var("KINAKAZE_PACKET_TRANSPORT_CHILD") else {
        return;
    };
    let fields: Vec<_> = config.split(':').collect();
    let kind = if fields[0] == "tcp" {
        Kind::Tcp
    } else {
        Kind::Udp
    };
    let ipv6 = fields[1] == "1";
    let port: u16 = fields[2].parse().unwrap();
    let id: u64 = fields[3].parse().unwrap();
    let local = process_endpoint(true, ipv6);
    let peer = process_endpoint(false, ipv6);
    let endpoint = SharedEndpoint::create(
        std::process::id() as u64,
        id,
        &[IpCidr::new(local.addr, if ipv6 { 64 } else { 24 })],
        2,
        kind,
    )
    .unwrap();
    match kind {
        Kind::Tcp => endpoint.listen(local).unwrap(),
        Kind::Udp => endpoint.bind_udp(local).unwrap(),
    }
    endpoint.attach_filter(&[6, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    let event = crate::fs::object::Object::owned(unsafe {
        OpenEventW(EVENT_MODIFY_STATE, 0, dropped_name(id).as_ptr())
    })
    .unwrap();
    let link = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    link.connect(("127.0.0.1", port)).unwrap();
    assert_eq!(link.send(b"ready").unwrap(), 5);
    let mut transport = Transport::new(endpoint, link).unwrap();
    let deadline = now_ms() + 5000;
    loop {
        let progress = transport.drive().unwrap();
        if progress.filtered != 0 {
            assert_eq!(
                progress.transmitted, 0,
                "rejected SYN must not generate a SYN-ACK"
            );
            break;
        }
        transport.wait(Some(deadline)).unwrap();
    }
    assert!(!transport.endpoint.readiness().unwrap().0);
    if kind == Kind::Tcp {
        assert_eq!(transport.endpoint.tcp_state(), Ok(TcpState::Listen));
    }
    transport.endpoint.detach_filter().unwrap();
    assert_ne!(unsafe { SetEvent(event.raw()) }, 0);
    if kind == Kind::Tcp {
        transport
            .wait_ready(Interest::Connected, Some(5000))
            .unwrap();
    }
    transport
        .wait_ready(Interest::Readable, Some(5000))
        .unwrap();
    let mut data = [0; 32];
    match kind {
        Kind::Tcp => {
            assert_eq!(transport.endpoint.recv_tcp(&mut data), Ok(4));
            transport.endpoint.send_tcp(&data[..4]).unwrap();
        }
        Kind::Udp => {
            assert_eq!(
                transport.endpoint.recv_udp(&mut data, false),
                Ok((4, peer, 4))
            );
            transport.endpoint.send_udp(&data[..4], peer).unwrap();
        }
    }
    assert_eq!(&data[..4], b"wire");
    assert!(transport.drive().unwrap().transmitted > 0);
    let completed = crate::fs::object::Object::owned(unsafe {
        OpenEventW(
            windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE,
            0,
            completed_name(id).as_ptr(),
        )
    })
    .unwrap();
    // Keep the private packet link alive until its peer consumed the echo. An
    // abrupt host UDP close is not the TCP/UDP filtering scenario under test.
    assert_eq!(
        unsafe { WaitForSingleObject(completed.raw(), 5000) },
        WAIT_OBJECT_0
    );
}

#[test]
fn independent_processes_filter_and_exchange_tcp_udp_ipv4_ipv6_with_events() {
    for (index, (kind, ipv6)) in [
        (Kind::Tcp, false),
        (Kind::Tcp, true),
        (Kind::Udp, false),
        (Kind::Udp, true),
    ]
    .into_iter()
    .enumerate()
    {
        let id = now_ms() as u64 + ((30 + index as u64) << 48);
        let completed = crate::fs::object::Object::owned(unsafe {
            CreateEventW(std::ptr::null(), 1, 0, completed_name(id).as_ptr())
        })
        .unwrap();
        let event = crate::fs::object::Object::owned(unsafe {
            CreateEventW(std::ptr::null(), 1, 0, dropped_name(id).as_ptr())
        })
        .unwrap();
        let link = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        link.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let port = link.local_addr().unwrap().port();
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "usernet::stack::transport::tests::child_runs_independent_event_transport",
                    "--test-threads=1",
                ])
                .env(
                    "KINAKAZE_PACKET_TRANSPORT_CHILD",
                    format!(
                        "{}:{}:{port}:{id}",
                        if kind == Kind::Tcp { "tcp" } else { "udp" },
                        usize::from(ipv6)
                    ),
                )
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .creation_flags(0x08000000)
                .spawn()
                .unwrap(),
        );
        let mut hello = [0; 16];
        let (n, remote) = link.recv_from(&mut hello).unwrap();
        assert_eq!(&hello[..n], b"ready");
        link.connect(remote).unwrap();
        link.set_read_timeout(None).unwrap();
        let local = process_endpoint(false, ipv6);
        let peer = process_endpoint(true, ipv6);
        let endpoint = SharedEndpoint::create(
            std::process::id() as u64,
            id,
            &[IpCidr::new(local.addr, if ipv6 { 64 } else { 24 })],
            1,
            kind,
        )
        .unwrap();
        match kind {
            Kind::Tcp => endpoint.connect(local, peer).unwrap(),
            Kind::Udp => endpoint.bind_udp(local).unwrap(),
        }
        let mut transport = Transport::new(endpoint, link).unwrap();
        if kind == Kind::Tcp {
            transport
                .wait_ready(Interest::Connected, Some(5000))
                .unwrap();
            transport.endpoint.send_tcp(b"wire").unwrap();
        } else {
            transport.endpoint.send_udp(b"drop", peer).unwrap();
            transport.drive().unwrap();
            assert_eq!(
                unsafe { WaitForSingleObject(event.raw(), 5000) },
                WAIT_OBJECT_0
            );
            assert_eq!(
                transport.endpoint.recv_udp(&mut [0; 8], false),
                Err(crate::EAGAIN)
            );
            transport.endpoint.send_udp(b"wire", peer).unwrap();
        }
        transport
            .wait_ready(Interest::Readable, Some(5000))
            .unwrap();
        let mut data = [0; 32];
        match kind {
            Kind::Tcp => assert_eq!(transport.endpoint.recv_tcp(&mut data), Ok(4)),
            Kind::Udp => assert_eq!(
                transport.endpoint.recv_udp(&mut data, false),
                Ok((4, peer, 4))
            ),
        }
        assert_eq!(&data[..4], b"wire");
        assert_ne!(unsafe { SetEvent(completed.raw()) }, 0);
        assert_eq!(
            unsafe { WaitForSingleObject(child.0.as_raw_handle() as HANDLE, 5000) },
            WAIT_OBJECT_0
        );
        assert!(child.0.wait().unwrap().success());
    }
}

fn pair(kind: Kind, ipv6: bool, suffix: u64) -> (Transport, Transport, IpEndpoint, IpEndpoint) {
    let address = |last| {
        if ipv6 {
            format!("fd95::{last}").parse().unwrap()
        } else {
            IpAddress::v4(10, 95, 0, last)
        }
    };
    let local = IpEndpoint::new(address(1), 40000);
    let peer = IpEndpoint::new(address(2), 8080);
    let make = |endpoint: IpEndpoint, slot: u64| {
        SharedEndpoint::create(
            std::process::id() as u64,
            now_ms() as u64 + (suffix << 48) + slot * (1 << 40),
            &[IpCidr::new(endpoint.addr, if ipv6 { 64 } else { 24 })],
            2,
            kind,
        )
        .unwrap()
    };
    let a = make(local, 0);
    let b = make(peer, 1);
    match kind {
        Kind::Tcp => {
            b.listen(peer).unwrap();
            a.connect(local, peer).unwrap();
        }
        Kind::Udp => {
            a.bind_udp(local).unwrap();
            b.bind_udp(peer).unwrap();
        }
    }
    let a_link = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let b_link = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    a_link.connect(b_link.local_addr().unwrap()).unwrap();
    b_link.connect(a_link.local_addr().unwrap()).unwrap();
    (
        Transport::new(a, a_link).unwrap(),
        Transport::new(b, b_link).unwrap(),
        local,
        peer,
    )
}

#[test]
fn tcp_and_udp_transfer_over_afd_event_link() {
    for (index, (kind, ipv6)) in [
        (Kind::Tcp, false),
        (Kind::Tcp, true),
        (Kind::Udp, false),
        (Kind::Udp, true),
    ]
    .into_iter()
    .enumerate()
    {
        let (mut a, mut b, local, peer) = pair(kind, ipv6, 20 + index as u64);
        let (done, completed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            if kind == Kind::Tcp {
                b.wait_ready(Interest::Connected, Some(3000)).unwrap();
            }
            b.wait_ready(Interest::Readable, Some(3000)).unwrap();
            let mut buffer = [0; 32];
            match kind {
                Kind::Tcp => {
                    assert_eq!(b.endpoint.recv_tcp(&mut buffer), Ok(4));
                    assert_eq!(b.endpoint.send_tcp(&buffer[..4]), Ok(4));
                }
                Kind::Udp => {
                    assert_eq!(b.endpoint.recv_udp(&mut buffer, false), Ok((4, local, 4)));
                    b.endpoint.send_udp(&buffer[..4], local).unwrap();
                }
            }
            assert_eq!(&buffer[..4], b"wire");
            b.drive().unwrap();
            completed
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
        });
        if kind == Kind::Tcp {
            a.wait_ready(Interest::Connected, Some(3000)).unwrap();
            a.endpoint.send_tcp(b"wire").unwrap();
        } else {
            a.endpoint.send_udp(b"wire", peer).unwrap();
        }
        a.wait_ready(Interest::Readable, Some(3000)).unwrap();
        let mut buffer = [0; 32];
        match kind {
            Kind::Tcp => assert_eq!(a.endpoint.recv_tcp(&mut buffer), Ok(4)),
            Kind::Udp => assert_eq!(a.endpoint.recv_udp(&mut buffer, false), Ok((4, peer, 4))),
        }
        assert_eq!(&buffer[..4], b"wire");
        done.send(()).unwrap();
        worker.join().unwrap();
    }
}

#[test]
fn idle_link_waits_for_deadline_and_interrupt_retires_afd_request() {
    let (mut a, _b, _, _) = pair(Kind::Udp, false, 24);
    assert_eq!(a.drive().unwrap().received, 0);
    let begin = Instant::now();
    a.wait(Some(now_ms() + 80)).unwrap();
    assert!(
        begin.elapsed().as_millis() >= 50,
        "idle UDP writability must not wake the packet receiver"
    );
    let interrupted = crate::interrupt::current();
    assert!(!interrupted.is_null());
    unsafe {
        windows_sys::Win32::System::Threading::SetEvent(interrupted);
    }
    assert_eq!(a.wait(None), Err(crate::EINTR));
    // The preceding cancellation has completed before reusing the same request
    // storage/event. A stale completion must not satisfy this separate wait.
    let begin = Instant::now();
    a.wait(Some(now_ms() + 80)).unwrap();
    assert!(begin.elapsed().as_millis() >= 50);
}

#[test]
fn peer_application_write_wakes_an_idle_transport_without_a_timer() {
    let (mut a, mut b, local, peer) = pair(Kind::Udp, false, 25);
    let endpoint = a.endpoint.clone();
    let (ready, registered) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        assert_eq!(a.drive().unwrap().next_deadline_ms, None);
        ready.send(()).unwrap();
        a.wait(Some(now_ms() + 3000)).unwrap();
        assert_eq!(a.drive().unwrap().transmitted, 1);
    });
    registered
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    endpoint.send_udp(b"command", peer).unwrap();
    b.wait_ready(Interest::Readable, Some(3000)).unwrap();
    let mut buffer = [0; 16];
    assert_eq!(b.endpoint.recv_udp(&mut buffer, false), Ok((7, local, 7)));
    assert_eq!(&buffer[..7], b"command");
    worker.join().unwrap();
}

#[test]
fn packet_filter_drops_before_readiness_and_detach_does_not_replay() {
    let (mut a, mut b, _, peer) = pair(Kind::Udp, false, 26);
    b.endpoint.attach_filter(&[6, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    a.endpoint.send_udp(b"drop", peer).unwrap();
    assert_eq!(a.drive().unwrap().transmitted, 1);
    b.wait(Some(now_ms() + 3000)).unwrap();
    let progress = b.drive().unwrap();
    assert_eq!((progress.received, progress.filtered), (1, 1));
    assert!(!b.endpoint.readiness().unwrap().0);
    b.endpoint.detach_filter().unwrap();
    assert_eq!(b.wait_ready(Interest::Readable, Some(50)), Err(ETIMEDOUT));
    a.endpoint.send_udp(b"pass", peer).unwrap();
    a.drive().unwrap();
    b.wait_ready(Interest::Readable, Some(3000)).unwrap();
    let mut buffer = [0; 16];
    assert_eq!(b.endpoint.recv_udp(&mut buffer, false).unwrap().0, 4);
    assert_eq!(&buffer[..4], b"pass");
}
