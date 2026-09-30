use super::*;

fn initialize(storage: &mut [usize; 5], kind: i32, robust: bool) -> usize {
    let mut attr = PthreadMutexAttr { kind: 0 };
    assert_eq!(unsafe { pthread_mutexattr_setpshared(&mut attr, 1) }, 0);
    assert_eq!(
        unsafe { pthread_mutexattr_setrobust(&mut attr, i32::from(robust)) },
        0
    );
    assert_eq!(unsafe { pthread_mutexattr_settype(&mut attr, kind) }, 0);
    let mut found = -1;
    assert_eq!(
        unsafe { pthread_mutexattr_getpshared(&attr, &mut found) },
        0
    );
    assert_eq!(found, 1);
    assert_eq!(unsafe { pthread_mutexattr_gettype(&attr, &mut found) }, 0);
    assert_eq!(found, kind);
    assert_eq!(
        unsafe { pthread_mutex_init(storage.as_mut_ptr(), &attr) },
        0
    );
    storage.as_mut_ptr() as usize
}
fn deadline(milliseconds: u64) -> Timespec {
    let now =
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + Duration::from_millis(milliseconds);
    Timespec {
        tv_sec: now.as_secs() as i64,
        tv_nsec: now.subsec_nanos() as i64,
    }
}

#[test]
fn attributes_types_deadlines_and_wrong_owner() {
    assert_eq!(core::mem::size_of::<PthreadMutexAttr>(), 4);
    for robust in [false, true] {
        for kind in 0..=3 {
            let mut object = [0; 5];
            let mutex = initialize(&mut object, kind, robust);
            let invalid = Timespec {
                tv_sec: 0,
                tv_nsec: -1,
            };
            assert_eq!(unsafe { pthread_mutex_timedlock(mutex as _, &invalid) }, 0);
            if kind == 1 {
                assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, 0);
                assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
            } else {
                assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EBUSY);
                if kind == 2 {
                    assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, EDEADLK);
                } else {
                    assert_eq!(
                        unsafe { pthread_mutex_timedlock(mutex as _, &invalid) },
                        EINVAL
                    );
                    assert_eq!(
                        unsafe { pthread_mutex_timedlock(mutex as _, &deadline(15)) },
                        ETIMEDOUT
                    );
                }
            }
            std::thread::spawn(move || {
                assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, EPERM);
                assert_eq!(unsafe { pthread_mutex_consistent(mutex as _) }, EINVAL);
                assert_eq!(
                    unsafe { pthread_mutex_timedlock(mutex as _, &deadline(15)) },
                    ETIMEDOUT
                );
            })
            .join()
            .unwrap();
            assert_eq!(pthread_mutex_destroy(mutex as _), EBUSY);
            assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
            assert_eq!(pthread_mutex_destroy(mutex as _), 0);
        }
    }
}

#[test]
fn owner_death_healing_poison_and_stalled_semantics() {
    for kind in 0..=3 {
        for poison in [false, true] {
            let mut object = [0; 5];
            let mutex = initialize(&mut object, kind, true);
            std::thread::spawn(move || {
                assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
                if kind == 1 {
                    assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
                }
            })
            .join()
            .unwrap();
            // Also exercise loss of the last idle native cache handle.
            let key = unsafe { key(mutex as _) }.unwrap();
            cache().rows.lock().unwrap().remove(&key);
            assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EOWNERDEAD);
            if !poison {
                assert_eq!(unsafe { pthread_mutex_consistent(mutex as _) }, 0);
            }
            assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
            assert_eq!(
                unsafe { pthread_mutex_trylock(mutex as _) },
                if poison { ENOTRECOVERABLE } else { 0 }
            );
            if !poison {
                assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
            }
            assert_eq!(pthread_mutex_destroy(mutex as _), 0);
        }
        let mut object = [0; 5];
        let mutex = initialize(&mut object, kind, false);
        std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        })
        .join()
        .unwrap();
        assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EBUSY);
        assert_eq!(
            unsafe { pthread_mutex_timedlock(mutex as _, &deadline(15)) },
            ETIMEDOUT
        );
        assert_eq!(pthread_mutex_destroy(mutex as _), EBUSY);
    }
}

#[test]
fn cache_has_a_capacity_and_reinitialization_gets_a_new_identity() {
    let mut objects = vec![[0; 5]; CACHE_LIMIT * 3];
    for object in &mut objects {
        let mutex = initialize(object, 0, false);
        let first = unsafe { key(mutex as _) }.unwrap();
        assert_eq!(pthread_mutex_destroy(mutex as _), 0);
        initialize(object, 0, false);
        assert_ne!(unsafe { key(mutex as _) }.unwrap(), first);
        assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    }
    assert!(cache().rows.lock().unwrap().len() <= CACHE_LIMIT);
    for object in &mut objects {
        assert_eq!(pthread_mutex_destroy(object.as_mut_ptr()), 0);
    }
    let mut object = [0; 5];
    let mutex = initialize(&mut object, 1, false);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
    assert_eq!(
        unsafe { pthread_mutex_init(mutex as _, core::ptr::null()) },
        0
    );
    assert!(!unsafe { is_mutex(mutex as _) });
    assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, 0);
    assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}

#[test]
fn private_condition_wait_reacquires_shared_mutex() {
    let mut object = [0; 5];
    let mutex = initialize(&mut object, 0, true);
    let mut condition = [0; 6];
    let cond = condition.as_mut_ptr() as usize;
    assert_eq!(
        unsafe { pthread_cond_init(cond as _, core::ptr::null()) },
        0
    );
    let (send, receive) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        send.send(()).unwrap();
        assert_eq!(
            unsafe { pthread_cond_timedwait(cond as _, mutex as _, &deadline(5000)) },
            EOWNERDEAD
        );
        assert_eq!(unsafe { pthread_mutex_consistent(mutex as _) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    });
    receive.recv_timeout(Duration::from_secs(2)).unwrap();
    std::thread::spawn(move || {
        assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        assert_eq!(unsafe { pthread_cond_signal(cond as _) }, 0);
    })
    .join()
    .unwrap();
    waiter.join().unwrap();
    assert_eq!(pthread_cond_destroy(cond as _), 0);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}

#[test]
#[ignore = "paired shared mutex cache benchmark"]
fn benchmark_shared_mutex_cache() {
    let mut object = [0; 5];
    let mutex = initialize(&mut object, 0, false);
    let iterations: u64 = std::env::var("KINAKAZE_BENCH_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let start = std::time::Instant::now();
    for _ in 0..iterations {
        assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    }
    println!(
        "SHARED_MUTEX_BENCH iterations={iterations} elapsed_ns={} cached={}",
        start.elapsed().as_nanos(),
        cached()
    );
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}
