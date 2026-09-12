use super::*;
use crate::socket::{self, AF_INET, SOCK_DGRAM};
use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};

fn inherited(pin: &crate::fs::object::Object) -> bool {
    let mut flags = 0;
    assert_ne!(unsafe { GetHandleInformation(pin.raw(), &mut flags) }, 0);
    flags & HANDLE_FLAG_INHERIT != 0
}
fn alias(fd: i32) -> i32 {
    let entry = crate::get(fd).unwrap();
    let socket =
        socket::import_rights(&socket::export_rights(entry.raw, std::process::id()).unwrap())
            .unwrap();
    let fd = crate::install_duplicate(socket.0, entry.kind, entry.flags, entry).unwrap();
    socket.into_raw();
    fd
}

#[test]
fn state_and_namespace_pins_follow_alias_exec_policy_and_last_local_close() {
    let fd = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();
    let alias = alias(fd);
    let endpoint = endpoint(fd).unwrap().unwrap();
    assert!(
        endpoint
            .handoff_pins()
            .unwrap()
            .iter()
            .all(|pin| inherited(pin))
    );
    crate::set_close_on_exec(fd, true).unwrap();
    crate::with_execve_handle_filter(|| {
        assert!(
            endpoint
                .handoff_pins()
                .unwrap()
                .iter()
                .all(|pin| inherited(pin))
        );
    })
    .unwrap();
    crate::set_close_on_exec(alias, true).unwrap();
    crate::with_execve_handle_filter(|| {
        assert!(
            endpoint
                .handoff_pins()
                .unwrap()
                .iter()
                .all(|pin| !inherited(pin))
        );
    })
    .unwrap();
    crate::with_exec_handle_filter(|| {
        assert!(
            endpoint
                .handoff_pins()
                .unwrap()
                .iter()
                .all(|pin| !inherited(pin))
        );
    })
    .unwrap();
    assert!(
        endpoint
            .handoff_pins()
            .unwrap()
            .iter()
            .all(|pin| inherited(pin))
    );
    crate::close(fd).unwrap();
    assert!(
        endpoint
            .handoff_pins()
            .unwrap()
            .iter()
            .all(|pin| inherited(pin))
    );
    crate::close(alias).unwrap();
    assert!(
        endpoint
            .handoff_pins()
            .unwrap()
            .iter()
            .all(|pin| !inherited(pin))
    );
}

#[test]
fn serialized_pins_retain_network_state_after_the_original_registry_is_gone() {
    let fd = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();
    let alias = alias(fd);
    let original = endpoint(fd).unwrap().unwrap();
    edit(&original, |state| {
        state.reuse = true;
        Ok(())
    })
    .unwrap();
    let id = original.store.id();
    let mut bytes = serialize(|candidate| candidate == fd || candidate == alias).unwrap();
    assert_eq!(bytes.len(), 8 + 52, "one handoff record per description");
    // Model the destination's independently owned inherited handles. Restoring
    // must consume these duplicates, not close the sender's original handles.
    for (index, pin) in original.handoff_pins().unwrap().iter().enumerate() {
        let handoff = crate::fs::object::Object::duplicate(pin.raw()).unwrap();
        let offset = 20 + index * 8;
        bytes[offset..offset + 8].copy_from_slice(&(handoff.raw() as u64).to_le_bytes());
        std::mem::forget(handoff);
    }
    closed(crate::get(fd).unwrap().description_id);
    drop(original);
    assert!(
        Store::user_object(id, false).is_ok(),
        "handoff pins retain state"
    );
    assert!(restore(&bytes));
    let restored = endpoint(alias).unwrap().unwrap();
    assert_eq!(restored.store.id(), id);
    assert!(read(&restored).unwrap().reuse);
    assert!(
        restored
            .handoff_pins()
            .unwrap()
            .iter()
            .all(|pin| inherited(pin))
    );
    crate::close(fd).unwrap();
    crate::close(alias).unwrap();
    drop(restored);
    assert!(
        Store::user_object(id, false).is_err(),
        "last reference releases state"
    );
}

#[test]
fn packet_buffers_and_filter_follow_the_descriptor_handoff_reference() {
    use super::stack::{
        IpAddress, IpCidr, IpEndpoint, Stack,
        shared::{Kind, SharedEndpoint},
    };
    let fd = socket::socket(AF_INET, SOCK_DGRAM, 0).unwrap();
    let original = endpoint(fd).unwrap().unwrap();
    let id = original.store.id();
    let domain = kinakaze_runtime::authority::domain_id();
    let address = IpEndpoint::new(IpAddress::v4(10, 98, 0, 1), 8080);
    let peer = IpEndpoint::new(IpAddress::v4(10, 98, 0, 2), 45000);
    let protocol =
        SharedEndpoint::create(domain, id, &[IpCidr::new(address.addr, 24)], 2, Kind::Udp).unwrap();
    protocol.bind_udp(address).unwrap();
    let mut client = Stack::new(&[IpCidr::new(peer.addr, 24)], 3, 61, 0).unwrap();
    let sender = client.bind_udp(peer).unwrap();
    client.send_udp(sender, b"before-handoff", address).unwrap();
    client.poll(0);
    protocol.poll(Some(client.take_packet().unwrap())).unwrap();
    protocol.attach_filter(&[6, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    protocol.lock_filter(true).unwrap();
    edit(&original, |state| {
        state.packet = Some(packet::Reference {
            arena: id,
            slot: u32::MAX,
            generation: 0,
            managed: false,
        });
        Ok(())
    })
    .unwrap();
    let bound = Endpoint::open(original.store.clone(), original._namespace.clone()).unwrap();
    publish(fd, crate::get(fd).unwrap().description_id, bound.clone()).unwrap();
    let mut frame = serialize(|candidate| candidate == fd).unwrap();
    for (index, pin) in bound.handoff_pins().unwrap().iter().enumerate() {
        let owned = crate::fs::object::Object::duplicate(pin.raw()).unwrap();
        let offset = 20 + index * 8;
        frame[offset..offset + 8].copy_from_slice(&(owned.raw() as u64).to_le_bytes());
        std::mem::forget(owned);
    }
    closed(crate::get(fd).unwrap().description_id);
    drop(original);
    drop(bound);
    drop(protocol);
    assert!(restore(&frame));
    let restored = endpoint(fd).unwrap().unwrap();
    let protocol = &restored.packet.as_ref().unwrap().endpoint;
    assert_eq!(protocol.filter_program().unwrap(), [6, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(protocol.detach_filter(), Err(crate::EPERM));
    let mut data = [0; 32];
    let (copied, _, full) = protocol.recv_udp(&mut data, false).unwrap();
    assert_eq!(&data[..copied], b"before-handoff");
    assert_eq!(full, copied);
    crate::close(fd).unwrap();
    drop(restored);
    assert!(SharedEndpoint::open(domain, id).is_err());
}
