"""Fork must preserve recursive ownership without retaining native SRW waiters."""
import ctypes as c
import os
import time

pthread = c.CDLL('libpthread.so.0')
p, i = c.c_void_p, c.c_int

def bind(name, *args):
    function = getattr(pthread, name)
    function.restype, function.argtypes = i, args
    return function

mutex, attribute = (c.c_ulong * 5)(), (c.c_int * 4)()
assert bind('pthread_mutexattr_init', p)(attribute) == 0
assert bind('pthread_mutexattr_settype', p, i)(attribute, 1) == 0
assert bind('pthread_mutex_init', p, p)(mutex, attribute) == 0
lock = bind('pthread_mutex_lock', p)
unlock = bind('pthread_mutex_unlock', p)
trylock = bind('pthread_mutex_trylock', p)
assert lock(mutex) == 0 and lock(mutex) == 0
result = []

@c.CFUNCTYPE(p, p)
def waiter(_):
    result.append(lock(mutex))
    result.append(unlock(mutex))
    return None

thread = c.c_ulong()
assert bind('pthread_create', p, p, p, p)(c.byref(thread), None, waiter, None) == 0
deadline = time.monotonic() + 5
while mutex[0] in (0, 1) and time.monotonic() < deadline:
    time.sleep(0.005)
assert mutex[0] not in (0, 1), 'Native waiter did not queue'
pid = os.fork()
if pid == 0:
    # The parent's waiter is absent here. Releasing the inherited recursion
    # must neither touch that waiter's native stack nor leave the mutex busy.
    valid = unlock(mutex) == 0 and unlock(mutex) == 0
    valid = valid and trylock(mutex) == 0 and unlock(mutex) == 0
    os._exit(0 if valid else 1)
assert os.waitpid(pid, 0) == (pid, 0)
assert unlock(mutex) == 0 and unlock(mutex) == 0
assert bind('pthread_join', c.c_ulong, p)(thread.value, None) == 0
assert result == [0, 0], result

def fork_with_other_owner(transient):
    state = c.c_int()
    release = c.c_int()
    @c.CFUNCTYPE(p, p)
    def holder(_):
        assert lock(mutex) == 0
        state.value = 1
        if transient:
            time.sleep(0.1)
        else:
            while not release.value:
                time.sleep(0.005)
        assert unlock(mutex) == 0
        return None
    assert bind('pthread_create', p, p, p, p)(c.byref(thread), None, holder, None) == 0
    while not state.value:
        time.sleep(0.005)
    # ctypes releases the GIL while libc forks, so the test holder can finish
    # its native critical section during the fork preparation phase.
    native_fork = c.CDLL('libc.so.6').fork
    native_fork.restype, native_fork.argtypes = i, []
    child = native_fork()
    assert child >= 0
    if child == 0:
        status = trylock(mutex)
        valid = status == (0 if transient else 16)
        if status == 0:
            valid = valid and unlock(mutex) == 0
        os._exit(0 if valid else 1)
    try:
        assert os.waitpid(child, 0) == (child, 0)
    finally:
        release.value = 1
        assert bind('pthread_join', c.c_ulong, p)(thread.value, None) == 0

fork_with_other_owner(True)
fork_with_other_owner(False)
assert bind('pthread_mutex_destroy', p)(mutex) == 0
assert bind('pthread_mutexattr_destroy', p)(attribute) == 0
print('RECURSIVE_MUTEX_FORK_NATIVE_WAITERS_OWNERSHIP_OK', flush=True)
