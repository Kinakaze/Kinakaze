use super::*;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
use windows_sys::Win32::System::Threading::{SetEvent, WaitForSingleObject};

fn record() -> Arc<procnet::Record> {
    procnet::Record::new(crate::usernet::current().unwrap(), SOCK_DGRAM).unwrap()
}
fn message(len: usize) -> Message {
    Message {
        source: UnixAddress::unnamed(),
        credentials: credentials::Sender::current(false).unwrap(),
        payload: vec![b'x'; len],
        rights: None,
    }
}
fn level(event: &crate::fs::object::Object) -> bool {
    match unsafe { WaitForSingleObject(event.raw(), 0) } {
        WAIT_OBJECT_0 => true,
        WAIT_TIMEOUT => false,
        other => panic!("event wait failed: {other}"),
    }
}
fn wait_watch(record: &procnet::Record, condition: u64) {
    let began = Instant::now();
    while !Queue::decode(&record.data().unwrap())
        .unwrap()
        .watches
        .iter()
        .any(|w| w.condition == condition)
    {
        assert!(
            began.elapsed() < Duration::from_secs(4),
            "watch was not registered"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn publication_before_registration_is_not_lost() {
    let record = record();
    update(&record, |q| {
        q.messages.push_back(message(1));
        Ok(())
    })
    .unwrap();
    let event = readiness::prepare(&record, readiness::READ, 0).unwrap();
    assert!(level(&event));
    update(&record, |q| {
        q.messages.clear();
        Ok(())
    })
    .unwrap();
    assert!(!level(&event));
    update(&record, |q| {
        q.messages.push_back(message(0));
        Ok(())
    })
    .unwrap();
    assert!(level(&event)); // an empty packet is still readable
}

#[test]
fn space_events_follow_each_message_length_and_slot_limit() {
    let record = record();
    update(&record, |q| {
        q.messages.push_back(message(LIMIT - 8));
        Ok(())
    })
    .unwrap();
    let small = readiness::prepare(&record, 8, 7).unwrap();
    let large = readiness::prepare(&record, 9, 7).unwrap();
    assert!(level(&small));
    assert!(!level(&large));
    update(&record, |q| {
        q.messages.clear();
        q.messages.extend((0..64).map(|_| message(0)));
        Ok(())
    })
    .unwrap();
    assert!(!level(&small));
    let empty = readiness::prepare(&record, 0, 7).unwrap();
    assert!(!level(&empty));
    update(&record, |q| {
        q.messages.pop_front();
        Ok(())
    })
    .unwrap();
    assert!(level(&small) && level(&large) && level(&empty));
}

#[test]
fn shared_levels_wake_all_observers_and_reset_after_consume() {
    let record = record();
    let first = readiness::prepare(&record, readiness::READ, 0).unwrap();
    let second = readiness::prepare(&record, readiness::READ, 0).unwrap();
    update(&record, |q| {
        q.messages.push_back(message(3));
        Ok(())
    })
    .unwrap();
    assert!(level(&first) && level(&second));
    update(&record, |q| {
        q.messages.pop_front();
        Ok(())
    })
    .unwrap();
    assert!(!level(&first) && !level(&second));
    assert_eq!(
        Queue::decode(&record.data().unwrap())
            .unwrap()
            .watches
            .len(),
        1
    );
}

#[test]
fn terminal_conditions_wake_blocked_readers_and_writers() {
    let record = record();
    update(&record, |q| {
        q.messages.push_back(message(LIMIT));
        Ok(())
    })
    .unwrap();
    let space = readiness::prepare(&record, 1, 7).unwrap();
    let own_close = readiness::prepare(&record, readiness::CLOSED, 0).unwrap();
    assert!(!level(&space) && !level(&own_close));
    update(&record, |q| {
        q.peer = 8;
        Ok(())
    })
    .unwrap();
    assert!(level(&space)); // permission changed while the sender was asleep
    update(&record, |q| {
        q.peer = 7;
        Ok(())
    })
    .unwrap();
    assert!(!level(&space));
    update(&record, |q| {
        q.read_closed = true;
        q.messages.clear();
        q.write_closed = true;
        Ok(())
    })
    .unwrap();
    let read = readiness::prepare(&record, readiness::READ, 0).unwrap();
    assert!(level(&space) && level(&read) && level(&own_close));
}

#[test]
fn abandoned_publisher_level_is_reconciled_before_sleep() {
    let record = record();
    let event = readiness::prepare(&record, readiness::READ, 0).unwrap();
    // Model death after pre-publication SetEvent, before the new bank commits.
    assert_ne!(unsafe { SetEvent(event.raw()) }, 0);
    assert!(level(&event));
    readiness::refresh(&record, readiness::READ, 0).unwrap();
    assert!(!level(&event));
}

#[test]
fn last_handle_close_retires_watches_without_owner_bookkeeping() {
    let record = record();
    let first = readiness::prepare(&record, readiness::READ, 0).unwrap();
    let second = readiness::prepare(&record, readiness::READ, 0).unwrap();
    drop(first);
    update(&record, |_| Ok(())).unwrap();
    assert_eq!(
        Queue::decode(&record.data().unwrap())
            .unwrap()
            .watches
            .len(),
        1
    );
    drop(second);
    update(&record, |_| Ok(())).unwrap();
    assert!(
        Queue::decode(&record.data().unwrap())
            .unwrap()
            .watches
            .is_empty()
    );
}

#[test]
fn cross_process_helper() {
    let Ok(id) = std::env::var("KINAKAZE_TEST_DGRAM_RECORD") else {
        return;
    };
    let record = procnet::Record::restore(id.parse().unwrap()).unwrap();
    if let Ok(phase) = std::env::var("KINAKAZE_TEST_DGRAM_PUBLISH_FAULT") {
        update(&record, |q| {
            if phase == "before" {
                q.messages.push_back(message(1));
            } else {
                q.messages.clear();
            }
            Ok(())
        })
        .unwrap();
        panic!("publisher fault did not run");
    }
    if std::env::var_os("KINAKAZE_TEST_DGRAM_DEAD_WAITER").is_some() {
        let _event = readiness::prepare(&record, 12, 7).unwrap();
        // No Rust destructor or explicit watch unregister runs on this path.
        std::process::exit(0);
    }
    update(&record, |q| {
        q.messages.push_back(message(5));
        Ok(())
    })
    .unwrap();
}

fn child(record: &procnet::Record, dead: bool) {
    child_mode(record, dead, None);
}
fn child_mode(record: &procnet::Record, dead: bool, fault: Option<&str>) {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "unix::datagram::tests::cross_process_helper",
            "--test-threads=1",
        ])
        .env("KINAKAZE_TEST_DGRAM_RECORD", record.id().to_string())
        .creation_flags(0x08000000);
    if dead {
        command.env("KINAKAZE_TEST_DGRAM_DEAD_WAITER", "1");
    }
    if let Some(phase) = fault {
        command.env("KINAKAZE_TEST_DGRAM_PUBLISH_FAULT", phase);
    }
    let result = command.output().unwrap();
    assert!(result.status.success(), "{result:?}");
}

#[test]
fn publisher_hard_exit_before_commit_keeps_old_packets_and_reconciles_wake() {
    let record = record();
    let event = readiness::prepare(&record, readiness::READ, 0).unwrap();
    child_mode(&record, false, Some("before"));
    assert!(level(&event));
    assert!(
        Queue::decode(&record.data().unwrap())
            .unwrap()
            .messages
            .is_empty()
    );
    readiness::refresh(&record, readiness::READ, 0).unwrap();
    assert!(!level(&event));
}

#[test]
fn publisher_hard_exit_after_commit_reconciles_stale_ready_level() {
    let record = record();
    update(&record, |q| {
        q.messages.push_back(message(1));
        Ok(())
    })
    .unwrap();
    let event = readiness::prepare(&record, readiness::READ, 0).unwrap();
    child_mode(&record, false, Some("after"));
    assert!(level(&event));
    assert!(
        Queue::decode(&record.data().unwrap())
            .unwrap()
            .messages
            .is_empty()
    );
    readiness::refresh(&record, readiness::READ, 0).unwrap();
    assert!(!level(&event));
}

#[test]
fn independent_process_wakes_a_shared_queue_event() {
    let record = record();
    let event = readiness::prepare(&record, readiness::READ, 0).unwrap();
    child(&record, false);
    assert_eq!(
        unsafe { WaitForSingleObject(event.raw(), 1000) },
        WAIT_OBJECT_0
    );
    assert_eq!(
        Queue::decode(&record.data().unwrap())
            .unwrap()
            .messages
            .len(),
        1
    );
}

#[test]
fn hard_exit_does_not_leave_a_native_waiter_owner() {
    let record = record();
    child(&record, true);
    assert_eq!(
        Queue::decode(&record.data().unwrap())
            .unwrap()
            .watches
            .len(),
        1
    );
    update(&record, |_| Ok(())).unwrap();
    assert!(
        Queue::decode(&record.data().unwrap())
            .unwrap()
            .watches
            .is_empty()
    );
}

#[test]
fn queue_codec_accepts_previous_packets_and_rejects_invalid_watches() {
    let record = record();
    let mut queue = Queue::decode(&record.data().unwrap()).unwrap();
    queue.messages.push_back(message(17));
    let bytes = queue.encode();
    assert_eq!(Queue::decode(&bytes).unwrap().messages[0].payload.len(), 17);
    let mut previous = bytes[..bytes.len() - 8].to_vec();
    previous[..8].copy_from_slice(b"KDGRAM02");
    assert!(Queue::decode(&previous).unwrap().watches.is_empty());
    queue.watches.push(readiness::Watch {
        condition: readiness::CLOSED + 1,
        sender: 0,
    });
    assert!(matches!(Queue::decode(&queue.encode()), Err(EIO)));
    queue.watches = vec![
        readiness::Watch {
            condition: 1,
            sender: 7
        };
        2
    ];
    assert!(matches!(Queue::decode(&queue.encode()), Err(EIO)));
}

struct Pair {
    left: i32,
    right: i32,
}
impl Pair {
    fn new() -> Self {
        let left = socket(SOCK_DGRAM, 0).unwrap();
        let right = socket(SOCK_DGRAM, 0).unwrap();
        let result = Self { left, right };
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let address = |side| {
            let mut bytes = (AF_UNIX as u16).to_ne_bytes().to_vec();
            bytes.push(0);
            bytes.extend(format!("dgram-events-{}-{nonce}-{side}", std::process::id()).bytes());
            bytes
        };
        let a = address(0);
        let b = address(1);
        unsafe {
            super::super::bind(left, a.as_ptr(), a.len() as i32).unwrap();
            super::super::bind(right, b.as_ptr(), b.len() as i32).unwrap();
            super::super::connect(left, b.as_ptr(), b.len() as i32).unwrap();
            super::super::connect(right, a.as_ptr(), a.len() as i32).unwrap();
        }
        result
    }
}
impl Drop for Pair {
    fn drop(&mut self) {
        let _ = crate::close(self.left);
        let _ = crate::close(self.right);
    }
}

#[test]
fn blocking_receive_and_shutdown_use_queue_events() {
    if !readiness::enabled() {
        return;
    }
    let pair = Pair::new();
    let right = pair.right;
    let record = snapshot(right).unwrap().record;
    let worker = std::thread::spawn(move || {
        let mut byte = [0];
        (crate::read(right, &mut byte), byte)
    });
    wait_watch(&record, readiness::READ);
    crate::write(pair.left, b"z").unwrap();
    assert_eq!(worker.join().unwrap(), (Ok(1), [b'z']));
    let worker = std::thread::spawn(move || crate::read(right, &mut [0]));
    wait_watch(&record, readiness::READ);
    shutdown(right, SHUT_RD).unwrap();
    assert_eq!(worker.join().unwrap(), Ok(0));
}

#[test]
fn blocking_send_wakes_when_capacity_is_released() {
    if !readiness::enabled() {
        return;
    }
    let pair = Pair::new();
    for _ in 0..64 {
        crate::write(pair.left, b"").unwrap();
    }
    let left = pair.left;
    let target = snapshot(pair.right).unwrap().record;
    let worker = std::thread::spawn(move || crate::write(left, b"x"));
    wait_watch(&target, 1);
    assert_eq!(crate::read(pair.right, &mut [0]), Ok(0));
    assert_eq!(worker.join().unwrap(), Ok(1));
}

#[test]
fn blocked_send_observes_own_write_shutdown() {
    if !readiness::enabled() {
        return;
    }
    let pair = Pair::new();
    for _ in 0..64 {
        crate::write(pair.left, b"").unwrap();
    }
    let left = pair.left;
    let own = snapshot(left).unwrap().record;
    let worker = std::thread::spawn(move || crate::write(left, b"x"));
    wait_watch(&own, readiness::CLOSED);
    shutdown(left, SHUT_WR).unwrap();
    assert_eq!(worker.join().unwrap(), Err(EPIPE));
}

#[test]
#[ignore = "paired release named datagram blocking and unblocked benchmark"]
fn benchmark_named_datagram_events() {
    const N: usize = 32;
    let pair = Pair::new();
    let right = pair.right;
    let (start_tx, start_rx) = std::sync::mpsc::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for _ in 0..N {
            start_tx.send(()).unwrap();
            assert_eq!(crate::read(right, &mut [0]), Ok(1));
            finish_tx.send(Instant::now()).unwrap();
        }
    });
    let mut receive_ns = 0;
    for _ in 0..N {
        start_rx.recv_timeout(Duration::from_secs(4)).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let began = Instant::now();
        crate::write(pair.left, b"x").unwrap();
        receive_ns += finish_rx
            .recv_timeout(Duration::from_secs(4))
            .unwrap()
            .duration_since(began)
            .as_nanos();
    }
    reader.join().unwrap();
    let mut send_ns = 0;
    for _ in 0..N {
        for _ in 0..64 {
            crate::write(pair.left, b"").unwrap();
        }
        let left = pair.left;
        let worker = std::thread::spawn(move || {
            assert_eq!(crate::write(left, b"x"), Ok(1));
            Instant::now()
        });
        std::thread::sleep(Duration::from_millis(2));
        let began = Instant::now();
        assert_eq!(crate::read(pair.right, &mut [0]), Ok(0));
        send_ns += worker.join().unwrap().duration_since(began).as_nanos();
        for _ in 0..64 {
            crate::read(pair.right, &mut [0]).unwrap();
        }
    }
    let began = Instant::now();
    for _ in 0..4096 {
        assert_eq!(crate::write(pair.left, b"x"), Ok(1));
        assert_eq!(crate::read(pair.right, &mut [0]), Ok(1));
    }
    let unblocked_ns = began.elapsed().as_nanos() / 4096;
    println!(
        "DGRAM_EVENTS_BENCH {{\"optimized\":{},\"receive_wake_ns\":{},\"send_wake_ns\":{},\"unblocked_pair_ns\":{unblocked_ns}}}",
        readiness::enabled(),
        receive_ns / N as u128,
        send_ns / N as u128
    );
}
