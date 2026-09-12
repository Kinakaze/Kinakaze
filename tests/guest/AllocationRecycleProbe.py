"""Transient C buffer recycling, page contents and allocator accounting."""
import ctypes as C
import json
import statistics
from pathlib import Path
fixture=C.CDLL(str(Path(__file__).with_suffix('.so')))
fixture.probe.argtypes=[C.POINTER(C.c_double),C.POINTER(C.c_size_t)]
times=(C.c_double*8)(); stats=(C.c_size_t*4)()
assert fixture.probe(times,stats)==0
print(json.dumps(dict(cold_ms=times[0],median_ms=statistics.median(times[1:]),samples_ms=list(times[1:]),
    arena_growth_bytes=stats[0],live_growth_bytes=stats[1],free_growth_bytes=stats[2],tail_free_bytes=stats[3])),flush=True)
print('AllocationRecycleProbe: PASS',flush=True)
