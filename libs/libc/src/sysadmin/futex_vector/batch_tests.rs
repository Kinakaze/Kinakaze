use super::*;
use core::ptr;
use std::sync::mpsc;

fn entry(word: &AtomicI32, private: bool) -> Entry {
    Entry {
        value: word.load(Ordering::Relaxed) as u32 as u64,
        address: word.as_ptr() as u64,
        flags: 2 | if private { 0x80 } else { 0 },
        reserved: 0,
    }
}

fn raw(entries: &[Entry], timeout: Option<&KernelTimespec>, clock: i32) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            449,
            entries.as_ptr() as u64,
            entries.len() as u64,
            0,
            timeout.map_or(0, |time| time as *const _ as u64),
            clock as u64,
            0,
        )
    }
}

fn deadline(milliseconds: i64) -> KernelTimespec {
    let mut time = KernelTimespec::default();
    assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut time) }, 0);
    time.tv_nsec += milliseconds * 1_000_000;
    time.tv_sec += time.tv_nsec / 1_000_000_000;
    time.tv_nsec %= 1_000_000_000;
    time
}

fn queued(word: &AtomicI32, count: usize) {
    let started = Instant::now();
    loop {
        let actual = futex_queues()
            .lock()
            .unwrap()
            .get(&(word.as_ptr() as usize))
            .map_or(0, VecDeque::len);
        if actual == count {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{actual} != {count}"
        );
        std::thread::yield_now();
    }
}

fn wake(word: &AtomicI32, private: bool) -> i64 {
    futex_syscall(
        word.as_ptr(),
        (FUTEX_WAKE | if private { FUTEX_PRIVATE_FLAG } else { 0 }) as i32,
        1,
        ptr::null(),
        ptr::null_mut(),
        0,
    )
}

#[test]
fn raw_waitv_validates_array_flags_values_and_absolute_timeout() {
    assert_eq!(size_of::<Entry>(), 24);
    let word = AtomicI32::new(7);
    let valid = entry(&word, true);
    let past = KernelTimespec::default();
    for (address, count, flags) in [(0, 1, 0), (1, 0, 0), (1, 129, 0), (1, 1, 1)] {
        assert_eq!(
            waitv(address, count, flags, ptr::null(), 999),
            -i64::from(EINVAL)
        );
    }
    assert_eq!(waitv(1, 1, 0, ptr::null(), 999), -i64::from(EFAULT));
    assert_eq!(raw(&[valid], Some(&past), 999), -i64::from(EINVAL));
    for flags in [0, 1, 3, 6, 0x102, 0x182] {
        assert_eq!(
            raw(&[Entry { flags, ..valid }], None, 999),
            -i64::from(EINVAL)
        );
    }
    assert_eq!(
        raw(
            &[Entry {
                reserved: 1,
                ..valid
            }],
            None,
            0
        ),
        -i64::from(EINVAL)
    );
    assert_eq!(
        raw(
            &[Entry {
                value: 1 << 32,
                ..valid
            }],
            None,
            0
        ),
        -i64::from(EINVAL)
    );
    assert_eq!(
        raw(
            &[Entry {
                address: 1,
                ..valid
            }],
            None,
            0
        ),
        -i64::from(EINVAL)
    );
    assert_eq!(
        raw(
            &[Entry {
                address: 4,
                ..valid
            }],
            None,
            0
        ),
        -i64::from(EFAULT)
    );
    // Linux parses every descriptor before resolving the first address.
    assert_eq!(
        raw(
            &[
                Entry {
                    address: 4,
                    ..valid
                },
                Entry {
                    reserved: 1,
                    ..valid
                }
            ],
            None,
            0
        ),
        -i64::from(EINVAL)
    );
    assert_eq!(
        raw(&[Entry { value: 8, ..valid }], None, 999),
        -i64::from(EAGAIN)
    );
    assert_eq!(
        raw(
            &[Entry { value: 8, ..valid }],
            Some(&past),
            CLOCK_MONOTONIC as i32
        ),
        -i64::from(EAGAIN)
    );
    assert_eq!(
        raw(&[valid], Some(&past), CLOCK_MONOTONIC as i32),
        -i64::from(ETIMEDOUT)
    );
    assert_eq!(
        raw(&[valid], Some(&past), CLOCK_REALTIME as i32),
        -i64::from(ETIMEDOUT)
    );
    queued(&word, 0);
}

#[test]
fn private_waitv_supports_128_members_and_removes_every_sibling() {
    let words = Arc::new((0..MAX).map(|_| AtomicI32::new(0)).collect::<Vec<_>>());
    let target = Arc::clone(&words);
    let thread = std::thread::spawn(move || {
        let entries: Vec<_> = target.iter().map(|word| entry(word, true)).collect();
        raw(&entries, Some(&deadline(5000)), CLOCK_MONOTONIC as i32)
    });
    queued(&words[127], 1);
    assert_eq!(wake(&words[127], true), 1);
    assert_eq!(thread.join().unwrap(), 127);
    for word in words.iter() {
        queued(word, 0);
        assert_eq!(wake(word, true), 0);
    }
}

#[test]
fn duplicate_private_members_count_rows_and_return_a_selected_index() {
    FUTEX_PRIVATE_BRIDGED.store(true, Ordering::Release);
    let word = Arc::new(AtomicI32::new(0));
    let target = Arc::clone(&word);
    let thread = std::thread::spawn(move || {
        let entries = vec![entry(&target, true); MAX];
        raw(&entries, Some(&deadline(5000)), CLOCK_MONOTONIC as i32)
    });
    queued(&word, MAX);
    // A spurious interrupt may retire this observed registration before the
    // wake acquires the queue lock. The reference path then republishes rows
    // individually. Wait for an actual selection, and join before asserting
    // so a failed attempt cannot contaminate subsequent queue tests.
    let started = Instant::now();
    let selected = loop {
        let selected = wake(&word, true);
        if selected != 0 || started.elapsed() >= Duration::from_secs(5) {
            break selected;
        }
        std::thread::yield_now();
    };
    let result = thread.join().unwrap();
    assert_eq!(selected, 1);
    assert_eq!(result, 0);
    queued(&word, 0);
}

#[test]
fn mixed_waitv_wakes_from_either_backend_and_failed_setup_publishes_nothing() {
    for selected in 0..2 {
        let words = Arc::new([AtomicI32::new(0), AtomicI32::new(0)]);
        let target = Arc::clone(&words);
        let thread = std::thread::spawn(move || {
            raw(
                &[entry(&target[0], true), entry(&target[1], false)],
                Some(&deadline(5000)),
                CLOCK_MONOTONIC as i32,
            )
        });
        queued(&words[0], 1);
        // Reference setup publishes the private member before the shared one.
        // Observing the first row does not prove the second is registered yet.
        let started = Instant::now();
        let woken = loop {
            let count = wake(&words[selected], selected == 0);
            if count != 0 || started.elapsed() >= Duration::from_secs(5) {
                break count;
            }
            std::thread::yield_now();
        };
        assert_eq!(thread.join().unwrap(), selected as i64);
        assert_eq!(woken, 1);
        assert_eq!(wake(&words[0], true), 0);
        assert_eq!(wake(&words[1], false), 0);
        let entries = [
            entry(&words[0], true),
            Entry {
                value: 1,
                ..entry(&words[1], false)
            },
        ];
        assert_eq!(raw(&entries, None, 0), -i64::from(EAGAIN));
        queued(&words[0], 0);
        assert_eq!(wake(&words[1], false), 0);
    }
}

#[test]
fn private_vector_requeue_follows_new_key_but_keeps_original_index() {
    let words = Arc::new([AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)]);
    let target = Arc::clone(&words);
    let thread = std::thread::spawn(move || {
        raw(
            &[entry(&target[0], true), entry(&target[1], true)],
            Some(&deadline(5000)),
            CLOCK_MONOTONIC as i32,
        )
    });
    queued(&words[1], 1);
    assert_eq!(
        futex_syscall(
            words[1].as_ptr(),
            (FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG) as i32,
            0,
            1usize as _,
            words[2].as_ptr(),
            0
        ),
        1
    );
    assert_eq!(wake(&words[1], true), 0);
    assert_eq!(wake(&words[2], true), 1);
    assert_eq!(thread.join().unwrap(), 1);
    queued(&words[0], 0);
    queued(&words[2], 0);
}

#[test]
fn vector_signal_cleanup_precedes_handler_and_restart_preserves_deadline() {
    static ADDRESS: AtomicUsize = AtomicUsize::new(0);
    static HANDLER_WAKE: AtomicI32 = AtomicI32::new(-1);
    unsafe extern "sysv64" fn handler(_: i32) {
        HANDLER_WAKE.store(
            futex_wake(ADDRESS.load(Ordering::Acquire) as *mut i32, 1, u32::MAX) as i32,
            Ordering::Release,
        );
    }
    let word = Arc::new(AtomicI32::new(0));
    ADDRESS.store(word.as_ptr() as usize, Ordering::Release);
    let action = |flags| signal::Action {
        disposition: signal::Disposition::Handle(handler, 0),
        flags,
        mask: 0,
        restorer: 0,
    };
    let previous = signal::sigaction(signal::SIGUSR2, Some(action(0))).unwrap();
    for restart in [false, true] {
        signal::sigaction(
            signal::SIGUSR2,
            Some(action(if restart { signal::SA_RESTART } else { 0 })),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let target = Arc::clone(&word);
        let thread = std::thread::spawn(move || {
            tx.send(interrupt::current_thread_id()).unwrap();
            raw(
                &[entry(&target, true)],
                Some(&deadline(150)),
                CLOCK_MONOTONIC as i32,
            )
        });
        let tid = rx.recv().unwrap();
        queued(&word, 1);
        signal::raise_thread_signal(tid, signal::SIGUSR2).unwrap();
        assert_eq!(
            thread.join().unwrap(),
            -i64::from(if restart { ETIMEDOUT } else { EINTR })
        );
        assert_eq!(HANDLER_WAKE.load(Ordering::Acquire), 0);
        queued(&word, 0);
    }
    signal::sigaction(signal::SIGUSR2, Some(previous)).unwrap();
}

#[test]
fn spurious_interrupt_rechecks_values_without_reporting_a_wake() {
    let word = Arc::new(AtomicI32::new(0));
    let (tx, rx) = mpsc::channel();
    let target = Arc::clone(&word);
    let thread = std::thread::spawn(move || {
        tx.send(interrupt::current_thread_id()).unwrap();
        raw(
            &[entry(&target, true)],
            Some(&deadline(5000)),
            CLOCK_MONOTONIC as i32,
        )
    });
    let tid = rx.recv().unwrap();
    queued(&word, 1);
    word.store(1, Ordering::SeqCst);
    interrupt::interrupt_thread(tid);
    assert_eq!(thread.join().unwrap(), -i64::from(EAGAIN));
    queued(&word, 0);
}

#[test]
fn key_faults_precede_values_but_private_unmapped_keys_need_no_page_probe() {
    let word = AtomicI32::new(7);
    for private in [false, true] {
        let first = Entry {
            value: 8,
            ..entry(&word, private)
        };
        let second = Entry {
            address: 4,
            ..entry(&word, private)
        };
        // An unflagged key resolves its backing page first. PRIVATE only
        // checks the aligned user VA; its later value read is never reached.
        assert_eq!(
            raw(&[first, second], None, 999),
            -i64::from(if private { EAGAIN } else { EFAULT })
        );
        assert_eq!(wake(&word, private), 0);
    }
}

#[test]
#[ignore = "paired release benchmark; tools/benchmark-futex-queues.py --case waitv-background"]
fn benchmark_vector_registration() {
    let words: Vec<_> = (0..MAX).map(|_| AtomicI32::new(0)).collect();
    // Live unrelated records make queue copying/death probes part of the
    // workload. They remain outside every tested memory key.
    let key = crate::futex::Key::anonymous([803, u64::from(std::process::id()), 0], 0);
    let mut background = crate::futex::WaitGroup::new().unwrap();
    for index in 0..64 {
        background.enqueue(index, 0, || Ok((key, 0))).unwrap();
    }
    let past = KernelTimespec::default();
    let measure = |count: usize, repetitions: usize, private: bool| {
        let entries: Vec<_> = words[..count]
            .iter()
            .map(|word| entry(word, private))
            .collect();
        let started = Instant::now();
        for _ in 0..repetitions {
            assert_eq!(
                raw(&entries, Some(&past), CLOCK_MONOTONIC as i32),
                -i64::from(ETIMEDOUT)
            );
        }
        started.elapsed().as_nanos()
    };
    let shared16 = measure(16, 100, false);
    let shared128 = measure(128, 20, false);
    let private128 = measure(128, 100, true);
    assert_eq!(background.finish(true), Ok(None));
    println!(
        "FUTEX_VECTOR_BENCH {{\"optimized\":{},\"shared16_100_ns\":{shared16},\"shared128_20_ns\":{shared128},\"private128_100_ns\":{private128}}}",
        crate::futex::optimized()
    );
}
