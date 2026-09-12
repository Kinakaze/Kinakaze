"""C buffer growth cost and preservation, without Python allocations in the loop."""
import ctypes as C
import json
import os
from pathlib import Path
import statistics
import threading

fixture = C.CDLL(str(Path(__file__).with_suffix('.so')))
fixture.probe.argtypes = [C.POINTER(C.c_double), C.POINTER(C.c_uint)]
milliseconds, moves = (C.c_double * 7)(), (C.c_uint * 7)()
result = fixture.probe(milliseconds, moves)
assert result == 0, result
print(json.dumps(dict(scope='C realloc: seven buffers, each 1 MiB to 32 MiB',
                      samples_ms=list(milliseconds), median_ms=statistics.median(milliseconds),
                      pointer_moves=list(moves))), flush=True)
fixture.concurrent_growth.argtypes = [C.c_uint]
results = []
threads = [threading.Thread(target=lambda seed=seed: results.append(fixture.concurrent_growth(seed)))
           for seed in range(8)]
for thread in threads: thread.start()
for thread in threads: thread.join(timeout=10)
assert all(not thread.is_alive() for thread in threads)
assert results == [0] * 8, results
fixture.grow_for_fork.restype = C.c_void_p
pointer = fixture.grow_for_fork()
assert pointer and C.c_ubyte.from_address(pointer).value == 0x73
libc = C.CDLL(None)
libc.free.argtypes = [C.c_void_p]
child = os.fork()
if child == 0:
    assert C.c_ubyte.from_address(pointer + 6*1024*1024-1).value == 0x92
    C.c_ubyte.from_address(pointer).value = 0x11
    libc.free(pointer)
    os._exit(0)
assert os.waitpid(child, 0) == (child, 0)
assert C.c_ubyte.from_address(pointer).value == 0x73
assert C.c_ubyte.from_address(pointer + 6*1024*1024-1).value == 0x92
libc.free(pointer)
print('AllocationGrowthProbe: PASS', flush=True)
