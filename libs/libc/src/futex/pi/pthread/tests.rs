use super::*;
use std::sync::{Arc, mpsc};
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, GetThreadPriority, SetThreadPriority,
};

#[test]
#[ignore = "isolated release PI mutex pairs with live idle PI participants"]
fn benchmark_pthread_pi_pairs() {
    const ITERATIONS: u32 = 500;
    super::install();
    crate::fsextra::kinakaze_abi_gettid();
    let mut metrics = Vec::new();
    for background in [0, 32] {
        let barrier = Arc::new(std::sync::Barrier::new(background + 1));
        let (ready, received) = mpsc::channel();
        let threads: Vec<_> = (0..background)
            .map(|_| {
                let barrier = barrier.clone();
                let ready = ready.clone();
                std::thread::spawn(move || {
                    crate::fsextra::kinakaze_abi_gettid();
                    ready.send(()).unwrap();
                    barrier.wait();
                })
            })
            .collect();
        for _ in 0..background {
            received.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        {
            let transaction = Transaction::begin().unwrap();
            let tasks = Tasks::shared().unwrap().load().unwrap();
            assert!(
                !tasks
                    .iter()
                    .any(|task| task.identity.host != std::process::id()
                        && !task.identity.record(Key([0; 5]), 0, 0, 0).dead()),
                "foreign PI participants would change the benchmark"
            );
            assert!(
                transaction.records.is_empty(),
                "unrelated futex rows would change the benchmark"
            );
        }
        for (name, shared, robust) in [
            ("private_stalled", false, false),
            ("shared_stalled", true, false),
            ("private_robust", false, true),
            ("shared_robust", true, true),
        ] {
            let mut mutex = [0; 5];
            init(&mut mutex, 0, shared, robust);
            let address = mutex.as_ptr() as usize;
            for _ in 0..16 {
                assert_eq!(lock(address), 0);
                assert_eq!(unlock(address), 0);
            }
            let start = Instant::now();
            for _ in 0..ITERATIONS {
                assert_eq!(lock(address), 0);
                assert_eq!(unlock(address), 0);
            }
            metrics.push(format!(
                "\"{name}_{background}_ns\":{}",
                start.elapsed().as_nanos() / u128::from(ITERATIONS)
            ));
            assert_eq!(libpthread::pthread_mutex_destroy(address as _), 0);
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
    }
    println!(
        "PTHREAD_PI_BENCH {{\"optimized\":{},\"iterations\":{ITERATIONS},{}}}",
        std::env::var_os("KINAKAZE_PTHREAD_PI_OPT").is_none_or(|v| v != "0"),
        metrics.join(",")
    );
}

fn init(mutex: &mut [usize; 5], kind: i32, shared: bool, robust: bool) {
    let mut attr = 0u32;
    let ptr = (&raw mut attr).cast();
    assert_eq!(unsafe { libpthread::pthread_mutexattr_init(ptr) }, 0);
    assert_eq!(
        unsafe { libpthread::pthread_mutexattr_settype(ptr, kind) },
        0
    );
    assert_eq!(
        unsafe { libpthread::pthread_mutexattr_setpshared(ptr, i32::from(shared)) },
        0
    );
    assert_eq!(
        unsafe { libpthread::pthread_mutexattr_setrobust(ptr, i32::from(robust)) },
        0
    );
    assert_eq!(
        unsafe { libpthread::pthread_mutexattr_setprotocol(ptr, 1) },
        0
    );
    let mut protocol = 0;
    assert_eq!(
        unsafe { libpthread::pthread_mutexattr_getprotocol(ptr, &raw mut protocol) },
        0
    );
    assert_eq!(protocol, 1);
    assert_eq!(
        unsafe { libpthread::pthread_mutex_init(mutex.as_mut_ptr(), ptr) },
        0
    );
    assert_eq!(
        atomic_word::read(mutex.as_ptr() as usize + 16).unwrap(),
        kind as u32
            | INHERIT
            | if robust { ROBUST } else { 0 }
            | if shared || robust { SHARED } else { 0 }
    );
}
fn lock(address: usize) -> i32 {
    unsafe { libpthread::pthread_mutex_lock(address as _) }
}
fn unlock(address: usize) -> i32 {
    unsafe { libpthread::pthread_mutex_unlock(address as _) }
}
fn try_lock(address: usize) -> i32 {
    unsafe { libpthread::pthread_mutex_trylock(address as _) }
}
fn deadline(clock: i32, millis: i64) -> libpthread::Timespec {
    let (sec, nano) = crate::time::read_clock(clock).unwrap();
    let value = i128::from(sec) * 1_000_000_000 + i128::from(nano) + i128::from(millis) * 1_000_000;
    libpthread::Timespec {
        tv_sec: (value / 1_000_000_000) as i64,
        tv_nsec: (value % 1_000_000_000) as i64,
    }
}
fn queued(address: usize, private: bool, expected: usize) {
    let (_, key) = crate::sysadmin::futex_pi::transaction(address, private).unwrap();
    let start = Instant::now();
    while count_for_test(key) != expected {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
}

#[test]
fn pthread_pi_free_lock_replays_interrupted_priority_restoration() {
    super::install();
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -2) }, 0);
    crate::fsextra::kinakaze_abi_gettid();
    for pending in [false, true] {
        let mut mutex = [0; 5];
        init(&mut mutex, 0, false, false);
        {
            let _transaction = Transaction::begin().unwrap();
            let shared = Tasks::shared().unwrap();
            let mut tasks = shared.load().unwrap();
            let identity = Identity::current().unwrap();
            let task = tasks
                .iter_mut()
                .find(|task| task.identity == identity)
                .unwrap();
            task.base = -2;
            // Model either cancellation before recomputing intent, or intent
            // published before the native restore. Both must force replay.
            task.applied = if pending { -2 } else { 2 };
            shared.commit(&tasks);
            shared
                .header()
                .next_token
                .store(u64::from(pending), Ordering::Release);
            assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 2) }, 0);
        }
        let address = mutex.as_ptr() as usize;
        assert_eq!(lock(address), 0);
        assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, -2);
        {
            let _transaction = Transaction::begin().unwrap();
            assert_eq!(
                Tasks::shared()
                    .unwrap()
                    .header()
                    .next_token
                    .load(Ordering::Acquire),
                0
            );
        }
        assert_eq!(unlock(address), 0);
        assert_eq!(libpthread::pthread_mutex_destroy(address as _), 0);
    }
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 0) }, 0);
}

#[test]
#[ignore = "native helper: holds a PI donation until TerminateProcess"]
fn pthread_pi_dead_donor_child() {
    let name = std::env::var("KINAKAZE_PI_MAPPING").unwrap();
    let values: Vec<u64> = std::env::var("KINAKAZE_PI_KEY")
        .unwrap()
        .split(',')
        .map(|part| part.parse().unwrap())
        .collect();
    let key = Key(values.try_into().unwrap());
    let (_section, view) = super::super::tests::mapping(&name);
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 2) }, 0);
    let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
    register_current(tid as i32).unwrap();
    let park = park().unwrap();
    let mut transaction = Transaction::begin().unwrap();
    assert!(matches!(
        acquire(&mut transaction, key, view.Value as usize, tid, false),
        Ok(Acquisition::Queued(_))
    ));
    drop(transaction);
    loop {
        unsafe { WaitForSingleObject(park, u32::MAX) };
    }
}

#[test]
fn pthread_pi_unlock_restores_base_after_noncooperative_donor_death() {
    use std::os::windows::{io::AsRawHandle, process::CommandExt};
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -2) }, 0);
    let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
    register_current(tid as i32).unwrap();
    park().unwrap();
    let backing = new_backing_id().unwrap();
    let key = Key::anonymous(backing, 0);
    let name = format!(
        r"Local\kinakaze.pthread.pi.donor.{}.{}.{}",
        backing[0], backing[1], backing[2]
    );
    let (_section, view) = super::super::tests::mapping(&name);
    let address = view.Value as usize;
    let kind = INHERIT | SHARED;
    put(address + 16, kind).unwrap();
    {
        let mut transaction = Transaction::begin().unwrap();
        assert!(matches!(
            first(&mut transaction, key, address, tid, false, false),
            Ok(Acquisition::Owned)
        ));
        assert_eq!(owned(&mut transaction, key, address, tid, kind), Ok(0));
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "futex::pi::pthread::tests::pthread_pi_dead_donor_child",
            "--ignored",
            "--nocapture",
        ])
        .env("KINAKAZE_PI_MAPPING", &name)
        .env(
            "KINAKAZE_PI_KEY",
            key.0.map(|part| part.to_string()).join(","),
        )
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    let start = Instant::now();
    while count_for_test(key) != 1 {
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::yield_now();
    }
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
    assert_ne!(
        unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(child.as_raw_handle(), 77)
        },
        0
    );
    assert_eq!(child.wait().unwrap().code(), Some(77));
    {
        let mut transaction = Transaction::begin().unwrap();
        assert!(
            transaction
                .records
                .iter()
                .any(|row| row.key == key && row.pi_wait() && row.dead())
        );
        super::super::unlock(&mut transaction, key, address, tid).unwrap();
    }
    unheld(address);
    assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, -2);
    unsafe { UnmapViewOfFile(view) };
    assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 0) }, 0);
}

#[test]
fn pthread_pi_attribute_layout_all_types_and_recursive_contracts() {
    for shared in [false, true] {
        for robust in [false, true] {
            for kind in 0..=3 {
                let mut mutex = [0; 5];
                init(&mut mutex, kind, shared, robust);
                let address = mutex.as_ptr() as usize;
                assert_eq!(lock(address), 0);
                assert_eq!(libpthread::pthread_mutex_destroy(address as _), EBUSY);
                if kind == 1 {
                    assert_eq!(try_lock(address), 0);
                    assert_eq!(atomic_word::read(address + 4), Ok(2));
                    assert_eq!(unlock(address), 0);
                    put(address + 4, u32::MAX).unwrap();
                    assert_eq!(try_lock(address), EAGAIN);
                    put(address + 4, 1).unwrap();
                } else {
                    assert_eq!(try_lock(address), EBUSY);
                }
                if kind == 2 {
                    assert_eq!(lock(address), EDEADLK);
                }
                std::thread::spawn(move || assert_eq!(unlock(address), EPERM))
                    .join()
                    .unwrap();
                assert_eq!(unlock(address), 0);
                assert_eq!(atomic_word::read(address), Ok(0));
                assert_eq!(libpthread::pthread_mutex_destroy(address as _), 0);
            }
        }
    }
}

#[test]
fn pthread_pi_timed_immediate_lock_ignores_timespec_and_wait_validates_both_clocks() {
    let mut mutex = [0; 5];
    init(&mut mutex, 0, false, false);
    let address = mutex.as_ptr() as usize;
    let invalid = libpthread::Timespec {
        tv_sec: 0,
        tv_nsec: 1_000_000_000,
    };
    for clock in [0, 1] {
        assert_eq!(
            unsafe { libpthread::pthread_mutex_clocklock(address as _, clock, &raw const invalid) },
            0
        );
        assert_eq!(
            unsafe { libpthread::pthread_mutex_clocklock(address as _, clock, &raw const invalid) },
            EINVAL
        );
        let time = deadline(clock, 10);
        assert_eq!(
            unsafe { libpthread::pthread_mutex_clocklock(address as _, clock, &raw const time) },
            110
        );
        assert_eq!(unlock(address), 0);
    }
    assert_eq!(libpthread::pthread_mutex_destroy(address as _), 0);
}

#[test]
fn pthread_pi_contention_performs_native_donation_and_restores_base() {
    for shared in [false, true] {
        assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), -2) }, 0);
        let mut storage = [0; 5];
        init(&mut storage, 0, shared, false);
        let mutex = Arc::new(storage);
        let address = mutex.as_ptr() as usize;
        assert_eq!(lock(address), 0);
        let child_mutex = mutex.clone();
        let (tx, rx) = mpsc::channel();
        let child = std::thread::spawn(move || {
            assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 2) }, 0);
            let address = child_mutex.as_ptr() as usize;
            assert_eq!(lock(address), 0);
            tx.send(()).unwrap();
            assert_eq!(unlock(address), 0);
        });
        queued(address, !shared, 1);
        assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, 2);
        assert_eq!(unlock(address), 0);
        assert_eq!(unsafe { GetThreadPriority(GetCurrentThread()) }, -2);
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        child.join().unwrap();
        assert_eq!(libpthread::pthread_mutex_destroy(address as _), 0);
        assert_ne!(unsafe { SetThreadPriority(GetCurrentThread(), 0) }, 0);
    }
}

#[test]
fn pthread_pi_robust_death_repair_poison_and_stalled_expiry() {
    for shared in [false, true] {
        for robust in [false, true] {
            let mut storage = [0; 5];
            init(&mut storage, 0, shared, robust);
            let mutex = Arc::new(storage);
            let child_mutex = mutex.clone();
            std::thread::spawn(move || assert_eq!(lock(child_mutex.as_ptr() as usize), 0))
                .join()
                .unwrap();
            let address = mutex.as_ptr() as usize;
            if robust {
                assert_eq!(try_lock(address), EOWNERDEAD);
                assert_eq!(
                    unsafe { libpthread::pthread_mutex_consistent(address as _) },
                    0
                );
                assert_eq!(unlock(address), 0);
                let child_mutex = mutex.clone();
                std::thread::spawn(move || assert_eq!(lock(child_mutex.as_ptr() as usize), 0))
                    .join()
                    .unwrap();
                assert_eq!(lock(address), EOWNERDEAD);
                assert_eq!(unlock(address), 0);
                assert_eq!(lock(address), ENOTRECOVERABLE);
            } else {
                assert_eq!(try_lock(address), EBUSY);
                let time = deadline(1, 10);
                assert_eq!(
                    unsafe {
                        libpthread::pthread_mutex_clocklock(address as _, 1, &raw const time)
                    },
                    110
                );
                // Dispose this deliberately stranded test object without changing
                // the public STALLED contract or leaving its pinned row in the bank.
                put(address, 0).unwrap();
            }
            assert_eq!(libpthread::pthread_mutex_destroy(address as _), 0);
        }
    }
}

#[test]
fn pthread_pi_condition_timeout_reacquires_in_both_parking_modes() {
    for shared in [false, true] {
        let mut mutex = [0; 5];
        init(&mut mutex, 0, shared, true);
        let address = mutex.as_ptr() as usize;
        let mut cond = [0; 6];
        let mut attr = 0u32;
        assert_eq!(
            unsafe { libpthread::pthread_condattr_init((&raw mut attr).cast()) },
            0
        );
        assert_eq!(
            unsafe {
                libpthread::pthread_condattr_setpshared((&raw mut attr).cast(), i32::from(shared))
            },
            0
        );
        assert_eq!(
            unsafe { libpthread::pthread_cond_init(cond.as_mut_ptr(), (&raw const attr).cast()) },
            0
        );
        assert_eq!(lock(address), 0);
        let time = deadline(1, 10);
        assert_eq!(
            unsafe {
                libpthread::pthread_cond_clockwait(
                    cond.as_mut_ptr(),
                    mutex.as_mut_ptr(),
                    1,
                    &raw const time,
                )
            },
            110
        );
        assert_eq!(try_lock(address), EBUSY);
        assert_eq!(unlock(address), 0);
        assert_eq!(libpthread::pthread_cond_destroy(cond.as_mut_ptr()), 0);
        assert_eq!(libpthread::pthread_mutex_destroy(mutex.as_mut_ptr()), 0);
    }
}

#[test]
fn pthread_pi_poison_rejects_waiters_that_already_queued() {
    let mut storage = [0; 5];
    init(&mut storage, 0, true, true);
    let mutex = Arc::new(storage);
    let child_mutex = mutex.clone();
    std::thread::spawn(move || assert_eq!(lock(child_mutex.as_ptr() as usize), 0))
        .join()
        .unwrap();
    let address = mutex.as_ptr() as usize;
    assert_eq!(lock(address), EOWNERDEAD);
    let mut waiters = Vec::new();
    for _ in 0..2 {
        let child_mutex = mutex.clone();
        waiters.push(std::thread::spawn(move || {
            let address = child_mutex.as_ptr() as usize;
            assert_eq!(lock(address), ENOTRECOVERABLE);
            assert_eq!(unlock(address), EPERM);
        }));
    }
    queued(address, false, 2);
    assert_eq!(unlock(address), 0);
    for waiter in waiters {
        waiter.join().unwrap();
    }
    assert_eq!(atomic_word::read(address), Ok(0));
    assert_eq!(try_lock(address), ENOTRECOVERABLE);
    assert_eq!(libpthread::pthread_mutex_destroy(address as _), 0);
}

#[test]
#[ignore = "native process helper: exits without Rust, pthread or robust cleanup"]
fn pthread_pi_claim_child() {
    let name = std::env::var("KINAKAZE_PI_MAPPING").unwrap();
    let values: Vec<u64> = std::env::var("KINAKAZE_PI_KEY")
        .unwrap()
        .split(',')
        .map(|part| part.parse().unwrap())
        .collect();
    let key = Key(values.try_into().unwrap());
    let (_section, view) = super::super::tests::mapping(&name);
    let kind = INHERIT
        | SHARED
        | if std::env::var("KINAKAZE_PI_ROBUST").unwrap() == "1" {
            ROBUST
        } else {
            0
        };
    let address = view.Value as usize;
    put(address + 16, kind).unwrap();
    let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
    register_current(tid as i32).unwrap();
    park().unwrap();
    let mut transaction = Transaction::begin().unwrap();
    assert!(matches!(
        first(
            &mut transaction,
            key,
            address,
            tid,
            false,
            kind & ROBUST != 0
        ),
        Ok(Acquisition::Owned)
    ));
    assert_eq!(owned(&mut transaction, key, address, tid, kind), Ok(0));
    // Unlike ExitProcess, TerminateProcess also skips DLL/TLS detach callbacks.
    assert_ne!(
        unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                77,
            )
        },
        0
    );
    panic!("TerminateProcess returned");
}

#[test]
fn pthread_pi_uncontended_noncooperative_exit_and_claim_crash_recovery() {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    for robust in [false, true] {
        let phases: &[&str] = if robust {
            &[
                "before-journal",
                "before-cas",
                "after-cas",
                "after-publication",
                "after-self-consume",
                "owned",
            ]
        } else {
            &["owned"]
        };
        for phase in phases {
            let backing = new_backing_id().unwrap();
            let key = Key::anonymous(backing, 0);
            let name = format!(
                r"Local\kinakaze.pthread.pi.test.{}.{}.{}",
                backing[0], backing[1], backing[2]
            );
            let (_section, view) = super::super::tests::mapping(&name);
            // Mirror public mutex_init retaining the numeric domain bank.
            drop(Transaction::begin().unwrap());
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "futex::pi::pthread::tests::pthread_pi_claim_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("KINAKAZE_PI_CRASH", phase)
                .env("KINAKAZE_PI_MAPPING", &name)
                .env(
                    "KINAKAZE_PI_KEY",
                    key.0.map(|part| part.to_string()).join(","),
                )
                .env("KINAKAZE_PI_ROBUST", if robust { "1" } else { "0" })
                .creation_flags(0x0800_0000)
                .spawn()
                .unwrap();
            assert_eq!(child.wait().unwrap().code(), Some(77));
            let address = view.Value as usize;
            let kind = atomic_word::read(address + 16).unwrap();
            let tid = crate::fsextra::kinakaze_abi_gettid() as u32;
            register_current(tid as i32).unwrap();
            park().unwrap();
            let mut transaction = Transaction::begin().unwrap();
            let acquisition = first(&mut transaction, key, address, tid, false, robust)
                .unwrap_or_else(|error| {
                    panic!(
                        "{robust}/{phase}: errno={error}, word={:?}, rows={:?}",
                        atomic_word::read(address),
                        transaction
                            .records
                            .iter()
                            .filter(|row| row.key == key)
                            .map(|row| (
                                row.token,
                                row.bitset,
                                row.reserved,
                                row.host,
                                row.thread,
                                row.dead()
                            ))
                            .collect::<Vec<_>>()
                    )
                });
            let acquired = match acquisition {
                Acquisition::Owned => true,
                Acquisition::Queued(token) => {
                    poll_policy(&mut transaction, key, address, token, true, robust).unwrap()
                }
            };
            if robust {
                assert!(acquired, "{phase}: recovery failed");
                assert_eq!(
                    owned(&mut transaction, key, address, tid, kind),
                    Ok(if *phase == "before-journal" {
                        0
                    } else {
                        EOWNERDEAD
                    }),
                    "{phase}"
                );
                put(address + 8, tid).unwrap();
                super::super::unlock(&mut transaction, key, address, tid).unwrap();
                unheld(address);
            } else {
                assert!(!acquired, "STALLED mutex must retain its dead owner");
                put(address, 0).unwrap();
                transaction.records.retain(|row| row.key != key);
                transaction.dirty = true;
            }
            transaction.commit();
            drop(transaction);
            unsafe { UnmapViewOfFile(view) };
        }
    }
}
