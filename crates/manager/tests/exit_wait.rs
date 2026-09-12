use kinakaze_v2_manager::{ClientId, Limits, PeerIdentity, StateManager};
use kinakaze_v2_protocol::{ClientRole, ErrorCode, Hello, PROTOCOL_VERSION, Reply, Request};

const SECRET: &str = "exit-wait-test-secret";

fn peer(host_pid: u32) -> PeerIdentity {
    PeerIdentity {
        host_pid,
        birth: 10_000 + u64::from(host_pid),
    }
}

fn connect(manager: &mut StateManager, role: ClientRole, peer: PeerIdentity) -> ClientId {
    manager
        .connect(
            Hello {
                version: PROTOCOL_VERSION,
                token: SECRET.into(),
                role,
                adoption_ticket: None,
            },
            peer,
        )
        .unwrap()
        .0
}

fn setup(limits: Limits) -> (StateManager, ClientId) {
    let mut manager = StateManager::with_limits(18, SECRET.into(), limits);
    let controller = connect(&mut manager, ClientRole::Controller, peer(100));
    (manager, controller)
}

fn worker(manager: &mut StateManager, native: PeerIdentity) -> (ClientId, u32) {
    let client = connect(manager, ClientRole::Worker, native);
    let pid = match manager.handle(client, Request::Identity).unwrap() {
        Reply::Identity(identity) => identity.pid,
        other => panic!("unexpected identity: {other:?}"),
    };
    (client, pid)
}

fn wait_error(manager: &mut StateManager, client: ClientId, pid: u32, code: ErrorCode) {
    assert_eq!(
        manager
            .handle(client, Request::AwaitExit { pid })
            .unwrap_err()
            .code,
        code
    );
}

fn exited(manager: &mut StateManager, controller: ClientId, pid: u32, status: i32) {
    assert_eq!(
        manager
            .handle(controller, Request::AwaitExit { pid })
            .unwrap(),
        Reply::Exit { status }
    );
}

fn prepare_exec(manager: &mut StateManager, owner: ClientId, target: PeerIdentity) -> u64 {
    match manager
        .handle(
            owner,
            Request::PrepareExec {
                request_key: 1,
                replacement_host_pid: target.host_pid,
                replacement_birth: target.birth,
            },
        )
        .unwrap()
    {
        Reply::ExecPrepared { transaction } => transaction,
        other => panic!("unexpected exec reply: {other:?}"),
    }
}

#[test]
fn pipe_eof_and_wrong_birth_cannot_complete_exit_wait_or_change_final_status() {
    let (mut manager, controller) = setup(Limits::default());
    let (owner, pid) = worker(&mut manager, peer(1));
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    manager.disconnect(owner);
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    manager.process_exited_with_status(
        PeerIdentity {
            host_pid: 1,
            birth: peer(1).birth + 1,
        },
        19,
    );
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    let exception_status = 0xc000_0005_u32 as i32;
    manager.process_exited_with_status(peer(1), exception_status);
    exited(&mut manager, controller, pid, exception_status);
    assert_eq!(manager.stats().processes, 0);
    manager.process_exited_with_status(peer(1), 0);
    exited(&mut manager, controller, pid, exception_status);
}

#[test]
fn exit_wait_follows_exec_replacement_until_final_native_owner_dies() {
    let (mut manager, controller) = setup(Limits::default());
    let (old, pid) = worker(&mut manager, peer(1));
    let transaction = prepare_exec(&mut manager, old, peer(2));
    let (replacement, replacement_pid) = worker(&mut manager, peer(2));
    assert_eq!(pid, replacement_pid);
    manager.handle(replacement, Request::MarkReady).unwrap();
    manager
        .handle(old, Request::CommitExec { transaction })
        .unwrap();
    wait_error(&mut manager, old, pid, ErrorCode::Unauthorized);
    manager.process_exited_with_status(peer(1), 99);
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    assert_eq!(manager.stats().processes, 1);
    manager.disconnect(replacement);
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    manager.process_exited_with_status(peer(2), 7);
    exited(&mut manager, controller, pid, 7);
    manager.process_exited_with_status(peer(1), 88);
    exited(&mut manager, controller, pid, 7);
    assert_eq!(manager.stats().processes, 0);
    assert_eq!(manager.stats().transactions, 0);
}

#[test]
fn failed_committed_candidate_waits_for_old_thread_group_death_and_preserves_its_status() {
    let (mut manager, controller) = setup(Limits::default());
    let (old, pid) = worker(&mut manager, peer(1));
    let transaction = prepare_exec(&mut manager, old, peer(2));
    let (candidate, _) = worker(&mut manager, peer(2));
    manager.handle(candidate, Request::MarkReady).unwrap();
    manager
        .handle(old, Request::CommitExec { transaction })
        .unwrap();
    manager.process_exited_with_status(peer(2), 127);
    manager.process_exited_with_status(peer(2), 99); // duplicate cannot overwrite the native outcome
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    manager.disconnect(old);
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    manager.process_exited_with_status(peer(1), 0);
    exited(&mut manager, controller, pid, 127);
    assert_eq!(manager.stats().processes, 0);
    assert_eq!(manager.stats().transactions, 0);
}

#[test]
fn uncommitted_candidate_death_does_not_complete_the_original_process() {
    let (mut manager, controller) = setup(Limits::default());
    let (owner, pid) = worker(&mut manager, peer(1));
    prepare_exec(&mut manager, owner, peer(2));
    let (candidate, _) = worker(&mut manager, peer(2));
    wait_error(&mut manager, candidate, pid, ErrorCode::Unauthorized);
    manager.handle(candidate, Request::MarkReady).unwrap();
    manager.process_exited_with_status(peer(2), 45);
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    manager.process_exited_with_status(peer(1), 11);
    exited(&mut manager, controller, pid, 11);
}

#[test]
fn original_owner_death_before_exec_commit_is_final_and_candidate_cannot_overwrite_it() {
    let (mut manager, controller) = setup(Limits::default());
    let (owner, pid) = worker(&mut manager, peer(1));
    prepare_exec(&mut manager, owner, peer(2));
    worker(&mut manager, peer(2));
    manager.process_exited_with_status(peer(1), 12);
    exited(&mut manager, controller, pid, 12);
    manager.process_exited_with_status(peer(2), 90);
    exited(&mut manager, controller, pid, 12);
}

#[test]
fn sibling_statuses_are_independent_and_controller_reads_do_not_reap_them() {
    let (mut manager, controller) = setup(Limits::default());
    let (_, first) = worker(&mut manager, peer(1));
    let (_, second) = worker(&mut manager, peer(2));
    manager.process_exited_with_status(peer(2), 37);
    exited(&mut manager, controller, second, 37);
    wait_error(&mut manager, controller, first, ErrorCode::NotReady);
    manager.process_exited_with_status(peer(1), -1);
    for _ in 0..2 {
        exited(&mut manager, controller, first, -1);
        exited(&mut manager, controller, second, 37);
    }
}

#[test]
fn bounded_exit_retention_evicts_by_completion_order_and_ignores_duplicate_notices() {
    let (mut manager, controller) = setup(Limits {
        max_exit_records: 2,
        ..Limits::default()
    });
    let (_, first) = worker(&mut manager, peer(1));
    let (_, second) = worker(&mut manager, peer(2));
    let (_, third) = worker(&mut manager, peer(3));
    manager.process_exited_with_status(peer(3), 30);
    manager.process_exited_with_status(peer(1), 10);
    manager.process_exited_with_status(peer(3), 31);
    manager.process_exited_with_status(peer(99), 99);
    exited(&mut manager, controller, third, 30);
    exited(&mut manager, controller, first, 10);
    manager.process_exited_with_status(peer(2), 20);
    wait_error(&mut manager, controller, third, ErrorCode::NotFound);
    exited(&mut manager, controller, first, 10);
    exited(&mut manager, controller, second, 20);
    manager.process_exited_with_status(peer(3), 32);
    wait_error(&mut manager, controller, third, ErrorCode::NotFound);
    exited(&mut manager, controller, first, 10);
}

#[test]
fn zero_retention_reclaims_processes_without_retaining_exit_records() {
    let (mut manager, controller) = setup(Limits {
        max_exit_records: 0,
        ..Limits::default()
    });
    let (_, pid) = worker(&mut manager, peer(1));
    wait_error(&mut manager, controller, pid, ErrorCode::NotReady);
    manager.process_exited_with_status(peer(1), 0);
    wait_error(&mut manager, controller, pid, ErrorCode::NotFound);
    assert_eq!(manager.stats().processes, 0);
}

#[test]
fn native_pid_reuse_and_delayed_old_birth_notification_do_not_complete_new_process() {
    let (mut manager, controller) = setup(Limits::default());
    let (_, first) = worker(&mut manager, peer(1));
    manager.process_exited_with_status(peer(1), 17);
    let reused = PeerIdentity {
        host_pid: 1,
        birth: peer(1).birth + 10,
    };
    let (_, second) = worker(&mut manager, reused);
    assert_ne!(first, second);
    manager.process_exited_with_status(peer(1), 18);
    exited(&mut manager, controller, first, 17);
    wait_error(&mut manager, controller, second, ErrorCode::NotReady);
    manager.process_exited_with_status(reused, 21);
    exited(&mut manager, controller, first, 17);
    exited(&mut manager, controller, second, 21);
}

#[test]
fn exit_wait_validates_pid_and_requires_controller_authority() {
    let (mut manager, controller) = setup(Limits::default());
    let (owner, pid) = worker(&mut manager, peer(1));
    for invalid in [0, i32::MAX as u32 + 1, u32::MAX] {
        wait_error(&mut manager, controller, invalid, ErrorCode::InvalidRequest);
    }
    for absent in [2, i32::MAX as u32] {
        wait_error(&mut manager, controller, absent, ErrorCode::NotFound);
    }
    wait_error(&mut manager, owner, pid, ErrorCode::Unauthorized);
    wait_error(&mut manager, owner, 999, ErrorCode::Unauthorized);
    manager.process_exited(peer(1));
    exited(&mut manager, controller, pid, 127);
}
