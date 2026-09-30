use super::*;
use kinakaze_v2_manager::StateManager;
use kinakaze_v2_protocol::{ClientRole, Hello, PROTOCOL_VERSION, Reply, Request};
use std::process::{Command, Stdio};

#[test]
fn parked_fixture() {
    let Some(name) = std::env::var_os("KINAKAZE_NATIVE_FORK_TEST_EVENT") else {
        return;
    };
    let name: Vec<u16> = name.encode_wide().chain(Some(0)).collect();
    let ready = own(unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) }).unwrap();
    assert_ne!(unsafe { SetEvent(ready.as_raw_handle()) }, 0);
    let wait = own(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) }).unwrap();
    unsafe {
        WaitForSingleObject(wait.as_raw_handle(), 30_000);
    }
}

fn fixture() -> Standby {
    let name = format!(
        "Local\\kinakaze.native-fork-test.{}",
        kinakaze_v2_host_win::random_token().unwrap()
    );
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let parked = own(unsafe { CreateEventW(ptr::null(), 1, 0, wide.as_ptr()) }).unwrap();
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
        // Guard tests never install a thread context; this unused field owns a
        // separate kernel capability so all four owners still exercise Drop.
        thread: event(),
        ready: event(),
        activate: event(),
        parked,
        peer: PeerIdentity {
            host_pid: native.pid(),
            birth: native.birth(),
        },
        tid: 0,
        since: Instant::now(),
        kill: true,
    };
    assert_eq!(
        unsafe { WaitForSingleObject(slot.parked.as_raw_handle(), 10_000) },
        WAIT_OBJECT_0
    );
    slot
}

fn hello(ticket: Option<String>) -> Hello {
    Hello {
        version: PROTOCOL_VERSION,
        token: "test".into(),
        role: ClientRole::Worker,
        adoption_ticket: ticket,
    }
}

#[test]
fn native_candidates_are_killed_on_abort_or_parent_death_but_survive_commit() {
    for action in [0, 1, 2] {
        let parent = fixture();
        let child = fixture();
        let native_child = ProcessHandle::open(child.peer.host_pid).unwrap();
        let mut manager = StateManager::new(1, "test".into());
        let client = manager.connect(hello(None), parent.peer).unwrap().0;
        let Reply::ForkPrepared(ticket) = manager
            .handle(client, Request::PrepareFork { request_key: 1 })
            .unwrap()
        else {
            panic!()
        };
        let mut state = State::default();
        let child_peer = child.peer;
        state.leases.push(Lease {
            transaction: ticket.transaction,
            parent: ProcessHandle::open(parent.peer.host_pid).unwrap(),
            child,
        });
        state.reap_leases(&manager);
        assert_eq!(state.leases.len(), 1);
        assert!(!native_child.has_exited().unwrap());
        match action {
            0 => {
                manager
                    .handle(
                        client,
                        Request::AbortFork {
                            transaction: ticket.transaction,
                        },
                    )
                    .unwrap();
            }
            1 => {
                // Native death must revoke even before the manager's exit watcher
                // has consumed its notification.
                let native_parent = ProcessHandle::open(parent.peer.host_pid).unwrap();
                native_parent.terminate(125).unwrap();
                native_parent.wait().unwrap();
            }
            2 => {
                let child = manager
                    .connect(hello(Some(ticket.token)), child_peer)
                    .unwrap()
                    .0;
                manager.handle(child, Request::MarkReady).unwrap();
                manager
                    .handle(
                        client,
                        Request::CommitFork {
                            transaction: ticket.transaction,
                        },
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        state.reap_leases(&manager);
        assert!(state.leases.is_empty());
        if action == 2 {
            assert!(!native_child.has_exited().unwrap());
            native_child.terminate(125).unwrap();
        }
        native_child.wait().unwrap();
    }
}

#[test]
fn failed_delivery_cancels_only_its_exact_parent_and_transaction() {
    let parent = fixture();
    let child = fixture();
    let native_child = ProcessHandle::open(child.peer.host_pid).unwrap();
    let pool = Pool {
        enabled: true,
        root: PathBuf::new(),
        dist: PathBuf::new(),
        state: Mutex::new(State::default()),
        creating: Mutex::new(()),
    };
    pool.state.lock().unwrap().leases.push(Lease {
        transaction: 7,
        parent: ProcessHandle::open(parent.peer.host_pid).unwrap(),
        child,
    });
    pool.cancel_delivery(
        PeerIdentity {
            birth: parent.peer.birth + 1,
            ..parent.peer
        },
        7,
    );
    pool.cancel_delivery(parent.peer, 8);
    assert_eq!(pool.state.lock().unwrap().leases.len(), 1);
    pool.cancel_delivery(parent.peer, 7);
    assert!(pool.state.lock().unwrap().leases.is_empty());
    native_child.wait().unwrap();
}

fn empty_template() -> Arc<Template> {
    Arc::new(Template {
        spec: Spec {
            tls_slots: 0,
            modules: Vec::new(),
        },
        _pins: Vec::new(),
    })
}

#[test]
fn an_unpublished_candidate_is_killed_after_stop_or_template_replacement() {
    for stop in [false, true] {
        let expected = empty_template();
        let candidate = fixture();
        let native = ProcessHandle::open(candidate.peer.host_pid).unwrap();
        let mut state = State {
            template: Some(if stop {
                Arc::clone(&expected)
            } else {
                empty_template()
            }),
            ..State::default()
        };
        state.publish(&expected, stop, Ok(candidate));
        assert!(state.slots.is_empty());
        native.wait().unwrap();
        assert!(native.has_exited().unwrap());
    }
}

#[test]
fn publishing_requires_the_same_template_generation_even_when_specs_match() {
    let expected = empty_template();
    let mut state = State {
        template: Some(Arc::clone(&expected)),
        ..State::default()
    };
    let candidate = fixture();
    let native = ProcessHandle::open(candidate.peer.host_pid).unwrap();
    state.publish(&expected, false, Ok(candidate));
    assert_eq!(state.slots.len(), 1);
    assert!(!native.has_exited().unwrap());
    state.slots.clear();
    native.wait().unwrap();
    state.template = Some(empty_template());
    state.publish(
        &expected,
        false,
        Err(io::Error::other("obsolete creation failure")),
    );
    assert_eq!(state.failures, 0);
    let current = Arc::clone(state.template.as_ref().unwrap());
    state.publish(
        &current,
        false,
        Err(io::Error::other("current creation failure")),
    );
    assert_eq!(state.failures, 1);
}

#[test]
fn creation_gate_allows_pool_access_but_blocks_shutdown_until_job_assignment() {
    use std::sync::mpsc;
    let pool = Arc::new(Pool::new(PathBuf::new(), PathBuf::new()));
    let stopping = Arc::new(AtomicBool::new(false));
    let (entered, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let creating = Arc::clone(&pool);
    let stopped = Arc::clone(&stopping);
    let thread = std::thread::spawn(move || {
        let result = creating.prepare(&stopped, || {
            entered.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            Err(io::Error::other("controlled creation failure"))
        });
        assert!(result.unwrap().is_err());
    });
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(pool.state.try_lock().is_ok());
    assert!(matches!(
        pool.creating.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    release.send(()).unwrap();
    {
        let _creation = pool.lock_creation();
        stopping.store(true, Ordering::Release);
    }
    thread.join().unwrap();
    assert!(
        pool.prepare(&stopping, || panic!("created after stop"))
            .is_none()
    );
}
