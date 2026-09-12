"""Bounded libc scans, including guard pages and warm memory throughput."""
import ctypes as C
import json
import statistics
import time

libc = C.CDLL(None, use_errno=True)
libc.memchr.argtypes = [C.c_void_p, C.c_int, C.c_size_t]
libc.memchr.restype = C.c_void_p
libc.memcmp.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t]
libc.memcmp.restype = C.c_int
libc.mmap.argtypes = [C.c_void_p, C.c_size_t, C.c_int, C.c_int, C.c_int, C.c_long]
libc.mmap.restype = C.c_void_p
libc.mprotect.argtypes = [C.c_void_p, C.c_size_t, C.c_int]
libc.munmap.argtypes = [C.c_void_p, C.c_size_t]


def checked_scans():
    # End each legal input immediately before an inaccessible page. A SIMD
    # implementation must not round its final read past the caller's length.
    base = libc.mmap(None, 8192, 3, 0x22, -1, 0)
    assert base not in (None, C.c_void_p(-1).value), C.get_errno()
    try:
        assert libc.mprotect(base + 4096, 4096, 0) == 0
        reference = C.create_string_buffer(b"a" * 512)
        for count in list(range(130)) + [255, 256, 257, 511]:
            start = base + 4096 - count
            C.memset(start, ord("a"), count)
            assert libc.memchr(start, ord("z"), count) is None
            assert libc.memcmp(start, reference, count) == 0
            for index in sorted({0, count // 2, count - 1}) if count else []:
                C.c_ubyte.from_address(start + index).value = 255
                assert libc.memchr(start, -1, count) == start + index
                assert libc.memcmp(start, reference, count) > 0
                assert libc.memcmp(reference, start, count) < 0
                C.c_ubyte.from_address(start + index).value = ord("a")
    finally:
        assert libc.munmap(base, 8192) == 0


def main():
    checked_scans()
    count = 8 * 1024 * 1024
    left = C.create_string_buffer(b"a" * count)
    right = C.create_string_buffer(b"a" * count)
    report = {"bytes_per_call": count, "calls_per_sample": 8}
    for name, operation in (
        ("memchr_missing", lambda: libc.memchr(left, ord("z"), count)),
        ("memcmp_equal", lambda: libc.memcmp(left, right, count)),
    ):
        samples = []
        for iteration in range(9):
            started = time.perf_counter_ns()
            for _ in range(8):
                assert not operation()
            elapsed = (time.perf_counter_ns() - started) / 1_000_000
            if iteration >= 2:
                samples.append(elapsed)
        median = statistics.median(samples)
        report[name] = {"median_ms": median, "gib_per_second": (count * 8 / 2**30) / (median / 1000), "samples_ms": samples}
    print(json.dumps(report, sort_keys=True), flush=True)
    print("MemoryScanProbe: PASS", flush=True)


if __name__ == "__main__":
    main()
