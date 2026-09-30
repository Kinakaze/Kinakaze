use super::*;
use core::sync::atomic::AtomicI64;
use kinakaze_vfs::{EINTR, interrupt, signal};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Pause {
    address: usize,
    copied: mpsc::SyncSender<()>,
    resume: mpsc::Receiver<()>,
}
static PAUSE: Mutex<Option<Pause>> = Mutex::new(None);

pub(super) fn pause_after_timeout_copy(address: usize) {
    let pause = {
        let mut state = PAUSE.lock().unwrap();
        if state.as_ref().is_some_and(|pause| pause.address == address) {
            state.take()
        } else {
            None
        }
    };
    if let Some(pause) = pause {
        pause.copied.send(()).unwrap();
        pause.resume.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}

fn raw(address: usize, value: u64, mask: u64, flags: u32, timeout: usize, clock: i32) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            455,
            address as u64,
            value,
            mask,
            flags as u64,
            timeout as u64,
            clock as u64,
        )
    }
}

fn modern_wake(word: &AtomicI32, private: bool, mask: u32) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            454,
            word.as_ptr() as u64,
            mask as u64,
            1,
            if private { 0x82 } else { 2 },
            0,
            0,
        )
    }
}

fn queued(word: &AtomicI32, private: bool, expected: usize) {
    let started = Instant::now();
    loop {
        let address = word.as_ptr() as usize;
        let actual = if private {
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
            crate::futex::count_for_test(crate::fdio::futex_key(address).unwrap())
        };
        if actual == expected {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{actual} != {expected}"
        );
        std::thread::yield_now();
    }
}

fn deadline(clock: i64, milliseconds: i64) -> KernelTimespec {
    let mut now = KernelTimespec::default();
    assert_eq!(unsafe { clock_gettime(clock, &raw mut now) }, 0);
    let nanos = now.tv_nsec + milliseconds * 1_000_000;
    KernelTimespec {
        tv_sec: now.tv_sec + nanos / 1_000_000_000,
        tv_nsec: nanos % 1_000_000_000,
    }
}

fn spawn(word: &Arc<AtomicI32>, private: bool, mask: u32) -> std::thread::JoinHandle<i64> {
    let word = Arc::clone(word);
    std::thread::spawn(move || {
        let timeout = deadline(CLOCK_MONOTONIC, 5000);
        raw(
            word.as_ptr() as usize,
            0,
            mask as u64,
            if private { 0x82 } else { 2 },
            &timeout as *const _ as usize,
            1,
        )
    })
}

#[test]
fn wait2_preserves_width_timeout_mask_and_key_validation_order() {
    let word = AtomicI32::new(7);
    let address = word.as_ptr() as usize;
    let past = KernelTimespec::default();
    let past_address = &past as *const _ as usize;
    let bad = KernelTimespec {
        tv_sec: -1,
        tv_nsec: 0,
    };
    for flags in [0, 1, 3, 6, 0x102, 0xffff_ffff] {
        assert_eq!(raw(1, 0, 1, flags, 1, 999), -i64::from(EINVAL));
    }
    for flags in [2, 0x82] {
        assert_eq!(raw(1, 1 << 32, 1, flags, 1, 999), -i64::from(EINVAL));
        assert_eq!(raw(1, 0, 1 << 32, flags, 1, 999), -i64::from(EINVAL));
        assert_eq!(raw(1, 0, 0, flags, 1, 999), -i64::from(EINVAL));
        assert_eq!(raw(1, 0, 0, flags, 1, 1), -i64::from(EFAULT));
        assert_eq!(
            raw(1, 0, 0, flags, &bad as *const _ as usize, 1),
            -i64::from(EINVAL)
        );
        assert_eq!(raw(1, 0, 0, flags, past_address, 1), -i64::from(EINVAL));
        assert_eq!(raw(1, 0, 1, flags, past_address, 1), -i64::from(EINVAL));
        for invalid_address in [0, 4, (1usize << 47) - 4096] {
            assert_eq!(
                raw(invalid_address, 0, 1, flags, past_address, 1),
                -i64::from(EFAULT)
            );
        }
        assert_eq!(raw(address, 8, 1, flags, 0, 999), -i64::from(EAGAIN));
        assert_eq!(
            raw(address, 8, 1, flags, past_address, 1),
            -i64::from(EAGAIN)
        );
        assert_eq!(
            raw(address, 7, 1, flags, past_address, 1),
            -i64::from(ETIMEDOUT)
        );
        assert_eq!(
            raw(address, 7, 1, flags, past_address, 0),
            -i64::from(ETIMEDOUT)
        );
    }
    word.store(-1, Ordering::Release);
    assert_eq!(
        raw(
            address,
            u64::from(u32::MAX),
            u64::from(u32::MAX),
            0x82,
            past_address,
            1
        ),
        -i64::from(ETIMEDOUT)
    );
}

#[test]
fn wait2_uses_legacy_and_modern_masked_wake_queues() {
    for private in [false, true] {
        for legacy in [false, true] {
            let word = Arc::new(AtomicI32::new(0));
            let waiter = spawn(&word, private, 8);
            queued(&word, private, 1);
            assert_eq!(modern_wake(&word, !private, u32::MAX), 0);
            assert_eq!(modern_wake(&word, private, 4), 0);
            let result = if legacy {
                futex_syscall(
                    word.as_ptr(),
                    (FUTEX_WAKE_BITSET | if private { FUTEX_PRIVATE_FLAG } else { 0 }) as i32,
                    1,
                    core::ptr::null(),
                    core::ptr::null_mut(),
                    8,
                )
            } else {
                modern_wake(&word, private, 8)
            };
            assert_eq!(result, 1);
            assert_eq!(waiter.join().unwrap(), 0);
            queued(&word, private, 0);
        }
    }
}

#[test]
fn wait2_does_not_read_timeout_again_after_it_was_copied() {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };
    for private in [false, true] {
        let word = Arc::new(AtomicI32::new(0));
        let page = unsafe {
            VirtualAlloc(
                core::ptr::null(),
                4096,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        assert!(!page.is_null());
        unsafe {
            page.cast::<KernelTimespec>()
                .write(deadline(CLOCK_MONOTONIC, 150))
        };
        let timeout = page as usize;
        let (copied, observe) = mpsc::sync_channel(1);
        let (resume, suspended) = mpsc::channel();
        *PAUSE.lock().unwrap() = Some(Pause {
            address: word.as_ptr() as usize,
            copied,
            resume: suspended,
        });
        let child_word = Arc::clone(&word);
        let waiter = std::thread::spawn(move || {
            let started = Instant::now();
            let result = raw(
                child_word.as_ptr() as usize,
                0,
                1,
                if private { 0x82 } else { 2 },
                timeout,
                1,
            );
            (result, started.elapsed())
        });
        observe.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(unsafe { VirtualFree(page, 0, MEM_RELEASE) }, 0);
        resume.send(()).unwrap();
        let (result, elapsed) = waiter.join().unwrap();
        assert_eq!(result, -i64::from(ETIMEDOUT));
        assert!(elapsed >= Duration::from_millis(75));
        assert_eq!(modern_wake(&word, private, u32::MAX), 0);
        queued(&word, private, 0);
    }
}

#[test]
fn wait2_requeues_to_all_private_shared_destinations_without_changing_mask() {
    #[repr(C)]
    struct Entry {
        value: u64,
        address: u64,
        flags: u32,
        reserved: u32,
    }
    for private in [false, true] {
        for target_private in [false, true] {
            let source = Arc::new(AtomicI32::new(0));
            let target = AtomicI32::new(0);
            let waiter = spawn(&source, private, 8);
            queued(&source, private, 1);
            let entries = [
                Entry {
                    value: 0,
                    address: source.as_ptr() as u64,
                    flags: if private { 0x82 } else { 2 },
                    reserved: 0,
                },
                Entry {
                    value: 17,
                    address: target.as_ptr() as u64,
                    flags: if target_private { 0x82 } else { 2 },
                    reserved: 0,
                },
            ];
            assert_eq!(
                unsafe { kinakaze_abi_syscall_raw(456, entries.as_ptr() as u64, 0, 0, 1, 0, 0) },
                1
            );
            queued(&source, private, 0);
            queued(&target, target_private, 1);
            assert_eq!(modern_wake(&source, private, u32::MAX), 0);
            assert_eq!(modern_wake(&target, target_private, 4), 0);
            assert_eq!(modern_wake(&target, target_private, 8), 1);
            assert_eq!(waiter.join().unwrap(), 0);
            queued(&target, target_private, 0);
        }
    }
}

#[test]
fn wait2_absolute_clocks_timeout_and_retire_the_waiter() {
    for private in [false, true] {
        for clock in [CLOCK_REALTIME, CLOCK_MONOTONIC] {
            let word = AtomicI32::new(0);
            let timeout = deadline(clock, 30);
            let started = Instant::now();
            assert_eq!(
                raw(
                    word.as_ptr() as usize,
                    0,
                    1,
                    if private { 0x82 } else { 2 },
                    &timeout as *const _ as usize,
                    clock as i32
                ),
                -i64::from(ETIMEDOUT)
            );
            assert!(started.elapsed() >= Duration::from_millis(15));
            assert_eq!(modern_wake(&word, private, u32::MAX), 0);
            queued(&word, private, 0);
        }
    }
}

#[test]
fn wait2_signal_retires_before_handler_and_restart_reparses_absolute_deadline() {
    static ADDRESS: AtomicUsize = AtomicUsize::new(0);
    static PRIVATE: AtomicBool = AtomicBool::new(false);
    static CHANGE_WORD: AtomicBool = AtomicBool::new(false);
    static WAKE_RESULT: AtomicI64 = AtomicI64::new(-1);
    unsafe extern "sysv64" fn handler(_: i32) {
        let address = ADDRESS.load(Ordering::Acquire);
        let private = PRIVATE.load(Ordering::Acquire);
        let key = FutexAddress::resolve(address as _, private).unwrap();
        WAKE_RESULT.store(futex_wake(key, 1, u32::MAX), Ordering::Release);
        if CHANGE_WORD.load(Ordering::Acquire) {
            futex_word(address as _)
                .unwrap()
                .store(1, Ordering::Release);
        }
    }
    #[repr(C)]
    struct Timeout {
        sec: AtomicI64,
        nsec: AtomicI64,
    }
    let action = |restart| signal::Action {
        disposition: signal::Disposition::Handle(handler, 0),
        flags: if restart { signal::SA_RESTART } else { 0 },
        mask: 0,
        restorer: 0,
    };
    let previous = signal::sigaction(signal::SIGUSR2, Some(action(false))).unwrap();
    for private in [false, true] {
        for (restart, changed, change_timeout) in [
            (false, false, true),
            (true, false, false),
            (true, false, true),
            (true, true, false),
            (true, true, true),
        ] {
            signal::sigaction(signal::SIGUSR2, Some(action(restart))).unwrap();
            let word = Arc::new(AtomicI32::new(0));
            ADDRESS.store(word.as_ptr() as usize, Ordering::Release);
            PRIVATE.store(private, Ordering::Release);
            CHANGE_WORD.store(changed, Ordering::Release);
            WAKE_RESULT.store(-1, Ordering::Release);
            let limit = deadline(CLOCK_MONOTONIC, 250);
            let timeout = Arc::new(Timeout {
                sec: AtomicI64::new(limit.tv_sec),
                nsec: AtomicI64::new(limit.tv_nsec),
            });
            let child_timeout = Arc::clone(&timeout);
            let child_word = Arc::clone(&word);
            let (tx, rx) = mpsc::channel();
            let waiter = std::thread::spawn(move || {
                tx.send(interrupt::current_thread_id()).unwrap();
                let started = Instant::now();
                let result = raw(
                    child_word.as_ptr() as usize,
                    0,
                    1,
                    if private { 0x82 } else { 2 },
                    Arc::as_ptr(&child_timeout) as usize,
                    1,
                );
                (result, started.elapsed())
            });
            let tid = rx.recv().unwrap();
            queued(&word, private, 1);
            // Synchronization above proves the original copy is finished.
            // A modern SA_RESTART reenters the syscall and copies arguments
            // again. An unchanged absolute deadline must not become relative.
            if change_timeout {
                timeout.sec.store(0, Ordering::Release);
                timeout.nsec.store(0, Ordering::Release);
            }
            signal::raise_thread_signal(tid, signal::SIGUSR2).unwrap();
            let (result, elapsed) = waiter.join().unwrap();
            let error = if changed {
                EAGAIN
            } else if restart {
                ETIMEDOUT
            } else {
                EINTR
            };
            assert_eq!(result, -i64::from(error));
            assert_eq!(WAKE_RESULT.load(Ordering::Acquire), 0);
            if restart && !changed && !change_timeout {
                assert!(elapsed >= Duration::from_millis(100));
            }
            if restart && !changed && change_timeout {
                assert!(elapsed < Duration::from_millis(200));
            }
            queued(&word, private, 0);
        }
    }
    signal::sigaction(signal::SIGUSR2, Some(previous)).unwrap();
}

#[test]
#[ignore = "isolated raw futex2 wait entry/retirement benchmark"]
fn benchmark_wait2_registration() {
    // Each benchmark subprocess owns its domain, so other live native tests
    // cannot add background rows or contend with this control measurement.
    let domain = 0x4550_2026_1001_0001u64 ^ (u64::from(std::process::id()) << 17);
    kinakaze_runtime::authority::install_helper_domain(domain).unwrap();
    let word = AtomicI32::new(0);
    let past = KernelTimespec::default();
    let timeout = &past as *const _ as usize;
    let iterations = 5000;
    let run = |private, count| {
        let started = Instant::now();
        for _ in 0..count {
            assert_eq!(
                raw(
                    word.as_ptr() as usize,
                    0,
                    1,
                    if private { 0x82 } else { 2 },
                    timeout,
                    1
                ),
                -i64::from(ETIMEDOUT)
            );
        }
        assert_eq!(modern_wake(&word, private, u32::MAX), 0);
        started.elapsed().as_nanos()
    };
    let private_ns = run(true, iterations);
    let shared_ns = run(false, iterations);
    let key = crate::futex::Key::anonymous([870, std::process::id() as u64, 1], 0);
    let background: Vec<_> = (0..16)
        .map(|_| {
            crate::futex::WaitGroup::enqueue_batch(&(0..128).collect::<Vec<_>>(), || {
                Ok(vec![key; 128])
            })
            .unwrap()
        })
        .collect();
    let background_iterations = 128;
    let shared_background_ns = run(false, background_iterations);
    drop(background);
    assert_eq!(crate::futex::count_for_test(key), 0);
    println!(
        "WAIT2_BENCH {{\"optimized\":{},\"iterations\":{},\"background_iterations\":{},\"private_ns\":{},\"shared_ns\":{},\"shared_background_ns\":{}}}",
        crate::futex::optimized(),
        iterations,
        background_iterations,
        private_ns,
        shared_ns,
        shared_background_ns
    );
}

#[test]
fn wait2_restart_rechecks_timeout_mapping_after_handler() {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    };
    unsafe extern "sysv64" fn handler(_: i32) {}
    let previous = signal::sigaction(
        signal::SIGUSR2,
        Some(signal::Action {
            disposition: signal::Disposition::Handle(handler, 0),
            flags: signal::SA_RESTART,
            mask: 0,
            restorer: 0,
        }),
    )
    .unwrap();
    for private in [false, true] {
        let word = Arc::new(AtomicI32::new(0));
        let page = unsafe {
            VirtualAlloc(
                core::ptr::null(),
                4096,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        assert!(!page.is_null());
        unsafe {
            page.cast::<KernelTimespec>()
                .write(deadline(CLOCK_MONOTONIC, 5000))
        };
        let timeout = page as usize;
        let child_word = Arc::clone(&word);
        let (tx, rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            tx.send(interrupt::current_thread_id()).unwrap();
            raw(
                child_word.as_ptr() as usize,
                0,
                1,
                if private { 0x82 } else { 2 },
                timeout,
                1,
            )
        });
        let tid = rx.recv().unwrap();
        queued(&word, private, 1);
        assert_ne!(unsafe { VirtualFree(page, 0, MEM_RELEASE) }, 0);
        signal::raise_thread_signal(tid, signal::SIGUSR2).unwrap();
        assert_eq!(waiter.join().unwrap(), -i64::from(EFAULT));
        assert_eq!(modern_wake(&word, private, u32::MAX), 0);
        queued(&word, private, 0);
    }
    signal::sigaction(signal::SIGUSR2, Some(previous)).unwrap();
}

#[test]
fn legacy_waits_restart_only_without_a_timeout_and_with_sa_restart() {
    static HANDLED: AtomicBool = AtomicBool::new(false);
    unsafe extern "sysv64" fn handler(_: i32) {
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
        for timed in [false, true] {
            for restart in [false, true] {
                HANDLED.store(false, Ordering::Release);
                signal::sigaction(signal::SIGUSR2, Some(action(restart))).unwrap();
                let word = Arc::new(AtomicI32::new(0));
                let child_word = Arc::clone(&word);
                let (tx, rx) = mpsc::channel();
                let waiter = std::thread::spawn(move || {
                    let timeout = KernelTimespec {
                        tv_sec: 5,
                        tv_nsec: 0,
                    };
                    tx.send(interrupt::current_thread_id()).unwrap();
                    unsafe {
                        kinakaze_abi_syscall_raw(
                            SYS_FUTEX,
                            child_word.as_ptr() as u64,
                            (FUTEX_WAIT | if private { FUTEX_PRIVATE_FLAG } else { 0 }) as u64,
                            0,
                            if timed {
                                &timeout as *const _ as u64
                            } else {
                                0
                            },
                            0,
                            0,
                        )
                    }
                });
                let tid = rx.recv().unwrap();
                queued(&word, private, 1);
                signal::raise_thread_signal(tid, signal::SIGUSR2).unwrap();
                let started = Instant::now();
                while !HANDLED.load(Ordering::Acquire) {
                    assert!(started.elapsed() < Duration::from_secs(5));
                    std::thread::yield_now();
                }
                if restart && !timed {
                    queued(&word, private, 1);
                    assert_eq!(modern_wake(&word, private, u32::MAX), 1);
                    assert_eq!(waiter.join().unwrap(), 0);
                } else {
                    assert_eq!(waiter.join().unwrap(), -i64::from(EINTR));
                }
                queued(&word, private, 0);
            }
        }
    }
    signal::sigaction(signal::SIGUSR2, Some(previous)).unwrap();
}
