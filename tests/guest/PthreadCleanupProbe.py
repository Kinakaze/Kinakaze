"""C cleanup handlers run in LIFO order before TLS destructors on exit/cancel."""
import ctypes as C
from pathlib import Path

C.CDLL('libpthread.so.0', mode=C.RTLD_GLOBAL)
probe = C.CDLL(str(Path(__file__).with_suffix('.so')))
probe.probe_cleanup.argtypes = [C.c_int]
for mode in (0, 1, 2, 3):
    result = probe.probe_cleanup(mode)
    assert result == 0, (mode, result)
probe.probe_fork_cleanup.argtypes = [C.c_int]
for mode in (0, 1, 2):
    result = probe.probe_fork_cleanup(mode)
    assert result == 0, ('fork', mode, result)
print('PTHREAD_C_CLEANUP_LIFECYCLE_OK', flush=True)
