"""Measure live process queries separately from filesystem metadata calls."""
import ctypes
import json
import os
import statistics
import time

libc = ctypes.CDLL('libc.so.6')
libc.getppid.argtypes = []
libc.getppid.restype = ctypes.c_int
parent = libc.getppid()
assert parent > 0

results = {}
for label, operation in (
    ('getppid', libc.getppid),
    ('stat_gedit', lambda: os.stat('/usr/bin/gedit').st_size),
):
    expected = operation()
    samples = []
    for _ in range(200):
        started = time.perf_counter_ns()
        actual = operation()
        samples.append((time.perf_counter_ns() - started) / 1000)
        assert actual == expected, (label, expected, actual)
    results[label] = {
        'median_us': round(statistics.median(samples), 2),
        'p95_us': round(sorted(samples)[189], 2),
        'total_ms': round(sum(samples) / 1000, 2),
    }
print(json.dumps(results), flush=True)
