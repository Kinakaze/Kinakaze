use kinakaze_v2_manager::{ClientId, PeerIdentity, StateManager};
use kinakaze_v2_protocol::{ClientRole, ErrorCode, Hello, PROTOCOL_VERSION, Reply, Request};

fn peer(pid: u32) -> PeerIdentity {
    PeerIdentity {
        host_pid: pid,
        birth: u64::from(pid) * 10,
    }
}
fn hello(role: ClientRole) -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        token: "images".into(),
        role,
        adoption_ticket: None,
    }
}
fn connect(manager: &mut StateManager, role: ClientRole, pid: u32) -> ClientId {
    manager.connect(hello(role), peer(pid)).unwrap().0
}
fn request() -> Request {
    Request::ImageSnapshot {
        source: 123,
        length: 8 * 1024 * 1024,
    }
}

fn catalog() -> Request {
    Request::NativeCatalog {
        directory: "C:\\session\\native".into(),
    }
}

#[test]
fn only_live_worker_owners_can_request_native_image_capabilities() {
    let mut manager = StateManager::new(1, "images".into());
    for (role, pid) in [
        (ClientRole::Controller, 10),
        (ClientRole::Launcher, 11),
        (ClientRole::Helper, 12),
    ] {
        let client = connect(&mut manager, role, pid);
        assert_eq!(
            manager.handle(client, request()).unwrap_err().code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            manager.handle(client, catalog()).unwrap_err().code,
            ErrorCode::Unauthorized
        );
    }
    let owner = connect(&mut manager, ClientRole::Worker, 20);
    assert_eq!(manager.handle(owner, request()).unwrap(), Reply::Ok);
    assert_eq!(manager.handle(owner, catalog()).unwrap(), Reply::Ok);
    let Reply::ForkPrepared(ticket) = manager
        .handle(owner, Request::PrepareFork { request_key: 1 })
        .unwrap()
    else {
        panic!()
    };
    let mut child_hello = hello(ClientRole::Worker);
    child_hello.adoption_ticket = Some(ticket.token);
    let child = manager.connect(child_hello, peer(21)).unwrap().0;
    assert_eq!(
        manager.handle(child, request()).unwrap_err().code,
        ErrorCode::NotReady
    );
    // Provider restoration needs immutable declarations before fork commit;
    // it cannot use them to mutate manager state or release guest execution.
    assert_eq!(manager.handle(child, catalog()).unwrap(), Reply::Ok);
    manager.process_exited(peer(20));
    assert!(manager.handle(owner, request()).is_err());
    assert!(manager.handle(child, catalog()).is_err());
}

#[test]
fn exec_candidate_can_read_images_but_retired_owner_cannot() {
    let mut manager = StateManager::new(1, "images".into());
    let owner = connect(&mut manager, ClientRole::Worker, 20);
    let Reply::ExecPrepared { transaction } = manager
        .handle(
            owner,
            Request::PrepareExec {
                request_key: 1,
                replacement_host_pid: 21,
                replacement_birth: peer(21).birth,
            },
        )
        .unwrap()
    else {
        panic!()
    };
    let child = connect(&mut manager, ClientRole::Worker, 21);
    assert_eq!(manager.handle(child, request()).unwrap(), Reply::Ok);
    assert_eq!(manager.handle(child, catalog()).unwrap(), Reply::Ok);
    manager.handle(child, Request::MarkReady).unwrap();
    manager
        .handle(owner, Request::CommitExec { transaction })
        .unwrap();
    assert_eq!(
        manager.handle(owner, request()).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        manager.handle(owner, catalog()).unwrap_err().code,
        ErrorCode::Unauthorized
    );
}

#[test]
fn v1_controllers_remain_compatible_and_unknown_versions_are_rejected() {
    let mut manager = StateManager::new(1, "images".into());
    let mut controller = hello(ClientRole::Controller);
    controller.version = 1;
    let client = manager.connect(controller, peer(10)).unwrap().0;
    assert!(matches!(
        manager.handle(client, Request::Stats).unwrap(),
        Reply::Stats(_)
    ));
    for version in [0, PROTOCOL_VERSION + 1] {
        let mut worker = hello(ClientRole::Worker);
        worker.version = version;
        assert_eq!(
            manager.connect(worker, peer(20)).unwrap_err().code,
            ErrorCode::VersionMismatch
        );
    }
}
