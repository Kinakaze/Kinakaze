use super::*;
use std::sync::mpsc;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    VirtualProtect,
};

struct Pause {
    address: usize,
    copied: mpsc::SyncSender<()>,
    resume: mpsc::Receiver<()>,
}
static PAUSE: Mutex<Option<Pause>> = Mutex::new(None);

pub(crate) fn pause_after_copy(address: usize) {
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

fn raw(word: usize, command: u32, private: bool, timeout: usize, mask: u32) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(
            202,
            word as u64,
            u64::from(command | if private { FUTEX_PRIVATE_FLAG } else { 0 }),
            0,
            timeout as u64,
            0,
            u64::from(mask),
        )
    }
}

#[test]
fn legacy_timeout_faults_precede_keys_masks_and_unsupported_command_flags() {
    let invalid = KernelTimespec {
        tv_sec: -1,
        tv_nsec: 0,
    };
    let zero = KernelTimespec::default();
    for private in [false, true] {
        for command in [
            FUTEX_WAIT,
            FUTEX_WAIT_BITSET,
            FUTEX_LOCK_PI,
            FUTEX_LOCK_PI2,
            FUTEX_WAIT_REQUEUE_PI,
        ] {
            for address in [0, 1] {
                assert_eq!(raw(address, command, private, 1, 0), -i64::from(EFAULT));
                assert_eq!(
                    raw(address, command, private, &raw const invalid as usize, 1),
                    -i64::from(EINVAL)
                );
            }
        }
        // This flag is refused by do_futex, after sys_futex has copied utime.
        assert_eq!(
            raw(0, FUTEX_WAIT | FUTEX_CLOCK_REALTIME, private, 1, 1),
            -i64::from(EFAULT)
        );
        assert_eq!(
            raw(
                0,
                FUTEX_WAIT | FUTEX_CLOCK_REALTIME,
                private,
                &raw const invalid as usize,
                1
            ),
            -i64::from(EINVAL)
        );
        assert_eq!(
            raw(
                0,
                FUTEX_WAIT | FUTEX_CLOCK_REALTIME,
                private,
                &raw const zero as usize,
                1
            ),
            -i64::from(ENOSYS)
        );
        // Empty masks precede key lookup, but do not precede timeout copying.
        assert_eq!(
            raw(0, FUTEX_WAIT_BITSET, private, &raw const zero as usize, 0),
            -i64::from(EINVAL)
        );
        assert_eq!(raw(0, FUTEX_WAIT_BITSET, private, 1, 0), -i64::from(EFAULT));
    }
}

#[test]
fn legacy_nontimed_commands_never_copy_the_fourth_argument() {
    for private in [false, true] {
        let word = AtomicI32::new(0);
        for command in [
            FUTEX_WAKE,
            FUTEX_WAKE_BITSET,
            FUTEX_REQUEUE,
            FUTEX_CMP_REQUEUE,
            FUTEX_WAKE_OP,
            7,
            8,
            12,
            63,
        ] {
            assert!(legacy(command, 0, 1usize as _).unwrap().is_none());
        }
        assert_eq!(raw(word.as_ptr() as usize, FUTEX_WAKE, private, 1, 1), 0);
        assert_eq!(
            raw(word.as_ptr() as usize, FUTEX_WAKE_BITSET, private, 1, 1),
            0
        );
        assert_eq!(raw(0, 63, private, 1, 1), -i64::from(ENOSYS));
        assert_eq!(
            raw(0, FUTEX_REQUEUE, private, u32::MAX as usize, 1),
            -i64::from(EINVAL)
        );
    }
}

#[test]
fn copied_timespec_may_be_unaligned_and_retains_clock_and_relative_rules() {
    let mut storage = [0u8; size_of::<KernelTimespec>() + 1];
    let timeout = unsafe { storage.as_mut_ptr().add(1).cast::<KernelTimespec>() };
    unsafe {
        timeout.write_unaligned(KernelTimespec {
            tv_sec: 0,
            tv_nsec: 123_456_789,
        })
    };
    assert_eq!(
        futex_timeout(timeout, false, false),
        Ok(Some(Duration::from_nanos(123_456_789)))
    );
    let relative = legacy(FUTEX_WAIT, 0, timeout).unwrap().unwrap();
    assert_eq!(relative.duration, Some(Duration::from_nanos(123_456_789)));
    for (command, realtime) in [
        (FUTEX_LOCK_PI, true),
        (FUTEX_LOCK_PI2, false),
        (FUTEX_WAIT_REQUEUE_PI, false),
        (FUTEX_WAIT_BITSET, false),
    ] {
        let mut now = KernelTimespec::default();
        assert_eq!(
            unsafe {
                clock_gettime(
                    if realtime {
                        CLOCK_REALTIME
                    } else {
                        CLOCK_MONOTONIC
                    },
                    &raw mut now,
                )
            },
            0
        );
        now.tv_sec += 5;
        unsafe { timeout.write_unaligned(now) };
        let prepared = legacy(command, 0, timeout).unwrap().unwrap();
        assert!(prepared.duration.unwrap() > Duration::from_secs(4));
        assert!(prepared.duration.unwrap() <= Duration::from_secs(5));
        assert!(
            legacy(command, 0, core::ptr::null())
                .unwrap()
                .unwrap()
                .duration
                .is_none()
        );
    }
    let mut now = KernelTimespec::default();
    assert_eq!(unsafe { clock_gettime(CLOCK_REALTIME, &raw mut now) }, 0);
    now.tv_sec += 5;
    unsafe { timeout.write_unaligned(now) };
    for command in [FUTEX_LOCK_PI2, FUTEX_WAIT_BITSET, FUTEX_WAIT_REQUEUE_PI] {
        let prepared = legacy(command, FUTEX_CLOCK_REALTIME, timeout)
            .unwrap()
            .unwrap();
        assert!(prepared.duration.unwrap() > Duration::from_secs(4));
        assert!(prepared.duration.unwrap() <= Duration::from_secs(5));
    }
    // An unaligned expired timeout still checks the expected value first.
    unsafe { timeout.write_unaligned(KernelTimespec::default()) };
    let word = AtomicI32::new(1);
    for private in [false, true] {
        for command in [FUTEX_WAIT, FUTEX_WAIT_BITSET] {
            assert_eq!(
                raw(
                    word.as_ptr() as usize,
                    command,
                    private,
                    timeout as usize,
                    1
                ),
                -i64::from(EAGAIN)
            );
        }
        word.store(0, Ordering::Release);
        assert_eq!(
            raw(
                word.as_ptr() as usize,
                FUTEX_WAIT_BITSET,
                private,
                timeout as usize,
                1
            ),
            -i64::from(ETIMEDOUT)
        );
        word.store(1, Ordering::Release);
    }
}

struct Mapping(*mut u8);
impl Mapping {
    fn new() -> Self {
        let pointer = unsafe {
            VirtualAlloc(
                core::ptr::null(),
                8192,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        }
        .cast::<u8>();
        assert!(!pointer.is_null());
        Self(pointer)
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        assert_ne!(unsafe { VirtualFree(self.0.cast(), 0, MEM_RELEASE) }, 0);
    }
}

#[test]
fn all_wait_apis_report_a_partial_timespec_copy_as_efault() {
    let mapping = Mapping::new();
    let timeout = unsafe { mapping.0.add(4096 - 8).cast::<KernelTimespec>() };
    unsafe { timeout.write_unaligned(KernelTimespec::default()) };
    let mut old = 0;
    assert_ne!(
        unsafe { VirtualProtect(mapping.0.add(4096).cast(), 4096, PAGE_NOACCESS, &mut old) },
        0
    );
    for private in [false, true] {
        let word = AtomicI32::new(0);
        for command in [
            FUTEX_WAIT,
            FUTEX_WAIT_BITSET,
            FUTEX_LOCK_PI,
            FUTEX_LOCK_PI2,
            FUTEX_WAIT_REQUEUE_PI,
        ] {
            assert_eq!(
                raw(0, command, private, timeout as usize, 1),
                -i64::from(EFAULT)
            );
        }
        let flags = if private { 0x82 } else { 2 };
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(455, word.as_ptr() as u64, 0, 1, flags, timeout as u64, 1)
            },
            -i64::from(EFAULT)
        );
        #[repr(C)]
        struct Entry(u64, u64, u32, u32);
        let entry = Entry(0, word.as_ptr() as u64, flags as u32, 0);
        // waitv rejects a null vector before inspecting the timeout. Supply a
        // real descriptor so this assertion exercises its timeout-copy stage.
        assert_eq!(
            unsafe { kinakaze_abi_syscall_raw(449, 0, 1, 0, timeout as u64, 1, 0) },
            -i64::from(EINVAL)
        );
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(449, &raw const entry as u64, 1, 0, timeout as u64, 1, 0)
            },
            -i64::from(EFAULT)
        );
    }
}

#[test]
fn legacy_attempt_keeps_copied_timeout_and_charges_prequeue_delay() {
    for private in [false, true] {
        for command in [FUTEX_WAIT, FUTEX_WAIT_BITSET] {
            for mismatch in [false, true] {
                let mapping = Mapping::new();
                let timeout = mapping.0.cast::<KernelTimespec>();
                let value = if command == FUTEX_WAIT {
                    KernelTimespec {
                        tv_sec: 0,
                        tv_nsec: 200_000_000,
                    }
                } else {
                    let mut now = KernelTimespec::default();
                    assert_eq!(unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut now) }, 0);
                    let nanos = now.tv_nsec + 200_000_000;
                    KernelTimespec {
                        tv_sec: now.tv_sec + nanos / 1_000_000_000,
                        tv_nsec: nanos % 1_000_000_000,
                    }
                };
                unsafe { timeout.write(value) };
                let word = Arc::new(AtomicI32::new(i32::from(mismatch)));
                let address = word.as_ptr() as usize;
                let (copied_tx, copied_rx) = mpsc::sync_channel(1);
                let (resume_tx, resume_rx) = mpsc::sync_channel(1);
                *PAUSE.lock().unwrap() = Some(Pause {
                    address,
                    copied: copied_tx,
                    resume: resume_rx,
                });
                let timeout_address = timeout as usize;
                let waiter = std::thread::spawn(move || {
                    let result = raw(word.as_ptr() as usize, command, private, timeout_address, 1);
                    let flags = if private { 0x82 } else { 2 };
                    assert_eq!(
                        unsafe {
                            kinakaze_abi_syscall_raw(
                                454,
                                word.as_ptr() as u64,
                                u64::from(u32::MAX),
                                1,
                                flags,
                                0,
                                0,
                            )
                        },
                        0
                    );
                    result
                });
                copied_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                drop(mapping); // Only the copied deadline may be used after this point.
                std::thread::sleep(Duration::from_millis(250));
                let resumed = Instant::now();
                resume_tx.send(()).unwrap();
                assert_eq!(
                    waiter.join().unwrap(),
                    -i64::from(if mismatch { EAGAIN } else { ETIMEDOUT })
                );
                assert!(
                    resumed.elapsed() < Duration::from_millis(150),
                    "a copied deadline must not gain a fresh relative budget"
                );
            }
        }
    }
}

#[test]
#[ignore = "isolated raw legacy futex wait entry/retirement benchmark"]
fn benchmark_legacy_wait_registration() {
    let domain = 0x2020_2026_1001_0001u64 ^ (u64::from(std::process::id()) << 17);
    kinakaze_runtime::authority::install_helper_domain(domain).unwrap();
    let word = AtomicI32::new(0);
    let past = KernelTimespec::default();
    let timeout = &raw const past as usize;
    let iterations = 5000;
    let run = |command, private, count| {
        let started = Instant::now();
        for _ in 0..count {
            assert_eq!(
                raw(word.as_ptr() as usize, command, private, timeout, 1),
                -i64::from(ETIMEDOUT)
            );
        }
        let flags = if private { 0x82 } else { 2 };
        assert_eq!(
            unsafe {
                kinakaze_abi_syscall_raw(
                    454,
                    word.as_ptr() as u64,
                    u64::from(u32::MAX),
                    1,
                    flags,
                    0,
                    0,
                )
            },
            0
        );
        started.elapsed().as_nanos()
    };
    let relative_private_ns = run(FUTEX_WAIT, true, iterations);
    let bitset_private_ns = run(FUTEX_WAIT_BITSET, true, iterations);
    let relative_shared_ns = run(FUTEX_WAIT, false, iterations);
    let bitset_shared_ns = run(FUTEX_WAIT_BITSET, false, iterations);
    let key = crate::futex::Key::anonymous([2020, u64::from(std::process::id()), 1], 0);
    let background: Vec<_> = (0..16)
        .map(|_| {
            crate::futex::WaitGroup::enqueue_batch(&(0..128).collect::<Vec<_>>(), || {
                Ok(vec![key; 128])
            })
            .unwrap()
        })
        .collect();
    let background_iterations = 128;
    let relative_background_ns = run(FUTEX_WAIT, false, background_iterations);
    let bitset_background_ns = run(FUTEX_WAIT_BITSET, false, background_iterations);
    drop(background);
    assert_eq!(crate::futex::count_for_test(key), 0);
    println!(
        "LEGACY_WAIT_BENCH {{\"optimized\":{},\"iterations\":{},\"background_iterations\":{},\"relative_private_ns\":{},\"bitset_private_ns\":{},\"relative_shared_ns\":{},\"bitset_shared_ns\":{},\"relative_background_ns\":{},\"bitset_background_ns\":{}}}",
        crate::futex::optimized(),
        iterations,
        background_iterations,
        relative_private_ns,
        bitset_private_ns,
        relative_shared_ns,
        bitset_shared_ns,
        relative_background_ns,
        bitset_background_ns
    );
}
