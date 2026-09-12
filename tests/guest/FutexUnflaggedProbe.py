"""Exercise libstdc++/OpenAL's unflagged futex waits on private storage."""
import ctypes as C
import errno
import mmap
import os
import threading
import time

libc = C.CDLL(None, use_errno=True)
syscall = libc.syscall
syscall.restype = C.c_long
syscall.argtypes = [C.c_long, C.c_void_p, C.c_int, C.c_uint,
                    C.c_void_p, C.c_void_p, C.c_uint]


class Timespec(C.Structure):
    _fields_ = [('seconds', C.c_long), ('nanoseconds', C.c_long)]


def futex(word, op, value=0, timeout=None, other=None, bits=0):
    C.set_errno(0)
    result = syscall(202, C.byref(word), op, value,
                     C.byref(timeout) if timeout is not None else None,
                     C.byref(other) if other is not None else None, bits)
    return -C.get_errno() if result == -1 else result


def exercise(word):
    word.value = 7
    assert futex(word, 0, 0, Timespec()) == -errno.EAGAIN
    assert futex(word, 0, 7, Timespec()) == -errno.ETIMEDOUT
    for flags in (0, 128):
        results = []
        waiter = threading.Thread(target=lambda: results.append(
            futex(word, flags, 7, Timespec(3, 0))))
        waiter.start()
        deadline = time.monotonic() + 2
        woken = 0
        while time.monotonic() < deadline and not woken:
            # Linux keeps flagged and unflagged wait queues distinct.
            assert futex(word, (flags ^ 128) | 1, 1) == 0
            woken = futex(word, flags | 1, 1)
            assert woken in (0, 1), woken
            if not woken:
                time.sleep(.001)
        waiter.join(4)
        assert not waiter.is_alive() and woken == 1 and results == [0], results
    # Absolute monotonic bitset timeout, with value check before expiry.
    assert futex(word, 9, 7, Timespec(), bits=1) == -errno.ETIMEDOUT
    assert futex(word, 9, 7, Timespec(), bits=0) == -errno.EINVAL


exercise(C.c_int())
# JVM hardware discovery forks before OpenAL starts: that can turn heap pages
# into native section views without changing their Linux private ownership.
child = os.fork()
if child == 0:
    os._exit(0)
assert os.waitpid(child, 0) == (child, 0)
exercise(C.c_int())
with mmap.mmap(-1, 4096, flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS) as memory:
    word = C.c_int.from_buffer(memory)
    exercise(word)
    del word
with mmap.mmap(-1, 4096, flags=mmap.MAP_SHARED | mmap.MAP_ANONYMOUS) as memory:
    word = C.c_int.from_buffer(memory)
    exercise(word)
    assert futex(word, 129, 1) == 0
    # Unflagged operations may mix private and shared keys.
    local = C.c_int(7)
    assert futex(local, 5, 0, other=word) == 0
    assert word.value == 0
    assert futex(local, 4, 0, other=word, bits=7) == 0
    del word
print('FUTEX_UNFLAGGED_PRIVATE_MEMORY_OK', flush=True)
