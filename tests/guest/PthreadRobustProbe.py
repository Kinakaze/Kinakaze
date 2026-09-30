"""Private robust mutex recovery on every thread exit path and across fork."""
import ctypes as C
from pathlib import Path

pthread = C.CDLL('libpthread.so.0', mode=C.RTLD_GLOBAL)
attr = C.c_int()
found = C.c_int()
assert pthread.pthread_mutexattr_init(C.byref(attr)) == 0
assert pthread.pthread_mutexattr_setrobust_np(C.byref(attr), 1) == 0
assert pthread.pthread_mutexattr_getrobust_np(C.byref(attr), C.byref(found)) == 0
assert found.value == 1
assert pthread.pthread_mutexattr_destroy(C.byref(attr)) == 0
probe = C.CDLL(str(Path(__file__).with_suffix('.so')))
probe.probe_robust_recovery.argtypes = [C.c_int, C.c_int, C.c_int]
for kind in range(4):
    for mode in range(4):
        for poison in (0, 1):
            result = probe.probe_robust_recovery(kind, mode, poison)
            assert result == 0, (kind, mode, poison, result)
probe.probe_robust_fork.argtypes = [C.c_int]
for phase in range(4):
    result = probe.probe_robust_fork(phase)
    assert result == 0, ('fork', phase, result)
print('PTHREAD_ROBUST_MUTEX_OK', flush=True)
