use kinakaze_v2_manager::{PeerIdentity, StateManager};
use kinakaze_v2_protocol::{ClientRole, ErrorCode, Hello, PROTOCOL_VERSION, Reply, Request};

fn setup() -> (StateManager, u64, u64, u32, PeerIdentity) {
    let mut manager = StateManager::new(1, "prewarm-test-credential".into());
    let mut connect = |role, pid| {
        manager
            .connect(
                Hello {
                    version: PROTOCOL_VERSION,
                    token: "prewarm-test-credential".into(),
                    role,
                    adoption_ticket: None,
                },
                PeerIdentity {
                    host_pid: pid,
                    birth: u64::from(pid) * 10,
                },
            )
            .unwrap()
            .0
    };
    let controller = connect(ClientRole::Controller, 1);
    let worker = connect(ClientRole::Worker, 2);
    let Reply::Identity(identity) = manager.handle(worker, Request::Identity).unwrap() else {
        panic!()
    };
    (
        manager,
        controller,
        worker,
        identity.pid,
        PeerIdentity {
            host_pid: 2,
            birth: 20,
        },
    )
}

#[test]
fn init_activation_is_explicit_idempotent_and_single_use() {
    let (mut manager, controller, worker, pid, _) = setup();
    assert_eq!(
        manager
            .handle(controller, Request::ActivatePrewarm { pid })
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    manager.handle(worker, Request::MarkPrewarmReady).unwrap();
    assert_eq!(
        manager
            .handle(controller, Request::AwaitPrewarmReady { pid })
            .unwrap(),
        Reply::Ok
    );
    assert_eq!(
        manager
            .handle(worker, Request::AwaitPrewarmActivation)
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    for _ in 0..2 {
        assert_eq!(
            manager
                .handle(controller, Request::ActivatePrewarm { pid })
                .unwrap(),
            Reply::Ok
        );
    }
    assert_eq!(
        manager
            .handle(worker, Request::AwaitPrewarmActivation)
            .unwrap(),
        Reply::Ok
    );
    assert_eq!(
        manager
            .handle(worker, Request::MarkPrewarmReady)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}

#[test]
fn worker_cannot_release_itself_and_exit_cannot_leave_a_ready_worker() {
    let (mut manager, controller, worker, pid, peer) = setup();
    manager.handle(worker, Request::MarkPrewarmReady).unwrap();
    assert_eq!(
        manager
            .handle(worker, Request::ActivatePrewarm { pid })
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        manager
            .handle(controller, Request::MarkPrewarmReady)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    manager.process_exited_with_status(peer, 7);
    assert_eq!(
        manager
            .handle(controller, Request::AwaitPrewarmReady { pid })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        manager
            .handle(controller, Request::ActivatePrewarm { pid })
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        manager
            .handle(controller, Request::AwaitExit { pid })
            .unwrap(),
        Reply::Exit { status: 7 }
    );
}

#[test]
fn controller_eof_does_not_activate_parked_worker() {
    let (mut manager, controller, worker, _, _) = setup();
    manager.handle(worker, Request::MarkPrewarmReady).unwrap();
    manager.disconnect(controller);
    assert_eq!(
        manager
            .handle(worker, Request::AwaitPrewarmActivation)
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
}

#[test]
fn fork_child_cannot_inherit_or_rearm_root_prewarming() {
    let (mut manager, controller, worker, pid, _) = setup();
    manager.handle(worker, Request::MarkPrewarmReady).unwrap();
    manager
        .handle(controller, Request::ActivatePrewarm { pid })
        .unwrap();
    let Reply::ForkPrepared(ticket) = manager
        .handle(worker, Request::PrepareFork { request_key: 1 })
        .unwrap()
    else {
        panic!()
    };
    let child = manager
        .connect(
            Hello {
                version: PROTOCOL_VERSION,
                token: "prewarm-test-credential".into(),
                role: ClientRole::Worker,
                adoption_ticket: Some(ticket.token),
            },
            PeerIdentity {
                host_pid: 3,
                birth: 30,
            },
        )
        .unwrap()
        .0;
    manager.handle(child, Request::MarkReady).unwrap();
    manager
        .handle(
            worker,
            Request::CommitFork {
                transaction: ticket.transaction,
            },
        )
        .unwrap();
    assert_eq!(
        manager
            .handle(child, Request::AwaitPrewarmActivation)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        manager
            .handle(child, Request::MarkPrewarmReady)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}
