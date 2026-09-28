"""Measure CPU and elapsed time of real timed signal waits, excluding startup."""
import json
import signal
import time

iterations = 40
timeout = .025
previous = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR1})
try:
    started = time.monotonic()
    cpu = time.process_time()
    for _ in range(iterations):
        assert signal.sigtimedwait({signal.SIGUSR1}, timeout) is None
    cpu = time.process_time() - cpu
    elapsed = time.monotonic() - started
    assert elapsed >= iterations * timeout
    print('SIGNAL_WAIT_PERF', json.dumps(dict(
        iterations=iterations, requested_ms=iterations * timeout * 1000,
        elapsed_ms=elapsed * 1000, cpu_ms=cpu * 1000)), flush=True)
finally:
    signal.pthread_sigmask(signal.SIG_SETMASK, previous)
