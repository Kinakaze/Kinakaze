use super::*;
use crate::socket;
use crate::usernet::stack::{
    IpCidr, Stack,
    shared::{Kind, SharedEndpoint},
    transport::{Interest, Transport},
};
use std::net::UdpSocket;
use std::os::windows::io::FromRawSocket;

fn fixture(last: u8) -> (i32, Arc<SharedEndpoint>, UdpSocket, IpEndpoint) {
    let fd = socket::socket(socket::AF_INET, socket::SOCK_DGRAM, 0).unwrap();
    let host = Address {
        family: 2,
        ip: [127, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ..Address::default()
    };
    let (bytes, length) = host.bytes(false);
    unsafe {
        socket::bind(fd, bytes.as_ptr(), length).unwrap();
    }
    let original = crate::usernet::endpoint(fd).unwrap().unwrap();
    let state = read(&original).unwrap();
    let address = IpEndpoint::new(IpAddress::v4(127, 99, 0, last), state.host.port);
    let root = SharedEndpoint::create(
        kinakaze_runtime::authority::domain_id(),
        original.store.id(),
        &[IpCidr::new(address.addr, 8)],
        1,
        Kind::Udp,
    )
    .unwrap();
    root.bind_udp(address).unwrap();
    edit(&original, |state| {
        state.packet = Some(Reference {
            arena: original.store.id(),
            slot: u32::MAX,
            generation: 0,
            managed: false,
        });
        state.local = super::address(address);
        Ok(())
    })
    .unwrap();
    let description = crate::get(fd).unwrap();
    publish(
        fd,
        description.description_id,
        Endpoint::open(original.store.clone(), original._namespace.clone()).unwrap(),
    )
    .unwrap();
    let link =
        socket::import_rights(&socket::export_rights(description.raw, std::process::id()).unwrap())
            .unwrap();
    (
        fd,
        root,
        unsafe { UdpSocket::from_raw_socket(link.into_raw() as _) },
        address,
    )
}

#[test]
fn descriptor_routes_carry_filtered_ip_frames_and_reject_an_unregistered_sender() {
    let (client_fd, client, client_link, client_address) = fixture(1);
    let (server_fd, server, server_link, server_address) = fixture(2);
    let host_server = server_link.local_addr().unwrap();
    let namespace = current().unwrap();
    let mut sender = Transport::routed(
        client.clone(),
        client_link,
        Arc::new(Router::new(namespace, None)),
    )
    .unwrap();
    let mut receiver = Transport::routed(
        server.clone(),
        server_link,
        Arc::new(Router::new(namespace, None)),
    )
    .unwrap();
    server.attach_filter(&[6, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    client.send_udp(b"drop", server_address).unwrap();
    assert_eq!(sender.drive().unwrap().transmitted, 1);
    assert_eq!(
        receiver.wait_ready(Interest::Readable, Some(30)),
        Err(crate::ETIMEDOUT)
    );
    assert!(!server.readiness().unwrap().0);
    server.detach_filter().unwrap();
    client.send_udp(b"routed", server_address).unwrap();
    sender.drive().unwrap();
    receiver.wait_ready(Interest::Readable, Some(1000)).unwrap();
    let mut data = [0; 32];
    let (n, peer, _) = server.recv_udp(&mut data, false).unwrap();
    assert_eq!(&data[..n], b"routed");
    assert_eq!(peer, client_address);

    let mut spoof = Stack::new(&[IpCidr::new(client_address.addr, 8)], 1, 1, 0).unwrap();
    let socket = spoof.bind_udp(client_address).unwrap();
    spoof.send_udp(socket, b"spoofed", server_address).unwrap();
    spoof.poll(0);
    let unregistered = UdpSocket::bind("127.0.0.1:0").unwrap();
    unregistered
        .send_to(&spoof.take_packet().unwrap(), host_server)
        .unwrap();
    assert_eq!(
        receiver.wait_ready(Interest::Readable, Some(30)),
        Err(crate::ETIMEDOUT)
    );
    assert!(!server.readiness().unwrap().0);
    crate::close(client_fd).unwrap();
    crate::close(server_fd).unwrap();
}

#[test]
fn vfs_socket_io_and_epoll_observe_protocol_payload_and_filtering() {
    use crate::epoll::*;
    let (client_fd, _, client_link, client_address) = fixture(3);
    let (server_fd, _, server_link, server_address) = fixture(4);
    drop(client_link);
    drop(server_link);
    let (destination, destination_len) = super::address(server_address).bytes(false);
    let send = |bytes: &[u8]| unsafe {
        socket::sendto(
            client_fd,
            bytes.as_ptr(),
            bytes.len(),
            0,
            destination.as_ptr(),
            destination_len,
        )
    };
    let epoll = epoll_create1(0).unwrap();
    epoll_ctl(
        epoll,
        EPOLL_CTL_ADD,
        server_fd,
        Some(EpollEvent {
            events: EPOLLIN | EPOLLET,
            data: 7,
        }),
    )
    .unwrap();
    let code = [6u8, 0, 0, 0, 0, 0, 0, 0];
    let mut program = [0u8; 16];
    program[..2].copy_from_slice(&1u16.to_le_bytes());
    program[8..].copy_from_slice(&(code.as_ptr() as u64).to_le_bytes());
    unsafe {
        socket::setsockopt(
            server_fd,
            socket::SOL_SOCKET,
            socket::SO_ATTACH_FILTER,
            program.as_ptr(),
            16,
        )
        .unwrap();
    }
    assert_eq!(send(b"filtered"), Ok(8));
    let mut events = [EpollEvent::default(); 2];
    assert_eq!(
        epoll_wait(epoll, &mut events, 30),
        Ok(0),
        "raw UDP input must not appear as guest readability"
    );
    let mut retained = [0; 8];
    let mut count = 1;
    unsafe {
        socket::getsockopt(
            server_fd,
            socket::SOL_SOCKET,
            socket::SO_GET_FILTER,
            retained.as_mut_ptr(),
            &mut count,
        )
        .unwrap();
    }
    assert_eq!(retained, code);
    assert_eq!(count, 1);
    unsafe {
        socket::setsockopt(
            server_fd,
            socket::SOL_SOCKET,
            socket::SO_DETACH_FILTER,
            [0u8; 4].as_ptr(),
            4,
        )
        .unwrap();
    }
    assert_eq!(send(b"payload"), Ok(7));
    assert_eq!(epoll_wait(epoll, &mut events, 1000), Ok(1));
    let cookie = events[0].data;
    assert_eq!(cookie, 7);
    assert_eq!(
        epoll_wait(epoll, &mut events, 0),
        Ok(0),
        "unchanged EPOLLET stays suppressed"
    );
    let mut data = [0; 32];
    let mut source = [0; 32];
    let mut source_len = 32;
    assert_eq!(
        unsafe {
            socket::recvfrom(
                server_fd,
                data.as_mut_ptr(),
                2,
                socket::MSG_PEEK | 0x20,
                source.as_mut_ptr(),
                &mut source_len,
            )
        },
        Ok(7)
    );
    assert_eq!(&data[..2], b"pa");
    assert_eq!(
        Address::parse(&source[..source_len as usize]).unwrap(),
        super::address(client_address)
    );
    assert_eq!(crate::read(server_fd, &mut data), Ok(7));
    assert_eq!(&data[..7], b"payload");
    assert_eq!(send(b"next"), Ok(4));
    assert_eq!(epoll_wait(epoll, &mut events, 1000), Ok(1));
    assert_eq!(crate::read(server_fd, &mut data), Ok(4));
    assert_eq!(&data[..4], b"next");
    crate::close(epoll).unwrap();
    crate::close(client_fd).unwrap();
    crate::close(server_fd).unwrap();
}

#[test]
fn packet_socket_lifecycle_supports_wildcard_autobind_and_connected_udp() {
    for family in [2, 10] {
        let create = || {
            let fd = socket::socket(family, socket::SOCK_DGRAM | socket::SOCK_NONBLOCK, 0).unwrap();
            crate::usernet::packet::initialize(fd).unwrap();
            assert!(
                crate::get(fd)
                    .unwrap()
                    .flags
                    .contains(crate::FdFlags::PACKET_SOCKET)
            );
            fd
        };
        let server = create();
        let client = create();
        let other = create();
        let wildcard = Address {
            family: family as u16,
            ..Address::default()
        };
        let (bytes, length) = wildcard.bytes(false);
        unsafe {
            socket::bind(server, bytes.as_ptr(), length).unwrap();
        }
        let mut destination = read(&crate::usernet::endpoint(server).unwrap().unwrap())
            .unwrap()
            .local
            .local_transport();
        destination.port = read(&crate::usernet::endpoint(server).unwrap().unwrap())
            .unwrap()
            .local
            .port;
        let (bytes, length) = destination.bytes(false);
        unsafe {
            socket::connect(client, bytes.as_ptr(), length).unwrap();
        }
        let peer = read(&crate::usernet::endpoint(client).unwrap().unwrap())
            .unwrap()
            .local;
        let (peer_bytes, peer_length) = peer.bytes(false);
        unsafe {
            socket::connect(server, peer_bytes.as_ptr(), peer_length).unwrap();
        }
        assert_eq!(crate::write(client, b"connected"), Ok(9));
        let mut data = [0; 32];
        assert_eq!(crate::read(server, &mut data), Ok(9));
        assert_eq!(&data[..9], b"connected");
        assert_eq!(
            unsafe { socket::sendto(other, b"foreign".as_ptr(), 7, 0, bytes.as_ptr(), length) },
            Ok(7)
        );
        assert_eq!(crate::read(server, &mut data), Err(crate::EAGAIN));
        unsafe {
            socket::connect(server, [0u8; 16].as_ptr(), 16).unwrap();
        }
        assert_eq!(
            unsafe { socket::sendto(other, b"allowed".as_ptr(), 7, 0, bytes.as_ptr(), length) },
            Ok(7)
        );
        assert_eq!(crate::read(server, &mut data), Ok(7));
        assert_eq!(&data[..7], b"allowed");
        for fd in [other, client, server] {
            crate::close(fd).unwrap();
        }
    }
}
