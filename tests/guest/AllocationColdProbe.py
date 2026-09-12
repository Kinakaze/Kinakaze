"""Fresh mixed-size allocations, payload validation and batched release."""
import ctypes as C
import json
from pathlib import Path

fixture = C.CDLL(str(Path(__file__).with_suffix('.so')))
fixture.probe.argtypes = [C.POINTER(C.c_double), C.POINTER(C.c_size_t)]
times = (C.c_double * 2)()
stats = (C.c_size_t * 4)()
assert fixture.probe(times, stats) == 0
assert stats[2] == 0, list(stats)
assert stats[0] == stats[3], list(stats)
print(json.dumps(dict(allocate_ms=times[0], validate_free_ms=times[1],
    arena_growth_bytes=stats[0], peak_live_growth_bytes=stats[1],
    retained_live_growth_bytes=stats[2], reusable_growth_bytes=stats[3])), flush=True)
print('AllocationColdProbe: PASS', flush=True)
