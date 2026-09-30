use super::*;
use kinakaze_v2_manager::StateManager;
use kinakaze_v2_protocol::{ClientRole, Hello, PROTOCOL_VERSION, Reply, Request};
use std::{
    os::windows::ffi::OsStrExt,
    process::{Command, Stdio},
};

fn fixture() -> Standby {
    let name = format!(
        "Local\\kinakaze.native-exec-test.{}",
        kinakaze_v2_host_win::random_token().unwrap()
    );
    let wide: Vec<u16> = std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let ready = own(unsafe { CreateEventW(ptr::null(), 1, 0, wide.as_ptr()) }).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "native_fork::tests::parked_fixture",
            "--nocapture",
        ])
        .env("KINAKAZE_NATIVE_FORK_TEST_EVENT", name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let native = ProcessHandle::open(child.id()).unwrap();
    let event = || own(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) }).unwrap();
    let slot = Standby {
        process: child.into(),
        ready,
        activate: event(),
        control: event(),
        peer: PeerIdentity {
            host_pid: native.pid(),
            birth: native.birth(),
        },
        since: Instant::now(),
        kill: true,
    };
    assert_eq!(
        unsafe { WaitForSingleObject(slot.ready.as_raw_handle(), 10_000) },
        WAIT_OBJECT_0
    );
    slot
}
fn hello() -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        token: "test".into(),
        role: ClientRole::Worker,
        adoption_ticket: None,
    }
}

#[test]
fn exec_candidates_die_on_abort_parent_death_and_abandonment_but_survive_commit() {
    for action in 0..4 {
        let parent = fixture();
        let child = fixture();
        let child_peer = child.peer;
        let child_handle = child.process.try_clone().unwrap();
        let mut manager = StateManager::new(1, "test".into());
        let owner = manager.connect(hello(), parent.peer).unwrap().0;
        let mut state = State::default();
        state.leases.push(Lease {
            parent: ProcessHandle::open(parent.peer.host_pid).unwrap(),
            child,
            delivered: Instant::now(),
        });
        state.reap(&manager);
        assert_eq!(state.leases.len(), 1);
        if action == 3 {
            state.leases[0].delivered = Instant::now()
                .checked_sub(TIMEOUT + Duration::from_secs(1))
                .unwrap();
        } else {
            let Reply::ExecPrepared { transaction } = manager
                .handle(
                    owner,
                    Request::PrepareExec {
                        request_key: 1,
                        replacement_host_pid: child_peer.host_pid,
                        replacement_birth: child_peer.birth,
                    },
                )
                .unwrap()
            else {
                panic!()
            };
            match action {
                0 => {
                    manager
                        .handle(owner, Request::AbortExec { transaction })
                        .unwrap();
                }
                1 => {
                    let native = ProcessHandle::open(parent.peer.host_pid).unwrap();
                    native.terminate(125).unwrap();
                    native.wait().unwrap();
                }
                2 => {
                    let candidate = manager.connect(hello(), child_peer).unwrap().0;
                    manager.handle(candidate, Request::MarkReady).unwrap();
                    manager
                        .handle(owner, Request::CommitExec { transaction })
                        .unwrap();
                    let native = ProcessHandle::open(parent.peer.host_pid).unwrap();
                    native.terminate(0).unwrap();
                    native.wait().unwrap();
                }
                _ => unreachable!(),
            }
        }
        state.reap(&manager);
        assert!(state.leases.is_empty());
        if action == 2 {
            assert_eq!(
                unsafe { WaitForSingleObject(child_handle.as_raw_handle(), 0) },
                WAIT_TIMEOUT
            );
            unsafe { TerminateProcess(child_handle.as_raw_handle(), 0) };
        }
        assert_eq!(
            unsafe { WaitForSingleObject(child_handle.as_raw_handle(), 2000) },
            WAIT_OBJECT_0
        );
    }
}

#[test]
fn failed_reply_revokes_only_the_exact_recipient_and_candidate() {
    let parent = fixture();
    let child = fixture();
    let child_peer = child.peer;
    let child_handle = child.process.try_clone().unwrap();
    let pool = Pool::new(PathBuf::new(), PathBuf::new());
    pool.state.lock().unwrap().leases.push(Lease {
        parent: ProcessHandle::open(parent.peer.host_pid).unwrap(),
        child,
        delivered: Instant::now(),
    });
    pool.cancel_delivery(
        PeerIdentity {
            birth: parent.peer.birth + 1,
            ..parent.peer
        },
        child_peer.host_pid,
    );
    pool.cancel_delivery(parent.peer, child_peer.host_pid + 1);
    assert_eq!(pool.state.lock().unwrap().leases.len(), 1);
    pool.cancel_delivery(parent.peer, child_peer.host_pid);
    assert!(pool.state.lock().unwrap().leases.is_empty());
    assert_eq!(
        unsafe { WaitForSingleObject(child_handle.as_raw_handle(), 2000) },
        WAIT_OBJECT_0
    );
}
