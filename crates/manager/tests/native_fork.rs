use kinakaze_v2_manager::{PeerIdentity, StateManager};
use kinakaze_v2_protocol::{
    ClientRole, ErrorCode, Hello, PROTOCOL_VERSION, Reply, Request,
    native_fork::{Module, Spec},
};

fn peer(pid: u32) -> PeerIdentity {
    PeerIdentity {
        host_pid: pid,
        birth: u64::from(pid) * 10,
    }
}
fn hello(role: ClientRole) -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        token: "forks".into(),
        role,
        adoption_ticket: None,
    }
}
fn spec() -> Spec {
    Spec {
        tls_slots: 3,
        modules: vec![Module {
            base: 0x10000,
            path: "C:\\native\\libc.so".into(),
        }],
    }
}
fn take(transaction: u64) -> Request {
    Request::NativeFork {
        transaction,
        spec: spec(),
    }
}

#[test]
fn only_the_active_reservation_owner_can_claim_a_native_bootstrap() {
    let mut manager = StateManager::new(1, "forks".into());
    let parent = manager
        .connect(hello(ClientRole::Worker), peer(10))
        .unwrap()
        .0;
    let stranger = manager
        .connect(hello(ClientRole::Worker), peer(11))
        .unwrap()
        .0;
    let Reply::ForkPrepared(ticket) = manager
        .handle(parent, Request::PrepareFork { request_key: 1 })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        manager.handle(parent, take(ticket.transaction)).unwrap(),
        Reply::Ok
    );
    assert_eq!(
        manager
            .handle(stranger, take(ticket.transaction))
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    for (index, role) in [
        ClientRole::Controller,
        ClientRole::Launcher,
        ClientRole::Helper,
    ]
    .into_iter()
    .enumerate()
    {
        let client = manager
            .connect(hello(role), peer(20 + index as u32))
            .unwrap()
            .0;
        assert_eq!(
            manager
                .handle(client, take(ticket.transaction))
                .unwrap_err()
                .code,
            ErrorCode::Unauthorized
        );
    }
    let mut invalid = spec();
    invalid.modules[0].base = 1;
    assert_eq!(
        manager
            .handle(
                parent,
                Request::NativeFork {
                    transaction: ticket.transaction,
                    spec: invalid
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        manager.native_fork_state(ticket.transaction, peer(30)),
        Some(false)
    );
    manager.process_exited(peer(10));
    assert_eq!(
        manager.native_fork_state(ticket.transaction, peer(30)),
        None
    );
    assert!(manager.handle(parent, take(ticket.transaction)).is_err());
}

#[test]
fn adoption_keeps_rollback_until_commit_and_abort_revokes_candidates() {
    for commit in [false, true] {
        let mut manager = StateManager::new(1, "forks".into());
        let parent = manager
            .connect(hello(ClientRole::Worker), peer(10))
            .unwrap()
            .0;
        let Reply::ForkPrepared(ticket) = manager
            .handle(parent, Request::PrepareFork { request_key: 1 })
            .unwrap()
        else {
            panic!()
        };
        let mut adoption = hello(ClientRole::Worker);
        adoption.adoption_ticket = Some(ticket.token);
        let child = manager.connect(adoption, peer(30)).unwrap().0;
        assert_eq!(
            manager
                .handle(parent, take(ticket.transaction))
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(
            manager
                .handle(child, take(ticket.transaction))
                .unwrap_err()
                .code,
            ErrorCode::NotReady
        );
        assert_eq!(
            manager.native_fork_state(ticket.transaction, peer(30)),
            Some(false)
        );
        manager.handle(child, Request::MarkReady).unwrap();
        assert_eq!(
            manager.native_fork_state(ticket.transaction, peer(30)),
            Some(false)
        );
        let request = if commit {
            Request::CommitFork {
                transaction: ticket.transaction,
            }
        } else {
            Request::AbortFork {
                transaction: ticket.transaction,
            }
        };
        manager.handle(parent, request).unwrap();
        assert_eq!(
            manager.native_fork_state(ticket.transaction, peer(30)),
            if commit { Some(true) } else { None }
        );
        if commit {
            manager.acknowledge(parent, ticket.transaction).unwrap();
            assert_eq!(
                manager.native_fork_state(ticket.transaction, peer(30)),
                Some(true)
            );
            manager.process_exited(peer(10));
            assert_eq!(
                manager.native_fork_state(ticket.transaction, peer(30)),
                Some(true)
            );
        }
    }
}
