"""Timed Python locks and POSIX semaphore deadlines must use the requested clock."""
import ctypes as C
import errno
import json
import threading
import time

measurements = {}
lock = threading.Lock()
lock.acquire()
started = time.monotonic()
assert not lock.acquire(timeout=.025)
elapsed = time.monotonic() - started
measurements['python_lock_timeout_ms'] = round(elapsed * 1000, 3)
assert .024 <= elapsed < .3, measurements
lock.release()

worker = threading.Thread(target=lambda: time.sleep(.025))
worker.start()
started = time.monotonic()
worker.join(.5)
assert not worker.is_alive(), 'timed join returned before its worker completed'
measurements['python_join_ms'] = round((time.monotonic() - started) * 1000, 3)

libc = C.CDLL(None, use_errno=True)
class Timespec(C.Structure):
    _fields_ = [('tv_sec', C.c_long), ('tv_nsec', C.c_long)]

semaphore = (C.c_long * 4)()
libc.sem_init.argtypes = [C.c_void_p, C.c_int, C.c_uint]
libc.sem_clockwait.argtypes = [C.c_void_p, C.c_int, C.POINTER(Timespec)]
libc.sem_timedwait.argtypes = [C.c_void_p, C.POINTER(Timespec)]
libc.sem_post.argtypes = libc.sem_destroy.argtypes = [C.c_void_p]
assert libc.sem_init(semaphore, 0, 0) == 0

def deadline(clock, delay):
    stamp = time.clock_gettime_ns(clock) + int(delay * 1e9)
    return Timespec(stamp // 1_000_000_000, stamp % 1_000_000_000)

try:
    for clock in (time.CLOCK_MONOTONIC, time.CLOCK_REALTIME):
        target = deadline(clock, .025)
        started = time.monotonic()
        assert libc.sem_clockwait(semaphore, clock, C.byref(target)) == -1
        assert C.get_errno() == errno.ETIMEDOUT, C.get_errno()
        elapsed = time.monotonic() - started
        assert .024 <= elapsed < .3, (clock, elapsed)
        measurements[f'clock_{clock}_timeout_ms'] = round(elapsed * 1000, 3)

        worker = threading.Thread(target=lambda: (time.sleep(.025), libc.sem_post(semaphore)))
        target = deadline(clock, .5)
        worker.start()
        assert libc.sem_clockwait(semaphore, clock, C.byref(target)) == 0
        worker.join(.5)
        assert not worker.is_alive()

    for clock, target in [(time.CLOCK_MONOTONIC, Timespec(0, -1)),
                          (time.CLOCK_REALTIME, Timespec(0, 1_000_000_000)),
                          (time.CLOCK_PROCESS_CPUTIME_ID, deadline(time.CLOCK_REALTIME, .1))]:
        assert libc.sem_clockwait(semaphore, clock, C.byref(target)) == -1
        assert C.get_errno() == errno.EINVAL, (clock, target.tv_nsec, C.get_errno())

    past = Timespec(-1, 0)
    assert libc.sem_timedwait(semaphore, C.byref(past)) == -1
    assert C.get_errno() == errno.ETIMEDOUT
    assert libc.sem_post(semaphore) == 0
    assert libc.sem_timedwait(semaphore, C.byref(past)) == 0, 'available token must beat expired deadline'

    # The nanosecond subtraction must borrow across a whole second correctly.
    target = deadline(time.CLOCK_REALTIME, 0)
    if target.tv_nsec < 850_000_000:
        time.sleep((850_000_000 - target.tv_nsec) / 1e9)
    target = deadline(time.CLOCK_REALTIME, .2)
    started = time.monotonic()
    assert libc.sem_timedwait(semaphore, C.byref(target)) == -1
    assert C.get_errno() == errno.ETIMEDOUT
    elapsed = time.monotonic() - started
    assert .199 <= elapsed < .5, elapsed
    measurements['realtime_second_boundary_ms'] = round(elapsed * 1000, 3)
finally:
    assert libc.sem_destroy(semaphore) == 0

print('SEMAPHORE_CLOCK_WAIT_OK', json.dumps(measurements), flush=True)
