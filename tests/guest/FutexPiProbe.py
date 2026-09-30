"""Raw PI lock ownership, FIFO handoff, deadlines, signals and shared aliases."""
import ctypes as c
import errno
import json
import mmap
import os
import signal
import tempfile
import threading
import time

libc = c.CDLL('libc.so.6', use_errno=True)
libc.syscall.restype = c.c_long
libc.syscall.argtypes = [c.c_long] + [c.c_ulonglong] * 6


def call(number, *arguments):
    c.set_errno(0)
    value = libc.syscall(number, *arguments, *([0] * (6 - len(arguments))))
    return value, c.get_errno()


class Timespec(c.Structure):
    _fields_ = [('seconds', c.c_longlong), ('nanoseconds', c.c_longlong)]


def deadline(clock=1, seconds=5):
    value = time.time() if clock == 0 else time.monotonic()
    value += seconds
    return Timespec(int(value), int((value % 1) * 1_000_000_000))


def pi(word, command, private, timeout=0):
    return call(202, c.addressof(word), command | (128 if private else 0), 0, timeout)


def until(condition):
    end = time.monotonic() + 5
    while not condition():
        assert time.monotonic() < end, 'PI state did not become visible'
        time.sleep(0.001)


rows = []
assert call(186)[0] == os.getpid(), 'leader TID must use the guest PID namespace'
for private in [False, True]:
    word = c.c_uint(0)
    tid = call(186)[0]
    for command in [6, 8, 13]:
        assert pi(word, command, private) == (0, 0)
        assert word.value == tid
        assert pi(word, command, private) == (-1, errno.EDEADLK)
        assert pi(word, 7, private, 1) == (0, 0)
        assert word.value == 0
    assert pi(word, 7, private) == (-1, errno.EPERM)
    word.value = 0xc0000000
    assert pi(word, 13, private) == (0, 0)
    assert word.value == (0x40000000 | tid)
    assert pi(word, 7, private) == (0, 0)
    assert pi(word, 13, private) == (0, 0)
    errors = []

    def busy():
        zero = Timespec(0, 0)
        errors.extend([pi(word, 8, private, 1), pi(word, 13, private, c.addressof(zero))])

    thread = threading.Thread(target=busy)
    thread.start()
    thread.join(5)
    assert not thread.is_alive()
    assert errors == [(-1, errno.EAGAIN), (-1, errno.ETIMEDOUT)], errors
    acquired = []
    queued = []
    results = []

    def wait(index):
        limit = deadline()
        queued.append(index)
        result = pi(word, 13, private, c.addressof(limit))
        results.append(result)
        if result == (0, 0):
            acquired.append(index)
            assert word.value & 0x3fffffff == call(186)[0]
            assert pi(word, 7, private) == (0, 0)

    # Use WAITERS plus a short registration settling period for the guest
    # schedule. Native tests additionally inspect each exact queue count.
    threads = []
    for index in range(3):
        thread = threading.Thread(target=wait, args=(index,))
        thread.start()
        threads.append(thread)
        until(lambda: len(queued) == index + 1 and word.value & 0x80000000)
        time.sleep(0.02)
    assert call(202, c.addressof(word), 1 | (128 if private else 0), 1) == (-1, errno.EINVAL)
    target = c.c_uint(0)
    assert call(202, c.addressof(word), 3 | (128 if private else 0), 0, 1, c.addressof(target)) == (-1, errno.EINVAL)
    assert pi(word, 7, private) == (0, 0)
    for thread in threads:
        thread.join(5)
        assert not thread.is_alive()
    assert results == [(0, 0)] * 3, results
    assert acquired == [0, 1, 2], acquired
    assert word.value == 0
    rows.append(dict(private=private, acquired=acquired, busy=errors))

class Action(c.Structure):
    _fields_ = [('handler', c.c_ulonglong), ('flags', c.c_ulonglong),
                ('restorer', c.c_ulonglong), ('mask', c.c_ulonglong)]


active = None
delivered = []
handler_wakes = []


@c.CFUNCTYPE(None, c.c_int)
def handler(number):
    word, private, limit, mutate = active
    handler_wakes.append(call(202, c.addressof(word), 1 | (128 if private else 0), 1))
    if mutate:
        limit.seconds = -1
    delivered.append(number)


for private in [False, True]:
    for command in [6, 13]:
        for restart in [False, True]:
            for mutate in [False, True]:
                word = c.c_uint(0)
                assert pi(word, 13, private) == (0, 0)
                limit = deadline(0 if command == 6 else 1)
                active = word, private, limit, mutate
                old = Action()
                action = Action(c.cast(handler, c.c_void_p).value, 0x10000000 if restart else 0, 0, 0)
                assert call(13, 12, c.addressof(action), c.addressof(old), 8) == (0, 0)
                result, ids = [], []
                before = len(delivered)

                def wait_signal():
                    ids.append(call(186)[0])
                    result.append(pi(word, command, private, c.addressof(limit)))
                    if result[-1] == (0, 0):
                        assert pi(word, 7, private) == (0, 0)

                waiter = threading.Thread(target=wait_signal)
                waiter.start()
                until(lambda: ids and call(202, c.addressof(word), 1 | (128 if private else 0), 1) == (-1, errno.EINVAL))
                assert call(234, os.getpid(), ids[0], 12) == (0, 0)
                until(lambda: len(delivered) == before + 1)
                assert handler_wakes[-1] == (0, 0), handler_wakes
                if not mutate:
                    until(lambda: call(202, c.addressof(word), 1 | (128 if private else 0), 1) == (-1, errno.EINVAL))
                assert pi(word, 7, private) == (0, 0)
                waiter.join(6)
                assert not waiter.is_alive()
                assert result == [(-1, errno.EINVAL) if mutate else (0, 0)], (private, command, restart, mutate, result)
                assert call(13, 12, c.addressof(old), 0, 8) == (0, 0)

# File-backed aliases are created independently in a fork child. The owner is
# the namespace leader, whose guest PID need not equal a native Windows TID.
with tempfile.TemporaryFile() as file:
    file.truncate(4096)
    parent_map = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    parent_word = c.c_uint.from_buffer(parent_map)
    ready_r, ready_w = os.pipe()
    release_r, release_w = os.pipe()
    child = os.fork()
    if child == 0:
        assert call(186)[0] == os.getpid(), 'fork child must reset its native leader identity'
        os.close(ready_r)
        os.close(release_w)
        child_map = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
        child_word = c.c_uint.from_buffer(child_map)
        assert c.addressof(child_word) != c.addressof(parent_word)
        assert pi(child_word, 13, False) == (0, 0)
        assert child_word.value == call(186)[0]
        os.write(ready_w, b'R')
        assert os.read(release_r, 1) == b'X'
        # Abrupt process exit deliberately skips registered native TLS drops.
        call(231, 0)
        os._exit(99)
    os.close(ready_w)
    os.close(release_r)
    assert os.read(ready_r, 1) == b'R'
    result = []

    def cross_process_wait():
        limit = deadline()
        result.append(pi(parent_word, 13, False, c.addressof(limit)))
        if result[-1] == (0, 0):
            assert parent_word.value & 0x40000000
            assert pi(parent_word, 7, False) == (0, 0)

    waiter = threading.Thread(target=cross_process_wait)
    waiter.start()
    until(lambda: parent_word.value & 0x80000000)
    os.write(release_w, b'X')
    waiter.join(8)
    assert not waiter.is_alive()
    assert result == [(0, 0)], result
    assert os.waitpid(child, 0)[1] == 0
    os.close(ready_r)
    os.close(release_w)
    del parent_word
    parent_map.close()

print(json.dumps(dict(private_and_unflagged=rows, cross_process_owner_exit=True)))
print('FUTEX_PI_PASS')
