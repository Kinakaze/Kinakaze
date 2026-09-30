use super::*;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

fn entry(word: &AtomicI32, private: bool) -> Entry {
    Entry {
        value: word.load(Ordering::SeqCst) as u32 as u64,
        address: word.as_ptr() as u64,
        flags: 2 | if private { 0x80 } else { 0 },
        reserved: 0,
    }
}

fn raw(entries: &[Entry; 2], wake: i32, transfer: i32) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            456,
            entries.as_ptr() as u64,
            0,
            wake as u64,
            transfer as u64,
            0,
            0,
        )
    }
}

fn queued(word: &AtomicI32, private: bool, count: usize) {
    let started = Instant::now();
    loop {
        let actual = if private {
            let address = word.as_ptr() as usize;
            let local = futex_queues()
                .lock()
                .unwrap()
                .get(&address)
                .map_or(0, VecDeque::len);
            local
                + if FUTEX_PRIVATE_BRIDGED.load(Ordering::Acquire) {
                    crate::futex::count_for_test(
                        crate::futex::Key::flagged_private(address).unwrap(),
                    )
                } else {
                    0
                }
        } else {
            crate::futex::count_for_test(crate::fdio::futex_key(word.as_ptr() as usize).unwrap())
        };
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

fn spawn(
    word: &Arc<AtomicI32>,
    private: bool,
    mask: u32,
    nanos: i64,
) -> std::thread::JoinHandle<i64> {
    let word = Arc::clone(word);
    std::thread::spawn(move || {
        let timeout = KernelTimespec {
            tv_sec: nanos / 1_000_000_000,
            tv_nsec: nanos % 1_000_000_000,
        };
        futex_wait(
            FutexAddress::resolve(word.as_ptr(), private).unwrap(),
            0,
            &timeout,
            false,
            false,
            mask,
        )
    })
}

fn wake(word: &AtomicI32, private: bool, mask: u32) -> i64 {
    futex_wake(
        FutexAddress::resolve(word.as_ptr(), private).unwrap(),
        1,
        mask,
    )
}

#[test]
fn requeue2_checks_descriptors_before_counts_and_preserves_comparison_order() {
    let source = AtomicI32::new(7);
    let target = AtomicI32::new(13);
    let valid = [entry(&source, true), entry(&target, false)];
    assert_eq!(syscall(0, 0, 0, 0), -i64::from(EINVAL));
    assert_eq!(syscall(1, 1, -1, -1), -i64::from(EINVAL));
    assert_eq!(syscall(1, 0, -1, -1), -i64::from(EFAULT));
    assert_eq!(raw(&valid, -1, 0), -i64::from(EINVAL));
    assert_eq!(raw(&valid, 0, -1), -i64::from(EINVAL));
    for command in [FUTEX_REQUEUE, FUTEX_CMP_REQUEUE] {
        assert_eq!(
            futex_syscall(
                4usize as _,
                command as i32,
                u32::MAX,
                core::ptr::null(),
                8usize as _,
                0
            ),
            -i64::from(EINVAL)
        );
        assert_eq!(
            futex_syscall(
                4usize as _,
                command as i32,
                0,
                u32::MAX as usize as _,
                8usize as _,
                0
            ),
            -i64::from(EINVAL)
        );
    }
    assert_eq!(
        futex_syscall(
            4usize as _,
            FUTEX_WAKE_BITSET as i32,
            1,
            core::ptr::null(),
            core::ptr::null_mut(),
            0
        ),
        -i64::from(EINVAL)
    );
    for flags in [0, 1, 3, 6, 0x102] {
        let bad = [valid[0], Entry { flags, ..valid[1] }];
        assert_eq!(raw(&bad, 0, 1), -i64::from(EINVAL));
    }
    for bad in [
        Entry {
            reserved: 1,
            ..valid[1]
        },
        Entry {
            value: 1 << 32,
            ..valid[1]
        },
    ] {
        assert_eq!(raw(&[valid[0], bad], 0, 1), -i64::from(EINVAL));
    }
    let mismatch = Entry {
        value: 8,
        ..valid[0]
    };
    assert_eq!(
        raw(
            &[
                mismatch,
                Entry {
                    address: 4,
                    ..valid[1]
                }
            ],
            0,
            1
        ),
        -i64::from(EFAULT)
    );
    assert_eq!(raw(&[mismatch, valid[1]], 0, 0), -i64::from(EAGAIN));
    assert_eq!(raw(&valid, 0, 0), 0);
    assert_eq!(
        raw(
            &[
                valid[0],
                Entry {
                    value: 99,
                    ..valid[1]
                }
            ],
            0,
            1
        ),
        0
    );
}

#[test]
fn requeue2_all_four_private_shared_directions_preserve_masks_and_wake_counts() {
    for from_private in [true, false] {
        for to_private in [true, false] {
            let source = Arc::new(AtomicI32::new(0));
            let target = AtomicI32::new(0);
            let first = spawn(&source, from_private, 1, 5_000_000_000);
            queued(&source, from_private, 1);
            let second = spawn(&source, from_private, 2, 5_000_000_000);
            queued(&source, from_private, 2);
            assert_eq!(
                raw(
                    &[entry(&source, from_private), entry(&target, to_private)],
                    1,
                    1
                ),
                2
            );
            assert_eq!(first.join().unwrap(), 0);
            assert_eq!(wake(&source, from_private, u32::MAX), 0);
            assert_eq!(wake(&target, to_private, 1), 0);
            assert_eq!(wake(&target, to_private, 2), 1);
            assert_eq!(second.join().unwrap(), 0);
            queued(&source, from_private, 0);
            queued(&target, to_private, 0);
        }
    }
}

#[test]
fn migration_to_same_virtual_address_changes_only_the_private_tag() {
    let word = Arc::new(AtomicI32::new(0));
    for private in [true, false] {
        let child = spawn(&word, private, u32::MAX, 5_000_000_000);
        queued(&word, private, 1);
        assert_eq!(
            raw(&[entry(&word, private), entry(&word, !private)], 0, 1),
            1
        );
        assert_eq!(wake(&word, private, u32::MAX), 0);
        assert_eq!(wake(&word, !private, u32::MAX), 1);
        assert_eq!(child.join().unwrap(), 0);
    }
}

#[test]
fn migrated_waiter_can_move_back_and_legacy_wake_op_observes_its_token() {
    let source = Arc::new(AtomicI32::new(0));
    let middle = AtomicI32::new(0);
    let target = AtomicI32::new(0);
    let child = spawn(&source, true, 4, 5_000_000_000);
    queued(&source, true, 1);
    assert_eq!(raw(&[entry(&source, true), entry(&middle, false)], 0, 1), 1);
    assert_eq!(raw(&[entry(&middle, false), entry(&target, true)], 0, 1), 1);
    assert_eq!(wake(&source, true, u32::MAX), 0);
    assert_eq!(wake(&middle, false, u32::MAX), 0);
    // SET 3, compare old == 0, wake one from the bridged private target.
    assert_eq!(
        futex_syscall(
            target.as_ptr(),
            (FUTEX_WAKE_OP | FUTEX_PRIVATE_FLAG) as i32,
            1,
            1usize as _,
            middle.as_ptr(),
            3 << 12
        ),
        1
    );
    assert_eq!(middle.load(Ordering::SeqCst), 3);
    assert_eq!(child.join().unwrap(), 0);
}

#[test]
fn migrated_timeout_cancels_target_token_without_claiming_success() {
    let source = Arc::new(AtomicI32::new(0));
    let target = AtomicI32::new(0);
    let child = spawn(&source, true, u32::MAX, 150_000_000);
    queued(&source, true, 1);
    assert_eq!(raw(&[entry(&source, true), entry(&target, false)], 0, 1), 1);
    assert_eq!(child.join().unwrap(), -i64::from(ETIMEDOUT));
    assert_eq!(wake(&target, false, u32::MAX), 0);
    queued(&source, true, 0);
}

#[test]
fn private_vector_member_follows_shared_migration_and_keeps_index() {
    let words = Arc::new([AtomicI32::new(0), AtomicI32::new(0)]);
    let target = AtomicI32::new(0);
    let child_words = Arc::clone(&words);
    let child = std::thread::spawn(move || {
        let entries = [entry(&child_words[0], true), entry(&child_words[1], true)];
        let mut end = KernelTimespec::default();
        assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut end) }, 0);
        end.tv_sec += 5;
        unsafe {
            kinakaze_abi_syscall_raw(
                449,
                entries.as_ptr() as u64,
                2,
                0,
                &raw const end as u64,
                CLOCK_MONOTONIC as u64,
                0,
            )
        }
    });
    queued(&words[1], true, 1);
    assert_eq!(
        raw(&[entry(&words[1], true), entry(&target, false)], 0, 1),
        1
    );
    assert_eq!(wake(&target, false, u32::MAX), 1);
    assert_eq!(child.join().unwrap(), 1);
    assert_eq!(wake(&words[0], true, u32::MAX), 0);
}

#[test]
fn migrated_wait_ignores_abandoned_notifications_and_retains_target_queue() {
    for private in [true, false] {
        for vector in [false, true] {
            for mode in [
                "before-commit",
                "partial-bank",
                "after-commit",
                "other-domain",
            ] {
                let words = Arc::new([AtomicI32::new(0), AtomicI32::new(0)]);
                let target = AtomicI32::new(0);
                let target_words = Arc::clone(&words);
                let (tx, rx) = std::sync::mpsc::channel();
                let child = std::thread::spawn(move || {
                    let result = if vector {
                        let entries = [
                            entry(&target_words[0], private),
                            entry(&target_words[1], private),
                        ];
                        let mut end = KernelTimespec::default();
                        assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut end) }, 0);
                        end.tv_sec += 15;
                        unsafe {
                            kinakaze_abi_syscall_raw(
                                449,
                                entries.as_ptr() as u64,
                                2,
                                0,
                                &raw const end as u64,
                                CLOCK_MONOTONIC as u64,
                                0,
                            )
                        }
                    } else {
                        let timeout = KernelTimespec {
                            tv_sec: 15,
                            tv_nsec: 0,
                        };
                        futex_wait(
                            FutexAddress::resolve(target_words[1].as_ptr(), private).unwrap(),
                            0,
                            &timeout,
                            false,
                            false,
                            u32::MAX,
                        )
                    };
                    tx.send(result).unwrap();
                });
                queued(&words[1], private, 1);
                assert_eq!(
                    raw(&[entry(&words[1], private), entry(&target, !private)], 0, 1),
                    1
                );
                let key = key(FutexAddress::resolve(target.as_ptr(), !private).unwrap()).unwrap();
                let output = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "futex::tests::transaction_child",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("KINAKAZE_FUTEX_TEST_MODE", mode)
                    .env(
                        "KINAKAZE_FUTEX_TEST_KEY",
                        crate::futex::key_parts_for_test(key)
                            .map(|part| part.to_string())
                            .join(","),
                    )
                    .creation_flags(0x0800_0000)
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(if mode == "other-domain" { 0 } else { 77 }),
                    "{mode}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                if mode != "after-commit" {
                    assert!(
                        rx.recv_timeout(Duration::from_millis(30)).is_err(),
                        "false wake: private={private}, vector={vector}, {mode}"
                    );
                    queued(&target, !private, 1);
                    queued(&words[1], private, 0);
                    assert_eq!(wake(&target, !private, u32::MAX), 1);
                }
                assert_eq!(
                    rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                    if vector { 1 } else { 0 }
                );
                child.join().unwrap();
                queued(&target, !private, 0);
                queued(&words[0], private, 0);
            }
        }
    }
}

#[test]
fn migrated_signal_retires_target_before_handler_and_restart_uses_original_address() {
    use kinakaze_vfs::{EINTR, interrupt, signal};
    static SOURCE: AtomicUsize = AtomicUsize::new(0);
    static TARGET: AtomicUsize = AtomicUsize::new(0);
    static HANDLER_WAKE: AtomicI32 = AtomicI32::new(-1);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "sysv64" fn handler(_: i32) {
        let source = FutexAddress::resolve(SOURCE.load(Ordering::Acquire) as _, true).unwrap();
        let target = FutexAddress::resolve(TARGET.load(Ordering::Acquire) as _, false).unwrap();
        HANDLER_WAKE.store(
            (futex_wake(source, 1, u32::MAX) + futex_wake(target, 1, u32::MAX)) as i32,
            Ordering::Release,
        );
        CALLS.fetch_add(1, Ordering::Release);
    }
    let source = Arc::new(AtomicI32::new(0));
    let target = AtomicI32::new(0);
    SOURCE.store(source.as_ptr() as usize, Ordering::Release);
    TARGET.store(target.as_ptr() as usize, Ordering::Release);
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
        let target_source = Arc::clone(&source);
        let (tx, rx) = std::sync::mpsc::channel();
        let child = std::thread::spawn(move || {
            tx.send(interrupt::current_thread_id()).unwrap();
            let timeout = KernelTimespec {
                tv_sec: 0,
                tv_nsec: 300_000_000,
            };
            futex_wait(
                FutexAddress::resolve(target_source.as_ptr(), true).unwrap(),
                0,
                &timeout,
                false,
                false,
                u32::MAX,
            )
        });
        let tid = rx.recv().unwrap();
        queued(&source, true, 1);
        assert_eq!(raw(&[entry(&source, true), entry(&target, false)], 0, 1), 1);
        signal::raise_thread_signal(tid, signal::SIGUSR2).unwrap();
        if restart {
            queued(&source, true, 1);
            queued(&target, false, 0);
        }
        assert_eq!(
            child.join().unwrap(),
            -i64::from(if restart { ETIMEDOUT } else { EINTR })
        );
        assert_eq!(HANDLER_WAKE.load(Ordering::Acquire), 0);
        queued(&source, true, 0);
        queued(&target, false, 0);
    }
    assert_eq!(CALLS.load(Ordering::Acquire), 2);
    signal::sigaction(signal::SIGUSR2, Some(previous)).unwrap();
}

#[test]
fn destination_waiters_precede_transfers_and_later_private_waiters() {
    let source = Arc::new(AtomicI32::new(0));
    let target = Arc::new(AtomicI32::new(0));
    let first = spawn(&target, true, 1, 5_000_000_000);
    queued(&target, true, 1);
    let second = spawn(&source, false, 2, 5_000_000_000);
    queued(&source, false, 1);
    assert_eq!(raw(&[entry(&source, false), entry(&target, true)], 0, 1), 1);
    let third = spawn(&target, true, 4, 5_000_000_000);
    queued(&target, true, 3);
    assert_eq!(wake(&target, true, u32::MAX), 1);
    assert_eq!(first.join().unwrap(), 0);
    assert_eq!(wake(&target, true, 1), 0);
    assert_eq!(wake(&target, true, u32::MAX), 1);
    assert_eq!(second.join().unwrap(), 0);
    assert_eq!(wake(&target, true, 2), 0);
    assert_eq!(wake(&target, true, u32::MAX), 1);
    assert_eq!(third.join().unwrap(), 0);
    queued(&target, true, 0);
}

#[test]
fn private_requeue_and_wake_resolve_unmapped_keys_without_reading_them() {
    let source = Arc::new(AtomicI32::new(0));
    for source_private in [true, false] {
        let child = spawn(&source, source_private, u32::MAX, 5_000_000_000);
        queued(&source, source_private, 1);
        let entries = [
            entry(&source, source_private),
            Entry {
                value: 99,
                address: 4,
                flags: 0x82,
                reserved: 0,
            },
        ];
        assert_eq!(raw(&entries, 0, 1), 1);
        assert_eq!(wake(&source, source_private, u32::MAX), 0);
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(454, 4, u32::MAX as u64, 1, 0x82, 0, 0) },
            1
        );
        assert_eq!(child.join().unwrap(), 0);
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(454, 4, u32::MAX as u64, 1, 0x82, 0, 0) },
            0
        );
        assert_eq!(
            raw(
                &[
                    Entry {
                        value: 1,
                        ..entries[0]
                    },
                    entries[1]
                ],
                0,
                1
            ),
            -i64::from(EAGAIN)
        );
    }
    assert_eq!(
        unsafe { kinakaze_abi_syscall_raw(454, 0, u32::MAX as u64, 1, 0x82, 0, 0) },
        0
    );
    assert_eq!(
        unsafe { kinakaze_abi_syscall_raw(454, u64::MAX - 3, 1, 1, 0x82, 0, 0) },
        -i64::from(EFAULT)
    );
    // A non-comparing legacy requeue also only looks up private keys.
    assert_eq!(
        futex_syscall(
            4usize as _,
            (FUTEX_REQUEUE | FUTEX_PRIVATE_FLAG) as i32,
            0,
            1usize as _,
            8usize as _,
            0
        ),
        0
    );
    assert_eq!(
        futex_syscall(
            4usize as _,
            (FUTEX_CMP_REQUEUE | FUTEX_PRIVATE_FLAG) as i32,
            0,
            1usize as _,
            8usize as _,
            0
        ),
        -i64::from(EFAULT)
    );
}

#[test]
#[ignore = "paired release benchmark; tools/benchmark-futex-queues.py --case requeue"]
fn benchmark_requeue2_paths() {
    // Time the actual raw 456 entry. Live wait groups provide committed value
    // records, without including guest thread scheduling in these timings.
    let source = AtomicI32::new(0);
    let target = AtomicI32::new(0);
    let private = [entry(&source, true), entry(&target, true)];
    let shared = [entry(&source, false), entry(&target, false)];
    let began = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(raw(&private, 0, 128), 0);
    }
    let private_empty = began.elapsed().as_nanos();
    let began = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(raw(&shared, 0, 128), 0);
    }
    let shared_empty = began.elapsed().as_nanos();
    let indices: Vec<_> = (0..128).collect();
    let background_key = crate::futex::Key::anonymous([17, 23, std::process::id() as u64], 0);
    let background: Vec<_> = (0..16)
        .map(|_| {
            crate::futex::WaitGroup::enqueue_batch(&indices, || Ok(vec![background_key; 128]))
                .unwrap()
        })
        .collect();
    let began = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(raw(&shared, 0, 128), 0);
    }
    let shared_unrelated = began.elapsed().as_nanos();
    let source_key = crate::fdio::futex_key(source.as_ptr() as usize).unwrap();
    let target_key = crate::fdio::futex_key(target.as_ptr() as usize).unwrap();
    let members =
        crate::futex::WaitGroup::enqueue_batch(&indices, || Ok(vec![source_key; 128])).unwrap();
    let reverse = [shared[1], shared[0]];
    let began = Instant::now();
    for index in 0..100 {
        assert_eq!(
            raw(if index % 2 == 0 { &shared } else { &reverse }, 0, 128),
            128
        );
    }
    let transfers = began.elapsed().as_nanos();
    assert_eq!(members.finish(true).unwrap(), None);
    drop(members);
    drop(background);
    assert_eq!(crate::futex::count_for_test(source_key), 0);
    assert_eq!(crate::futex::count_for_test(target_key), 0);
    assert_eq!(crate::futex::count_for_test(background_key), 0);
    println!(
        "REQUEUE_BENCH {{\"optimized\":{},\"private_empty_10000_ns\":{private_empty},\"shared_empty_10000_ns\":{shared_empty},\"shared_unrelated_10000_ns\":{shared_unrelated},\"transfer128_100_ns\":{transfers}}}",
        crate::futex::optimized()
    );
}

#[test]
fn pending_default_ignored_sigchld_does_not_unqueue_scalar_or_vector_waits() {
    use kinakaze_vfs::signal;
    let previous = signal::sigaction(signal::SIGCHLD, Some(signal::Action::default())).unwrap();
    for private in [true, false] {
        for vector in [true, false] {
            signal::raise_signal(signal::SIGCHLD).unwrap();
            let source = Arc::new(AtomicI32::new(0));
            let child_source = Arc::clone(&source);
            let child = std::thread::spawn(move || {
                if vector {
                    let entries = [entry(&child_source, private); 64];
                    let mut end = KernelTimespec::default();
                    assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut end) }, 0);
                    end.tv_sec += 5;
                    // Direct entry leaves queued SIGCHLD for the wait loop,
                    // rather than an outer syscall boundary's delivery gate.
                    super::super::futex_vector::waitv(
                        entries.as_ptr() as usize,
                        64,
                        0,
                        &end,
                        CLOCK_MONOTONIC as i32,
                    )
                } else {
                    let timeout = KernelTimespec {
                        tv_sec: 5,
                        tv_nsec: 0,
                    };
                    futex_wait(
                        FutexAddress::resolve(child_source.as_ptr(), private).unwrap(),
                        0,
                        &timeout,
                        false,
                        false,
                        u32::MAX,
                    )
                }
            });
            queued(&source, private, if vector { 64 } else { 1 });
            assert_eq!(wake(&source, private, u32::MAX), 1);
            assert_eq!(child.join().unwrap(), 0);
            queued(&source, private, 0);
        }
    }
    signal::sigaction(signal::SIGCHLD, Some(previous)).unwrap();
}
