use kinakaze_v2_manager::{ClientId, PeerIdentity, StateManager};
use kinakaze_v2_protocol::{
    ClientRole, ErrorCode, Hello, PROTOCOL_VERSION, PoolLaunch, Reply, Request,
};

fn connect(manager: &mut StateManager, role: ClientRole, pid: u32) -> ClientId {
    manager
        .connect(
            Hello {
                version: PROTOCOL_VERSION,
                token: "pool-secret".into(),
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
}

fn setup() -> (StateManager, ClientId, ClientId, ClientId) {
    let mut manager = StateManager::new(1, "pool-secret".into());
    let controller = connect(&mut manager, ClientRole::Controller, 10);
    let first = connect(&mut manager, ClientRole::Worker, 20);
    let second = connect(&mut manager, ClientRole::Worker, 30);
    manager.handle(first, Request::MarkPoolReady).unwrap();
    manager.handle(second, Request::MarkPoolReady).unwrap();
    (manager, controller, first, second)
}

fn launch(program: &str) -> PoolLaunch {
    PoolLaunch {
        arguments: vec![program.into(), "argument with spaces".into()],
        cwd: "/tmp".into(),
        environment: Some(vec!["APPLICATION_VALUE=one".into()]),
    }
}

fn reserve(manager: &mut StateManager, controller: ClientId) -> u32 {
    let Reply::PoolWorker { pid, .. } = manager
        .handle(controller, Request::ReservePoolWorker)
        .unwrap()
    else {
        panic!()
    };
    pid
}

#[test]
fn reservations_are_exclusive_and_eof_only_returns_unused_workers() {
    let (mut manager, controller, first, _) = setup();
    let other = connect(&mut manager, ClientRole::Controller, 10);
    let pid = reserve(&mut manager, controller);
    let other_pid = reserve(&mut manager, other);
    assert_ne!(pid, other_pid);
    assert_eq!(manager.pool_ready_count(), 0);
    assert_eq!(
        manager
            .handle(
                other,
                Request::ActivatePoolWorker {
                    pid,
                    launch: launch("/bin/a")
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    manager.disconnect(controller);
    assert_eq!(manager.pool_ready_count(), 1);
    assert_eq!(
        manager
            .handle(first, Request::AwaitPoolActivation)
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    assert_eq!(reserve(&mut manager, other), pid);
    manager
        .handle(other, Request::ReleasePoolWorker { pid })
        .unwrap();
    assert_eq!(manager.pool_ready_count(), 1);
}

#[test]
fn unrelated_applications_are_bound_once_after_reservation() {
    let (mut manager, controller, first, second) = setup();
    for (worker, program) in [(first, "/usr/bin/java"), (second, "/usr/bin/python3")] {
        let pid = reserve(&mut manager, controller);
        let config = launch(program);
        for _ in 0..2 {
            manager
                .handle(
                    controller,
                    Request::ActivatePoolWorker {
                        pid,
                        launch: config.clone(),
                    },
                )
                .unwrap();
        }
        assert_eq!(
            manager
                .handle(worker, Request::AwaitPoolActivation)
                .unwrap(),
            Reply::PoolLaunch(config)
        );
        assert_eq!(
            manager
                .handle(
                    controller,
                    Request::ActivatePoolWorker {
                        pid,
                        launch: launch("/bin/different")
                    }
                )
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            manager
                .handle(worker, Request::MarkPoolReady)
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            manager
                .handle(worker, Request::MarkPrewarmReady)
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            manager
                .handle(controller, Request::ActivatePrewarm { pid })
                .unwrap_err()
                .code,
            ErrorCode::NotReady
        );
    }
    manager.disconnect(controller);
    assert_eq!(manager.pool_ready_count(), 0);
}

#[test]
fn invalid_launch_does_not_consume_the_reservation_or_release_guest_code() {
    let (mut manager, controller, first, _) = setup();
    let pid = reserve(&mut manager, controller);
    for bad in [
        PoolLaunch {
            arguments: vec![],
            ..launch("/bin/a")
        },
        launch("relative"),
        PoolLaunch {
            cwd: "relative".into(),
            ..launch("/bin/a")
        },
        PoolLaunch {
            environment: Some(vec!["BROKEN=\0".into()]),
            ..launch("/bin/a")
        },
    ] {
        assert_eq!(
            manager
                .handle(controller, Request::ActivatePoolWorker { pid, launch: bad })
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            manager
                .handle(first, Request::AwaitPoolActivation)
                .unwrap_err()
                .code,
            ErrorCode::NotReady
        );
    }
    manager
        .handle(
            controller,
            Request::ActivatePoolWorker {
                pid,
                launch: launch("/bin/a"),
            },
        )
        .unwrap();
}

#[test]
fn worker_cannot_reserve_or_activate_and_dead_worker_cannot_be_reused() {
    let (mut manager, controller, first, _) = setup();
    let pid = reserve(&mut manager, controller);
    for request in [
        Request::ReservePoolWorker,
        Request::ReleasePoolWorker { pid },
        Request::ActivatePoolWorker {
            pid,
            launch: launch("/bin/a"),
        },
        Request::AwaitPoolReady { minimum: 1 },
    ] {
        assert_eq!(
            manager.handle(first, request).unwrap_err().code,
            ErrorCode::Unauthorized
        );
    }
    manager.process_exited_with_status(
        PeerIdentity {
            host_pid: 20,
            birth: 200,
        },
        7,
    );
    assert_eq!(
        manager
            .handle(
                controller,
                Request::ActivatePoolWorker {
                    pid,
                    launch: launch("/bin/a")
                }
            )
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
    assert_ne!(reserve(&mut manager, controller), pid);
}
