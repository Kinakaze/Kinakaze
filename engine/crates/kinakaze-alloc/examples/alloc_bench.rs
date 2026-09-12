use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::time::Instant;

const BATCH: usize = 64;
const SMALL_SIZES: [usize; 13] = [16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024];
const MIXED_SIZES: [usize; 13] = [
    16, 48, 128, 384, 1024, 2048, 4096, 4097, 8192, 12_000, 32_000, 48_000, 65_536,
];

fn run_worker<const N: usize>(seed: usize, rounds: usize, sizes: &[usize]) {
    let mut pointers = [std::ptr::null_mut(); N];
    for round in 0..rounds {
        for (index, pointer) in pointers.iter_mut().enumerate() {
            let size = sizes[(round + index + seed) % sizes.len()];
            *pointer = unsafe { kinakaze_alloc::malloc(black_box(size)) };
            assert!(!pointer.is_null());
            unsafe { pointer.write((index ^ round) as u8) };
        }
        for pointer in pointers.iter().rev() {
            unsafe { kinakaze_alloc::free(*pointer) };
        }
    }
}

fn benchmark<const N: usize>(
    profile: &str,
    threads: usize,
    rounds: usize,
    sizes: &'static [usize],
) {
    run_worker::<N>(0, 32, sizes);
    let barrier = Arc::new(Barrier::new(threads + 1));
    let mut workers = Vec::with_capacity(threads);
    for index in 0..threads {
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            run_worker::<N>(index, rounds, sizes);
        }));
    }
    barrier.wait();
    let started = Instant::now();
    for worker in workers {
        worker.join().unwrap();
    }
    let elapsed = started.elapsed();
    let pairs = threads * rounds * N;
    println!(
        "profile={profile} threads={threads} allocation_pairs={pairs} elapsed_ms={:.3} pairs_per_second={:.0}",
        elapsed.as_secs_f64() * 1_000.0,
        pairs as f64 / elapsed.as_secs_f64()
    );
}

fn main() {
    kinakaze_alloc::initialize().expect("fixed allocator arena");
    let default_threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .min(8);
    let threads = std::env::var("KINAKAZE_ALLOC_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or(default_threads);
    benchmark::<BATCH>("small", threads, 2_000, &SMALL_SIZES);
    benchmark::<BATCH>("mixed", threads, 500, &MIXED_SIZES);
    benchmark::<1024>("bulk", threads, 500, &[128]);
}
