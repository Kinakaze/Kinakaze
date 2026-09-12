"""Measure joined worker churn and heap reuse, including live return values."""
import ctypes as C
import json
from pathlib import Path
import statistics

library = C.CDLL(str(Path(__file__).with_suffix('.so')))
library.probe.argtypes = [C.POINTER(C.c_double), C.POINTER(C.c_size_t)]
library.probe.restype = C.c_int
elapsed = (C.c_double * 4)()
stats = (C.c_size_t * 12)()
assert library.probe(elapsed, stats) == 0
print(json.dumps({
    'scope': '24 sequential create/allocate/free/join cycles per sample; committed heap is not RSS',
    'workers_24': {'median_ms': statistics.median(elapsed), 'samples_ms': list(elapsed)},
    'arena_growth_bytes': stats[9],
    'live_growth_bytes': stats[10],
    'reusable_growth_bytes': stats[11],
    'arena_growth_by_round': [stats[i * 3] for i in range(4)],
}, sort_keys=True), flush=True)
print('AllocationThreadRecycleProbe: PASS', flush=True)
