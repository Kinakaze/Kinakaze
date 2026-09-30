use super::*;

#[test]
fn shared_queue_compaction_preserves_fifo_masks_and_requeue_tokens() {
    let word = AtomicI32::new(0);
    let key = Key::private(word.as_ptr() as usize).unwrap();
    let destination = Key::anonymous([1, 2, 3], 42);
    let waiters: Vec<_> = (0..24)
        .map(|i| Waiter::enqueue(0, 1 << (i % 3), &|| Ok((key, 0))).unwrap())
        .collect();
    assert_eq!(wake(key, 3, 2), Ok(3));
    for (i, waiter) in waiters.iter().enumerate() {
        assert_eq!(waiter.finish(false).unwrap(), matches!(i, 1 | 4 | 7));
    }
    assert_eq!(requeue(key, destination, 2, 5, None), Ok(7));
    assert_eq!(wake(destination, 100, u32::MAX), Ok(5));
    assert_eq!(wake(key, 100, u32::MAX), Ok(14));
    assert!(waiters.iter().all(|w| w.finish(false).unwrap()));
    assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    assert_eq!(wake(destination, 1, u32::MAX), Ok(0));
}

#[test]
fn canceled_token_is_retired_without_reporting_a_wake() {
    let key = Key::anonymous([4, 5, 6], 7);
    let waiter = Waiter::enqueue(0, u32::MAX, &|| Ok((key, 0))).unwrap();
    assert!(!waiter.finish(true).unwrap());
    assert!(!waiter.finish(false).unwrap());
    assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    drop(waiter);
}

#[test]
#[ignore = "paired release benchmark; tools/benchmark-futex-queues.py"]
fn benchmark_shared_queue() {
    let word = AtomicI32::new(0);
    let key = Key::private(word.as_ptr() as usize).unwrap();
    let other = Key::anonymous([8, 9, 10], 11);
    let began = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    }
    let empty = began.elapsed().as_nanos();
    let background: Vec<_> = (0..256)
        .map(|_| Waiter::enqueue(0, u32::MAX, &|| Ok((other, 0))).unwrap())
        .collect();
    let began = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(wake(key, 1, u32::MAX), Ok(0));
    }
    let unrelated = began.elapsed().as_nanos();
    let began = Instant::now();
    for _ in 0..1_000 {
        let waiter = Waiter::enqueue(0, u32::MAX, &|| Ok((key, 0))).unwrap();
        assert_eq!(wake(key, 1, u32::MAX), Ok(1));
        assert!(waiter.finish(false).unwrap());
    }
    let enqueue_wake = began.elapsed().as_nanos();
    drop(background);
    println!(
        "FUTEX_BENCH {{\"optimized\":{},\"empty_10000_ns\":{empty},\"unrelated_10000_ns\":{unrelated},\"enqueue_wake_1000_ns\":{enqueue_wake}}}",
        optimized()
    );
}
