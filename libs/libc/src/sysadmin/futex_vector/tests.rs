use super::*;
use core::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32};

fn entry(word: &AtomicI32, private: bool) -> Entry {
    Entry {
        value: word.load(Ordering::SeqCst) as u32 as u64,
        address: word.as_ptr() as u64,
        flags: 2 | if private { 0x80 } else { 0 },
        reserved: 0,
    }
}

fn deadline(nanos: u64) -> KernelTimespec {
    let mut now = KernelTimespec::default();
    assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut now) }, 0);
    now.tv_sec += (nanos / 1_000_000_000) as i64;
    now.tv_nsec += (nanos % 1_000_000_000) as i64;
    if now.tv_nsec >= 1_000_000_000 {
        now.tv_sec += 1;
        now.tv_nsec -= 1_000_000_000;
    }
    now
}

fn call(entries: &[Entry], timeout: &KernelTimespec) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            449,
            entries.as_ptr() as u64,
            entries.len() as u64,
            0,
            timeout as *const _ as u64,
            CLOCK_MONOTONIC as u64,
            0,
        )
    }
}

fn queued(word: &AtomicI32, count: usize) {
    let started = Instant::now();
    loop {
        let length = futex_queues()
            .lock()
            .unwrap()
            .get(&(word.as_ptr() as usize))
            .map_or(0, VecDeque::len);
        if length == count {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
}

#[test]
fn waitv_validates_abi_flags_sizes_faults_and_timeout_order() {
    let word = AtomicI32::new(7);
    let valid = entry(&word, true);
    let address = &valid as *const _ as usize;
    let past = KernelTimespec::default();
    for (pointer, count, flags) in [(0, 1, 0), (1, 0, 0), (1, 129, 0), (1, 1, 1)] {
        assert_eq!(
            waitv(pointer, count, flags, ptr::null(), 999),
            -i64::from(EINVAL)
        );
    }
    assert_eq!(waitv(1, 1, 0, ptr::null(), 999), -i64::from(EFAULT));
    assert_eq!(waitv(1, 1, 0, &past, 999), -i64::from(EINVAL));
    assert_eq!(waitv(address, 1, 0, 1usize as _, 1), -i64::from(EFAULT));
    for flags in [0, 1, 3, 6, 0x102, 0x182] {
        let invalid = Entry { flags, ..valid };
        assert_eq!(call(&[invalid], &past), -i64::from(EINVAL));
    }
    for invalid in [
        Entry {
            reserved: 1,
            ..valid
        },
        Entry {
            value: 1 << 32,
            ..valid
        },
    ] {
        assert_eq!(call(&[invalid], &past), -i64::from(EINVAL));
    }
    assert_eq!(
        call(
            &[Entry {
                address: 1,
                ..valid
            }],
            &past
        ),
        -i64::from(EINVAL)
    );
    assert_eq!(
        call(
            &[Entry {
                address: 0,
                ..valid
            }],
            &past
        ),
        -i64::from(EFAULT)
    );
    assert_eq!(
        call(&[Entry { value: 8, ..valid }], &past),
        -i64::from(EAGAIN)
    );
    assert_eq!(call(&[valid], &past), -i64::from(ETIMEDOUT));
    assert_eq!(queued_empty(), 0);
}

fn queued_empty() -> usize {
    futex_queues()
        .lock()
        .unwrap()
        .values()
        .map(VecDeque::len)
        .sum()
}

#[test]
fn waitv_returns_original_index_after_requeue_and_retires_other_entries() {
    let words = Arc::new([AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)]);
    let target = AtomicI32::new(0);
    let child_words = Arc::clone(&words);
    let child = std::thread::spawn(move || {
        let entries: Vec<_> = child_words.iter().map(|w| entry(w, true)).collect();
        call(&entries, &deadline(5_000_000_000))
    });
    queued(&words[2], 1);
    assert_eq!(
        futex_syscall(
            words[1].as_ptr(),
            (FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG) as i32,
            0,
            1usize as _,
            target.as_ptr(),
            0,
        ),
        1
    );
    assert_eq!(futex_wake(words[1].as_ptr(), 1, u32::MAX), 0);
    assert_eq!(futex_wake(target.as_ptr(), 1, u32::MAX), 1);
    assert_eq!(child.join().unwrap(), 1);
    for word in words.iter() {
        assert_eq!(futex_wake(word.as_ptr(), 1, u32::MAX), 0);
    }
    assert_eq!(queued_empty(), 0);
}

#[test]
fn waitv_mixes_private_unflagged_entries_and_preserves_linux_namespaces() {
    let words = Arc::new([AtomicI32::new(0), AtomicI32::new(0), AtomicI32::new(0)]);
    let child_words = Arc::clone(&words);
    let child = std::thread::spawn(move || {
        let entries = [
            entry(&child_words[0], true),
            entry(&child_words[1], false),
            entry(&child_words[2], true),
        ];
        call(&entries, &deadline(5_000_000_000))
    });
    queued(&words[2], 1);
    assert_eq!(futex_wake(words[1].as_ptr(), 1, u32::MAX), 0);
    let shared = FutexAddress::resolve(words[1].as_ptr(), false).unwrap();
    assert_eq!(futex_wake(shared, 1, u32::MAX), 1);
    assert_eq!(child.join().unwrap(), 1);
    assert_eq!(futex_wake(shared, 1, u32::MAX), 0);
    assert_eq!(queued_empty(), 0);
}

#[test]
fn waitv_all_128_unflagged_addresses_share_one_native_event() {
    let words: Arc<Vec<_>> = Arc::new((0..128).map(|_| AtomicI32::new(0)).collect());
    let child_words = Arc::clone(&words);
    let child = std::thread::spawn(move || {
        let entries: Vec<_> = child_words.iter().map(|w| entry(w, false)).collect();
        call(&entries, &deadline(5_000_000_000))
    });
    let last = FutexAddress::resolve(words[127].as_ptr(), false).unwrap();
    let began = Instant::now();
    while futex_wake(last, 1, u32::MAX) == 0 {
        assert!(began.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
    assert_eq!(child.join().unwrap(), 127);
    for word in words.iter() {
        assert_eq!(
            futex_wake(
                FutexAddress::resolve(word.as_ptr(), false).unwrap(),
                1,
                u32::MAX
            ),
            0
        );
    }
}

#[test]
fn waitv_mismatch_and_timeout_roll_back_all_prior_registrations() {
    let words = [AtomicI32::new(7), AtomicI32::new(8), AtomicI32::new(9)];
    for private in [false, true] {
        let mut entries: Vec<_> = words.iter().map(|w| entry(w, private)).collect();
        entries[2].value = 17;
        assert_eq!(call(&entries, &deadline(5_000_000_000)), -i64::from(EAGAIN));
        entries[2].value = 9;
        assert_eq!(
            call(&entries, &KernelTimespec::default()),
            -i64::from(ETIMEDOUT)
        );
        for word in &words {
            assert_eq!(
                futex_wake(
                    FutexAddress::resolve(word.as_ptr(), private).unwrap(),
                    1,
                    u32::MAX
                ),
                0
            );
        }
    }
}

#[test]
fn waitv_duplicate_addresses_are_independent_entries() {
    let word = Arc::new(AtomicI32::new(0));
    let target = Arc::clone(&word);
    let child = std::thread::spawn(move || {
        let entries = [entry(&target, true); 128];
        call(&entries, &deadline(5_000_000_000))
    });
    queued(&word, 128);
    assert_eq!(futex_wake(word.as_ptr(), 1, u32::MAX), 1);
    assert_eq!(child.join().unwrap(), 0);
    assert_eq!(queued_empty(), 0);
}

#[test]
fn waitv_signals_unqueue_before_handler_and_restart_keeps_deadline() {
    static CALLS: AtomicU32 = AtomicU32::new(0);
    static PRIVATE: AtomicUsize = AtomicUsize::new(0);
    static SHARED: AtomicUsize = AtomicUsize::new(0);
    static HANDLER_WAKE: AtomicI32 = AtomicI32::new(-1);
    unsafe extern "sysv64" fn handler(_: i32) {
        let first = futex_wake(PRIVATE.load(Ordering::Acquire) as *mut i32, 1, u32::MAX);
        let second = futex_wake(
            FutexAddress::resolve(SHARED.load(Ordering::Acquire) as _, false).unwrap(),
            1,
            u32::MAX,
        );
        HANDLER_WAKE.store((first + second) as i32, Ordering::Release);
        CALLS.fetch_add(1, Ordering::AcqRel);
    }
    let words = Arc::new([AtomicI32::new(0), AtomicI32::new(0)]);
    PRIVATE.store(words[1].as_ptr() as usize, Ordering::Release);
    SHARED.store(words[0].as_ptr() as usize, Ordering::Release);
    let action = |flags| signal::Action {
        disposition: signal::Disposition::Handle(handler, 0),
        flags,
        mask: 0,
        restorer: 0,
    };
    let previous = signal::sigaction(signal::SIGUSR2, Some(action(0))).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let target = Arc::clone(&words);
    let child = std::thread::spawn(move || {
        tx.send(interrupt::current_thread_id()).unwrap();
        call(
            &[entry(&target[0], false), entry(&target[1], true)],
            &deadline(5_000_000_000),
        )
    });
    let tid = rx.recv().unwrap();
    queued(&words[1], 1);
    signal::raise_thread_signal(tid, signal::SIGUSR2).unwrap();
    assert_eq!(child.join().unwrap(), -i64::from(EINTR));
    assert_eq!(HANDLER_WAKE.load(Ordering::Acquire), 0);

    signal::sigaction(signal::SIGUSR2, Some(action(signal::SA_RESTART))).unwrap();
    let ready = Arc::new(AtomicBool::new(false));
    let target_ready = Arc::clone(&ready);
    let target = Arc::clone(&words);
    let (tx, rx) = std::sync::mpsc::channel();
    let child = std::thread::spawn(move || {
        let end = deadline(80_000_000);
        tx.send(interrupt::current_thread_id()).unwrap();
        while !target_ready.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        call(&[entry(&target[0], false), entry(&target[1], true)], &end)
    });
    signal::raise_thread_signal(rx.recv().unwrap(), signal::SIGUSR2).unwrap();
    ready.store(true, Ordering::Release);
    assert_eq!(child.join().unwrap(), -i64::from(ETIMEDOUT));
    assert_eq!(CALLS.load(Ordering::Acquire), 2);
    assert_eq!(HANDLER_WAKE.load(Ordering::Acquire), 0);
    signal::sigaction(signal::SIGUSR2, Some(previous)).unwrap();
    assert_eq!(queued_empty(), 0);
}

#[test]
#[ignore = "paired release benchmark; tools/benchmark-futex-queues.py --case waitv"]
fn benchmark_waitv_registration() {
    // Actual raw syscall dispatch, with an expired absolute deadline: measure
    // complete vector registration/retirement, not scheduler latency. Every
    // invocation checks the result and leaves no observable wait records.
    let words: Vec<_> = (0..128).map(|_| AtomicI32::new(0)).collect();
    let past = KernelTimespec::default();
    let measure = |entries: &[Entry], iterations| {
        let began = Instant::now();
        for _ in 0..iterations {
            assert_eq!(call(entries, &past), -i64::from(ETIMEDOUT));
        }
        for entry in entries {
            let address =
                FutexAddress::resolve(entry.address as _, entry.flags & 0x80 != 0).unwrap();
            assert_eq!(futex_wake(address, 1, u32::MAX), 0);
        }
        began.elapsed().as_nanos()
    };
    let private: Vec<_> = words[..8].iter().map(|w| entry(w, true)).collect();
    let shared: Vec<_> = words.iter().map(|w| entry(w, false)).collect();
    let mixed: Vec<_> = words[..32]
        .iter()
        .enumerate()
        .map(|(i, w)| entry(w, i % 2 == 0))
        .collect();
    let private_8_1000_ns = measure(&private, 1000);
    let shared_128_256_ns = measure(&shared, 256);
    let mixed_32_1000_ns = measure(&mixed, 1000);
    println!(
        "WAITV_BENCH {{\"optimized\":{},\"private_8_1000_ns\":{private_8_1000_ns},\"shared_128_256_ns\":{shared_128_256_ns},\"mixed_32_1000_ns\":{mixed_32_1000_ns}}}",
        crate::futex::optimized()
    );
}
