"""C hot allocation/free batches; warm local reuse and central contention."""
import ctypes as C
import json
from pathlib import Path
import statistics

lib = C.CDLL(str(Path(__file__).with_suffix('.so')))
lib.probe.argtypes = [C.c_uint, C.c_uint, C.c_uint, C.POINTER(C.c_double)]
lib.probe.restype = C.c_int
results = {}
for threads, batch, rounds in ((1, 32, 2048), (1, 512, 128), (4, 512, 128), (8, 512, 128)):
    samples = []
    for repeat in range(7):
        elapsed = C.c_double()
        assert lib.probe(threads, batch, rounds, C.byref(elapsed)) == 0
        samples.append(elapsed.value)
    results[f'threads_{threads}_batch_{batch}'] = {
        'milliseconds': samples, 'median_ms': statistics.median(samples),
        'allocation_free_pairs': threads * batch * rounds}
print(json.dumps(results), flush=True)
print('AllocationBatchProbe: PASS', flush=True)
