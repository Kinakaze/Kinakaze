use super::*;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn deadline(after: Duration) -> Timespec {
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + after;
    Timespec {
        tv_sec: time.as_secs() as i64,
        tv_nsec: time.subsec_nanos() as i64,
    }
}

fn wait_for_rows(address: usize, condition: bool, count: usize) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let actual = queues()
            .rows
            .lock()
            .unwrap()
            .get(&(address, condition))
            .map_or(0, Vec::len);
        if actual == count {
            return;
        }
        assert!(
            Instant::now() < until,
            "expected {count} rows, got {actual}"
        );
        std::thread::yield_now();
    }
}

#[test]
fn timed_waiters_all_progress_after_unlock_without_polling() {
    let mut mutex = [0usize; 5];
    let address = mutex.as_mut_ptr() as usize;
    assert_eq!(unsafe { pthread_mutex_lock(address as _) }, 0);
    let count = std::sync::Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let count = count.clone();
            std::thread::spawn(move || {
                assert_eq!(
                    unsafe {
                        timed_mutex(
                            address as _,
                            CLOCK_REALTIME,
                            deadline(Duration::from_secs(5)),
                        )
                    },
                    0
                );
                let previous = count.load(Ordering::Relaxed);
                std::thread::yield_now();
                count.store(previous + 1, Ordering::Relaxed);
                assert_eq!(unsafe { pthread_mutex_unlock(address as _) }, 0);
            })
        })
        .collect();
    wait_for_rows(address, false, 8);
    assert_eq!(unsafe { pthread_mutex_unlock(address as _) }, 0);
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(count.load(Ordering::Acquire), 8);
    wait_for_rows(address, false, 0);
    assert_eq!(pthread_mutex_destroy(address as _), 0);
}

#[test]
fn condition_release_wakes_timed_mutex_and_signal_relocks_errorcheck() {
    let mut mutex = [0usize; 5];
    let mut cond = [0usize; 6];
    let address = mutex.as_mut_ptr() as usize;
    let condition_address = cond.as_mut_ptr() as usize;
    let attr = PthreadMutexAttr {
        kind: PTHREAD_MUTEX_ERRORCHECK,
    };
    assert_eq!(unsafe { pthread_mutex_init(address as _, &attr) }, 0);
    assert_eq!(unsafe { pthread_mutex_lock(address as _) }, 0);
    let worker = std::thread::spawn(move || {
        assert_eq!(
            unsafe {
                timed_mutex(
                    address as _,
                    CLOCK_REALTIME,
                    deadline(Duration::from_secs(5)),
                )
            },
            0
        );
        assert_eq!(signal(condition_address as _, false), 0);
        assert_eq!(unsafe { pthread_mutex_unlock(address as _) }, 0);
    });
    wait_for_rows(address, false, 1);
    assert_eq!(
        unsafe {
            condition(
                condition_address as _,
                address as _,
                Some((deadline(Duration::from_secs(5)), CLOCK_REALTIME)),
            )
        },
        0
    );
    assert_eq!(unsafe { pthread_mutex_trylock(address as _) }, EBUSY);
    assert_eq!(unsafe { pthread_mutex_unlock(address as _) }, 0);
    worker.join().unwrap();
    wait_for_rows(condition_address, true, 0);
    assert_eq!(pthread_cond_destroy(condition_address as _), 0);
    assert_eq!(pthread_mutex_destroy(address as _), 0);
}

#[test]
fn event_deadlines_retire_rows_and_never_release_invalid_timespec() {
    let mut mutex = [0usize; 5];
    let mut cond = [0usize; 6];
    let address = mutex.as_mut_ptr() as usize;
    assert_eq!(unsafe { pthread_mutex_lock(address as _) }, 0);
    let worker = std::thread::spawn(move || unsafe {
        timed_mutex(
            address as _,
            CLOCK_REALTIME,
            deadline(Duration::from_millis(25)),
        )
    });
    assert_eq!(worker.join().unwrap(), ETIMEDOUT);
    wait_for_rows(address, false, 0);
    let invalid = Timespec {
        tv_sec: 0,
        tv_nsec: 1_000_000_000,
    };
    assert_eq!(
        unsafe {
            condition(
                cond.as_mut_ptr(),
                address as _,
                Some((invalid, CLOCK_REALTIME)),
            )
        },
        EINVAL
    );
    assert_eq!(unsafe { pthread_mutex_trylock(address as _) }, EBUSY);
    let past = Timespec {
        tv_sec: -1,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe {
            condition(
                cond.as_mut_ptr(),
                address as _,
                Some((past, CLOCK_REALTIME)),
            )
        },
        ETIMEDOUT
    );
    assert_eq!(unsafe { pthread_mutex_trylock(address as _) }, EBUSY);
    assert_eq!(unsafe { pthread_mutex_unlock(address as _) }, 0);
    assert_eq!(pthread_mutex_destroy(address as _), 0);
}

struct CancelProbe {
    mutex: usize,
    cond: usize,
    key: u32,
    cleaned: AtomicBool,
}

unsafe extern "sysv64" fn cancelled_cleanup(argument: *mut c_void) {
    let state = unsafe { &*argument.cast::<CancelProbe>() };
    assert_eq!(unsafe { pthread_mutex_trylock(state.mutex as _) }, EBUSY);
    // ERRORCHECK verifies that this exact cleanup thread owns the mutex.
    assert_eq!(unsafe { pthread_mutex_unlock(state.mutex as _) }, 0);
    state.cleaned.store(true, Ordering::Release);
}

unsafe extern "sysv64" fn cancellable_condition(argument: *mut c_void) -> *mut c_void {
    let state = unsafe { &*argument.cast::<CancelProbe>() };
    assert_eq!(unsafe { pthread_setspecific(state.key, argument) }, 0);
    assert_eq!(unsafe { pthread_mutex_lock(state.mutex as _) }, 0);
    assert_eq!(
        unsafe { condition(state.cond as _, state.mutex as _, None) },
        0
    );
    0x52usize as _
}

#[test]
fn cancelled_selected_waiter_replaces_signal_and_cleanup_owns_mutex() {
    let mut mutex = [0usize; 5];
    let mut cond = [0usize; 6];
    let address = mutex.as_mut_ptr() as usize;
    let condition_address = cond.as_mut_ptr() as usize;
    let attr = PthreadMutexAttr {
        kind: PTHREAD_MUTEX_ERRORCHECK,
    };
    assert_eq!(unsafe { pthread_mutex_init(address as _, &attr) }, 0);
    let mut key = 0;
    assert_eq!(
        unsafe { pthread_key_create(&mut key, Some(cancelled_cleanup)) },
        0
    );
    let mut probes: Vec<_> = (0..2)
        .map(|_| {
            Box::new(CancelProbe {
                mutex: address,
                cond: condition_address,
                key,
                cleaned: AtomicBool::new(false),
            })
        })
        .collect();
    let mut threads = [0usize; 2];
    for index in 0..2 {
        assert_eq!(
            unsafe {
                pthread_create(
                    &mut threads[index],
                    ptr::null(),
                    Some(cancellable_condition),
                    (&mut *probes[index] as *mut CancelProbe).cast(),
                )
            },
            0
        );
        wait_for_rows(condition_address, true, index + 1);
    }
    // Hold the mutex across selection/cancellation so the selected thread
    // cannot finish the condition call before the cancellation request.
    assert_eq!(unsafe { pthread_mutex_lock(address as _) }, 0);
    assert_eq!(signal(condition_address as _, false), 0);
    assert_eq!(pthread_cancel(threads[0]), 0);
    assert_eq!(unsafe { pthread_mutex_unlock(address as _) }, 0);
    let until = Instant::now() + Duration::from_secs(5);
    while probes.iter().any(|p| !p.cleaned.load(Ordering::Acquire)) && Instant::now() < until {
        std::thread::yield_now();
    }
    let completed = probes.iter().all(|p| p.cleaned.load(Ordering::Acquire));
    if !completed {
        signal(condition_address as _, true);
    }
    let mut results = [ptr::null_mut(); 2];
    for index in 0..2 {
        assert_eq!(
            unsafe { pthread_join(threads[index], &mut results[index]) },
            0
        );
    }
    assert!(
        completed,
        "a cancelled selected waiter consumed the remaining signal"
    );
    assert_eq!(results[0] as usize, PTHREAD_CANCELED);
    assert_eq!(results[1] as usize, 0x52);
    wait_for_rows(condition_address, true, 0);
    assert_eq!(pthread_key_delete(key), 0);
    assert_eq!(pthread_cond_destroy(condition_address as _), 0);
    assert_eq!(pthread_mutex_destroy(address as _), 0);
}

fn thread_cpu_ns() -> u64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let mut times: [FILETIME; 4] = unsafe { core::mem::zeroed() };
    let base = times.as_mut_ptr();
    assert_ne!(
        unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                base,
                base.add(1),
                base.add(2),
                base.add(3),
            )
        },
        0
    );
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    (ticks(times[2]) + ticks(times[3])) * 100
}

fn thread_cycles() -> u64 {
    use windows_sys::Win32::System::Threading::GetCurrentThread;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> i32;
    }
    let mut cycles = 0;
    assert_ne!(
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) },
        0
    );
    cycles
}

#[test]
#[ignore = "paired release pthread park CPU/latency benchmark"]
fn benchmark_event_parking() {
    let mut cpu_ns = 0;
    let mut cycles = 0;
    let mut acquire_ns = 0;
    for _ in 0..20 {
        let mut mutex = [0usize; 5];
        let address = mutex.as_mut_ptr() as usize;
        assert_eq!(unsafe { pthread_mutex_lock(address as _) }, 0);
        let release = std::sync::Arc::new(Mutex::new(None::<Instant>));
        let shared_release = release.clone();
        let (ready, started) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let cpu = thread_cpu_ns();
            let cycle_start = thread_cycles();
            ready.send(()).unwrap();
            assert_eq!(
                unsafe { pthread_mutex_timedlock(address as _, &deadline(Duration::from_secs(5))) },
                0
            );
            let elapsed = shared_release.lock().unwrap().unwrap().elapsed().as_nanos() as u64;
            let cpu = thread_cpu_ns() - cpu;
            let cycle_delta = thread_cycles() - cycle_start;
            assert_eq!(unsafe { pthread_mutex_unlock(address as _) }, 0);
            (cpu, elapsed, cycle_delta)
        });
        started.recv().unwrap();
        std::thread::sleep(Duration::from_millis(25));
        *release.lock().unwrap() = Some(Instant::now());
        assert_eq!(unsafe { pthread_mutex_unlock(address as _) }, 0);
        let (cpu, elapsed, cycle_delta) = worker.join().unwrap();
        cpu_ns += cpu;
        acquire_ns += elapsed;
        cycles += cycle_delta;
        assert_eq!(pthread_mutex_destroy(address as _), 0);
    }
    println!(
        "PTHREAD_PARK_BENCH {{\"optimized\":{},\"blocked_cpu_20_ns\":{cpu_ns},\"blocked_20_cycles\":{cycles},\"unlock_to_acquire_20_ns\":{acquire_ns}}}",
        enabled()
    );
}
