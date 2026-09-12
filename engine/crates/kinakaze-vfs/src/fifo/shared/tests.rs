use super::*;
use std::os::windows::io::AsRawHandle;

struct Fixture {
    marker: std::fs::File,
    path: PathBuf,
    private: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("kinakaze-fifo-ring-{}-{nonce}", std::process::id()));
        let marker = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let private = storage_root()
            .unwrap()
            .join(identity(marker.as_raw_handle()).unwrap());
        Self {
            marker,
            path,
            private,
        }
    }
    fn attach(&self) -> Arc<Channel> {
        Channel::attach(self.marker.as_raw_handle()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).unwrap();
        if self.private.exists() {
            std::fs::remove_dir_all(&self.private).unwrap();
        }
    }
}

#[test]
fn unchanged_readiness_and_byte_io_do_not_rescan_endpoint_names() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let endpoint = channel.open(O_RDWR | O_NONBLOCK).unwrap();
    // Consume the endpoint creation notification before the measured work.
    let watch_event = channel.watch.lock().unwrap().event();
    assert_eq!(
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(watch_event, 5000) },
        WAIT_OBJECT_0
    );
    endpoint.context.readiness().unwrap();
    let scans = channel.scans.load(std::sync::atomic::Ordering::Relaxed);
    let started = std::time::Instant::now();
    for _ in 0..1000 {
        assert!(!endpoint.context.readiness().unwrap().readable);
        assert_eq!(endpoint.context.flags().unwrap(), O_RDWR | O_NONBLOCK);
        assert_eq!(endpoint.context.queued().unwrap(), 0);
        assert_eq!(endpoint.context.write(b"x"), Ok(1));
        let mut byte = [0];
        assert_eq!(endpoint.context.read(&mut byte), Ok(1));
        assert_eq!(byte, [b'x']);
    }
    let added = channel.scans.load(std::sync::atomic::Ordering::Relaxed) - scans;
    eprintln!(
        "fifo unchanged: 1000 rounds, {added} directory scans, {:?}",
        started.elapsed()
    );
    assert_eq!(
        added, 0,
        "unchanged endpoints must not trigger directory enumeration"
    );
}

#[test]
fn abandoned_mutex_repairs_counts_even_without_directory_notification() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let endpoint = channel.open(O_RDWR | O_NONBLOCK).unwrap();
    endpoint.context.readiness().unwrap();
    let other = channel.clone();
    std::thread::spawn(move || {
        let mut guard = other.lock().unwrap();
        // Simulate an interrupted shared-state update without touching tokens.
        guard.state().readers = 0;
        guard.state().writers = 0;
        std::mem::forget(guard);
    })
    .join()
    .unwrap();
    assert_eq!(endpoint.context.write(b"recovered"), Ok(9));
    assert_eq!(endpoint.context.read(&mut [0; 9]), Ok(9));
    let mut guard = channel.lock().unwrap();
    assert_eq!((guard.state().readers, guard.state().writers), (1, 1));
}

#[test]
fn cancelled_directory_subscription_recovers_then_observes_last_close() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let reader = channel.open(O_RDONLY | O_NONBLOCK).unwrap();
    let writer = channel.open(O_WRONLY | O_NONBLOCK).unwrap();
    reader.context.readiness().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while unsafe {
        windows_sys::Win32::System::Threading::WaitForSingleObject(
            channel.watch.lock().unwrap().event(),
            0,
        )
    } == WAIT_OBJECT_0
    {
        assert!(std::time::Instant::now() < deadline);
        reader.context.readiness().unwrap();
    }
    let event = {
        let watch = channel.watch.lock().unwrap();
        // Cancel precisely this test's own pending directory subscription.
        watch.cancel_for_test();
        watch.event()
    };
    assert_eq!(
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(event, 5000) },
        WAIT_OBJECT_0
    );
    assert!(!reader.context.readiness().unwrap().hangup);
    let (_, waiting) = reader.context.prepare_wait().unwrap();
    drop(writer);
    assert_eq!(
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                waiting.handles()[1] as HANDLE,
                5000,
            )
        },
        WAIT_OBJECT_0
    );
    assert!(reader.context.readiness().unwrap().hangup);
    assert_eq!(reader.context.read(&mut [0]), Ok(0));
}

#[test]
fn rdwr_and_independent_endpoints_share_one_queue() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let both = channel.open(O_RDWR | O_NONBLOCK).unwrap();
    let reader = fixture.attach().open(O_RDONLY | O_NONBLOCK).unwrap();
    let writer = fixture.attach().open(O_WRONLY | O_NONBLOCK).unwrap();
    let mut bytes = [0; 16];
    assert_eq!(both.context.read(&mut bytes), Err(EAGAIN));
    assert_eq!(writer.context.write(b"named queue"), Ok(11));
    assert_eq!(both.context.read(&mut bytes[..5]), Ok(5));
    assert_eq!(&bytes[..5], b"named");
    assert_eq!(reader.context.read(&mut bytes), Ok(6));
    assert_eq!(&bytes[..6], b" queue");
    assert_eq!(both.context.write(b"self"), Ok(4));
    assert_eq!(both.context.read(&mut bytes), Ok(4));
    assert_eq!(&bytes[..4], b"self");
    assert_eq!(reader.context.write(b"wrong"), Err(EBADF));
    assert_eq!(writer.context.read(&mut bytes), Err(EBADF));
}

#[test]
fn initial_no_writer_eof_hup_and_later_writer_reconnect() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    assert!(matches!(channel.open(O_WRONLY | O_NONBLOCK), Err(ENXIO)));
    let reader = channel.open(O_RDONLY | O_NONBLOCK).unwrap();
    let mut byte = [0];
    assert_eq!(reader.context.read(&mut byte), Ok(0));
    assert!(!reader.context.readiness().unwrap().hangup);
    let writer = channel.open(O_WRONLY | O_NONBLOCK).unwrap();
    assert_eq!(reader.context.read(&mut byte), Err(EAGAIN));
    writer.context.write(b"x").unwrap();
    drop(writer);
    let ready = reader.context.readiness().unwrap();
    assert!(ready.readable && ready.hangup);
    assert_eq!(reader.context.read(&mut byte), Ok(1));
    assert_eq!(reader.context.read(&mut byte), Ok(0));
    let writer = channel.open(O_WRONLY | O_NONBLOCK).unwrap();
    assert!(!reader.context.readiness().unwrap().hangup);
    drop(reader);
    assert!(writer.context.readiness().unwrap().error);
    assert_eq!(writer.context.write(b"x"), Err(EPIPE));
}

#[test]
fn reader_opened_after_an_existing_writer_reports_that_writers_final_close() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let writer = channel.open(O_RDWR | O_NONBLOCK).unwrap();
    let reader = channel.open(O_RDONLY | O_NONBLOCK).unwrap();
    assert!(!reader.context.readiness().unwrap().hangup);
    let (_, waiting) = reader.context.prepare_wait().unwrap();
    drop(writer);
    waiting.wait().unwrap();
    assert!(reader.context.readiness().unwrap().hangup);
    assert_eq!(reader.context.read(&mut [0; 1]), Ok(0));
}

#[test]
fn pipe_buf_is_atomic_and_large_nonblocking_writes_can_be_partial() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let endpoint = channel.open(O_RDWR | O_NONBLOCK).unwrap();
    assert_eq!(endpoint.context.write(&vec![1; CAPACITY]), Ok(CAPACITY));
    assert_eq!(endpoint.context.write(b"x"), Err(EAGAIN));
    let mut room = [0; 100];
    endpoint.context.read(&mut room).unwrap();
    assert_eq!(endpoint.context.write(&[2; 101]), Err(EAGAIN));
    assert_eq!(endpoint.context.queued(), Ok(CAPACITY - 100));
    assert_eq!(endpoint.context.write(&vec![3; PIPE_BUF + 1]), Ok(100));
    assert_eq!(endpoint.context.queued(), Ok(CAPACITY));
}

#[test]
fn independent_restore_keeps_queue_and_shared_status_but_new_open_does_not() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let endpoint = channel.open(O_RDWR | O_NONBLOCK).unwrap();
    endpoint.context.write(b"retained").unwrap();
    let (restored, id) = Channel::restore(endpoint.token.0).unwrap();
    let restored = Context {
        channel: restored,
        id,
    };
    restored.set_flags(false, true).unwrap();
    assert_eq!(
        endpoint.context.flags().unwrap() & (O_NONBLOCK | O_APPEND),
        O_APPEND
    );
    let independent = channel.open(O_RDWR | O_NONBLOCK).unwrap();
    assert_eq!(
        independent.context.flags().unwrap() & (O_NONBLOCK | O_APPEND),
        O_NONBLOCK
    );
    let mut bytes = [0; 8];
    assert_eq!(restored.read(&mut bytes), Ok(8));
    assert_eq!(&bytes, b"retained");
}

#[test]
fn empty_endpoint_set_restarts_without_stale_ring_data() {
    let fixture = Fixture::new();
    {
        let endpoint = fixture.attach().open(O_RDWR | O_NONBLOCK).unwrap();
        endpoint.context.write(b"discard on last close").unwrap();
    }
    let endpoint = fixture.attach().open(O_RDWR | O_NONBLOCK).unwrap();
    assert_eq!(endpoint.context.queued(), Ok(0));
    assert_eq!(endpoint.context.read(&mut [0; 32]), Err(EAGAIN));
}

#[test]
fn blocking_open_handshakes_and_epoch_wait_has_no_lost_wakeup() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let other = channel.clone();
    let thread = std::thread::spawn(move || {
        let writer = other.open(O_WRONLY).unwrap();
        assert_eq!(writer.context.write(b"connected"), Ok(9));
    });
    let reader = channel.open(O_RDONLY).unwrap();
    let mut bytes = [0; 9];
    assert_eq!(reader.context.read(&mut bytes), Ok(9));
    assert_eq!(&bytes, b"connected");
    thread.join().unwrap();
    let writer = channel.open(O_WRONLY | O_NONBLOCK).unwrap();
    let (ready, registration) = reader.context.prepare_wait().unwrap();
    assert!(!ready.readable && !ready.hangup);
    writer.context.write(b"x").unwrap();
    // The mutation completed before this wait started. Its old epoch stays set.
    assert_eq!(
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                registration.handles()[0] as HANDLE,
                5000,
            )
        },
        WAIT_OBJECT_0
    );
    assert!(reader.context.readiness().unwrap().readable);
}

#[test]
fn multiple_writers_keep_each_pipe_buf_record_contiguous() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let reader = channel.open(O_RDONLY | O_NONBLOCK).unwrap();
    let writers = (0..4)
        .map(|_| channel.open(O_WRONLY).unwrap())
        .collect::<Vec<_>>();
    reader.context.set_flags(false, false).unwrap();
    let threads = writers
        .into_iter()
        .enumerate()
        .map(|(id, writer)| {
            std::thread::spawn(move || {
                let _retain_kernel_token = writer.token;
                for sequence in 0..100u8 {
                    let mut record = [id as u8; 257];
                    record[0] = sequence;
                    assert_eq!(writer.context.write(&record), Ok(record.len()));
                }
            })
        })
        .collect::<Vec<_>>();
    let mut combined = Vec::new();
    let mut buffer = [0; 2003];
    while combined.len() < 4 * 100 * 257 {
        let count = reader.context.read(&mut buffer).unwrap();
        assert!(count > 0);
        combined.extend_from_slice(&buffer[..count]);
    }
    for thread in threads {
        thread.join().unwrap();
    }
    let mut sequences = [0; 4];
    for record in combined.chunks_exact(257) {
        let id = record[1] as usize;
        assert!(id < 4);
        assert_eq!(record[0] as usize, sequences[id]);
        assert!(record[1..].iter().all(|byte| *byte == id as u8));
        sequences[id] += 1;
    }
    assert_eq!(sequences, [100; 4]);
}

#[test]
fn abandoned_owner_between_notification_and_epoch_commit_does_not_lose_waiters() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let endpoint = channel.open(O_RDWR | O_NONBLOCK).unwrap();
    let (_, waiting) = endpoint.context.prepare_wait().unwrap();
    let other = channel.clone();
    std::thread::spawn(move || {
        let mut guard = other.lock().unwrap();
        let before = guard.state().epoch;
        assert_eq!(guard.notify_waited_epoch().unwrap(), before);
        assert_eq!(guard.state().epoch, before);
        // Simulate death precisely between native SetEvent and the shared-state
        // commit. Exiting this thread abandons its native mutex without cleanup.
        std::mem::forget(guard);
    })
    .join()
    .unwrap();
    assert_eq!(
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                waiting.handles()[0] as HANDLE,
                5000,
            )
        },
        WAIT_OBJECT_0
    );
    let (_, next) = endpoint.context.prepare_wait().unwrap();
    assert_ne!(waiting.handles()[0], next.handles()[0]);
    assert_eq!(
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                next.handles()[0] as HANDLE,
                0,
            )
        },
        windows_sys::Win32::Foundation::WAIT_TIMEOUT
    );
}

#[test]
fn blocking_io_returns_eintr_and_keeps_atomic_write_uncommitted() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let endpoint = channel.open(O_RDWR).unwrap();
    let context = endpoint.context.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        assert!(!crate::interrupt::current().is_null());
        sender.send(crate::interrupt::current_thread_id()).unwrap();
        context.read(&mut [0; 1])
    });
    assert!(crate::interrupt::interrupt_thread(receiver.recv().unwrap()));
    assert_eq!(thread.join().unwrap(), Err(EINTR));
    assert_eq!(endpoint.context.write(&vec![1; CAPACITY]), Ok(CAPACITY));
    let context = endpoint.context.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        assert!(!crate::interrupt::current().is_null());
        sender.send(crate::interrupt::current_thread_id()).unwrap();
        context.write(&[2; PIPE_BUF])
    });
    assert!(crate::interrupt::interrupt_thread(receiver.recv().unwrap()));
    assert_eq!(thread.join().unwrap(), Err(EINTR));
    assert_eq!(endpoint.context.queued(), Ok(CAPACITY));
}

#[test]
fn interrupted_open_keeps_its_endpoint_on_restart_and_retires_it_on_eintr() {
    for restart in [false, true] {
        let fixture = Fixture::new();
        let channel = fixture.attach();
        let worker_channel = channel.clone();
        let (tid_tx, tid_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            assert!(!crate::interrupt::current().is_null());
            tid_tx.send(crate::interrupt::current_thread_id()).unwrap();
            let mut calls = 0;
            let result = worker_channel.open_with_restart(O_WRONLY, &mut || {
                calls += 1;
                if restart {
                    // Reenter this inode from the handler: no mutex may be
                    // held here. Close the reader before returning, so only
                    // the original generation can complete the pending open.
                    drop(worker_channel.open(O_RDONLY | O_NONBLOCK).unwrap());
                }
                restart
            });
            let result = result.map(|opened| opened.context.id);
            done_tx.send((result, calls)).unwrap();
        });
        let tid = tid_rx.recv().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let pending_id = loop {
            let mut guard = channel.lock().unwrap();
            let state = guard.state();
            if state.writers == 1 {
                break state.writer_generation;
            }
            drop(guard);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert!(crate::interrupt::interrupt_thread(tid));
        let (result, calls) = done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        worker.join().unwrap();
        assert_eq!(calls, 1);
        assert_eq!(result, if restart { Ok(pending_id) } else { Err(EINTR) });
        let mut guard = channel.lock().unwrap();
        assert_eq!(guard.state().writers, 0);
        assert_eq!(guard.state().readers, 0);
    }
}

#[test]
fn death_after_watch_reset_before_reconcile_still_notifies_existing_waiters() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let reader = channel.open(O_RDONLY | O_NONBLOCK).unwrap();
    let writer = channel.open(O_WRONLY | O_NONBLOCK).unwrap();
    let (_, waiting) = reader.context.prepare_wait().unwrap();
    drop(writer);
    assert_eq!(
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                waiting.handles()[1] as HANDLE,
                5000,
            )
        },
        WAIT_OBJECT_0
    );
    let other = channel.clone();
    std::thread::spawn(move || {
        assert_eq!(wait_handles(&[other.gate.0], true), Ok(WAIT_OBJECT_0));
        let mut guard = Guard { channel: &other };
        // Stop precisely after consuming the native directory event, before
        // reconciling readers/writers. No subsequent owner is needed to wake
        // the old epoch; this remains correct on immediate process termination.
        guard.refresh_watch().unwrap();
        std::mem::forget(guard);
    })
    .join()
    .unwrap();
    assert_eq!(
        unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(
                waiting.handles()[0] as HANDLE,
                0,
            )
        },
        WAIT_OBJECT_0
    );
    assert!(reader.context.readiness().unwrap().hangup);
}

#[test]
fn reader_loss_after_partial_write_preserves_count_and_requests_sigpipe() {
    let fixture = Fixture::new();
    let channel = fixture.attach();
    let reader = channel.open(O_RDONLY | O_NONBLOCK).unwrap();
    let writer = channel.open(O_WRONLY).unwrap();
    let (_, waiting) = reader.context.prepare_wait().unwrap();
    let thread = std::thread::spawn(move || {
        let _retain_kernel_token = writer.token;
        writer.context.write_report(&vec![7; CAPACITY + 1])
    });
    waiting.wait().unwrap();
    assert_eq!(reader.context.queued(), Ok(CAPACITY));
    drop(reader);
    let (result, sigpipe) = thread.join().unwrap();
    assert_eq!(result, Ok(CAPACITY));
    assert!(sigpipe);
}
