use super::*;

// Own every fd even on assertion failure; these tests share the native process
// with other VFS tests and must not leave named endpoints behind.
struct Fds(Vec<i32>);
impl Fds {
    fn close(&mut self, fd: i32) {
        crate::close(fd).unwrap();
        self.0.retain(|&owned| owned != fd);
    }
    fn socket(&mut self, flags: i32) -> i32 {
        let fd = socket(SOCK_STREAM | flags, 0).unwrap();
        self.0.push(fd);
        fd
    }
    fn accept(&mut self, listener: i32) -> i32 {
        let fd = unsafe {
            accept(
                listener,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                SOCK_NONBLOCK,
            )
        }
        .unwrap();
        self.0.push(fd);
        fd
    }
}
impl Drop for Fds {
    fn drop(&mut self) {
        for fd in self.0.drain(..).rev() {
            let _ = crate::close(fd);
        }
    }
}

fn address() -> Vec<u8> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("connect-{}-{nonce}", std::process::id());
    let mut address = vec![0; 3 + name.len()];
    address[..2].copy_from_slice(&(AF_UNIX as u16).to_ne_bytes());
    address[3..].copy_from_slice(name.as_bytes());
    address
}

#[test]
fn unpublished_connector_helper() {
    let Ok(encoded) = std::env::var("KINAKAZE_LISTENER_FAULT_ADDRESS") else {
        return;
    };
    let address = encoded
        .as_bytes()
        .chunks(2)
        .map(|s| u8::from_str_radix(std::str::from_utf8(s).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
    let mut fds = Fds(Vec::new());
    let client = fds.socket(SOCK_NONBLOCK);
    unsafe { connect(client, address.as_ptr(), address.len() as i32) }.unwrap();
    panic!("fault injection did not run");
}

#[test]
fn connector_dying_before_publication_does_not_leak_listener_handles() {
    use std::os::windows::process::CommandExt;
    // Isolate the listener resource set from other tests' live queues. Windows
    // threadpool handle counts are not a reliable proxy for these resources.
    const ISOLATED: &str = "KINAKAZE_LISTENER_FAULT_ISOLATED";
    if std::env::var_os(ISOLATED).as_deref() != Some(std::ffi::OsStr::new("1")) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "unix::connect_tests::connector_dying_before_publication_does_not_leak_listener_handles",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(ISOLATED, "1")
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    let mut fds = Fds(Vec::new());
    let address = address();
    let listener = fds.socket(SOCK_NONBLOCK);
    unsafe { bind(listener, address.as_ptr(), address.len() as i32) }.unwrap();
    listen(listener, 8).unwrap();
    let warm = fds.socket(SOCK_NONBLOCK);
    unsafe { connect(warm, address.as_ptr(), address.len() as i32) }.unwrap();
    let accepted = fds.accept(listener);
    fds.close(warm);
    fds.close(accepted);
    listener::flush_test_resources();
    let warm = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "unix::connect_tests::unpublished_connector_helper",
        ])
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert!(warm.status.success());
    assert!(
        !poll_readiness(listener)
            .unwrap()
            .contains(Readiness::READABLE)
    );
    assert_eq!(listener::flush_test_resources(), (0, 0));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "unix::connect_tests::unpublished_connector_helper",
            "--nocapture",
        ])
        .env(
            "KINAKAZE_LISTENER_FAULT_ADDRESS",
            address
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        )
        .env("KINAKAZE_LISTENER_DIE_BEFORE_PUBLISH", "1")
        .creation_flags(0x0800_0000)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(77), "{output:?}");
    let retained = listener::flush_test_resources();
    assert!(
        !poll_readiness(listener)
            .unwrap()
            .contains(Readiness::READABLE)
    );
    assert_eq!(
        retained,
        (0, 0),
        "unpublished remote handles or their cleanup pool survived the connector"
    );
}

#[test]
fn dying_acceptor_helper() {
    let Ok(id) = std::env::var("KINAKAZE_LISTENER_ACCEPT_POOL") else {
        return;
    };
    let lease = listener::Lease::restore(id.parse().unwrap()).unwrap();
    let _ = lease.accept(true).unwrap();
    panic!("dequeue fault injection did not run");
}

#[test]
fn acceptor_death_preserves_uncommitted_dequeue_and_retires_committed_dequeue() {
    use std::os::windows::process::CommandExt;
    for committed in [false, true] {
        let mut fds = Fds(Vec::new());
        let address = address();
        let listener = fds.socket(SOCK_NONBLOCK);
        unsafe { bind(listener, address.as_ptr(), address.len() as i32) }.unwrap();
        listen(listener, 8).unwrap();
        let client = fds.socket(SOCK_NONBLOCK);
        unsafe { connect(client, address.as_ptr(), address.len() as i32) }.unwrap();
        if !committed {
            crate::write(client, b"retained").unwrap();
            fds.close(client);
        }
        let pool = snapshot(listener).unwrap().listener.unwrap().id();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "unix::connect_tests::dying_acceptor_helper",
                "--nocapture",
            ])
            .env("KINAKAZE_LISTENER_ACCEPT_POOL", pool.to_string())
            .env(
                if committed {
                    "KINAKAZE_LISTENER_DIE_AFTER_DEQUEUE"
                } else {
                    "KINAKAZE_LISTENER_DIE_BEFORE_DEQUEUE_PUBLISH"
                },
                "1",
            )
            .creation_flags(0x0800_0000)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                panic!("owned acceptor helper timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(if committed { 78 } else { 79 }),
            "{output:?}"
        );
        listener::flush_test_resources();
        if committed {
            assert_eq!(
                crate::read(client, &mut [0]),
                Ok(0),
                "orphan listener reference delayed EOF"
            );
            assert!(
                !poll_readiness(listener)
                    .unwrap()
                    .contains(Readiness::READABLE)
            );
        } else {
            let accepted = fds.accept(listener);
            let mut bytes = [0; 8];
            assert_eq!(crate::read(accepted, &mut bytes), Ok(8));
            assert_eq!(&bytes, b"retained");
            assert_eq!(crate::read(accepted, &mut bytes), Ok(0));
        }
        let next = fds.socket(SOCK_NONBLOCK);
        unsafe { connect(next, address.as_ptr(), address.len() as i32) }.unwrap();
        let accepted = fds.accept(listener);
        crate::write(next, b"n").unwrap();
        assert_eq!(crate::read(accepted, &mut [0]), Ok(1));
    }
}

#[test]
fn full_unix_queue_reports_retryable_error_without_a_phantom_completion() {
    let mut fds = Fds(Vec::new());
    let address = address();
    let listener = fds.socket(SOCK_NONBLOCK);
    unsafe { bind(listener, address.as_ptr(), address.len() as i32) }.unwrap();
    // Linux permits one pending AF_UNIX connection for backlog zero. Use that
    // boundary independently of the separately missing larger-backlog support.
    listen(listener, 0).unwrap();
    let first = fds.socket(SOCK_NONBLOCK);
    unsafe { connect(first, address.as_ptr(), address.len() as i32) }.unwrap();
    let retry = fds.socket(SOCK_NONBLOCK);
    assert_eq!(
        unsafe { connect(retry, address.as_ptr(), address.len() as i32) },
        Err(EAGAIN)
    );
    assert_eq!(snapshot(retry).unwrap().state, State::Idle);
    assert_eq!(get(retry).unwrap().raw, 0);
    assert!(!poll_readiness(retry).unwrap().contains(Readiness::WRITABLE));

    let accepted = fds.accept(listener);
    unsafe { connect(retry, address.as_ptr(), address.len() as i32) }.unwrap();
    let accepted_retry = fds.accept(listener);
    crate::write(first, b"first").unwrap();
    crate::write(retry, b"retry").unwrap();
    let mut bytes = [0; 5];
    assert_eq!(crate::read(accepted, &mut bytes), Ok(5));
    assert_eq!(&bytes, b"first");
    assert_eq!(crate::read(accepted_retry, &mut bytes), Ok(5));
    assert_eq!(&bytes, b"retry");
}

#[test]
fn listener_native_notification_survives_cancellation_and_accept_rebind() {
    use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let mut fds = Fds(Vec::new());
    let address = address();
    let listener = fds.socket(SOCK_NONBLOCK);
    unsafe { bind(listener, address.as_ptr(), address.len() as i32) }.unwrap();
    listen(listener, 0).unwrap();
    let cancelled = readiness::prepare(listener).unwrap().unwrap();
    let wait = readiness::prepare(listener).unwrap().unwrap();
    assert_eq!(unsafe { WaitForSingleObject(wait.raw(), 0) }, WAIT_TIMEOUT);
    drop(cancelled);
    let client = fds.socket(SOCK_NONBLOCK);
    unsafe { connect(client, address.as_ptr(), address.len() as i32) }.unwrap();
    // Check the native event directly: no epoll timer or readiness polling can
    // turn this assertion green. Enrollment does not consume the connection.
    assert_eq!(unsafe { WaitForSingleObject(wait.raw(), 0) }, WAIT_OBJECT_0);
    let accepted = fds.accept(listener);
    let replacement = readiness::prepare(listener).unwrap().unwrap();
    drop(wait);
    assert_eq!(
        unsafe { WaitForSingleObject(replacement.raw(), 0) },
        WAIT_TIMEOUT
    );
    crate::write(client, b"x").unwrap();
    assert_eq!(crate::read(accepted, &mut [0; 1]), Ok(1));
    let second = fds.socket(SOCK_NONBLOCK);
    unsafe { connect(second, address.as_ptr(), address.len() as i32) }.unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(replacement.raw(), 0) },
        WAIT_OBJECT_0
    );
    let _accepted_second = fds.accept(listener);
}

#[test]
fn connect_restarts_metadata_interruptions_but_preserves_nonrestart_handlers() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DELIVERED: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "sysv64" fn handler(_: i32) {
        DELIVERED.fetch_add(1, Ordering::SeqCst);
    }
    let mask = signal::sigprocmask(signal::SIG_SETMASK, 0).unwrap();
    for disposition in [
        signal::Disposition::Ignore,
        signal::Disposition::Handle(handler, 0),
    ] {
        for restart in [false, true] {
            DELIVERED.store(0, Ordering::SeqCst);
            let previous = signal::sigaction(
                signal::SIGUSR1,
                Some(signal::Action {
                    disposition,
                    flags: if restart { signal::SA_RESTART } else { 0 },
                    ..signal::Action::default()
                }),
            )
            .unwrap();
            let mut calls = 0;
            let result = connect_with_restart(|| {
                calls += 1;
                if calls == 1 {
                    signal::raise_thread_signal(interrupt::current_thread_id(), signal::SIGUSR1)
                        .unwrap();
                    Err(EINTR)
                } else {
                    Ok(19)
                }
            });
            signal::sigaction(signal::SIGUSR1, Some(previous)).unwrap();
            let handled = matches!(disposition, signal::Disposition::Handle(..));
            assert_eq!(DELIVERED.load(Ordering::SeqCst), usize::from(handled));
            assert_eq!(
                result,
                if handled && !restart {
                    Err(EINTR)
                } else {
                    Ok(19)
                }
            );
            assert_eq!(calls, if handled && !restart { 1 } else { 2 });
        }
    }
    let mut calls = 0;
    assert_eq!(
        connect_with_restart(|| {
            calls += 1;
            if calls == 1 { Err(EINTR) } else { Ok(31) }
        }),
        Ok(31)
    );
    assert_eq!(calls, 2);
    signal::sigprocmask(signal::SIG_SETMASK, mask).unwrap();
}

#[test]
fn client_waits_for_connection_attributes_after_native_endpoint_publication() {
    let address = address();
    let parsed = unsafe { parse_address(address.as_ptr(), address.len() as i32) }.unwrap();
    let name = parsed.pipe_name_in(crate::usernet::current().unwrap());
    let publication = InstancePublication::acquire(&name).unwrap();
    let server =
        crate::fs::object::Object::owned(create_native_instance(&name, false, true).unwrap())
            .unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let client = std::thread::spawn(move || {
        let mut fds = Fds(Vec::new());
        let fd = fds.socket(SOCK_NONBLOCK);
        let result = unsafe { connect(fd, address.as_ptr(), address.len() as i32) };
        sent.send(result).unwrap();
        result
    });
    // Native connection arrival proves the client is able to open this name;
    // its success must remain pending until the ancillary state is published.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !pipe_information(server.raw()).is_some_and(|info| {
        matches!(
            info.named_pipe_state,
            FILE_PIPE_CONNECTED_STATE | FILE_PIPE_CLOSING_STATE
        )
    }) {
        assert!(
            std::time::Instant::now() < deadline,
            "client did not open the published native endpoint"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(matches!(
        received.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    let _state = ancillary::State::create(server.raw()).unwrap();
    drop(publication);
    assert_eq!(
        received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        Ok(())
    );
    assert_eq!(client.join().unwrap(), Ok(()));
}

#[test]
fn accept_emfile_preserves_the_pending_connection() {
    struct Limit(u64, u64);
    impl Drop for Limit {
        fn drop(&mut self) {
            crate::job::set_nofile_limits(0, self.0, self.1).unwrap();
        }
    }
    let mut fds = Fds(Vec::new());
    let address = address();
    let listener = fds.socket(SOCK_NONBLOCK);
    unsafe { bind(listener, address.as_ptr(), address.len() as i32) }.unwrap();
    listen(listener, 8).unwrap();
    let client = fds.socket(SOCK_NONBLOCK);
    unsafe { connect(client, address.as_ptr(), address.len() as i32) }.unwrap();
    crate::write(client, b"pending").unwrap();
    let old = crate::job::nofile_limits(0).unwrap();
    let limit = Limit(old.0, old.1);
    crate::job::set_nofile_limits(0, 0, old.1).unwrap();
    assert_eq!(
        unsafe {
            accept(
                listener,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                SOCK_NONBLOCK,
            )
        },
        Err(crate::EMFILE)
    );
    drop(limit);
    let accepted = fds.accept(listener);
    let mut bytes = [0; 7];
    assert_eq!(crate::read(accepted, &mut bytes), Ok(7));
    assert_eq!(&bytes, b"pending");
}

#[test]
fn blocking_connect_wakes_when_accept_releases_space_or_last_listener_closes() {
    use std::sync::mpsc;
    use std::time::Duration;
    for close_listener in [false, true] {
        let mut fds = Fds(Vec::new());
        let address = address();
        let listener = fds.socket(SOCK_NONBLOCK);
        unsafe { bind(listener, address.as_ptr(), address.len() as i32) }.unwrap();
        listen(listener, 0).unwrap();
        let first = fds.socket(SOCK_NONBLOCK);
        unsafe { connect(first, address.as_ptr(), address.len() as i32) }.unwrap();
        let (started, enrolled) = mpsc::channel();
        let (finished, result) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let mut fds = Fds(Vec::new());
            let second = fds.socket(0);
            started.send(()).unwrap();
            finished
                .send(unsafe { connect(second, address.as_ptr(), address.len() as i32) })
                .unwrap();
        });
        enrolled.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        if close_listener {
            fds.close(listener);
        } else {
            fds.accept(listener);
        }
        assert_eq!(
            result.recv_timeout(Duration::from_secs(2)).unwrap(),
            if close_listener {
                Err(ECONNREFUSED)
            } else {
                Ok(())
            }
        );
        waiter.join().unwrap();
    }
}

#[test]
fn listen_transition_and_connect_rejection_are_shared_by_bound_aliases() {
    let mut fds = Fds(Vec::new());
    let address = address();
    let listener = fds.socket(SOCK_NONBLOCK);
    unsafe { bind(listener, address.as_ptr(), address.len() as i32) }.unwrap();
    let client = fds.socket(SOCK_NONBLOCK);
    assert_eq!(
        unsafe { connect(client, address.as_ptr(), address.len() as i32) },
        Err(ECONNREFUSED)
    );
    let entry = get(listener).unwrap();
    let copy = crate::fs::object::Object::duplicate(entry.raw as HANDLE).unwrap();
    let alias =
        crate::install_duplicate(copy.raw() as usize, entry.kind, entry.flags, entry).unwrap();
    copy.into_raw();
    duplicate(listener, alias, get(alias).unwrap().raw).unwrap();
    fds.0.push(alias);
    listen(listener, 8).unwrap();
    assert_eq!(snapshot(alias).unwrap().state, State::Listening);
    fds.close(listener);
    unsafe { connect(client, address.as_ptr(), address.len() as i32) }.unwrap();
    let accepted = fds.accept(alias);
    crate::write(client, b"a").unwrap();
    assert_eq!(crate::read(accepted, &mut [0]), Ok(1));
}
