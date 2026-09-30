use super::*;
use std::sync::mpsc;
use std::time::Instant;

fn initialize(cond: &mut [usize; 6], mutex: &mut [usize; 5], clock: i32) -> (usize, usize) {
    let mut attr = PthreadCondAttr { clock_id: 0 };
    assert_eq!(unsafe { pthread_condattr_init(&mut attr) }, 0);
    assert_eq!(unsafe { pthread_condattr_setpshared(&mut attr, 1) }, 0);
    assert_eq!(unsafe { pthread_condattr_setclock(&mut attr, clock) }, 0);
    let mut found = -1;
    assert_eq!(unsafe { pthread_condattr_getpshared(&attr, &mut found) }, 0);
    assert_eq!(found, 1);
    assert_eq!(unsafe { pthread_condattr_getclock(&attr, &mut found) }, 0);
    assert_eq!(found, clock);
    assert_eq!(attr.clock_id, 1 | (clock << 1));
    assert_eq!(
        unsafe { pthread_cond_init(cond.as_mut_ptr(), (&attr as *const PthreadCondAttr).cast()) },
        0
    );
    let mut attr = PthreadMutexAttr { kind: 0 };
    assert_eq!(unsafe { pthread_mutexattr_setpshared(&mut attr, 1) }, 0);
    assert_eq!(unsafe { pthread_mutexattr_setrobust(&mut attr, 1) }, 0);
    assert_eq!(unsafe { pthread_mutexattr_settype(&mut attr, 2) }, 0);
    assert_eq!(unsafe { pthread_mutex_init(mutex.as_mut_ptr(), &attr) }, 0);
    (cond.as_mut_ptr() as usize, mutex.as_mut_ptr() as usize)
}
fn deadline(clock: i32, milliseconds: u64) -> Timespec {
    let time = if clock == CLOCK_MONOTONIC {
        monotonic_now().unwrap()
    } else {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
    } + Duration::from_millis(milliseconds);
    Timespec {
        tv_sec: time.as_secs() as i64,
        tv_nsec: time.subsec_nanos() as i64,
    }
}
fn count(cond: usize) -> usize {
    let bank = bank(unsafe { key(cond as _) }.unwrap()).unwrap();
    let _guard = bank.acquire().unwrap();
    bank.load().unwrap().len()
}
fn rows(cond: usize, wanted: usize) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if count(cond) == wanted {
            return;
        }
        assert!(Instant::now() < until, "waiting for {wanted} shared rows");
        std::thread::yield_now();
    }
}

#[test]
fn attributes_deadlines_reinitialization_and_mutex_ownership() {
    for clock in [0, 1] {
        let mut cond = [0; 6];
        let mut mutex = [0; 5];
        let (cond, mutex) = initialize(&mut cond, &mut mutex, clock);
        assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        let invalid = Timespec {
            tv_sec: 0,
            tv_nsec: -1,
        };
        assert_eq!(
            unsafe { pthread_cond_timedwait(cond as _, mutex as _, &invalid) },
            EINVAL
        );
        assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EBUSY);
        let start = Instant::now();
        assert_eq!(
            unsafe { pthread_cond_timedwait(cond as _, mutex as _, &deadline(clock, 20)) },
            ETIMEDOUT
        );
        assert!(start.elapsed() >= Duration::from_millis(15));
        assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EBUSY);
        let expired = Timespec {
            tv_sec: -1,
            tv_nsec: 0,
        };
        assert_eq!(
            unsafe { pthread_cond_clockwait(cond as _, mutex as _, clock, &expired) },
            ETIMEDOUT
        );
        assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
        assert_eq!(count(cond), 0);
        assert_eq!(
            unsafe { field(cond as _, 40) }.load(Ordering::Acquire) >> 3,
            0
        );
        assert_eq!(pthread_cond_destroy(cond as _), 0);
        assert_eq!(unsafe { pthread_cond_init(cond as _, ptr::null()) }, 0);
        assert!(!unsafe { is_cond(cond as _) });
        assert_eq!(pthread_cond_destroy(cond as _), 0);
        assert_eq!(pthread_mutex_destroy(mutex as _), 0);
    }
}

#[test]
fn signal_selects_one_and_broadcast_releases_all_waiters() {
    let mut cond = [0; 6];
    let mut mutex = [0; 5];
    let (cond, mutex) = initialize(&mut cond, &mut mutex, 1);
    let (send, receive) = mpsc::channel();
    let mut workers = Vec::new();
    for index in 0..8 {
        let send = send.clone();
        workers.push(std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
            assert_eq!(
                unsafe { pthread_cond_timedwait(cond as _, mutex as _, &deadline(1, 5000)) },
                0
            );
            assert_eq!(unsafe { pthread_mutex_trylock(mutex as _) }, EBUSY);
            assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
            send.send(index).unwrap();
        }));
    }
    rows(cond, 8);
    assert_eq!(pthread_cond_destroy(cond as _), EBUSY);
    assert_eq!(unsafe { pthread_cond_signal(cond as _) }, 0);
    receive.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(receive.recv_timeout(Duration::from_millis(20)).is_err());
    rows(cond, 7);
    assert_eq!(unsafe { pthread_cond_broadcast(cond as _) }, 0);
    for _ in 0..7 {
        receive.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(count(cond), 0);
    assert_eq!(pthread_cond_destroy(cond as _), 0);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}

struct Cancel {
    cond: usize,
    mutex: usize,
    key: u32,
    cleaned: AtomicBool,
}
unsafe extern "sysv64" fn cleanup(argument: *mut c_void) {
    let state = unsafe { &*argument.cast::<Cancel>() };
    assert_eq!(unsafe { pthread_mutex_unlock(state.mutex as _) }, 0);
    state.cleaned.store(true, Ordering::Release);
}
unsafe extern "sysv64" fn cancelled_waiter(argument: *mut c_void) -> *mut c_void {
    let state = unsafe { &*argument.cast::<Cancel>() };
    assert_eq!(pthread_setspecific(state.key, argument), 0);
    assert_eq!(unsafe { pthread_mutex_lock(state.mutex as _) }, 0);
    assert_eq!(
        unsafe { pthread_cond_timedwait(state.cond as _, state.mutex as _, &deadline(1, 5000)) },
        0
    );
    ptr::null_mut()
}
#[test]
fn cancelling_selected_waiter_replaces_signal_and_cleanup_owns_mutex() {
    let mut cond = [0; 6];
    let mut mutex = [0; 5];
    let (cond, mutex) = initialize(&mut cond, &mut mutex, 1);
    let mut key_id = 0;
    assert_eq!(unsafe { pthread_key_create(&mut key_id, Some(cleanup)) }, 0);
    let mut state = Cancel {
        cond,
        mutex,
        key: key_id,
        cleaned: AtomicBool::new(false),
    };
    let mut victim = 0;
    assert_eq!(
        unsafe {
            pthread_create(
                &mut victim,
                ptr::null(),
                Some(cancelled_waiter),
                (&mut state as *mut Cancel).cast(),
            )
        },
        0
    );
    rows(cond, 1);
    let survivor = std::thread::spawn(move || {
        assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
        assert_eq!(
            unsafe { pthread_cond_timedwait(cond as _, mutex as _, &deadline(1, 5000)) },
            0
        );
        assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    });
    rows(cond, 2);
    assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
    assert_eq!(unsafe { pthread_cond_signal(cond as _) }, 0);
    rows(cond, 1); // victim consumed the selection and is reacquiring this mutex
    assert_eq!(pthread_cancel(victim), 0);
    assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
    let mut result = ptr::null_mut();
    assert_eq!(unsafe { pthread_join(victim, &mut result) }, 0);
    assert_eq!(result as usize, PTHREAD_CANCELED);
    assert!(state.cleaned.load(Ordering::Acquire));
    survivor.join().unwrap();
    assert_eq!(count(cond), 0);
    assert_eq!(pthread_key_delete(key_id), 0);
    assert_eq!(pthread_cond_destroy(cond as _), 0);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}

#[test]
#[ignore = "native subprocess abandoned condition transaction helper"]
fn transaction_child() {
    let words: Vec<u64> = std::env::var("KINAKAZE_COND_TEST_KEY")
        .unwrap()
        .split(',')
        .map(|word| word.parse().unwrap())
        .collect();
    let key = Key {
        domain: words[0],
        creator: words[1] as u32,
        birth: words[2],
        generation: words[3],
    };
    let bank = bank(key).unwrap();
    let _guard = bank.acquire().unwrap();
    let mut rows = bank.load().unwrap();
    assert_eq!(rows.len(), 1);
    bank.notify(&mut rows, 1, false).unwrap();
    match std::env::var("KINAKAZE_COND_TEST_MODE").unwrap().as_str() {
        "after" => bank.commit(&rows),
        "partial" => {
            let next = 1 - bank.header().active.load(Ordering::Acquire);
            unsafe { ptr::addr_of_mut!((*bank.rows(next)).count).write(u64::MAX) };
        }
        "before" => {}
        _ => panic!("unknown helper mode"),
    }
    unsafe { windows_sys::Win32::System::Threading::ExitProcess(77) };
}
#[test]
fn abandoned_signaler_keeps_only_committed_selections() {
    use std::os::windows::process::CommandExt;
    for mode in ["before", "partial", "after"] {
        let mut cond = [0; 6];
        let mut mutex = [0; 5];
        let (cond, mutex) = initialize(&mut cond, &mut mutex, 0);
        let (send, receive) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_mutex_lock(mutex as _) }, 0);
            let result =
                unsafe { pthread_cond_timedwait(cond as _, mutex as _, &deadline(0, 5000)) };
            assert_eq!(unsafe { pthread_mutex_unlock(mutex as _) }, 0);
            send.send(result).unwrap();
        });
        rows(cond, 1);
        let key = unsafe { key(cond as _) }.unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "shared::condition::tests::transaction_child",
                "--ignored",
                "--nocapture",
            ])
            .env(
                "KINAKAZE_COND_TEST_KEY",
                format!(
                    "{},{},{},{}",
                    key.domain, key.creator, key.birth, key.generation
                ),
            )
            .env("KINAKAZE_COND_TEST_MODE", mode)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(77),
            "{}",
            String::from_utf8_lossy(&child.stdout)
        );
        if mode != "after" {
            assert!(receive.recv_timeout(Duration::from_millis(25)).is_err());
            assert_eq!(unsafe { pthread_cond_signal(cond as _) }, 0);
        }
        assert_eq!(receive.recv_timeout(Duration::from_secs(5)).unwrap(), 0);
        waiter.join().unwrap();
        assert_eq!(pthread_cond_destroy(cond as _), 0);
        assert_eq!(pthread_mutex_destroy(mutex as _), 0);
    }
}

#[test]
#[ignore = "paired empty shared condition signal benchmark"]
fn benchmark_empty_shared_signal() {
    let mut cond = [0; 6];
    let mut mutex = [0; 5];
    let (cond, mutex) = initialize(&mut cond, &mut mutex, 0);
    let iterations = 10_000;
    let start = Instant::now();
    for _ in 0..iterations {
        assert_eq!(unsafe { pthread_cond_signal(cond as _) }, 0);
    }
    println!(
        "SHARED_COND_BENCH iterations={iterations} elapsed_ns={} cached={}",
        start.elapsed().as_nanos(),
        cached()
    );
    assert_eq!(pthread_cond_destroy(cond as _), 0);
    assert_eq!(pthread_mutex_destroy(mutex as _), 0);
}
