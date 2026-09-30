use super::*;
use core::sync::atomic::AtomicU32;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, GetThreadPriority, SetThreadPriority,
};

fn raw(address: usize, command: u32, private: bool, timeout: usize) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            202,
            address as u64,
            u64::from(command | if private { FUTEX_PRIVATE_FLAG } else { 0 }),
            0,
            timeout as u64,
            0,
            0,
        )
    }
}
fn key(word: &AtomicU32, private: bool) -> crate::futex::Key {
    futex_requeue::key(FutexAddress::resolve(word.as_ptr().cast(), private).unwrap()).unwrap()
}
fn queued(word: &AtomicU32, private: bool, count: usize) {
    let started = Instant::now();
    while crate::futex::count_for_test(key(word, private)) != count {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "expected {count} PI waiters"
        );
        std::thread::yield_now();
    }
}

#[test]
fn pi_lock_trylock_unlock_owner_bits_and_error_precedence() {
    for private in [false, true] {
        let word = AtomicU32::new(0);
        let address = word.as_ptr() as usize;
        let tid = current_tid() as u32;
        for command in [FUTEX_LOCK_PI, FUTEX_LOCK_PI2, FUTEX_TRYLOCK_PI] {
            assert_eq!(raw(address, command, private, 0), 0);
            assert_eq!(word.load(Ordering::Acquire), tid);
            assert_eq!(raw(address, command, private, 0), -35);
            assert_eq!(raw(address, FUTEX_UNLOCK_PI, private, 1), 0);
            assert_eq!(word.load(Ordering::Acquire), 0);
        }
        word.store(0xc000_0000, Ordering::Release);
        assert_eq!(raw(address, FUTEX_LOCK_PI2, private, 0), 0);
        assert_eq!(word.load(Ordering::Acquire), 0x4000_0000 | tid);
        assert_eq!(raw(address, FUTEX_UNLOCK_PI, private, 0), 0);
        assert_eq!(raw(address, FUTEX_UNLOCK_PI, private, 0), -i64::from(EPERM));
        assert_eq!(raw(1, FUTEX_LOCK_PI, private, 0), -i64::from(EINVAL));
        let bytes = [0u8; 12];
        assert_eq!(
            raw(bytes.as_ptr() as usize + 1, FUTEX_UNLOCK_PI, private, 0),
            -i64::from(EPERM)
        );
        assert_eq!(raw(1, FUTEX_UNLOCK_PI, private, 0), -i64::from(EFAULT));
        assert_eq!(
            raw(address, FUTEX_LOCK_PI | FUTEX_CLOCK_REALTIME, private, 0),
            -i64::from(ENOSYS)
        );
        assert_eq!(
            raw(address, FUTEX_LOCK_PI2 | FUTEX_CLOCK_REALTIME, private, 0),
            0
        );
        assert_eq!(raw(address, FUTEX_UNLOCK_PI, private, 0), 0);
    }
}

#[test]
fn pi_contention_handoff_rejects_plain_wake_and_requeue() {
    for private in [false, true] {
        let word = Arc::new(AtomicU32::new(0));
        let target = AtomicU32::new(0);
        let address = word.as_ptr() as usize;
        assert_eq!(raw(address, FUTEX_LOCK_PI2, private, 0), 0);
        let target_word = Arc::clone(&word);
        let (tx, rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let tid = current_tid() as u32;
            assert_eq!(
                raw(target_word.as_ptr() as usize, FUTEX_LOCK_PI2, private, 0),
                0
            );
            assert_eq!(target_word.load(Ordering::Acquire) & 0x3fff_ffff, tid);
            tx.send(tid).unwrap();
            assert_eq!(
                raw(target_word.as_ptr() as usize, FUTEX_UNLOCK_PI, private, 0),
                0
            );
        });
        queued(&word, private, 1);
        assert_eq!(
            futex_wake(
                FutexAddress::resolve(word.as_ptr().cast(), private).unwrap(),
                1,
                u32::MAX
            ),
            -i64::from(EINVAL)
        );
        let result = unsafe {
            kinakaze_abi_syscall_raw(
                202,
                address as _,
                u64::from(FUTEX_REQUEUE | if private { 128 } else { 0 }),
                0,
                1,
                target.as_ptr() as _,
                0,
            )
        };
        assert_eq!(result, -i64::from(EINVAL));
        assert_eq!(raw(address, FUTEX_UNLOCK_PI, private, 0), 0);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        waiter.join().unwrap();
        assert_eq!(word.load(Ordering::Acquire), 0);
        queued(&word, private, 0);
    }
}

#[test]
fn pi_busy_trylock_and_absolute_expiry_retire_waiters() {
    for private in [false, true] {
        let word = Arc::new(AtomicU32::new(0));
        assert_eq!(raw(word.as_ptr() as usize, FUTEX_LOCK_PI2, private, 0), 0);
        let target = Arc::clone(&word);
        std::thread::spawn(move || {
            let zero = KernelTimespec::default();
            assert_eq!(
                raw(target.as_ptr() as usize, FUTEX_TRYLOCK_PI, private, 1),
                -i64::from(EAGAIN)
            );
            assert_eq!(
                raw(
                    target.as_ptr() as usize,
                    FUTEX_LOCK_PI2,
                    private,
                    &raw const zero as usize
                ),
                -i64::from(ETIMEDOUT)
            );
        })
        .join()
        .unwrap();
        queued(&word, private, 0);
        assert_eq!(raw(word.as_ptr() as usize, FUTEX_UNLOCK_PI, private, 0), 0);
    }
}

#[test]
fn pi_owner_exit_transfers_to_contended_waiter_without_robust_registration() {
    for private in [false, true] {
        let word = Arc::new(AtomicU32::new(0));
        let target = Arc::clone(&word);
        let (locked_tx, locked_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();
        let owner = std::thread::spawn(move || {
            assert_eq!(raw(target.as_ptr() as usize, FUTEX_LOCK_PI2, private, 0), 0);
            locked_tx.send(()).unwrap();
            exit_rx.recv().unwrap();
        });
        locked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let target = Arc::clone(&word);
        let waiter = std::thread::spawn(move || {
            assert_eq!(raw(target.as_ptr() as usize, FUTEX_LOCK_PI2, private, 0), 0);
            assert_eq!(target.load(Ordering::Acquire) & 0x4000_0000, 0x4000_0000);
            assert_eq!(
                raw(target.as_ptr() as usize, FUTEX_UNLOCK_PI, private, 0),
                0
            );
        });
        queued(&word, private, 1);
        exit_tx.send(()).unwrap();
        owner.join().unwrap();
        waiter.join().unwrap();
        queued(&word, private, 0);
    }
}

#[test]
fn pi_native_donation_restores_base_after_unlock() {
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -2) }, 0);
    let word = Arc::new(AtomicU32::new(0));
    assert_eq!(raw(word.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
    let target = Arc::clone(&word);
    let waiter = std::thread::spawn(move || {
        assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 2) }, 0);
        assert_eq!(raw(target.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
        assert_eq!(raw(target.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    });
    queued(&word, true, 1);
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
    assert_eq!(raw(word.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, -2);
    waiter.join().unwrap();
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 0) }, 0);
}

#[test]
fn pi_guest_base_priority_change_preserves_active_donation() {
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -2) }, 0);
    let word = Arc::new(AtomicU32::new(0));
    assert_eq!(raw(word.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
    let target = Arc::clone(&word);
    let waiter = std::thread::spawn(move || {
        assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 2) }, 0);
        assert_eq!(raw(target.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
        assert_eq!(raw(target.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    });
    queued(&word, true, 1);
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
    let param = libpthread::sched::SchedParam { sched_priority: 0 };
    assert_eq!(
        unsafe {
            libpthread::sched::pthread_setschedparam(
                libpthread::pthread_self(),
                0,
                &raw const param,
            )
        },
        0
    );
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
    assert_eq!(raw(word.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 0);
    waiter.join().unwrap();
}

#[test]
fn pi_two_lock_cycle_returns_deadlock_then_unwinds_normally() {
    let first = Arc::new(AtomicU32::new(0));
    let second = Arc::new(AtomicU32::new(0));
    assert_eq!(raw(first.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
    let (tx, rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let a = Arc::clone(&first);
    let b = Arc::clone(&second);
    let other = std::thread::spawn(move || {
        assert_eq!(raw(b.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
        tx.send(()).unwrap();
        go_rx.recv().unwrap();
        queued(&b, true, 1);
        assert_eq!(raw(a.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), -35);
        assert_eq!(raw(b.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    go_tx.send(()).unwrap();
    assert_eq!(raw(second.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
    assert_eq!(raw(second.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    assert_eq!(raw(first.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    other.join().unwrap();
}

#[test]
fn pi_multiple_waiters_keep_fifo_and_state_until_last_handoff() {
    for private in [false, true] {
        let word = Arc::new(AtomicU32::new(0));
        assert_eq!(raw(word.as_ptr() as usize, FUTEX_LOCK_PI2, private, 0), 0);
        let (tx, rx) = mpsc::channel();
        let mut workers = Vec::new();
        for index in 0..3 {
            let target = Arc::clone(&word);
            let tx = tx.clone();
            workers.push(std::thread::spawn(move || {
                assert_eq!(raw(target.as_ptr() as usize, FUTEX_LOCK_PI2, private, 0), 0);
                tx.send(index).unwrap();
                assert_eq!(
                    raw(target.as_ptr() as usize, FUTEX_UNLOCK_PI, private, 0),
                    0
                );
            }));
            queued(&word, private, index + 1);
        }
        assert_eq!(raw(word.as_ptr() as usize, FUTEX_UNLOCK_PI, private, 0), 0);
        for index in 0..3 {
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), index);
        }
        for worker in workers {
            worker.join().unwrap();
        }
        queued(&word, private, 0);
    }
}

#[test]
fn pi_nested_locks_propagate_and_restore_native_donation() {
    let first = Arc::new(AtomicU32::new(0));
    let second = Arc::new(AtomicU32::new(0));
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -2) }, 0);
    assert_eq!(raw(first.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
    let (tx, rx) = mpsc::channel();
    let a = Arc::clone(&first);
    let b = Arc::clone(&second);
    let middle = std::thread::spawn(move || {
        assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -1) }, 0);
        assert_eq!(raw(b.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
        tx.send(()).unwrap();
        assert_eq!(raw(a.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
        assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
        assert_eq!(raw(a.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
        assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
        assert_eq!(raw(b.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
        assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, -1);
    });
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    queued(&first, true, 1);
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, -1);
    let b = Arc::clone(&second);
    let high = std::thread::spawn(move || {
        assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 2) }, 0);
        assert_eq!(raw(b.as_ptr() as usize, FUTEX_LOCK_PI2, true, 0), 0);
        assert_eq!(raw(b.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    });
    queued(&second, true, 1);
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
    assert_eq!(raw(first.as_ptr() as usize, FUTEX_UNLOCK_PI, true, 0), 0);
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, -2);
    middle.join().unwrap();
    high.join().unwrap();
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 0) }, 0);
}

#[test]
#[ignore = "isolated raw PI acquisition/unlock path benchmark"]
fn benchmark_raw_pi_pairs() {
    for private in [false, true] {
        for command in [FUTEX_LOCK_PI, FUTEX_TRYLOCK_PI, FUTEX_LOCK_PI2] {
            let word = AtomicU32::new(0);
            let address = word.as_ptr() as usize;
            assert_eq!(raw(address, command, private, 0), 0);
            assert_eq!(raw(address, FUTEX_UNLOCK_PI, private, 0), 0);
            let iterations = 2000;
            let started = Instant::now();
            for _ in 0..iterations {
                assert_eq!(raw(address, command, private, 0), 0);
                assert_eq!(raw(address, FUTEX_UNLOCK_PI, private, 0), 0);
            }
            println!(
                "PI_METRIC command={command} private={} pairs={iterations} elapsed_ns={}",
                u8::from(private),
                started.elapsed().as_nanos()
            );
        }
    }
}
