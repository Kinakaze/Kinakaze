use crate::{epoll, socket};
use std::time::{Duration, Instant};

struct Fd(i32);
impl Drop for Fd {
    fn drop(&mut self) {
        let _ = crate::close(self.0);
    }
}
fn tcp(family: i32, nonblocking: bool) -> Fd {
    Fd(socket::socket(
        family,
        socket::SOCK_STREAM
            | if nonblocking {
                socket::SOCK_NONBLOCK
            } else {
                0
            },
        socket::IPPROTO_TCP,
    )
    .unwrap())
}
fn closed_port(family: i32) -> (Fd, Vec<u8>) {
    let fd = tcp(family, false);
    let mut address = vec![0u8; if family == socket::AF_INET { 16 } else { 28 }];
    address[..2].copy_from_slice(&(family as u16).to_ne_bytes());
    if family == socket::AF_INET {
        address[4..8].copy_from_slice(&[127, 0, 0, 1]);
    } else {
        address[23] = 1;
    }
    unsafe { socket::bind(fd.0, address.as_ptr(), address.len() as i32).unwrap() };
    let mut length = address.len() as i32;
    unsafe { socket::getsockname(fd.0, address.as_mut_ptr(), &mut length).unwrap() };
    (fd, address)
}

#[test]
fn closed_loopback_ports_wake_epoll_with_refusal_before_a_two_second_deadline() {
    let _scope = crate::usernet::scope(1);
    for family in [socket::AF_INET, socket::AF_INET6] {
        let (_reserved, address) = closed_port(family);
        let client = tcp(family, true);
        let poll = Fd(epoll::epoll_create1(0).unwrap());
        epoll::epoll_ctl(
            poll.0,
            epoll::EPOLL_CTL_ADD,
            client.0,
            Some(epoll::EpollEvent {
                events: epoll::EPOLLOUT | epoll::EPOLLET,
                data: 13,
            }),
        )
        .unwrap();
        let start = Instant::now();
        let result = unsafe { socket::connect(client.0, address.as_ptr(), address.len() as i32) };
        if result == Err(crate::EINPROGRESS) {
            let mut events = [epoll::EpollEvent::default(); 1];
            assert_eq!(epoll::epoll_wait(poll.0, &mut events, 1500), Ok(1));
            assert_ne!(events[0].events & epoll::EPOLLERR, 0);
            let mut error = 0i32;
            let mut length = size_of::<i32>() as i32;
            unsafe {
                socket::getsockopt(
                    client.0,
                    socket::SOL_SOCKET,
                    socket::SO_ERROR,
                    (&raw mut error).cast(),
                    &mut length,
                )
                .unwrap()
            };
            assert_eq!(error, crate::ECONNREFUSED);
        } else {
            assert_eq!(result, Err(crate::ECONNREFUSED));
        }
        assert!(start.elapsed() < Duration::from_millis(1500));
    }
}

#[test]
fn blocking_and_ipv4_mapped_loopback_refusal_are_fast() {
    let _scope = crate::usernet::scope(1);
    for family in [socket::AF_INET, socket::AF_INET6] {
        let (_reserved, address) = closed_port(family);
        let client = tcp(family, false);
        let start = Instant::now();
        assert_eq!(
            unsafe { socket::connect(client.0, address.as_ptr(), address.len() as i32) },
            Err(crate::ECONNREFUSED)
        );
        assert!(start.elapsed() < Duration::from_millis(1500));
    }
    let (_reserved, ipv4) = closed_port(socket::AF_INET);
    let client = tcp(socket::AF_INET6, false);
    let off = 0i32;
    unsafe {
        socket::setsockopt(
            client.0,
            socket::IPPROTO_IPV6,
            26,
            (&raw const off).cast(),
            size_of::<i32>() as i32,
        )
        .unwrap()
    };
    let mut mapped = [0u8; 28];
    mapped[..2].copy_from_slice(&(socket::AF_INET6 as u16).to_ne_bytes());
    mapped[2..4].copy_from_slice(&ipv4[2..4]);
    mapped[18..20].fill(255);
    mapped[20..24].copy_from_slice(&ipv4[4..8]);
    let start = Instant::now();
    assert_eq!(
        unsafe { socket::connect(client.0, mapped.as_ptr(), mapped.len() as i32) },
        Err(crate::ECONNREFUSED)
    );
    assert!(start.elapsed() < Duration::from_millis(1500));
}

#[test]
fn leaving_loopback_restores_system_retransmission_policy() {
    let _scope = crate::usernet::scope(1);
    let (_reserved, address) = closed_port(socket::AF_INET);
    let client = tcp(socket::AF_INET, false);
    let raw = crate::get(client.0).unwrap().raw;
    super::configure(raw, &address).unwrap();
    let mut remote = address.clone();
    remote[4..8].copy_from_slice(&[192, 0, 2, 1]);
    super::configure(raw, &remote).unwrap();
    // Exercise the resulting native policy against an owned closed port.
    // No packet is sent to the documentation-only remote address above.
    let start = Instant::now();
    let result = unsafe {
        windows_sys::Win32::Networking::WinSock::connect(
            raw,
            address.as_ptr().cast(),
            address.len() as i32,
        )
    };
    assert_eq!(
        result,
        windows_sys::Win32::Networking::WinSock::SOCKET_ERROR
    );
    socket::wait_readiness(raw, socket::Readiness::WRITABLE, Some(5000)).unwrap();
    assert!(start.elapsed() >= Duration::from_millis(1500));
}

#[test]
fn udp_connect_and_payload_are_unchanged() {
    let _scope = crate::usernet::scope(1);
    let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let client =
        Fd(socket::socket(socket::AF_INET, socket::SOCK_DGRAM, socket::IPPROTO_UDP).unwrap());
    let mut address = [0u8; 16];
    address[..2].copy_from_slice(&(socket::AF_INET as u16).to_ne_bytes());
    address[2..4].copy_from_slice(&server.local_addr().unwrap().port().to_be_bytes());
    address[4..8].copy_from_slice(&[127, 0, 0, 1]);
    unsafe { socket::connect(client.0, address.as_ptr(), address.len() as i32).unwrap() };
    assert_eq!(crate::write(client.0, b"udp"), Ok(3));
    let mut bytes = [0; 3];
    assert_eq!(server.recv(&mut bytes).unwrap(), 3);
    assert_eq!(&bytes, b"udp");
}
