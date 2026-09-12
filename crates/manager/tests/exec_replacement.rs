use kinakaze_v2_manager::{ClientId, Limits, PeerIdentity, StateManager};
use kinakaze_v2_protocol::{
    ClientRole, ErrorCode, ForkPolicy, ForkTicket, Hello, PROTOCOL_VERSION, ProcessIdentity, Reply,
    Request,
};

const SECRET: &str = "exec-test-secret";

fn manager() -> StateManager {
    StateManager::new(71, SECRET.into())
}

fn peer(host_pid: u32) -> PeerIdentity {
    PeerIdentity {
        host_pid,
        birth: 100_000 + u64::from(host_pid),
    }
}

fn hello(role: ClientRole) -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        token: SECRET.into(),
        role,
        adoption_ticket: None,
    }
}

fn worker(manager: &mut StateManager, host_pid: u32) -> ClientId {
    manager
        .connect(hello(ClientRole::Worker), peer(host_pid))
        .unwrap()
        .0
}

fn identity(manager: &mut StateManager, client: ClientId) -> ProcessIdentity {
    match manager.handle(client, Request::Identity).unwrap() {
        Reply::Identity(identity) => identity,
        other => panic!("unexpected identity reply: {other:?}"),
    }
}

fn setup_state(manager: &mut StateManager, client: ClientId) -> u64 {
    manager
        .handle(
            client,
            Request::RegisterModule {
                module_id: 1,
                schema: 2,
            },
        )
        .unwrap();
    match manager
        .handle(
            client,
            Request::DefineState {
                module_id: 1,
                name: "retained".into(),
                initial: 41,
                fork: ForkPolicy::Copy,
            },
        )
        .unwrap()
    {
        Reply::State {
            object_id,
            value: 41,
        } => object_id,
        other => panic!("unexpected state reply: {other:?}"),
    }
}

fn read_state() -> Request {
    Request::ReadState {
        module_id: 1,
        name: "retained".into(),
    }
}

fn exec_request(request_key: u64, target: u32) -> Request {
    Request::PrepareExec {
        request_key,
        replacement_host_pid: target,
        replacement_birth: peer(target).birth,
    }
}

fn prepare(manager: &mut StateManager, owner: ClientId, key: u64, target: u32) -> u64 {
    match manager.handle(owner, exec_request(key, target)).unwrap() {
        Reply::ExecPrepared { transaction } => transaction,
        other => panic!("unexpected exec prepare reply: {other:?}"),
    }
}

fn ready(manager: &mut StateManager, candidate: ClientId) {
    assert_eq!(
        manager.handle(candidate, Request::MarkReady).unwrap(),
        Reply::Ok
    );
}

fn commit(manager: &mut StateManager, owner: ClientId, transaction: u64) {
    assert_eq!(
        manager
            .handle(owner, Request::CommitExec { transaction })
            .unwrap(),
        Reply::Ok
    );
}

fn assert_error(
    manager: &mut StateManager,
    client: ClientId,
    request: Request,
    expected: ErrorCode,
) {
    assert_eq!(manager.handle(client, request).unwrap_err().code, expected);
}

fn fork(
    manager: &mut StateManager,
    owner: ClientId,
    key: u64,
    host_pid: u32,
) -> (ClientId, ForkTicket) {
    let ticket = match manager
        .handle(owner, Request::PrepareFork { request_key: key })
        .unwrap()
    {
        Reply::ForkPrepared(ticket) => ticket,
        other => panic!("unexpected fork prepare reply: {other:?}"),
    };
    let mut child_hello = hello(ClientRole::Worker);
    child_hello.adoption_ticket = Some(ticket.token.clone());
    let child = manager.connect(child_hello, peer(host_pid)).unwrap().0;
    ready(manager, child);
    manager
        .handle(
            owner,
            Request::CommitFork {
                transaction: ticket.transaction,
            },
        )
        .unwrap();
    (child, ticket)
}

#[test]
fn exec_commit_preserves_pid_parent_children_modules_and_same_state_objects() {
    let mut manager = manager();
    let root = worker(&mut manager, 10);
    let (owner, _) = fork(&mut manager, root, 1, 11);
    let object_id = setup_state(&mut manager, owner);
    let before_identity = identity(&mut manager, owner);
    let (child, _) = fork(&mut manager, owner, 1, 12);
    let before = manager.stats();
    let transaction = prepare(&mut manager, owner, 1, 13);
    assert_eq!(prepare(&mut manager, owner, 1, 13), transaction);
    assert_error(
        &mut manager,
        owner,
        Request::AwaitExecReady { transaction },
        ErrorCode::NotReady,
    );
    assert_error(
        &mut manager,
        owner,
        Request::CommitExec { transaction },
        ErrorCode::NotReady,
    );
    let candidate = worker(&mut manager, 13);
    assert_eq!(identity(&mut manager, candidate), before_identity);
    assert_eq!(manager.stats().processes, before.processes);
    assert_eq!(manager.stats().objects, before.objects);
    assert_error(&mut manager, candidate, read_state(), ErrorCode::NotReady);
    assert_error(
        &mut manager,
        candidate,
        Request::AwaitActivation,
        ErrorCode::NotReady,
    );
    manager
        .handle(
            candidate,
            Request::RegisterModule {
                module_id: 1,
                schema: 2,
            },
        )
        .unwrap();
    assert_error(
        &mut manager,
        candidate,
        Request::RegisterModule {
            module_id: 1,
            schema: 3,
        },
        ErrorCode::Conflict,
    );
    assert_error(
        &mut manager,
        candidate,
        Request::RegisterModule {
            module_id: 2,
            schema: 1,
        },
        ErrorCode::NotReady,
    );
    ready(&mut manager, candidate);
    ready(&mut manager, candidate);
    assert_eq!(
        manager
            .handle(owner, Request::AwaitExecReady { transaction })
            .unwrap(),
        Reply::Ok
    );
    manager
        .handle(
            owner,
            Request::WriteState {
                module_id: 1,
                name: "retained".into(),
                value: 99,
            },
        )
        .unwrap();
    commit(&mut manager, owner, transaction);
    assert_error(
        &mut manager,
        candidate,
        Request::AwaitActivation,
        ErrorCode::NotReady,
    );
    manager.process_exited(peer(11));
    assert_eq!(
        manager.handle(candidate, Request::AwaitActivation).unwrap(),
        Reply::Ok
    );
    assert_eq!(identity(&mut manager, candidate), before_identity);
    assert_eq!(
        manager.handle(candidate, read_state()).unwrap(),
        Reply::State {
            object_id,
            value: 99
        }
    );
    assert_eq!(
        identity(&mut manager, child).parent_pid,
        before_identity.pid
    );
    assert_error(
        &mut manager,
        owner,
        Request::Identity,
        ErrorCode::Unauthorized,
    );
    assert_error(&mut manager, owner, read_state(), ErrorCode::Unauthorized);
    manager.process_exited(peer(11));
    assert_eq!(
        identity(&mut manager, child).parent_pid,
        before_identity.pid
    );
    assert_eq!(identity(&mut manager, candidate), before_identity);
    assert_eq!(manager.stats().processes, before.processes);
    manager.process_exited(peer(13));
    assert_eq!(identity(&mut manager, child).parent_pid, 0);
    assert_eq!(manager.stats().processes, before.processes - 1);
}

#[test]
fn retired_worker_replay_eof_and_death_cannot_revoke_successor_transactions() {
    let mut manager = manager();
    let old = worker(&mut manager, 1);
    let transaction = prepare(&mut manager, old, 1, 2);
    let replacement = worker(&mut manager, 2);
    ready(&mut manager, replacement);
    commit(&mut manager, old, transaction);
    commit(&mut manager, old, transaction);
    assert_error(
        &mut manager,
        old,
        Request::AbortExec { transaction },
        ErrorCode::Conflict,
    );
    assert_error(
        &mut manager,
        old,
        exec_request(1, 2),
        ErrorCode::Unauthorized,
    );
    manager.disconnect(old);
    assert_error(
        &mut manager,
        replacement,
        Request::AwaitActivation,
        ErrorCode::NotReady,
    );
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker), peer(1))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        manager
            .connect(hello(ClientRole::Controller), peer(1))
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    manager.process_exited(peer(1));
    let second = prepare(&mut manager, replacement, 2, 3);
    let candidate = worker(&mut manager, 3);
    ready(&mut manager, candidate);
    let fork_ticket = match manager
        .handle(replacement, Request::PrepareFork { request_key: 1 })
        .unwrap()
    {
        Reply::ForkPrepared(ticket) => ticket,
        other => panic!("unexpected reply: {other:?}"),
    };
    manager.process_exited(peer(1)); // duplicate old death cannot abort successor work
    assert_eq!(
        manager
            .handle(
                replacement,
                Request::AwaitExecReady {
                    transaction: second
                }
            )
            .unwrap(),
        Reply::Ok
    );
    let mut child_hello = hello(ClientRole::Worker);
    child_hello.adoption_ticket = Some(fork_ticket.token);
    let child = manager.connect(child_hello, peer(4)).unwrap().0;
    ready(&mut manager, child);
    commit(&mut manager, replacement, second);
    manager.process_exited(peer(2));
    manager
        .handle(
            candidate,
            Request::CommitFork {
                transaction: fork_ticket.transaction,
            },
        )
        .unwrap();
    manager.process_exited(peer(2));
    assert_eq!(
        manager.handle(candidate, Request::AwaitActivation).unwrap(),
        Reply::Ok
    );
    assert_eq!(
        identity(&mut manager, child).parent_pid,
        identity(&mut manager, candidate).pid
    );
}

#[test]
fn abort_before_or_after_readiness_preserves_owner_and_revokes_candidate() {
    for mark_ready in [false, true] {
        let mut manager = manager();
        let owner = worker(&mut manager, 1);
        let object_id = setup_state(&mut manager, owner);
        let transaction = prepare(&mut manager, owner, 1, 2);
        let candidate = worker(&mut manager, 2);
        if mark_ready {
            ready(&mut manager, candidate);
        }
        for _ in 0..2 {
            assert_eq!(
                manager
                    .handle(owner, Request::AbortExec { transaction })
                    .unwrap(),
                Reply::Ok
            );
        }
        assert_error(
            &mut manager,
            owner,
            Request::CommitExec { transaction },
            ErrorCode::Aborted,
        );
        assert_error(
            &mut manager,
            owner,
            Request::AwaitExecReady { transaction },
            ErrorCode::Aborted,
        );
        assert_error(&mut manager, owner, exec_request(1, 2), ErrorCode::Aborted);
        assert_error(
            &mut manager,
            candidate,
            Request::Identity,
            ErrorCode::Aborted,
        );
        assert_error(
            &mut manager,
            candidate,
            Request::AwaitActivation,
            ErrorCode::Aborted,
        );
        assert_eq!(
            manager.handle(owner, read_state()).unwrap(),
            Reply::State {
                object_id,
                value: 41
            }
        );
        manager.disconnect(candidate);
        assert_eq!(
            manager
                .connect(hello(ClientRole::Worker), peer(2))
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        manager.process_exited(peer(2));
        assert_eq!(manager.stats().processes, 1);
        assert_eq!(manager.stats().objects, 1);
        prepare(&mut manager, owner, 2, 3);
    }
}

#[test]
fn candidate_disconnect_and_native_exit_abort_ready_exec() {
    for native_exit in [false, true] {
        let mut manager = manager();
        let owner = worker(&mut manager, 1);
        let transaction = prepare(&mut manager, owner, 1, 2);
        let candidate = worker(&mut manager, 2);
        ready(&mut manager, candidate);
        manager.process_exited(PeerIdentity {
            host_pid: 2,
            birth: peer(2).birth + 1,
        });
        assert_eq!(
            manager
                .handle(owner, Request::AwaitExecReady { transaction })
                .unwrap(),
            Reply::Ok
        );
        if native_exit {
            manager.process_exited(peer(2));
        } else {
            manager.disconnect(candidate);
        }
        assert_error(
            &mut manager,
            owner,
            Request::CommitExec { transaction },
            ErrorCode::Aborted,
        );
        assert_eq!(manager.stats().processes, 1);
        assert_eq!(identity(&mut manager, owner).pid, 1);
    }
}

#[test]
fn original_owner_disconnect_and_native_exit_abort_uncommitted_replacement() {
    for native_exit in [false, true] {
        let mut manager = manager();
        let owner = worker(&mut manager, 1);
        setup_state(&mut manager, owner);
        prepare(&mut manager, owner, 1, 2);
        let candidate = worker(&mut manager, 2);
        ready(&mut manager, candidate);
        if native_exit {
            manager.process_exited(peer(1));
        } else {
            manager.disconnect(owner);
        }
        assert_error(
            &mut manager,
            candidate,
            Request::AwaitActivation,
            ErrorCode::Aborted,
        );
        assert_error(&mut manager, candidate, Request::Stats, ErrorCode::Aborted);
        assert_eq!(manager.stats().processes, usize::from(!native_exit));
        assert_eq!(manager.stats().objects, usize::from(!native_exit));
        manager.process_exited(peer(1));
        manager.process_exited(peer(2));
        assert_eq!(manager.stats().processes, 0);
        assert_eq!(manager.stats().objects, 0);
        assert_eq!(manager.stats().transactions, 0);
        assert_eq!(manager.stats().clients, 0);
    }
}

#[test]
fn death_before_candidate_hello_revokes_reserved_replacement() {
    let mut manager = manager();
    let owner = worker(&mut manager, 1);
    let transaction = prepare(&mut manager, owner, 1, 2);
    manager.process_exited(peer(2));
    assert_error(
        &mut manager,
        owner,
        Request::CommitExec { transaction },
        ErrorCode::Aborted,
    );
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker), peer(2))
            .unwrap_err()
            .code,
        ErrorCode::Aborted
    );
    assert_eq!(manager.stats().processes, 1);
}

#[test]
fn target_birth_is_exact_and_target_reservations_cannot_be_stolen_or_replayed() {
    let mut manager = manager();
    let owner = worker(&mut manager, 1);
    let stranger = worker(&mut manager, 3);
    let controller = manager
        .connect(hello(ClientRole::Controller), peer(4))
        .unwrap()
        .0;
    for target in [1, 3, 4] {
        assert_error(
            &mut manager,
            owner,
            exec_request(1, target),
            ErrorCode::Conflict,
        );
    }
    let transaction = prepare(&mut manager, owner, 1, 2);
    assert_error(&mut manager, owner, exec_request(1, 5), ErrorCode::Conflict);
    assert_error(&mut manager, owner, exec_request(2, 5), ErrorCode::Conflict);
    assert_error(
        &mut manager,
        stranger,
        exec_request(1, 2),
        ErrorCode::Conflict,
    );
    for request in [
        Request::AwaitExecReady { transaction },
        Request::CommitExec { transaction },
        Request::AbortExec { transaction },
    ] {
        assert_error(
            &mut manager,
            stranger,
            request.clone(),
            ErrorCode::Unauthorized,
        );
        assert_error(&mut manager, controller, request, ErrorCode::Unauthorized);
    }
    let different_birth = PeerIdentity {
        host_pid: 2,
        birth: peer(2).birth + 1,
    };
    let wrong = manager
        .connect(hello(ClientRole::Worker), different_birth)
        .unwrap()
        .0;
    assert_ne!(
        identity(&mut manager, wrong).pid,
        identity(&mut manager, owner).pid
    );
    let candidate = worker(&mut manager, 2);
    assert_eq!(
        identity(&mut manager, candidate),
        identity(&mut manager, owner)
    );
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker), peer(2))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    ready(&mut manager, candidate);
    commit(&mut manager, owner, transaction);
    manager.process_exited(peer(1));
    assert_error(
        &mut manager,
        candidate,
        exec_request(1, 2),
        ErrorCode::Conflict,
    );
    manager.disconnect(candidate);
    assert_eq!(
        manager
            .connect(hello(ClientRole::Worker), peer(2))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}

#[test]
fn candidate_and_retired_clients_have_no_controller_authority() {
    let mut manager = manager();
    let owner = worker(&mut manager, 1);
    let transaction = prepare(&mut manager, owner, 1, 2);
    assert_eq!(
        manager
            .connect(hello(ClientRole::Controller), peer(2))
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let candidate = worker(&mut manager, 2);
    for request in [Request::Stats, Request::Shutdown] {
        assert_error(&mut manager, candidate, request, ErrorCode::Unauthorized);
    }
    assert_error(
        &mut manager,
        candidate,
        Request::AwaitExecReady { transaction },
        ErrorCode::NotReady,
    );
    assert_error(
        &mut manager,
        candidate,
        Request::PrepareFork { request_key: 1 },
        ErrorCode::NotReady,
    );
    ready(&mut manager, candidate);
    commit(&mut manager, owner, transaction);
    for request in [Request::Stats, Request::Shutdown] {
        assert_error(&mut manager, owner, request, ErrorCode::Unauthorized);
    }
    assert_eq!(
        manager
            .connect(hello(ClientRole::Controller), peer(1))
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        manager
            .connect(hello(ClientRole::Controller), peer(2))
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    assert!(!manager.is_shutdown_requested());
}

#[test]
fn exec_shares_transaction_and_request_key_quotas_without_allocating_a_process() {
    let mut manager = StateManager::with_limits(
        71,
        SECRET.into(),
        Limits {
            max_processes: 1,
            max_request_keys_per_process: 1,
            ..Limits::default()
        },
    );
    let owner = worker(&mut manager, 1);
    let before = manager.stats();
    let transaction = prepare(&mut manager, owner, 1, 2);
    assert_eq!(manager.stats().processes, before.processes);
    assert_eq!(manager.stats().objects, before.objects);
    assert_eq!(manager.stats().transactions, before.transactions + 1);
    let candidate = worker(&mut manager, 2);
    ready(&mut manager, candidate);
    commit(&mut manager, owner, transaction);
    let before = manager.stats();
    manager.process_exited(peer(1));
    let before = kinakaze_v2_protocol::Stats {
        clients: before.clients - 1,
        ..before
    };
    assert_error(
        &mut manager,
        candidate,
        exec_request(2, 3),
        ErrorCode::LimitExceeded,
    );
    assert_error(
        &mut manager,
        candidate,
        Request::PrepareFork { request_key: 1 },
        ErrorCode::LimitExceeded,
    );
    assert_eq!(manager.stats(), before);

    let mut manager = StateManager::with_limits(
        71,
        SECRET.into(),
        Limits {
            max_transactions: 1,
            ..Limits::default()
        },
    );
    let owner = worker(&mut manager, 1);
    let transaction = prepare(&mut manager, owner, 1, 2);
    manager
        .handle(owner, Request::AbortExec { transaction })
        .unwrap();
    assert_error(
        &mut manager,
        owner,
        exec_request(2, 3),
        ErrorCode::LimitExceeded,
    );
    assert_error(
        &mut manager,
        owner,
        Request::PrepareFork { request_key: 1 },
        ErrorCode::LimitExceeded,
    );
}

#[test]
fn invalid_exec_target_does_not_consume_request_keys_or_transactions() {
    let mut manager = manager();
    let owner = worker(&mut manager, 1);
    let before = manager.stats();
    for (replacement_host_pid, replacement_birth) in [(0, 1), (1, 0)] {
        assert_error(
            &mut manager,
            owner,
            Request::PrepareExec {
                request_key: 1,
                replacement_host_pid,
                replacement_birth,
            },
            ErrorCode::InvalidRequest,
        );
    }
    assert_eq!(manager.stats(), before);
    assert_eq!(prepare(&mut manager, owner, 1, 2), 1);
}

#[test]
fn clone_parent_restrictions_and_fork_readiness_use_creator_authority() {
    let mut manager = manager();
    let root = worker(&mut manager, 1);
    assert_error(
        &mut manager,
        root,
        Request::PrepareForkWithParent {
            request_key: 1,
            parent_pid: 0,
        },
        ErrorCode::InvalidRequest,
    );
    let (owner, _) = fork(&mut manager, root, 1, 2);
    let parent_pid = identity(&mut manager, root).pid;
    assert_error(
        &mut manager,
        owner,
        Request::PrepareForkWithParent {
            request_key: 1,
            parent_pid: 999,
        },
        ErrorCode::Unauthorized,
    );
    let request = Request::PrepareForkWithParent {
        request_key: 1,
        parent_pid,
    };
    let ticket = match manager.handle(owner, request.clone()).unwrap() {
        Reply::ForkPrepared(ticket) => ticket,
        other => panic!("unexpected reply: {other:?}"),
    };
    assert_eq!(ticket.child.parent_pid, parent_pid);
    assert_eq!(
        manager.handle(owner, request).unwrap(),
        Reply::ForkPrepared(ticket.clone())
    );
    assert_error(
        &mut manager,
        owner,
        Request::PrepareFork { request_key: 1 },
        ErrorCode::Conflict,
    );
    assert_error(
        &mut manager,
        owner,
        Request::AwaitForkReady {
            transaction: ticket.transaction,
        },
        ErrorCode::NotReady,
    );
    assert_error(
        &mut manager,
        root,
        Request::AwaitForkReady {
            transaction: ticket.transaction,
        },
        ErrorCode::Unauthorized,
    );
    let mut child_hello = hello(ClientRole::Worker);
    child_hello.adoption_ticket = Some(ticket.token);
    let child = manager.connect(child_hello, peer(3)).unwrap().0;
    assert_error(
        &mut manager,
        child,
        Request::AwaitForkReady {
            transaction: ticket.transaction,
        },
        ErrorCode::NotReady,
    );
    ready(&mut manager, child);
    assert_eq!(
        manager
            .handle(
                owner,
                Request::AwaitForkReady {
                    transaction: ticket.transaction
                }
            )
            .unwrap(),
        Reply::Ok
    );
    manager
        .handle(
            owner,
            Request::CommitFork {
                transaction: ticket.transaction,
            },
        )
        .unwrap();
    assert_eq!(
        manager
            .handle(
                owner,
                Request::AwaitForkReady {
                    transaction: ticket.transaction
                }
            )
            .unwrap(),
        Reply::Ok
    );
    assert_error(
        &mut manager,
        child,
        Request::AwaitForkReady {
            transaction: ticket.transaction,
        },
        ErrorCode::Unauthorized,
    );
    assert_eq!(identity(&mut manager, child).parent_pid, parent_pid);
    let ticket = match manager
        .handle(owner, Request::PrepareFork { request_key: 2 })
        .unwrap()
    {
        Reply::ForkPrepared(ticket) => ticket,
        other => panic!("unexpected reply: {other:?}"),
    };
    manager
        .handle(
            owner,
            Request::AbortFork {
                transaction: ticket.transaction,
            },
        )
        .unwrap();
    assert_error(
        &mut manager,
        owner,
        Request::AwaitForkReady {
            transaction: ticket.transaction,
        },
        ErrorCode::Aborted,
    );
}

#[test]
fn clone_parent_reserved_child_reparents_when_its_logical_parent_dies() {
    let mut manager = manager();
    let root = worker(&mut manager, 1);
    let (creator, _) = fork(&mut manager, root, 1, 2);
    let ticket = match manager
        .handle(
            creator,
            Request::PrepareForkWithParent {
                request_key: 1,
                parent_pid: 1,
            },
        )
        .unwrap()
    {
        Reply::ForkPrepared(ticket) => ticket,
        other => panic!("unexpected reply: {other:?}"),
    };
    manager.process_exited(peer(1));
    assert_eq!(identity(&mut manager, creator).parent_pid, 0);
    assert_error(
        &mut manager,
        creator,
        Request::PrepareForkWithParent {
            request_key: 1,
            parent_pid: 0,
        },
        ErrorCode::Conflict,
    );
    let mut child_hello = hello(ClientRole::Worker);
    child_hello.adoption_ticket = Some(ticket.token);
    let child = manager.connect(child_hello, peer(3)).unwrap().0;
    assert_eq!(identity(&mut manager, child).parent_pid, 0);
    ready(&mut manager, child);
    manager
        .handle(
            creator,
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
fn retired_native_references_remain_bounded_after_logical_process_exit() {
    let mut manager = StateManager::with_limits(
        71,
        SECRET.into(),
        Limits {
            max_transactions: 1,
            ..Limits::default()
        },
    );
    let old = worker(&mut manager, 1);
    let transaction = prepare(&mut manager, old, 1, 2);
    let replacement = worker(&mut manager, 2);
    ready(&mut manager, replacement);
    commit(&mut manager, old, transaction);
    manager.disconnect(old);
    manager.process_exited(peer(2));
    assert_eq!(manager.stats().transactions, 1);
    let next = worker(&mut manager, 3);
    assert_error(
        &mut manager,
        next,
        exec_request(1, 4),
        ErrorCode::LimitExceeded,
    );
    manager.process_exited(PeerIdentity {
        host_pid: 1,
        birth: peer(1).birth + 1,
    });
    assert_error(
        &mut manager,
        next,
        exec_request(1, 4),
        ErrorCode::LimitExceeded,
    );
    manager.process_exited(peer(1));
    prepare(&mut manager, next, 1, 4);
}

#[test]
fn fork_adoption_cannot_consume_reserved_exec_target() {
    let mut manager = manager();
    let owner = worker(&mut manager, 1);
    let ticket = match manager
        .handle(owner, Request::PrepareFork { request_key: 1 })
        .unwrap()
    {
        Reply::ForkPrepared(ticket) => ticket,
        other => panic!("unexpected reply: {other:?}"),
    };
    prepare(&mut manager, owner, 1, 2);
    let mut child_hello = hello(ClientRole::Worker);
    child_hello.adoption_ticket = Some(ticket.token);
    assert_eq!(
        manager
            .connect(child_hello.clone(), peer(2))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let child = manager.connect(child_hello, peer(3)).unwrap().0;
    assert_eq!(identity(&mut manager, child).pid, ticket.child.pid);
    let candidate = worker(&mut manager, 2);
    assert_eq!(
        identity(&mut manager, candidate),
        identity(&mut manager, owner)
    );
}

#[test]
fn committed_candidate_cannot_execute_until_exact_old_native_owner_dies() {
    let mut manager = manager();
    let old = worker(&mut manager, 1);
    let transaction = prepare(&mut manager, old, 1, 2);
    let replacement = worker(&mut manager, 2);
    ready(&mut manager, replacement);
    commit(&mut manager, old, transaction);
    ready(&mut manager, replacement); // must not regress Committed to Ready
    commit(&mut manager, old, transaction);
    assert_error(
        &mut manager,
        old,
        Request::AbortExec { transaction },
        ErrorCode::Conflict,
    );
    assert_error(
        &mut manager,
        replacement,
        Request::AwaitActivation,
        ErrorCode::NotReady,
    );
    manager.disconnect(old);
    assert_error(
        &mut manager,
        replacement,
        Request::AwaitActivation,
        ErrorCode::NotReady,
    );
    manager.process_exited(PeerIdentity {
        host_pid: 1,
        birth: peer(1).birth + 1,
    });
    assert_error(
        &mut manager,
        replacement,
        Request::AwaitActivation,
        ErrorCode::NotReady,
    );
    manager.process_exited(peer(1));
    assert_eq!(
        manager
            .handle(replacement, Request::AwaitActivation)
            .unwrap(),
        Reply::Ok
    );
}

#[test]
fn process_memory_resolution_uses_active_logical_descendants_and_native_birth() {
    let mut manager = manager();
    let parent = worker(&mut manager, 10);
    let outsider = worker(&mut manager, 99);
    let parent_pid = identity(&mut manager, parent).pid;
    let (child, child_ticket) = fork(&mut manager, parent, 1, 11);
    let (grandchild, grandchild_ticket) = fork(&mut manager, child, 1, 12);
    for write in [false, true] {
        for (pid, native) in [
            (parent_pid, 10),
            (child_ticket.child.pid, 11),
            (grandchild_ticket.child.pid, 12),
        ] {
            assert_eq!(
                manager
                    .handle(parent, Request::ProcessMemoryTarget { pid, write })
                    .unwrap(),
                Reply::ProcessMemoryTarget {
                    host_pid: native,
                    birth: peer(native).birth
                }
            );
        }
        assert_error(
            &mut manager,
            child,
            Request::ProcessMemoryTarget {
                pid: parent_pid,
                write,
            },
            ErrorCode::Unauthorized,
        );
        assert_error(
            &mut manager,
            outsider,
            Request::ProcessMemoryTarget {
                pid: child_ticket.child.pid,
                write,
            },
            ErrorCode::Unauthorized,
        );
    }
    assert_error(
        &mut manager,
        parent,
        Request::ProcessMemoryTarget {
            pid: 11,
            write: false,
        },
        ErrorCode::NotFound,
    );
    let tx = prepare(&mut manager, child, 2, 13);
    let candidate = worker(&mut manager, 13);
    assert_error(
        &mut manager,
        candidate,
        Request::ProcessMemoryTarget {
            pid: grandchild_ticket.child.pid,
            write: false,
        },
        ErrorCode::NotReady,
    );
    ready(&mut manager, candidate);
    commit(&mut manager, child, tx);
    assert_error(
        &mut manager,
        parent,
        Request::ProcessMemoryTarget {
            pid: child_ticket.child.pid,
            write: false,
        },
        ErrorCode::NotFound,
    );
    manager.process_exited(peer(11));
    assert_eq!(
        manager
            .handle(
                parent,
                Request::ProcessMemoryTarget {
                    pid: child_ticket.child.pid,
                    write: true
                }
            )
            .unwrap(),
        Reply::ProcessMemoryTarget {
            host_pid: 13,
            birth: peer(13).birth
        }
    );
    manager.process_exited(peer(13));
    assert_error(
        &mut manager,
        parent,
        Request::ProcessMemoryTarget {
            pid: grandchild_ticket.child.pid,
            write: false,
        },
        ErrorCode::Unauthorized,
    );
    assert_eq!(identity(&mut manager, grandchild).parent_pid, 0);
}
