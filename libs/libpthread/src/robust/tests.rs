use super::*;

fn initialize(mutex: &mut [usize; 5], kind: i32) -> usize {
    let mut attr = PthreadMutexAttr { kind: 0 };
    assert_eq!(unsafe { pthread_mutexattr_init(&mut attr) }, 0);
    assert_eq!(unsafe { pthread_mutexattr_setrobust(&mut attr, 1) }, 0);
    assert_eq!(unsafe { pthread_mutexattr_settype(&mut attr, kind) }, 0);
    let mut found = -1;
    assert_eq!(unsafe { pthread_mutexattr_getrobust(&attr, &mut found) }, 0);
    assert_eq!(found, 1);
    assert_eq!(unsafe { pthread_mutexattr_gettype(&attr, &mut found) }, 0);
    assert_eq!(found, kind);
    assert_eq!(unsafe { pthread_mutex_init(mutex.as_mut_ptr(), &attr) }, 0);
    mutex.as_mut_ptr() as usize
}

fn realtime(milliseconds: u64) -> Timespec {
    let value =
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + Duration::from_millis(milliseconds);
    Timespec {
        tv_sec: value.as_secs() as i64,
        tv_nsec: value.subsec_nanos() as i64,
    }
}

#[test]
fn native_owner_death_recovery_preserves_type_and_ownership() {
    for kind in [0, 1, 2, 3] {
        let mut storage = [0; 5];
        let mutex = initialize(&mut storage, kind);
        std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
            if kind == 1 {
                assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
            }
        })
        .join()
        .unwrap();
        assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EOWNERDEAD);
        std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_mutex_consistent(mutex as _) }, EINVAL);
            assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, EPERM);
        })
        .join()
        .unwrap();
        assert_eq!(unsafe { pthread_mutex_consistent(mutex as _) }, 0);
        assert_eq!(unsafe { pthread_mutex_consistent(mutex as _) }, EINVAL);
        if kind == 1 {
            assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, 0);
            assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
        } else {
            assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EBUSY);
        }
        assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
        std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
            assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
        })
        .join()
        .unwrap();
        assert_eq!(pthread_mutex_destroy(mutex as _), 0);
    }
}

#[test]
fn repeated_owner_death_and_unlock_without_repair_poison_the_mutex() {
    let mut storage = [0; 5];
    let mutex = initialize(&mut storage, 0);
    for expected in [0, EOWNERDEAD, EOWNERDEAD] {
        std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, expected);
        })
        .join()
        .unwrap();
    }
    assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, EOWNERDEAD);
    assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    let invalid = Timespec {
        tv_sec: 0,
        tv_nsec: -1,
    };
    assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, ENOTRECOVERABLE);
    assert_eq!(
        unsafe { pthread_mutex_trylock(mutex as _) },
        ENOTRECOVERABLE
    );
    assert_eq!(
        unsafe { pthread_mutex_timedlock(mutex as _, &invalid) },
        ENOTRECOVERABLE
    );
    assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, EPERM);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}

#[test]
fn robust_timed_lock_handles_expiry_self_deadlock_and_immediate_invalid_timespec() {
    let mut storage = [0; 5];
    let mutex = initialize(&mut storage, 0);
    let invalid = Timespec {
        tv_sec: -1,
        tv_nsec: -1,
    };
    assert_eq!(unsafe { pthread_mutex_timedlock(mutex as _, &invalid) }, 0);
    assert_eq!(
        unsafe { pthread_mutex_timedlock(mutex as _, &invalid) },
        EINVAL
    );
    let start = std::time::Instant::now();
    assert_eq!(
        unsafe { pthread_mutex_timedlock(mutex as _, &realtime(25)) },
        ETIMEDOUT
    );
    assert!(start.elapsed() >= Duration::from_millis(20));
    std::thread::spawn(move || {
        assert_eq!(
            unsafe { pthread_mutex_timedlock(mutex as _, &realtime(25)) },
            ETIMEDOUT
        );
        assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, EPERM);
    })
    .join()
    .unwrap();
    assert_eq!(pthread_mutex_destroy(mutex as _), EBUSY);
    assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}

#[test]
fn condition_wait_reacquires_abandoned_robust_mutex_and_reports_owner_death() {
    let mut storage = [0; 5];
    let mutex = initialize(&mut storage, 0);
    let mut condition = [0usize; 6];
    let cond = condition.as_mut_ptr() as usize;
    assert_eq!(
        unsafe { pthread_cond_init(cond as _, core::ptr::null()) },
        0
    );
    let (ready, receive) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        ready.send(()).unwrap();
        assert_eq!(
            unsafe { pthread_cond_timedwait(cond as _, mutex as _, &realtime(5000)) },
            EOWNERDEAD
        );
        assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EBUSY);
        assert_eq!(unsafe { pthread_mutex_consistent(mutex as _) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    });
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    std::thread::spawn(move || {
        assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        assert_eq!(unsafe { pthread_cond_signal(cond as _) }, 0);
        // Return while owning it: the condition waiter must observe native
        // abandonment rather than a mere selected condition event.
    })
    .join()
    .unwrap();
    waiter.join().unwrap();
    assert_eq!(pthread_cond_destroy(cond as _), 0);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}

#[test]
fn fork_robust_extension_rejects_corrupt_counts_states_and_duplicate_addresses() {
    let decode = |words: &[u64]| {
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        read_snapshot(&mut ForkReader {
            bytes: &bytes,
            at: 0,
        })
    };
    assert!(decode(&[]).unwrap().is_empty());
    assert_eq!(decode(&[1, 0x1000, 1, 3, 2, 1]).unwrap().len(), 1);
    for row in [
        vec![u64::MAX],
        vec![1, 0x1000],
        vec![1, 0, 0, 0, 0, 0],
        vec![1, 0x1000, 0, 3, 2, 0],
        vec![1, 0x1000, 1, 0, 2, 0],
        vec![1, 0x1000, 1, 3, 2, 2],
        vec![1, 0x1000, 1, 0, 0, 3],
        vec![2, 0x1000, 0, 0, 0, 0, 0x1000, 0, 0, 0, 0],
    ] {
        assert!(decode(&row).is_none(), "accepted {row:?}");
    }
}
