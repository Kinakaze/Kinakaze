use super::*;
use core::sync::atomic::AtomicU32;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn raw(
    source: usize,
    target: usize,
    command: u32,
    private: bool,
    expected: u32,
    fourth: usize,
    comparison: u32,
) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            202,
            source as _,
            (command | if private { 128 } else { 0 }) as _,
            expected as _,
            fourth as _,
            target as _,
            comparison as _,
        )
    }
}
fn pi(word: &AtomicU32, command: u32, private: bool) -> i64 {
    raw(word.as_ptr() as usize, 0, command, private, 0, 0, 0)
}
fn count(word: &AtomicU32, private: bool, expected: usize) {
    let key =
        futex_requeue::key(FutexAddress::resolve(word.as_ptr().cast(), private).unwrap()).unwrap();
    let start = Instant::now();
    while crate::futex::count_for_test(key) != expected {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "expected {expected} waiters"
        );
        std::thread::yield_now();
    }
}
fn deadline(realtime: bool, milliseconds: i64) -> KernelTimespec {
    let mut time = KernelTimespec::default();
    assert_eq!(
        unsafe {
            crate::time::kinakaze_abi_clock_gettime(
                if realtime { 0 } else { 1 },
                (&raw mut time).cast(),
            )
        },
        0
    );
    time.tv_nsec += milliseconds * 1_000_000;
    time.tv_sec += time.tv_nsec / 1_000_000_000;
    time.tv_nsec %= 1_000_000_000;
    time
}

#[test]
fn requeue_pi_validation_copy_precedence_and_both_absolute_clocks() {
    for private in [false, true] {
        let source = AtomicU32::new(7);
        let target = AtomicU32::new(0);
        let a = source.as_ptr() as usize;
        let b = target.as_ptr() as usize;
        for wake in [0, 2, u32::MAX] {
            assert_eq!(raw(1, 0, FUTEX_CMP_REQUEUE_PI, private, wake, 0, 0), -22);
        }
        assert_eq!(
            raw(a, b, FUTEX_CMP_REQUEUE_PI, private, 1, u32::MAX as usize, 7),
            -22
        );
        assert_eq!(raw(a, a, FUTEX_WAIT_REQUEUE_PI, private, 7, 1, 0), -14);
        assert_eq!(raw(a, a, FUTEX_WAIT_REQUEUE_PI, private, 7, 0, 0), -22);
        assert_eq!(raw(a, a, FUTEX_CMP_REQUEUE_PI, private, 1, 0, 7), -22);
        assert_eq!(raw(a, b, FUTEX_CMP_REQUEUE_PI, private, 1, 0, 6), -11);
        assert_eq!(raw(a, b, FUTEX_CMP_REQUEUE_PI, private, 1, 0, 7), 0);
        for realtime in [false, true] {
            let command = FUTEX_WAIT_REQUEUE_PI | if realtime { 256 } else { 0 };
            let zero = KernelTimespec::default();
            assert_eq!(
                raw(a, b, command, private, 6, &raw const zero as usize, 0),
                -11
            );
            assert_eq!(
                raw(a, b, command, private, 7, &raw const zero as usize, 0),
                -110
            );
            let time = deadline(realtime, 15);
            assert_eq!(
                raw(a, b, command, private, 7, &raw const time as usize, 0),
                -110
            );
            count(&source, private, 0);
            count(&target, private, 0);
        }
        assert_eq!(source.load(Ordering::Acquire), 7);
        assert_eq!(target.load(Ordering::Acquire), 0);
    }
}

#[test]
fn requeue_pi_free_destination_returns_only_with_ownership() {
    for private in [false, true] {
        let pair = Arc::new([AtomicU32::new(9), AtomicU32::new(0)]);
        let child_pair = pair.clone();
        let thread = std::thread::spawn(move || {
            let time = deadline(false, 5000);
            let tid = current_tid() as u32;
            assert_eq!(
                raw(
                    child_pair[0].as_ptr() as usize,
                    child_pair[1].as_ptr() as usize,
                    FUTEX_WAIT_REQUEUE_PI,
                    private,
                    9,
                    &raw const time as usize,
                    0
                ),
                0
            );
            assert_eq!(child_pair[1].load(Ordering::Acquire) & 0x3fffffff, tid);
            assert_eq!(pi(&child_pair[1], FUTEX_UNLOCK_PI, private), 0);
        });
        count(&pair[0], private, 1);
        for command in [FUTEX_WAKE, FUTEX_LOCK_PI2, FUTEX_REQUEUE] {
            assert_eq!(
                raw(
                    pair[0].as_ptr() as usize,
                    pair[1].as_ptr() as usize,
                    command,
                    private,
                    if command == FUTEX_WAKE { 1 } else { 0 },
                    if command == FUTEX_REQUEUE { 1 } else { 0 },
                    0
                ),
                -22
            );
        }
        assert_eq!(
            raw(
                pair[0].as_ptr() as usize,
                pair[1].as_ptr() as usize,
                FUTEX_CMP_REQUEUE_PI,
                private,
                1,
                0,
                9
            ),
            1
        );
        thread.join().unwrap();
        count(&pair[0], private, 0);
        count(&pair[1], private, 0);
        assert_eq!(pair[1].load(Ordering::Acquire), 0);
    }
}

#[test]
fn requeue_pi_destination_fifo_precedes_older_source_tokens() {
    for private in [false, true] {
        let pair = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        assert_eq!(pi(&pair[1], FUTEX_LOCK_PI2, private), 0);
        let (tx, rx) = mpsc::channel();
        let spawn = |index: usize, source: bool| {
            let pair = pair.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let time = deadline(false, 5000);
                let result = if source {
                    raw(
                        pair[0].as_ptr() as usize,
                        pair[1].as_ptr() as usize,
                        FUTEX_WAIT_REQUEUE_PI,
                        private,
                        0,
                        &raw const time as usize,
                        0,
                    )
                } else {
                    pi(&pair[1], FUTEX_LOCK_PI2, private)
                };
                assert_eq!(result, 0);
                tx.send(index).unwrap();
                assert_eq!(pi(&pair[1], FUTEX_UNLOCK_PI, private), 0);
            })
        };
        let older = spawn(1, true);
        count(&pair[0], private, 1);
        let existing = spawn(0, false);
        count(&pair[1], private, 1);
        let newer = spawn(2, true);
        count(&pair[0], private, 2);
        assert_eq!(
            raw(
                pair[0].as_ptr() as usize,
                pair[1].as_ptr() as usize,
                FUTEX_CMP_REQUEUE_PI,
                private,
                1,
                1,
                0
            ),
            2
        );
        count(&pair[1], private, 3);
        assert!(rx.recv_timeout(Duration::from_millis(15)).is_err());
        assert_eq!(pi(&pair[1], FUTEX_UNLOCK_PI, private), 0);
        assert_eq!(
            (0..3)
                .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        for thread in [older, existing, newer] {
            thread.join().unwrap();
        }
        count(&pair[0], private, 0);
        count(&pair[1], private, 0);
    }
}

#[test]
fn requeue_pi_timeout_after_transfer_retires_donation_and_link() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, GetThreadPriority, SetThreadPriority,
    };
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -2) }, 0);
    let pair = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
    assert_eq!(pi(&pair[1], FUTEX_LOCK_PI2, true), 0);
    let child_pair = pair.clone();
    let child = std::thread::spawn(move || {
        assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 2) }, 0);
        let time = deadline(false, 200);
        assert_eq!(
            raw(
                child_pair[0].as_ptr() as usize,
                child_pair[1].as_ptr() as usize,
                FUTEX_WAIT_REQUEUE_PI,
                true,
                0,
                &raw const time as usize,
                0
            ),
            -110
        );
    });
    count(&pair[0], true, 1);
    assert_eq!(
        raw(
            pair[0].as_ptr() as usize,
            pair[1].as_ptr() as usize,
            FUTEX_CMP_REQUEUE_PI,
            true,
            1,
            0,
            0
        ),
        1
    );
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
    child.join().unwrap();
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, -2);
    count(&pair[0], true, 0);
    count(&pair[1], true, 0);
    assert_eq!(pi(&pair[1], FUTEX_UNLOCK_PI, true), 0);
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 0) }, 0);
}

#[test]
fn requeue_pi_wrong_target_preserves_waiter_and_can_retry() {
    for private in [false, true] {
        let pair = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        let other = AtomicU32::new(0);
        let child_pair = pair.clone();
        let child = std::thread::spawn(move || {
            let time = deadline(false, 5000);
            assert_eq!(
                raw(
                    child_pair[0].as_ptr() as usize,
                    child_pair[1].as_ptr() as usize,
                    FUTEX_WAIT_REQUEUE_PI,
                    private,
                    0,
                    &raw const time as usize,
                    0
                ),
                0
            );
            assert_eq!(pi(&child_pair[1], FUTEX_UNLOCK_PI, private), 0);
        });
        count(&pair[0], private, 1);
        assert_eq!(
            raw(
                pair[0].as_ptr() as usize,
                other.as_ptr() as usize,
                FUTEX_CMP_REQUEUE_PI,
                private,
                1,
                0,
                0
            ),
            -22
        );
        count(&pair[0], private, 1);
        assert_eq!(
            raw(
                pair[0].as_ptr() as usize,
                pair[1].as_ptr() as usize,
                FUTEX_CMP_REQUEUE_PI,
                private,
                1,
                0,
                0
            ),
            1
        );
        child.join().unwrap();
    }
}
