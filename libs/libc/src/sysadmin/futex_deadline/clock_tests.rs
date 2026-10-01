use super::*;
use std::sync::atomic::AtomicU32;

fn raw(number: i64, args: [u64; 6]) -> i64 {
    unsafe {
        kinakaze_abi_syscall_raw(number, args[0], args[1], args[2], args[3], args[4], args[5])
    }
}

fn future(clock: i32, millis: i64) -> KernelTimespec {
    let (seconds, nanos) = crate::time::read_clock(clock).unwrap();
    let until =
        i128::from(seconds) * 1_000_000_000 + i128::from(nanos) + i128::from(millis) * 1_000_000;
    KernelTimespec {
        tv_sec: (until / 1_000_000_000) as i64,
        tv_nsec: (until % 1_000_000_000) as i64,
    }
}

fn key(word: &AtomicU32, private: bool) -> crate::futex::Key {
    futex_requeue::key(FutexAddress::resolve(word.as_ptr().cast(), private).unwrap()).unwrap()
}

#[test]
fn raw_future_absolute_waits_keep_clock_and_retire_all_registration_metadata() {
    #[repr(C)]
    struct Entry(u64, u64, u32, u32);
    for private in [false, true] {
        let word = AtomicU32::new(0);
        let target = AtomicU32::new(0);
        let address = word.as_ptr() as u64;
        let destination = target.as_ptr() as u64;
        let private_flag = if private { 128 } else { 0 };
        let flags = private_flag | 2;
        let entry = Entry(0, address, flags, 0);
        for clock in [0, 1] {
            for kind in 0..4 {
                let started = Instant::now();
                let time = future(clock, 20);
                let timeout = &raw const time as u64;
                let clock_flag = if clock == 0 { 256 } else { 0 };
                let result = match kind {
                    0 => raw(
                        202,
                        [
                            address,
                            u64::from(9 | private_flag | clock_flag),
                            0,
                            timeout,
                            0,
                            1,
                        ],
                    ),
                    1 => raw(
                        455,
                        [address, 0, 1, u64::from(flags), timeout, clock as u64],
                    ),
                    2 => raw(
                        449,
                        [&raw const entry as u64, 1, 0, timeout, clock as u64, 0],
                    ),
                    _ => raw(
                        202,
                        [
                            address,
                            u64::from(11 | private_flag | clock_flag),
                            0,
                            timeout,
                            destination,
                            0,
                        ],
                    ),
                };
                assert_eq!(
                    result,
                    -i64::from(ETIMEDOUT),
                    "private={private} clock={clock} kind={kind}"
                );
                assert!(started.elapsed() >= Duration::from_millis(10));
                assert!(started.elapsed() < Duration::from_secs(5));
                assert_eq!(
                    raw(
                        454,
                        [address, u64::from(u32::MAX), 1, u64::from(flags), 0, 0]
                    ),
                    0
                );
                assert_eq!(
                    crate::futex::total_records_for_test(&[
                        key(&word, private),
                        key(&target, private)
                    ]),
                    0
                );
            }
        }
    }
}

#[test]
fn raw_pi_future_absolute_timeout_preserves_live_owner_and_retires_waiter() {
    for private in [false, true] {
        let word = AtomicU32::new(0);
        let address = word.as_ptr() as u64;
        let flag = if private { 128 } else { 0 };
        for (command, clock) in [(6, 0), (13, 1), (13 | 256, 0)] {
            assert_eq!(raw(202, [address, 13 | flag, 0, 0, 0, 0]), 0);
            let owner = word.load(Ordering::Acquire) & 0x3fff_ffff;
            std::thread::spawn(move || {
                let started = Instant::now();
                let time = future(clock, 20);
                assert_eq!(
                    raw(
                        202,
                        [address, command | flag, 0, &raw const time as u64, 0, 0]
                    ),
                    -i64::from(ETIMEDOUT)
                );
                assert!(started.elapsed() >= Duration::from_millis(10));
            })
            .join()
            .unwrap();
            assert_eq!(word.load(Ordering::Acquire) & 0x3fff_ffff, owner);
            assert_eq!(crate::futex::count_for_test(key(&word, private)), 0);
            assert_eq!(raw(202, [address, 7 | flag, 0, 0, 0, 0]), 0);
            assert_eq!(word.load(Ordering::Acquire), 0);
            assert_eq!(
                crate::futex::total_records_for_test(&[key(&word, private)]),
                0
            );
        }
    }
}

#[test]
fn requeued_future_absolute_timeout_retires_source_binding_and_target_waiter() {
    for private in [false, true] {
        for clock in [0, 1] {
            let source = AtomicU32::new(0);
            let target = AtomicU32::new(0);
            let a = source.as_ptr() as u64;
            let b = target.as_ptr() as u64;
            let flag = if private { 128 } else { 0 };
            let source_key = key(&source, private);
            let target_key = key(&target, private);
            assert_eq!(raw(202, [b, 13 | flag, 0, 0, 0, 0]), 0);
            let waiter = std::thread::spawn(move || {
                let time = future(clock, 500);
                raw(
                    202,
                    [
                        a,
                        11 | flag | if clock == 0 { 256 } else { 0 },
                        0,
                        &raw const time as u64,
                        b,
                        0,
                    ],
                )
            });
            let started = Instant::now();
            while crate::futex::count_for_test(source_key) != 1 {
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::yield_now();
            }
            assert_eq!(raw(202, [a, 12 | flag, 1, 0, b, 0]), 1);
            assert_eq!(crate::futex::count_for_test(target_key), 1);
            assert_eq!(waiter.join().unwrap(), -i64::from(ETIMEDOUT));
            assert_eq!(crate::futex::count_for_test(target_key), 0);
            assert_eq!(crate::futex::total_records_for_test(&[source_key]), 0);
            assert_eq!(raw(202, [b, 7 | flag, 0, 0, 0, 0]), 0);
            assert_eq!(
                crate::futex::total_records_for_test(&[source_key, target_key]),
                0
            );
        }
    }
}
