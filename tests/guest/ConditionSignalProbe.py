"""Directed signals must reach both forms of pthread condition wait."""
import ctypes as c
import errno
import threading
import time

lib = c.CDLL(None)
callback = c.CFUNCTYPE(None, c.c_int)
lib.pthread_self.restype = c.c_size_t
lib.pthread_kill.argtypes = [c.c_size_t, c.c_int]
lib.signal.argtypes = [c.c_int, callback]
lib.signal.restype = c.c_void_p
for name in ('pthread_mutex_init', 'pthread_cond_init', 'pthread_cond_wait'):
    getattr(lib, name).argtypes = [c.c_void_p, c.c_void_p]
for name in ('pthread_mutex_lock', 'pthread_mutex_trylock', 'pthread_mutex_unlock',
             'pthread_mutex_destroy', 'pthread_cond_destroy'):
    getattr(lib, name).argtypes = [c.c_void_p]
lib.pthread_cond_timedwait.argtypes = [c.c_void_p, c.c_void_p, c.c_void_p]

class Timespec(c.Structure):
    _fields_ = [('seconds', c.c_long), ('nanos', c.c_long)]

errors = []
for timed in (False, True):
    mutex, condition = (c.c_ulong * 8)(), (c.c_ulong * 8)()
    ready, handled = threading.Event(), threading.Event()
    identity = []
    assert lib.pthread_mutex_init(mutex, None) == 0
    assert lib.pthread_cond_init(condition, None) == 0

    @callback
    def handler(_number):
        # The interrupted wait must release its mutex before invoking us.
        result = lib.pthread_mutex_trylock(mutex)
        if result == 0:
            lib.pthread_mutex_unlock(mutex)
        else:
            errors.append(('handler-mutex', result))
        handled.set()

    assert lib.signal(12, handler) != c.c_void_p(-1).value

    def wait():
        try:
            identity.append(lib.pthread_self())
            assert lib.pthread_mutex_lock(mutex) == 0
            try:
                ready.set()
                deadline = time.monotonic() + 3
                absolute = Timespec(int(time.time()) + 5, 0)
                while not handled.is_set():
                    assert time.monotonic() < deadline, 'signal never delivered'
                    result = (lib.pthread_cond_timedwait(condition, mutex, c.byref(absolute))
                              if timed else lib.pthread_cond_wait(condition, mutex))
                    assert result in (0, errno.ETIMEDOUT), result
                # A normal mutex must again be held before returning to us.
                assert lib.pthread_mutex_trylock(mutex) == errno.EBUSY
            finally:
                assert lib.pthread_mutex_unlock(mutex) == 0
        except BaseException as error:
            errors.append(str(error))

    thread = threading.Thread(target=wait)
    thread.start()
    assert ready.wait(3)
    time.sleep(.05)
    assert lib.pthread_kill(identity[0], 12) == 0
    thread.join(6)
    assert not thread.is_alive()
    assert handled.is_set(), errors
    assert not errors, errors
    assert lib.pthread_cond_destroy(condition) == 0
    assert lib.pthread_mutex_destroy(mutex) == 0

print('CONDITION_SIGNAL_MUTEX_REACQUIRED_OK', flush=True)
