"""Measure the production assembly guard, JIT trap cache and VMA copy loops.

Compiles the actual dependency-free Rust modules at opt-level=3. Timings are
microbenchmarks, not an end-to-end syscall/ptrace throughput claim. No guest or
debugger is started. Storage is bounded and all subprocesses have deadlines.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile


SOURCE = r'''
#![allow(dead_code)]
use std::{collections::BTreeMap, hint::black_box, sync::Mutex, time::Instant};
#[path = "@GUARD@"] mod guard;
#[path = "@TRAPS@"] mod traps;
#[path = "@COPY@"] mod copy;
#[link(name = "kernel32")]
unsafe extern "system" {
    fn IsDebuggerPresent() -> i32;
    fn GetCurrentProcess() -> *mut core::ffi::c_void;
    fn GetCurrentThread() -> *mut core::ffi::c_void;
    fn GetProcessAffinityMask(process: *mut core::ffi::c_void, process_mask: *mut usize, system_mask: *mut usize) -> i32;
    fn SetThreadAffinityMask(thread: *mut core::ffi::c_void, mask: usize) -> usize;
}

fn measure(name: &str, bytes: usize, mut operation: impl FnMut()) {
    for _ in 0..128 { operation(); }
    let start = Instant::now();
    for _ in 0..1024 { operation(); }
    let elapsed = start.elapsed().as_nanos().max(1);
    let iterations = (80_000_000u128 * 1024 / elapsed).clamp(32, 20_000_000) as usize;
    let mut samples = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..iterations { operation(); }
        samples.push(start.elapsed().as_secs_f64() * 1e9 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    println!("{{\"case\":\"{name}\",\"bytes\":{bytes},\"median_ns\":{},\"min_ns\":{},\"max_ns\":{},\"iterations\":{iterations}}}", samples[2], samples[0], samples[4]);
}
fn main() {
    assert!(!guard::debugger_present(), "benchmark must run without a debugger");
    assert_eq!(guard::debugger_present(), unsafe { IsDebuggerPresent() != 0 });
    // Keep a hybrid CPU's performance/efficiency core migration from changing
    // the comparison halfway through a set. Only this benchmark is pinned.
    let (mut process_mask, mut system_mask) = (0usize, 0usize);
    assert_ne!(unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process_mask, &mut system_mask) }, 0);
    let affinity = process_mask & process_mask.wrapping_neg();
    assert_ne!(unsafe { SetThreadAffinityMask(GetCurrentThread(), affinity) }, 0);
    println!("{{\"avx2\":{},\"cache_slots\":16384,\"thread_affinity_mask\":{affinity}}}", std::is_x86_feature_detected!("avx2"));
    measure("guard_windows_api", 0, || { black_box(unsafe { IsDebuggerPresent() }); });
    measure("guard_assembly", 0, || { black_box(guard::debugger_present()); });
    let tree = Mutex::new(BTreeMap::new());
    let base = 0x123456000usize;
    for i in 0..1024 { tree.lock().unwrap().insert(base + i * 16, 0usize); traps::syscall(base + i * 16); }
    let mut cursor = 0usize;
    measure("jit_trap_mutex_map", 0, || { cursor = (cursor + 1) & 1023; black_box(tree.lock().unwrap().get(&black_box(base + cursor * 16)).copied()); });
    measure("jit_trap_bounded_cache", 0, || { cursor = (cursor + 1) & 1023; black_box(traps::is_syscall(black_box(base + cursor * 16))); });
    traps::forget(base, 1024 * 16);
    for length in [256usize, 4096, 16384, 65536, 524288, 786432, 1048576, 1310720, 2097152, 8388608] {
        // mremap copies page-aligned VMAs; ordinary Vec alignment would unfairly
        // penalize 32-byte stores crossing cache lines on some processors.
        let layout = std::alloc::Layout::from_size_align(length + 64, 4096).unwrap();
        let src = unsafe { std::alloc::alloc(layout) };
        let dst = unsafe { std::alloc::alloc(layout) };
        assert!(!src.is_null() && !dst.is_null());
        unsafe { std::ptr::write_bytes(src, 0x5a, length + 64); std::ptr::write_bytes(dst, 0, length + 64); }
        measure("copy_compiler", length, || unsafe { std::ptr::copy_nonoverlapping(black_box(src), black_box(dst), black_box(length)); black_box(std::slice::from_raw_parts(dst, length)); });
        if std::is_x86_feature_detected!("avx2") {
            measure("copy_avx2", length, || unsafe { copy::avx2(black_box(src), black_box(dst), black_box(length)); black_box(std::slice::from_raw_parts(dst, length)); });
        }
        measure("copy_dispatch", length, || unsafe { copy::copy(black_box(src), black_box(dst), black_box(length)); black_box(std::slice::from_raw_parts(dst, length)); });
        unsafe {
            assert_eq!(std::slice::from_raw_parts(dst, length), std::slice::from_raw_parts(src, length));
            std::alloc::dealloc(src, layout); std::alloc::dealloc(dst, layout);
        }
    }
}
'''


def run(root, output):
    if os.name != 'nt' or platform.machine().lower() not in ('amd64', 'x86_64'):
        raise SystemExit('This benchmark requires Windows x64.')
    compiler = shutil.which('rustc')
    if not compiler:
        raise SystemExit('rustc is required.')
    source = SOURCE
    hashes = {}
    for key, path in {
        'GUARD': 'libs/libc/src/ptrace/fast.rs',
        'TRAPS': 'engine/crates/guest-engine/src/execution/traps.rs',
        'COPY': 'libs/libc/src/fdio/remap/copy.rs',
    }.items():
        source = source.replace('@' + key + '@', (root / path).as_posix())
        hashes[path] = hashlib.sha256((root / path).read_bytes()).hexdigest()
    with tempfile.TemporaryDirectory(prefix='kinakaze-fastpaths-') as directory:
        directory = Path(directory)
        path = directory / 'benchmark.rs'
        executable = directory / 'benchmark.exe'
        path.write_text(source, encoding='utf-8')
        subprocess.run([compiler, '--edition=2024', '-C', 'opt-level=3',
                        str(path), '-o', str(executable)], check=True, timeout=120)
        completed = subprocess.run([str(executable)], check=True,
                                   capture_output=True, text=True, timeout=90)
    records = [json.loads(line) for line in completed.stdout.splitlines() if line]
    report = dict(platform=platform.platform(), processor=platform.processor(),
                  rustc=subprocess.check_output([compiler, '--version'], text=True).strip(),
                  compiler_options=['--edition=2024', '-C', 'opt-level=3'],
                  scope=__doc__, source_sha256=hashes,
                  capabilities=records[0], measurements=records[1:])
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2), encoding='utf-8')
    for record in records[1:]:
        print(f"{record['case']:26s} {record['bytes']:8d} bytes {record['median_ns']:12.3f} ns")
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('artifacts/syscall-fastpaths.json'))
    arguments = parser.parse_args()
    run(Path(__file__).resolve().parents[1], arguments.output)
