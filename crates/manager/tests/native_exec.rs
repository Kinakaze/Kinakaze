use kinakaze_v2_manager::{NativeExecState as S, PeerIdentity, StateManager};
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
        token: "execs".into(),
        role,
        adoption_ticket: None,
    }
}

#[test]
fn only_active_workers_can_request_exec_stock() {
    let mut manager = StateManager::new(1, "execs".into());
    let parent = manager
        .connect(hello(ClientRole::Worker), peer(10))
        .unwrap()
        .0;
    assert_eq!(
        manager.handle(parent, Request::NativeExec).unwrap(),
        Reply::Ok
    );
    for (i, role) in [
        ClientRole::Controller,
        ClientRole::Launcher,
        ClientRole::Helper,
    ]
    .into_iter()
    .enumerate()
    {
        let client = manager.connect(hello(role), peer(20 + i as u32)).unwrap().0;
        assert_eq!(
            manager
                .handle(client, Request::NativeExec)
                .unwrap_err()
                .code,
            ErrorCode::Unauthorized
        );
    }
    let Reply::ExecPrepared { transaction: _ } = manager
        .handle(
            parent,
            Request::PrepareExec {
                request_key: 1,
                replacement_host_pid: 30,
                replacement_birth: peer(30).birth,
            },
        )
        .unwrap()
    else {
        panic!()
    };
    let child = manager
        .connect(hello(ClientRole::Worker), peer(30))
        .unwrap()
        .0;
    assert_eq!(
        manager.handle(child, Request::NativeExec).unwrap_err().code,
        ErrorCode::NotReady
    );
    manager.process_exited(peer(10));
    assert!(manager.handle(parent, Request::NativeExec).is_err());
}

#[test]
fn exec_custody_tracks_exact_native_identities_and_irreversible_commit() {
    for commit in [false, true] {
        let mut manager = StateManager::new(1, "execs".into());
        let parent = manager
            .connect(hello(ClientRole::Worker), peer(10))
            .unwrap()
            .0;
        assert_eq!(manager.native_exec_state(peer(10), peer(30)), S::Unreserved);
        assert_eq!(manager.native_exec_state(peer(11), peer(30)), S::Aborted);
        let Reply::ExecPrepared { transaction } = manager
            .handle(
                parent,
                Request::PrepareExec {
                    request_key: 1,
                    replacement_host_pid: 30,
                    replacement_birth: peer(30).birth,
                },
            )
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(manager.native_exec_state(peer(10), peer(30)), S::Pending);
        assert_eq!(
            manager.native_exec_state(
                peer(10),
                PeerIdentity {
                    birth: 99,
                    ..peer(30)
                }
            ),
            S::Unreserved
        );
        let child = manager
            .connect(hello(ClientRole::Worker), peer(30))
            .unwrap()
            .0;
        manager.handle(child, Request::MarkReady).unwrap();
        assert_eq!(manager.native_exec_state(peer(10), peer(30)), S::Pending);
        manager
            .handle(
                parent,
                if commit {
                    Request::CommitExec { transaction }
                } else {
                    Request::AbortExec { transaction }
                },
            )
            .unwrap();
        assert_eq!(
            manager.native_exec_state(peer(10), peer(30)),
            if commit { S::Committed } else { S::Aborted }
        );
        manager.process_exited(peer(10));
        assert_eq!(
            manager.native_exec_state(peer(10), peer(30)),
            if commit { S::Committed } else { S::Aborted }
        );
    }
}
