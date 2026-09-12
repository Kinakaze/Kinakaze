"""Cancelled blocking and timed cond waits unwind with the mutex held."""
import ctypes as C
import time
from pathlib import Path

C.CDLL('libpthread.so.0', mode=C.RTLD_GLOBAL)
probe = C.CDLL(str(Path(__file__).with_suffix('.so')))
probe.probe_cond_cancel.argtypes = [C.c_int]
for timed in (0, 1):
    start = time.monotonic()
    result = probe.probe_cond_cancel(timed)
    assert result == 0, (timed, result)
    assert time.monotonic() - start < 3
print('PTHREAD_COND_CANCEL_CLEANUP_OK', flush=True)
