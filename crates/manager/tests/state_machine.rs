use kinakaze_v2_manager::{ClientId, Limits, PeerIdentity, StateManager};
use kinakaze_v2_protocol::{
    ClientRole, ErrorCode, ForkPolicy, ForkTicket, Hello, PROTOCOL_VERSION, ProcessIdentity, Reply,
    Request,
};

const SECRET: &str = "unit-test-session-secret";

#[test]
fn helpers_are_authenticated_but_own_no_linux_process_or_control_privileges() {
    let mut manager = manager();
    let mut bad = hello(ClientRole::Helper, None);
    bad.token.clear();
    assert_eq!(
        manager.connect(bad, peer(1)).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    assert!(
        manager
            .connect(hello(ClientRole::Helper, Some("ticket".into())), peer(1))
            .is_err()
    );
    let (helper, reply) = manager
        .connect(hello(ClientRole::Helper, None), peer(1))
        .unwrap();
    assert_eq!(
        reply,
        Reply::Hello {
            epoch: 17,
            process: None
        }
    );
    assert_eq!(manager.stats().processes, 0);
    for request in [
        Request::Identity,
        Request::Stats,
        Request::Shutdown,
        Request::PrepareFork { request_key: 1 },
    ] {
        assert_eq!(
            manager.handle(helper, request).unwrap_err().code,
            ErrorCode::Unauthorized
        );
    }
    for role in [
        ClientRole::Worker,
        ClientRole::Controller,
        ClientRole::Helper,
    ] {
        assert!(manager.connect(hello(role, None), peer(1)).is_err());
    }
    let worker = worker(&mut manager, 2);
    assert_eq!(identity(&mut manager, worker).pid, 1);
    assert!(
        manager
            .connect(hello(ClientRole::Helper, None), peer(2))
            .is_err()
    );
    manager.disconnect(helper);
    assert_eq!(manager.stats().processes, 1);
    assert!(!manager.is_shutdown_requested());
}

fn manager() -> StateManager {
    StateManager::new(17, SECRET.into())
}

fn peer(host_pid: u32) -> PeerIdentity {
    PeerIdentity {
        host_pid,
        birth: 10_000 + u64::from(host_pid),
    }
}

fn hello(role: ClientRole, adoption_ticket: Option<String>) -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        token: SECRET.into(),
        role,
        adoption_ticket,
    }
}

fn worker(manager: &mut StateManager, native_pid: u32) -> ClientId {
    manager
        .connect(hello(ClientRole::Worker, None), peer(native_pid))
        .unwrap()
        .0
}

fn identity(manager: &mut StateManager, client: ClientId) -> ProcessIdentity {
    match manager.handle(client, Request::Identity).unwrap() {
        Reply::Identity(identity) => identity,
        other => panic!("unexpected identity response: {other:?}"),
    }
}

fn register(manager: &mut StateManager, client: ClientId) {
    assert_eq!(
        manager
            .handle(
                client,
                Request::RegisterModule {
                    module_id: 1,
                    schema: 1
                }
            )
            .unwrap(),
        Reply::Ok
    );
}

fn define(manager: &mut StateManager, client: ClientId, name: &str, fork: ForkPolicy) -> u64 {
    match manager
        .handle(
            client,
            Request::DefineState {
                module_id: 1,
                name: name.into(),
                initial: 10,
                fork,
            },
        )
        .unwrap()
    {
        Reply::State {
            object_id,
            value: 10,
        } => object_id,
        other => panic!("unexpected define response: {other:?}"),
    }
}

fn write(manager: &mut StateManager, client: ClientId, name: &str, value: u64) {
    assert_eq!(
        manager
            .handle(
                client,
                Request::WriteState {
                    module_id: 1,
                    name: name.into(),
                    value
                }
            )
            .unwrap(),
        Reply::Ok
    );
}

fn read(manager: &mut StateManager, client: ClientId, name: &str) -> (u64, u64) {
    match manager
        .handle(
            client,
            Request::ReadState {
                module_id: 1,
                name: name.into(),
            },
        )
        .unwrap()
    {
        Reply::State { object_id, value } => (object_id, value),
        other => panic!("unexpected read response: {other:?}"),
    }
}

fn prepare(manager: &mut StateManager, parent: ClientId, request_key: u64) -> ForkTicket {
    match manager
        .handle(parent, Request::PrepareFork { request_key })
        .unwrap()
    {
        Reply::ForkPrepared(ticket) => ticket,
        other => panic!("unexpected prepare response: {other:?}"),
    }
}

fn adopt(manager: &mut StateManager, ticket: &ForkTicket, native_pid: u32) -> ClientId {
    let (child, reply) = manager
        .connect(
            hello(ClientRole::Worker, Some(ticket.token.clone())),
            peer(native_pid),
        )
        .unwrap();
    assert_eq!(
        reply,
        Reply::Hello {
            epoch: 17,
            process: Some(ticket.child)
        }
    );
    child
}

fn activate(manager: &mut StateManager, parent: ClientId, child: ClientId, ticket: &ForkTicket) {
    manager.handle(child, Request::MarkReady).unwrap();
    manager
        .handle(
            parent,
            Request::CommitFork {
                transaction: ticket.transaction,
            },
        )
        .unwrap();
    assert_eq!(
        manager.handle(child, Request::AwaitActivation).unwrap(),
        Reply::Ok
    );
}

#[test]
fn all_fork_policies_snapshot_at_prepare_and_publish_only_after_commit() {
    let mut manager = manager();
    let parent = worker(&mut manager, 100);
    register(&mut manager, parent);
    let copy_id = define(&mut manager, parent, "copy", ForkPolicy::Copy);
    let share_id = define(&mut manager, parent, "share", ForkPolicy::Share);
    let reset_id = define(&mut manager, parent, "reset", ForkPolicy::Reset);
    for name in ["copy", "share", "reset"] {
        write(&mut manager, parent, name, 20);
    }
    let ticket = prepare(&mut manager, parent, 71);
    assert_eq!(ticket.token.len(), 64);
    assert_eq!(manager.stats().objects, 5);
    for name in ["copy", "share", "reset"] {
        write(&mut manager, parent, name, 30);
    }
    let child = adopt(&mut manager, &ticket, 101);
    assert_eq!(
        identity(&mut manager, child).parent_pid,
        identity(&mut manager, parent).pid
    );
    assert_eq!(
        manager
            .handle(
                child,
                Request::ReadState {
                    module_id: 1,
                    name: "copy".into()
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    assert_eq!(
        manager
            .handle(child, Request::PrepareFork { request_key: 1 })
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    register(&mut manager, child);
    assert_eq!(
        manager
            .handle(
                child,
                Request::RegisterModule {
                    module_id: 2,
                    schema: 1
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    assert_eq!(
        manager
            .handle(
                parent,
                Request::CommitFork {
                    transaction: ticket.transaction
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    assert_eq!(
        manager
            .handle(child, Request::AwaitActivation)
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    manager.handle(child, Request::MarkReady).unwrap();
    manager.handle(child, Request::MarkReady).unwrap();
    assert_eq!(
        manager
            .handle(child, Request::AwaitActivation)
            .unwrap_err()
            .code,
        ErrorCode::NotReady
    );
    activate(&mut manager, parent, child, &ticket);
    let (child_copy, copy) = read(&mut manager, child, "copy");
    let (child_share, share) = read(&mut manager, child, "share");
    let (child_reset, reset) = read(&mut manager, child, "reset");
    assert_eq!((copy, share, reset), (20, 30, 10));
    assert_ne!(child_copy, copy_id);
    assert_eq!(child_share, share_id);
    assert_ne!(child_reset, reset_id);
    for name in ["copy", "share", "reset"] {
        write(&mut manager, child, name, 99);
    }
    assert_eq!(read(&mut manager, parent, "copy").1, 30);
    assert_eq!(read(&mut manager, parent, "share").1, 99);
    assert_eq!(read(&mut manager, parent, "reset").1, 30);
}

#[test]
fn credentials_version_roles_and_one_process_per_worker_are_enforced() {
    let mut manager = manager();
    let mut bad = hello(ClientRole::Worker, None);
    bad.token = "wrong".into();
    assert_eq!(
        manager.connect(bad, peer(1)).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    let mut bad = hello(ClientRole::Worker, None);
    bad.version += 1;
    assert_eq!(
        manager.connect(bad, peer(1)).unwrap_err().code,
        ErrorCode::VersionMismatch
    );
    assert_eq!(manager.stats().processes, 0);
    let parent = worker(&mut manager, 1);
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker, None), peer(1))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        manager.handle(parent, Request::Shutdown).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    let controller = manager
        .connect(hello(ClientRole::Controller, None), peer(2))
        .unwrap()
        .0;
    assert_eq!(
        manager
            .handle(controller, Request::Identity)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    assert_eq!(manager.stats().processes, 1);
    assert_eq!(
        manager.handle(9999, Request::Stats).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    manager.handle(controller, Request::Shutdown).unwrap();
    assert!(manager.is_shutdown_requested());
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker, None), peer(3))
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
}

#[test]
fn schema_conflicts_and_state_ownership_do_not_mutate_existing_values() {
    let mut manager = manager();
    let owner = worker(&mut manager, 1);
    let other = worker(&mut manager, 2);
    register(&mut manager, owner);
    let object = define(&mut manager, owner, "local", ForkPolicy::Copy);
    assert_eq!(
        manager
            .handle(
                other,
                Request::RegisterModule {
                    module_id: 1,
                    schema: 2
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    register(&mut manager, other);
    assert_eq!(
        manager
            .handle(
                other,
                Request::ReadState {
                    module_id: 1,
                    name: "local".into()
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        manager
            .handle(
                other,
                Request::WriteState {
                    module_id: 1,
                    name: "local".into(),
                    value: 55
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    write(&mut manager, owner, "local", 88);
    assert_eq!(
        manager
            .handle(
                owner,
                Request::DefineState {
                    module_id: 1,
                    name: "local".into(),
                    initial: 10,
                    fork: ForkPolicy::Copy
                }
            )
            .unwrap(),
        Reply::State {
            object_id: object,
            value: 88
        }
    );
    for (initial, fork) in [(11, ForkPolicy::Copy), (10, ForkPolicy::Share)] {
        assert_eq!(
            manager
                .handle(
                    owner,
                    Request::DefineState {
                        module_id: 1,
                        name: "local".into(),
                        initial,
                        fork
                    }
                )
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
    }
    assert_eq!(manager.stats().objects, 1);
    assert_eq!(read(&mut manager, owner, "local"), (object, 88));
}

#[test]
fn prepare_commit_and_abort_replays_obey_transaction_ownership() {
    let mut manager = manager();
    let parent = worker(&mut manager, 1);
    let stranger = worker(&mut manager, 2);
    let ticket = prepare(&mut manager, parent, 11);
    assert_eq!(prepare(&mut manager, parent, 11), ticket);
    assert_eq!(manager.stats().transactions, 1);
    for request in [
        Request::CommitFork {
            transaction: ticket.transaction,
        },
        Request::AbortFork {
            transaction: ticket.transaction,
        },
    ] {
        assert_eq!(
            manager.handle(stranger, request).unwrap_err().code,
            ErrorCode::Unauthorized
        );
    }
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker, Some("0".repeat(64))), peer(3))
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        manager
            .connect(
                hello(ClientRole::Controller, Some(ticket.token.clone())),
                peer(3)
            )
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let child = adopt(&mut manager, &ticket, 3);
    assert_eq!(
        manager
            .connect(
                hello(ClientRole::Worker, Some(ticket.token.clone())),
                peer(4)
            )
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    activate(&mut manager, parent, child, &ticket);
    assert_eq!(
        manager
            .handle(
                parent,
                Request::CommitFork {
                    transaction: ticket.transaction
                }
            )
            .unwrap(),
        Reply::Ok
    );
    assert_eq!(
        manager
            .handle(
                parent,
                Request::AbortFork {
                    transaction: ticket.transaction
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(prepare(&mut manager, parent, 11), ticket);
    let aborted = prepare(&mut manager, parent, 12);
    assert_ne!(aborted.token, ticket.token);
    for _ in 0..2 {
        manager
            .handle(
                parent,
                Request::AbortFork {
                    transaction: aborted.transaction,
                },
            )
            .unwrap();
    }
    assert_eq!(
        manager
            .handle(
                parent,
                Request::CommitFork {
                    transaction: aborted.transaction
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
    assert_eq!(
        manager
            .handle(parent, Request::PrepareFork { request_key: 12 })
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker, Some(aborted.token)), peer(4))
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
}

#[test]
fn abort_releases_reserved_objects_but_retains_attached_child_until_native_exit() {
    let mut manager = manager();
    let parent = worker(&mut manager, 1);
    register(&mut manager, parent);
    define(&mut manager, parent, "copy", ForkPolicy::Copy);
    let prepared = prepare(&mut manager, parent, 1);
    assert_eq!(manager.stats().objects, 2);
    manager
        .handle(
            parent,
            Request::AbortFork {
                transaction: prepared.transaction,
            },
        )
        .unwrap();
    assert_eq!(manager.stats().objects, 1);
    let ticket = prepare(&mut manager, parent, 2);
    let child = adopt(&mut manager, &ticket, 2);
    manager.handle(child, Request::MarkReady).unwrap();
    manager
        .handle(
            parent,
            Request::AbortFork {
                transaction: ticket.transaction,
            },
        )
        .unwrap();
    assert_eq!(
        manager
            .handle(child, Request::AwaitActivation)
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
    assert_eq!(manager.stats().objects, 2);
    manager.disconnect(child);
    assert_eq!(manager.stats().processes, 2);
    assert_eq!(manager.stats().objects, 2);
    manager.process_exited(peer(2));
    assert_eq!(manager.stats().processes, 1);
    assert_eq!(manager.stats().objects, 1);
}

#[test]
fn pipe_eof_revokes_client_without_claiming_process_death() {
    let mut manager = manager();
    let parent = worker(&mut manager, 1);
    register(&mut manager, parent);
    define(&mut manager, parent, "copy", ForkPolicy::Copy);
    let ticket = prepare(&mut manager, parent, 1);
    manager.disconnect(parent);
    assert_eq!(manager.stats().clients, 0);
    assert_eq!(manager.stats().processes, 1);
    assert_eq!(manager.stats().objects, 1);
    assert_eq!(
        manager.handle(parent, Request::Identity).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker, None), peer(1))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker, Some(ticket.token)), peer(2))
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
    manager.process_exited(PeerIdentity {
        host_pid: 1,
        birth: peer(1).birth + 1,
    });
    assert_eq!(manager.stats().processes, 1);
    manager.process_exited(peer(1));
    manager.process_exited(peer(1));
    assert_eq!(manager.stats().processes, 0);
    assert_eq!(manager.stats().objects, 0);
    assert_eq!(manager.stats().transactions, 0);
    let replacement = worker(&mut manager, 1);
    assert_ne!(identity(&mut manager, replacement).pid, 1);
}

#[test]
fn native_child_exit_aborts_ready_fork_before_parent_can_commit() {
    let mut manager = manager();
    let parent = worker(&mut manager, 1);
    let ticket = prepare(&mut manager, parent, 1);
    let child = adopt(&mut manager, &ticket, 2);
    manager.handle(child, Request::MarkReady).unwrap();
    manager.process_exited(peer(2));
    assert_eq!(
        manager
            .handle(
                parent,
                Request::CommitFork {
                    transaction: ticket.transaction
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
    assert_eq!(manager.stats().processes, 1);
}

#[test]
fn parent_exit_reparents_committed_child_and_preserves_shared_objects() {
    let mut manager = manager();
    let parent = worker(&mut manager, 1);
    register(&mut manager, parent);
    let object = define(&mut manager, parent, "shared", ForkPolicy::Share);
    let ticket = prepare(&mut manager, parent, 1);
    let child = adopt(&mut manager, &ticket, 2);
    activate(&mut manager, parent, child, &ticket);
    assert_ne!(identity(&mut manager, child).parent_pid, 0);
    manager.process_exited(peer(1));
    assert_eq!(identity(&mut manager, child).parent_pid, 0);
    assert_eq!(read(&mut manager, child, "shared"), (object, 10));
    assert_eq!(manager.stats().processes, 1);
    assert_eq!(manager.stats().objects, 1);
    assert_eq!(manager.stats().transactions, 0);
    manager.process_exited(peer(2));
    assert_eq!(manager.stats().objects, 0);
}

#[test]
fn parent_exit_aborts_pending_child_but_keeps_its_live_process_reference() {
    let mut manager = manager();
    let parent = worker(&mut manager, 1);
    let ticket = prepare(&mut manager, parent, 1);
    let child = adopt(&mut manager, &ticket, 2);
    manager.process_exited(peer(1));
    assert_eq!(identity(&mut manager, child).parent_pid, 0);
    assert_eq!(
        manager
            .handle(child, Request::AwaitActivation)
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
    assert_eq!(manager.stats().processes, 1);
    manager.process_exited(peer(2));
    assert_eq!(manager.stats().processes, 0);
}

#[test]
fn limits_reject_work_atomically_and_reservations_count_toward_process_quota() {
    let limits = Limits {
        max_clients: 3,
        max_processes: 2,
        max_modules: 1,
        max_states_per_process: 1,
        max_state_name_bytes: 8,
        max_request_keys_per_process: 1,
        max_transactions: 1,
        max_objects: 2,
        max_exit_records: 2,
    };
    let mut manager = StateManager::with_limits(17, SECRET.into(), limits);
    let parent = worker(&mut manager, 1);
    register(&mut manager, parent);
    assert_eq!(
        manager
            .handle(
                parent,
                Request::RegisterModule {
                    module_id: 2,
                    schema: 1
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        manager
            .handle(
                parent,
                Request::DefineState {
                    module_id: 1,
                    name: "123456789".into(),
                    initial: 10,
                    fork: ForkPolicy::Copy
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    for name in ["", "bad\0name", "bad\nname"] {
        assert_eq!(
            manager
                .handle(
                    parent,
                    Request::DefineState {
                        module_id: 1,
                        name: name.into(),
                        initial: 10,
                        fork: ForkPolicy::Copy
                    }
                )
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
    }
    define(&mut manager, parent, "one", ForkPolicy::Copy);
    assert_eq!(
        manager
            .handle(
                parent,
                Request::DefineState {
                    module_id: 1,
                    name: "two".into(),
                    initial: 10,
                    fork: ForkPolicy::Copy
                }
            )
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    let ticket = prepare(&mut manager, parent, 1);
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker, None), peer(9))
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(
        manager
            .handle(parent, Request::PrepareFork { request_key: 2 })
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(prepare(&mut manager, parent, 1), ticket);
    let child = adopt(&mut manager, &ticket, 2);
    activate(&mut manager, parent, child, &ticket);
    let before = manager.stats();
    assert_eq!(
        manager
            .handle(child, Request::PrepareFork { request_key: 1 })
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(manager.stats(), before);
    manager
        .connect(hello(ClientRole::Controller, None), peer(3))
        .unwrap();
    assert_eq!(
        manager
            .connect(hello(ClientRole::Controller, None), peer(4))
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
}

#[test]
fn object_quota_failure_does_not_consume_pid_request_key_or_transaction() {
    let mut manager = StateManager::with_limits(
        17,
        SECRET.into(),
        Limits {
            max_objects: 1,
            ..Limits::default()
        },
    );
    let parent = worker(&mut manager, 1);
    register(&mut manager, parent);
    define(&mut manager, parent, "copy", ForkPolicy::Copy);
    let before = manager.stats();
    assert_eq!(
        manager
            .handle(parent, Request::PrepareFork { request_key: 1 })
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    assert_eq!(manager.stats(), before);
    assert_eq!(
        manager
            .handle(parent, Request::PrepareFork { request_key: 1 })
            .unwrap_err()
            .code,
        ErrorCode::LimitExceeded
    );
    let second = worker(&mut manager, 2);
    assert_eq!(identity(&mut manager, second).pid, 2);
}
