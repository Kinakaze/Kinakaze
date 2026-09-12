use super::*;
use crate::{FdFlags, namespaces, route_state, socket};

struct Namespace {
    fd: i32,
    id: u64,
}
impl Namespace {
    fn current() -> Self {
        Self {
            fd: namespaces::open_process(crate::job::process_id(), namespaces::NET, FdFlags::NONE)
                .unwrap(),
            id: current().unwrap(),
        }
    }
    fn new() -> Self {
        namespaces::prepare(0x40000000).unwrap().install().unwrap();
        Self::current()
    }
}
impl Drop for Namespace {
    fn drop(&mut self) {
        crate::close(self.fd).unwrap();
    }
}
fn link(index: u32, name: &str, kind: &str, master: u32, peer: u32) -> route_state::Link {
    route_state::Link {
        index,
        name: name.into(),
        kind: kind.into(),
        flags: 1,
        mtu: 1500,
        address: vec![2, 0, 0, 0, 0, index as u8],
        broadcast: vec![255; 6],
        master,
        peer,
        attributes: Vec::new(),
    }
}
fn endpoint_address(last: u8, port: u16) -> Address {
    Address {
        family: 2,
        port,
        ip: [10, 202, 0, last, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        scope: 0,
    }
}
fn configure(ns: u64, index: u32, peer: u32, last: u8) {
    route_state::transaction_for(ns, |state| {
        state.links.push(link(index, "eth0", "veth", 0, peer));
        state.addresses.push(route_state::Address {
            index,
            family: 2,
            prefix_len: 24,
            flags: 0,
            scope: 0,
            attributes: vec![route_state::Attribute {
                kind: 2,
                value: vec![10, 202, 0, last],
            }],
        });
        Ok::<_, i32>(())
    })
    .unwrap()
    .unwrap();
}

#[test]
fn two_private_namespaces_exchange_tcp_through_the_same_bridge() {
    let original = Namespace::current();
    let root = Namespace::new();
    let server_ns = Namespace::new();
    let client_ns = Namespace::new();
    namespaces::enter(original.fd, 0x40000000, |_| Ok(())).unwrap();
    route_state::transaction_for(root.id, |state| {
        state.links.extend([
            link(701, "br0", "bridge", 0, 0),
            link(702, "port1", "veth", 701, 704),
            link(703, "port2", "veth", 701, 705),
        ]);
        Ok::<_, i32>(())
    })
    .unwrap()
    .unwrap();
    configure(server_ns.id, 704, 702, 2);
    configure(client_ns.id, 705, 703, 3);
    let server = {
        let _scope = scope(server_ns.id);
        let fd = socket::socket(2, 1, 0).unwrap();
        let (bytes, len) = Address {
            family: 2,
            ..Address::default()
        }
        .bytes(false);
        unsafe {
            socket::bind(fd, bytes.as_ptr(), len).unwrap();
        }
        socket::listen(fd, 4).unwrap();
        fd
    };
    let port = read(&endpoint(server).unwrap().unwrap())
        .unwrap()
        .local
        .port;
    let client = {
        let _scope = scope(client_ns.id);
        let fd = socket::socket(2, 1, 0).unwrap();
        let (bytes, len) = endpoint_address(2, port).bytes(false);
        unsafe {
            socket::connect(fd, bytes.as_ptr(), len).expect("cross-namespace TCP connect");
        }
        fd
    };
    let accepted =
        unsafe { socket::accept(server, std::ptr::null_mut(), std::ptr::null_mut(), 0).unwrap() };
    crate::write(client, b"bridge").unwrap();
    let mut bytes = [0; 6];
    assert_eq!(crate::read(accepted, &mut bytes).unwrap(), 6);
    assert_eq!(&bytes, b"bridge");
    for fd in [accepted, client] {
        crate::close(fd).unwrap();
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "usernet::bridge_tests::bridge_client_helper",
            "--ignored",
            "--nocapture",
        ])
        .env("KINAKAZE_TEST_BRIDGE_NS", client_ns.id.to_string())
        .env("KINAKAZE_TEST_BRIDGE_PORT", port.to_string())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            panic!(
                "cross-process bridge client timed out: {:?}",
                child.wait_with_output().unwrap()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let accepted =
        unsafe { socket::accept(server, std::ptr::null_mut(), std::ptr::null_mut(), 0).unwrap() };
    assert_eq!(crate::read(accepted, &mut bytes).unwrap(), 6);
    assert_eq!(&bytes, b"remote");
    crate::close(accepted).unwrap();
    crate::close(server).unwrap();
}

#[test]
#[ignore = "invoked in an independent native process by the bridge test"]
fn bridge_client_helper() {
    let namespace = std::env::var("KINAKAZE_TEST_BRIDGE_NS")
        .unwrap()
        .parse()
        .unwrap();
    let port = std::env::var("KINAKAZE_TEST_BRIDGE_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let _scope = scope(namespace);
    let client = socket::socket(2, 1, 0).unwrap();
    let (bytes, length) = endpoint_address(2, port).bytes(false);
    unsafe {
        socket::connect(client, bytes.as_ptr(), length).unwrap();
    }
    assert_eq!(crate::write(client, b"remote").unwrap(), 6);
    crate::close(client).unwrap();
}
