"""C11 thread results, once, mutex return codes and condition wake/relock."""
import ctypes
import time

libc = ctypes.CDLL(None)
pointer = ctypes.c_void_p
thread_type = ctypes.c_ulong
callback_type = ctypes.CFUNCTYPE(ctypes.c_int, pointer)
once_callback_type = ctypes.CFUNCTYPE(None)

class Timespec(ctypes.Structure):
    _fields_ = [('seconds', ctypes.c_long), ('nanoseconds', ctypes.c_long)]

for name, arguments in {
    'mtx_init': [pointer, ctypes.c_int], 'mtx_lock': [pointer], 'mtx_trylock': [pointer],
    'mtx_unlock': [pointer], 'mtx_destroy': [pointer], 'cnd_init': [pointer],
    'cnd_wait': [pointer, pointer], 'cnd_timedwait': [pointer, pointer, pointer],
    'cnd_signal': [pointer], 'cnd_broadcast': [pointer], 'cnd_destroy': [pointer],
    'thrd_create': [pointer, callback_type, pointer], 'thrd_join': [thread_type, pointer],
    'thrd_equal': [thread_type, thread_type], 'call_once': [pointer, once_callback_type],
}.items():
    getattr(libc, name).argtypes = arguments
libc.thrd_current.restype = thread_type
mutex = (ctypes.c_ulong * 5)()
condition = (ctypes.c_ulong * 6)()
recursive = (ctypes.c_ulong * 5)()
once = ctypes.c_uint()
started = 0
released = False
initialized = 0
failures = []

@once_callback_type
def initialize():
    global initialized
    initialized += 1

@callback_type
def worker(argument):
    global started
    try:
        libc.call_once(ctypes.byref(once), initialize)
        libc.call_once(ctypes.byref(once), initialize)
        assert libc.thrd_equal(libc.thrd_current(), libc.thrd_current()) != 0
        assert libc.mtx_lock(mutex) == 0
        started += 1
        assert libc.cnd_broadcast(condition) == 0
        while not released:
            assert libc.cnd_wait(condition, mutex) == 0
        assert libc.mtx_unlock(mutex) == 0
        return -17
    except BaseException as error:
        failures.append(repr(error))
        return 99

assert libc.mtx_init(mutex, 0) == 0
assert libc.cnd_init(condition) == 0
assert libc.mtx_init(recursive, 3) == 0
assert libc.mtx_lock(recursive) == 0
assert libc.mtx_trylock(recursive) == 0
assert libc.mtx_unlock(recursive) == 0
assert libc.mtx_unlock(recursive) == 0
libc.mtx_destroy(recursive)
assert libc.mtx_init(recursive, 8) == 2
assert libc.mtx_lock(mutex) == 0
assert libc.mtx_trylock(mutex) == 1
expired = Timespec(int(time.time()) - 1, 0)
assert libc.cnd_timedwait(condition, mutex, ctypes.byref(expired)) == 4
assert libc.mtx_trylock(mutex) == 1
threads = [thread_type(), thread_type()]
for thread in threads:
    assert libc.thrd_create(ctypes.byref(thread), worker, None) == 0
while started != len(threads):
    deadline = Timespec(int(time.time()) + 10, 0)
    assert libc.cnd_timedwait(condition, mutex, ctypes.byref(deadline)) == 0
released = True
assert libc.cnd_broadcast(condition) == 0
assert libc.mtx_unlock(mutex) == 0
for thread in threads:
    result = ctypes.c_int(0)
    assert libc.thrd_join(thread, ctypes.byref(result)) == 0
    assert result.value == -17, (result.value, failures)
assert initialized == 1 and not failures, (initialized, failures)
libc.cnd_destroy(condition)
libc.mtx_destroy(mutex)
print('C11_THREADS_ONCE_MUTEX_CONDITION_TIMEOUT_JOIN_OK')
