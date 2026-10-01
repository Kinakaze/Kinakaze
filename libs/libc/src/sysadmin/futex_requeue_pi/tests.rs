use super::*;
use core::sync::atomic::AtomicU32;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn raw(
    source: usize,
    command: u32,
    private: bool,
    value: u32,
    fourth: usize,
    target: usize,
    comparison: u32,
) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            202,
            source as _,
            u64::from(command | if private { 128 } else { 0 }),
            u64::from(value),
            fourth as _,
            target as _,
            u64::from(comparison),
        )
    }
}
fn count(word: &AtomicU32, private: bool) -> usize {
    crate::futex::count_for_test(
        futex_requeue::key(FutexAddress::resolve(word.as_ptr().cast(), private).unwrap()).unwrap(),
    )
}
fn queued(word: &AtomicU32, private: bool, amount: usize) {
    let started = Instant::now();
    while count(word, private) != amount {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "expected {amount} rows, got {}",
            count(word, private)
        );
        std::thread::yield_now();
    }
}
fn lock(word: &AtomicU32, private: bool) -> i64 {
    raw(word.as_ptr() as usize, 13, private, 0, 0, 0, 0)
}
fn unlock(word: &AtomicU32, private: bool) -> i64 {
    raw(word.as_ptr() as usize, 7, private, 0, 0, 0, 0)
}

#[test]
fn pi_requeue_empty_and_argument_error_order() {
    for private in [false, true] {
        let source = AtomicU32::new(0);
        let target = AtomicU32::new(0);
        let a = source.as_ptr() as usize;
        let b = target.as_ptr() as usize;
        assert_eq!(raw(a, 12, private, 1, 0, b, 0), 0);
        for wake in [0, 2, u32::MAX] {
            assert_eq!(raw(1, 12, private, wake, 0, 0, 0), -i64::from(EINVAL));
        }
        assert_eq!(
            raw(1, 12, private, 1, u32::MAX as usize, 0, 0),
            -i64::from(EINVAL)
        );
        assert_eq!(raw(0, 12, private, 1, 0, 0, 0), -i64::from(EINVAL));
        assert_eq!(raw(a, 11, private, 0, 1, a, 0), -i64::from(EFAULT));
        assert_eq!(raw(a, 11, private, 0, 0, a, 0), -i64::from(EINVAL));
        let zero = KernelTimespec::default();
        assert_eq!(
            raw(a, 11, private, 0, &raw const zero as usize, b, 0),
            -i64::from(ETIMEDOUT)
        );
        assert_eq!(
            raw(
                a,
                11 | FUTEX_CLOCK_REALTIME,
                private,
                0,
                &raw const zero as usize,
                b,
                0
            ),
            -i64::from(ETIMEDOUT)
        );
        if private {
            assert_eq!(raw(a, 12, true, 1, 0, 0, 1), -i64::from(EAGAIN));
            assert_eq!(raw(a, 12, true, 1, 0, 0, 0), -i64::from(EFAULT));
            assert_eq!(raw(a, 11, true, 1, 0, 0, 0), -i64::from(EAGAIN));
            assert_eq!(
                raw(a, 11, true, 0, &raw const zero as usize, 0, 0),
                -i64::from(ETIMEDOUT)
            );
        }
    }
}

#[test]
fn pi_requeue_proxy_free_acquires_for_waiter_and_preserves_requested_waiters_bit() {
    for private in [false, true] {
        for transfer in [0, 3] {
            let source = Arc::new(AtomicU32::new(0));
            let target = Arc::new(AtomicU32::new(0x8000_0000));
            let a = Arc::clone(&source);
            let b = Arc::clone(&target);
            let (tx, rx) = mpsc::channel();
            let (go_tx, go_rx) = mpsc::channel();
            let waiter = std::thread::spawn(move || {
                let tid = current_tid() as u32;
                assert_eq!(
                    raw(
                        a.as_ptr() as usize,
                        11,
                        private,
                        0,
                        0,
                        b.as_ptr() as usize,
                        0
                    ),
                    0
                );
                tx.send((tid, b.load(Ordering::Acquire))).unwrap();
                go_rx.recv().unwrap();
                assert_eq!(unlock(&b, private), 0);
            });
            queued(&source, private, 1);
            assert_eq!(
                raw(
                    source.as_ptr() as usize,
                    12,
                    private,
                    1,
                    transfer,
                    target.as_ptr() as usize,
                    0
                ),
                1
            );
            let (tid, value) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(value, tid | if transfer == 0 { 0 } else { 0x8000_0000 });
            queued(&source, private, 0);
            queued(&target, private, 0);
            go_tx.send(()).unwrap();
            waiter.join().unwrap();
        }
    }
}

#[test]
fn pi_requeue_busy_zero_transfer_still_moves_one_without_acquiring_for_caller() {
    for private in [false, true] {
        let source = Arc::new(AtomicU32::new(0));
        let target = Arc::new(AtomicU32::new(0));
        assert_eq!(lock(&target, private), 0);
        let a = Arc::clone(&source);
        let b = Arc::clone(&target);
        let waiter = std::thread::spawn(move || {
            assert_eq!(
                raw(
                    a.as_ptr() as usize,
                    11,
                    private,
                    0,
                    0,
                    b.as_ptr() as usize,
                    0
                ),
                0
            );
            assert_eq!(
                b.load(Ordering::Acquire) & 0x3fff_ffff,
                current_tid() as u32
            );
            assert_eq!(unlock(&b, private), 0);
        });
        queued(&source, private, 1);
        assert_eq!(
            raw(source.as_ptr() as usize, 1, private, 1, 0, 0, 0),
            -i64::from(EINVAL)
        );
        assert_eq!(
            raw(
                source.as_ptr() as usize,
                3,
                private,
                0,
                1,
                target.as_ptr() as usize,
                0
            ),
            -i64::from(EINVAL)
        );
        assert_eq!(
            raw(
                source.as_ptr() as usize,
                12,
                private,
                1,
                0,
                target.as_ptr() as usize,
                0
            ),
            1
        );
        queued(&source, private, 0);
        queued(&target, private, 1);
        assert_eq!(
            target.load(Ordering::Acquire) & 0x3fff_ffff,
            current_tid() as u32
        );
        assert_eq!(unlock(&target, private), 0);
        waiter.join().unwrap();
    }
}

#[test]
fn pi_requeue_wrong_target_keeps_original_source_registration() {
    let source = Arc::new(AtomicU32::new(0));
    let target = Arc::new(AtomicU32::new(0));
    let wrong = AtomicU32::new(0);
    let a = Arc::clone(&source);
    let b = Arc::clone(&target);
    let waiter = std::thread::spawn(move || {
        assert_eq!(
            raw(a.as_ptr() as usize, 11, true, 0, 0, b.as_ptr() as usize, 0),
            0
        );
        assert_eq!(unlock(&b, true), 0);
    });
    queued(&source, true, 1);
    assert_eq!(
        raw(
            source.as_ptr() as usize,
            12,
            true,
            1,
            0,
            wrong.as_ptr() as usize,
            0
        ),
        -i64::from(EINVAL)
    );
    queued(&source, true, 1);
    assert_eq!(wrong.load(Ordering::Acquire), 0);
    assert_eq!(
        raw(
            source.as_ptr() as usize,
            12,
            true,
            1,
            0,
            target.as_ptr() as usize,
            0
        ),
        1
    );
    waiter.join().unwrap();
}

#[test]
fn pi_requeue_keeps_existing_destination_fifo_ahead_of_older_source_tokens() {
    for private in [false, true] {
        let source = Arc::new(AtomicU32::new(0));
        let target = Arc::new(AtomicU32::new(0));
        assert_eq!(lock(&target, private), 0);
        let (tx, rx) = mpsc::channel();
        let a = Arc::clone(&source);
        let b = Arc::clone(&target);
        let src_tx = tx.clone();
        let source_waiter = std::thread::spawn(move || {
            assert_eq!(
                raw(
                    a.as_ptr() as usize,
                    11,
                    private,
                    0,
                    0,
                    b.as_ptr() as usize,
                    0
                ),
                0
            );
            src_tx.send(1).unwrap();
            assert_eq!(unlock(&b, private), 0);
        });
        queued(&source, private, 1);
        let b = Arc::clone(&target);
        let destination_waiter = std::thread::spawn(move || {
            assert_eq!(lock(&b, private), 0);
            tx.send(0).unwrap();
            assert_eq!(unlock(&b, private), 0);
        });
        queued(&target, private, 1);
        assert_eq!(
            raw(
                source.as_ptr() as usize,
                12,
                private,
                1,
                0,
                target.as_ptr() as usize,
                0
            ),
            1
        );
        queued(&target, private, 2);
        assert_eq!(unlock(&target, private), 0);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 1);
        source_waiter.join().unwrap();
        destination_waiter.join().unwrap();
    }
}

#[test]
fn pi_requeue_batch_budget_and_fifo_cover_both_source_and_target() {
    for private in [false, true] {
        let source = Arc::new(AtomicU32::new(0));
        let target = Arc::new(AtomicU32::new(0));
        assert_eq!(lock(&target, private), 0);
        let (tx, rx) = mpsc::channel();
        let mut workers = Vec::new();
        for index in 0..5 {
            let a = Arc::clone(&source);
            let b = Arc::clone(&target);
            let tx = tx.clone();
            workers.push(std::thread::spawn(move || {
                assert_eq!(
                    raw(
                        a.as_ptr() as usize,
                        11,
                        private,
                        0,
                        0,
                        b.as_ptr() as usize,
                        0
                    ),
                    0
                );
                tx.send(index).unwrap();
                assert_eq!(unlock(&b, private), 0);
            }));
            queued(&source, private, index + 1);
        }
        assert_eq!(
            raw(
                source.as_ptr() as usize,
                12,
                private,
                1,
                2,
                target.as_ptr() as usize,
                0
            ),
            3
        );
        queued(&source, private, 2);
        queued(&target, private, 3);
        assert_eq!(
            raw(
                source.as_ptr() as usize,
                12,
                private,
                1,
                2,
                target.as_ptr() as usize,
                0
            ),
            2
        );
        queued(&source, private, 0);
        queued(&target, private, 5);
        assert_eq!(unlock(&target, private), 0);
        for index in 0..5 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), index);
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }
}

#[test]
fn pi_requeue_later_wrong_target_preserves_earlier_proxy_grant() {
    let source = Arc::new(AtomicU32::new(0));
    let target = Arc::new(AtomicU32::new(0));
    let other = Arc::new(AtomicU32::new(0));
    let (tx, rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let a = Arc::clone(&source);
    let b = Arc::clone(&target);
    let first = std::thread::spawn(move || {
        assert_eq!(
            raw(a.as_ptr() as usize, 11, true, 0, 0, b.as_ptr() as usize, 0),
            0
        );
        tx.send(()).unwrap();
        go_rx.recv().unwrap();
        assert_eq!(unlock(&b, true), 0);
    });
    queued(&source, true, 1);
    let a = Arc::clone(&source);
    let b = Arc::clone(&other);
    let second = std::thread::spawn(move || {
        assert_eq!(
            raw(a.as_ptr() as usize, 11, true, 0, 0, b.as_ptr() as usize, 0),
            0
        );
        assert_eq!(unlock(&b, true), 0);
    });
    queued(&source, true, 2);
    assert_eq!(
        raw(
            source.as_ptr() as usize,
            12,
            true,
            1,
            5,
            target.as_ptr() as usize,
            0
        ),
        -i64::from(EINVAL)
    );
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    queued(&source, true, 1);
    assert_ne!(target.load(Ordering::Acquire) & 0x3fff_ffff, 0);
    assert_eq!(
        raw(
            source.as_ptr() as usize,
            12,
            true,
            1,
            0,
            other.as_ptr() as usize,
            0
        ),
        1
    );
    second.join().unwrap();
    go_tx.send(()).unwrap();
    first.join().unwrap();
}

#[test]
fn pi_requeue_private_proxy_write_fault_restores_source_for_retry() {
    use windows_sys::Win32::System::Memory::*;
    let pointer = unsafe {
        VirtualAlloc(
            core::ptr::null(),
            4096,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    assert!(!pointer.is_null());
    let target_address = pointer as usize;
    let source = Arc::new(AtomicU32::new(0));
    let a = Arc::clone(&source);
    let waiter = std::thread::spawn(move || {
        assert_eq!(
            raw(a.as_ptr() as usize, 11, true, 0, 0, target_address, 0),
            0
        );
        assert_eq!(raw(target_address, 7, true, 0, 0, 0, 0), 0);
    });
    queued(&source, true, 1);
    let mut old = 0;
    assert_ne!(
        unsafe { VirtualProtect(pointer, 4096, PAGE_READONLY, &mut old) },
        0
    );
    assert_eq!(
        raw(source.as_ptr() as usize, 12, true, 1, 0, target_address, 0),
        -i64::from(EFAULT)
    );
    queued(&source, true, 1);
    assert_eq!(unsafe { (pointer as *const u32).read() }, 0);
    assert_ne!(
        unsafe { VirtualProtect(pointer, 4096, PAGE_READWRITE, &mut old) },
        0
    );
    assert_eq!(
        raw(source.as_ptr() as usize, 12, true, 1, 0, target_address, 0),
        1
    );
    waiter.join().unwrap();
    assert_ne!(unsafe { VirtualFree(pointer, 0, MEM_RELEASE) }, 0);
}

fn deadline(milliseconds: i64) -> KernelTimespec {
    let mut value = KernelTimespec::default();
    assert_eq!(unsafe { clock_gettime(1, &raw mut value) }, 0);
    let nanos = value.tv_nsec + milliseconds * 1_000_000;
    value.tv_sec += nanos / 1_000_000_000;
    value.tv_nsec = nanos % 1_000_000_000;
    value
}

#[test]
fn pi_requeue_timeout_before_and_after_transfer_retires_both_phases() {
    for private in [false, true] {
        for moved in [false, true] {
            let source = Arc::new(AtomicU32::new(0));
            let target = Arc::new(AtomicU32::new(0));
            assert_eq!(lock(&target, private), 0);
            let a = Arc::clone(&source);
            let b = Arc::clone(&target);
            let waiter = std::thread::spawn(move || {
                let limit = deadline(200);
                raw(
                    a.as_ptr() as usize,
                    11,
                    private,
                    0,
                    &raw const limit as usize,
                    b.as_ptr() as usize,
                    0,
                )
            });
            queued(&source, private, 1);
            if moved {
                assert_eq!(
                    raw(
                        source.as_ptr() as usize,
                        12,
                        private,
                        1,
                        0,
                        target.as_ptr() as usize,
                        0
                    ),
                    1
                );
                queued(&target, private, 1);
            }
            assert_eq!(waiter.join().unwrap(), -i64::from(ETIMEDOUT));
            queued(&source, private, 0);
            queued(&target, private, 0);
            assert_eq!(unlock(&target, private), 0);
        }
    }
}

#[test]
fn pi_requeue_signal_restarts_source_but_returns_again_after_migration() {
    use core::sync::atomic::AtomicI64;
    static HANDLED: AtomicBool = AtomicBool::new(false);
    static ADDRESS: AtomicUsize = AtomicUsize::new(0);
    static PRIVATE: AtomicBool = AtomicBool::new(false);
    static MUTATE: AtomicUsize = AtomicUsize::new(0);
    static WAKE_RESULT: AtomicI64 = AtomicI64::new(i64::MAX);
    unsafe extern "sysv64" fn handler(_: i32) {
        WAKE_RESULT.store(
            raw(
                ADDRESS.load(Ordering::Acquire),
                1,
                PRIVATE.load(Ordering::Acquire),
                1,
                0,
                0,
                0,
            ),
            Ordering::Release,
        );
        let pointer = MUTATE.load(Ordering::Acquire);
        if pointer != 0 {
            unsafe { (*(pointer as *mut KernelTimespec)).tv_sec = -1 };
        }
        HANDLED.store(true, Ordering::Release);
    }
    let action = |restart| signal::Action {
        disposition: signal::Disposition::Handle(handler, 0),
        flags: if restart { signal::SA_RESTART } else { 0 },
        mask: 0,
        restorer: 0,
    };
    let previous = signal::sigaction(signal::SIGUSR2, Some(action(false))).unwrap();
    for private in [false, true] {
        for restart in [false, true] {
            for (moved, mutate) in [(false, false), (false, true), (true, false)] {
                HANDLED.store(false, Ordering::Release);
                WAKE_RESULT.store(i64::MAX, Ordering::Release);
                PRIVATE.store(private, Ordering::Release);
                signal::sigaction(signal::SIGUSR2, Some(action(restart))).unwrap();
                let source = Arc::new(AtomicU32::new(0));
                let target = Arc::new(AtomicU32::new(0));
                assert_eq!(lock(&target, private), 0);
                ADDRESS.store(
                    if moved {
                        target.as_ptr() as usize
                    } else {
                        source.as_ptr() as usize
                    },
                    Ordering::Release,
                );
                let limit = Box::new(deadline(5000));
                let pointer = &*limit as *const _ as usize;
                MUTATE.store(if mutate { pointer } else { 0 }, Ordering::Release);
                let a = Arc::clone(&source);
                let b = Arc::clone(&target);
                let (tx, rx) = mpsc::channel();
                let waiter = std::thread::spawn(move || {
                    tx.send(interrupt::current_thread_id()).unwrap();
                    let result = raw(
                        a.as_ptr() as usize,
                        11,
                        private,
                        0,
                        pointer,
                        b.as_ptr() as usize,
                        0,
                    );
                    if result == 0 {
                        assert_eq!(unlock(&b, private), 0);
                    }
                    result
                });
                let tid = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                queued(&source, private, 1);
                if moved {
                    assert_eq!(
                        raw(
                            source.as_ptr() as usize,
                            12,
                            private,
                            1,
                            0,
                            target.as_ptr() as usize,
                            0
                        ),
                        1
                    );
                    queued(&target, private, 1);
                }
                signal::raise_thread_signal(tid, signal::SIGUSR2).unwrap();
                let started = Instant::now();
                while !HANDLED.load(Ordering::Acquire) {
                    assert!(started.elapsed() < Duration::from_secs(5));
                    std::thread::yield_now();
                }
                assert_eq!(
                    WAKE_RESULT.load(Ordering::Acquire),
                    0,
                    "handler saw its outer waiter"
                );
                if !moved && !mutate {
                    queued(&source, private, 1);
                    assert_eq!(
                        raw(
                            source.as_ptr() as usize,
                            12,
                            private,
                            1,
                            0,
                            target.as_ptr() as usize,
                            0
                        ),
                        1
                    );
                }
                assert_eq!(unlock(&target, private), 0);
                assert_eq!(
                    waiter.join().unwrap(),
                    if moved {
                        -i64::from(EAGAIN)
                    } else if mutate {
                        -i64::from(EINVAL)
                    } else {
                        0
                    }
                );
                queued(&source, private, 0);
                queued(&target, private, 0);
            }
        }
    }
    signal::sigaction(signal::SIGUSR2, Some(previous)).unwrap();
}
