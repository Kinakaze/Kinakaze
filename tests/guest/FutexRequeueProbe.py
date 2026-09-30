"""Raw futex2 requeue: mixed flags, aliases, vector indexes and fork wakeups."""
import ctypes as c
import errno
import json
import mmap
import os
import tempfile
import threading
import time
import traceback

libc = c.CDLL('libc.so.6', use_errno=True)
libc.syscall.restype = c.c_long
libc.syscall.argtypes = [c.c_long] + [c.c_ulonglong] * 6


def call(number, *arguments):
    c.set_errno(0)
    result = libc.syscall(number, *arguments, *([0] * (6 - len(arguments))))
    return result, c.get_errno()


class Entry(c.Structure):
    _fields_ = [('value', c.c_ulonglong), ('address', c.c_ulonglong),
                ('flags', c.c_uint), ('reserved', c.c_uint)]


class Timespec(c.Structure):
    _fields_ = [('seconds', c.c_longlong), ('nanoseconds', c.c_longlong)]


def entry(address, flags, expected=None):
    value = c.c_uint.from_address(address).value if expected is None else expected
    return Entry(value, address, flags, 0)


def requeue(source, source_flags, target, target_flags, wake=0, transfer=1, expected=None):
    entries = (Entry * 2)(entry(source, source_flags, expected), entry(target, target_flags, 0))
    return call(456, c.addressof(entries), 0, wake, transfer)


def queued(address, flags, count):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        result, error = requeue(address, flags, address, flags, transfer=0x7fffffff)
        assert error == 0 and result >= 0, (result, error)
        if result == count:
            return
        time.sleep(.001)
    raise AssertionError(('queue size', address, flags, count, result))


def wake(address, flags, mask=0xffffffff):
    return call(454, address, mask, 1, flags)


def wait(address, flags, mask=0xffffffff, seconds=8):
    end = time.monotonic_ns() + int(seconds * 1_000_000_000)
    timeout = Timespec(end // 1_000_000_000, end % 1_000_000_000)
    return call(202, address, 9 | (flags & 0x80), c.c_uint.from_address(address).value,
                c.addressof(timeout), 0, mask)


def spawn(body):
    pid = os.fork()
    if pid == 0:
        try:
            body()
        except BaseException:
            traceback.print_exc()
            os._exit(1)
        os._exit(0)
    return pid


def reap(pid):
    result = os.waitpid(pid, 0)
    assert result == (pid, 0), result


assert c.sizeof(Entry) == 24
word = c.c_int(7)
target_word = c.c_int(13)
address, target_address = c.addressof(word), c.addressof(target_word)
valid = (Entry * 2)(entry(address, 0x82), entry(target_address, 2))
assert call(456, 0) == (-1, errno.EINVAL)
assert call(456, 1, 1, -1, -1) == (-1, errno.EINVAL)
assert call(456, 1, 0, -1, -1) == (-1, errno.EFAULT)
assert call(456, c.addressof(valid), 0, -1, 0) == (-1, errno.EINVAL)
for invalid in [Entry(13, target_address, 6, 0), Entry(13, target_address, 2, 1),
                Entry(1 << 32, target_address, 2, 0)]:
    entries = (Entry * 2)(valid[0], invalid)
    assert call(456, c.addressof(entries)) == (-1, errno.EINVAL)
assert requeue(address, 0x82, 4, 2, expected=8) == (-1, errno.EFAULT)
assert requeue(address, 0x82, target_address, 2, transfer=0, expected=8) == (-1, errno.EAGAIN)
assert requeue(address, 0x82, target_address, 2, transfer=0) == (0, 0)
valid[1].value = 99  # The target descriptor's value is validated, not compared.
assert call(456, c.addressof(valid), 0, 0, 1) == (0, 0)
assert requeue(address, 0x82, 4, 0x82, expected=8) == (-1, errno.EAGAIN)
assert wake(4, 0x82) == (0, 0)
assert wake(0, 0x82) == (0, 0)

for source_flags in [0x82, 2]:
    for target_flags in [0x82, 2]:
        words = (c.c_int * 2)()
        source, target = c.addressof(words), c.addressof(words) + 4
        results = [[], []]
        first = threading.Thread(target=lambda: results[0].append(wait(source, source_flags, 1)))
        second = threading.Thread(target=lambda: results[1].append(wait(source, source_flags, 2)))
        first.start()
        queued(source, source_flags, 1)
        second.start()
        queued(source, source_flags, 2)
        assert requeue(source, source_flags, target, target_flags, wake=1) == (2, 0)
        first.join(timeout=9)
        assert not first.is_alive() and results[0] == [(0, 0)], results
        assert wake(source, source_flags) == (0, 0)
        assert wake(target, target_flags, 1) == (0, 0)
        assert wake(target, target_flags, 2) == (1, 0)
        second.join(timeout=9)
        assert not second.is_alive() and results[1] == [(0, 0)], results
        queued(target, target_flags, 0)

# A tag change at one VA must transfer ownership rather than merge queues.
word.value = 0
result = []
thread = threading.Thread(target=lambda: result.append(wait(address, 0x82)))
thread.start()
queued(address, 0x82, 1)
assert requeue(address, 0x82, 4, 0x82) == (1, 0)
assert wake(4, 0x82) == (1, 0)
thread.join(timeout=9)
assert not thread.is_alive() and result == [(0, 0)], result
for flags in [0x82, 2]:
    other_flags = flags ^ 0x80
    result = []
    thread = threading.Thread(target=lambda: result.append(wait(address, flags)))
    thread.start()
    queued(address, flags, 1)
    assert requeue(address, flags, address, other_flags) == (1, 0)
    assert wake(address, flags) == (0, 0)
    assert wake(address, other_flags) == (1, 0)
    thread.join(timeout=9)
    assert not thread.is_alive() and result == [(0, 0)], result

words = (c.c_int * 3)()
source, sibling, target = [c.addressof(words) + i * 4 for i in range(3)]
vector = (Entry * 2)(entry(sibling, 0x82), entry(source, 0x82))
result = []


def vector_wait():
    end = time.monotonic_ns() + 8_000_000_000
    timeout = Timespec(end // 1_000_000_000, end % 1_000_000_000)
    result.append(call(449, c.addressof(vector), 2, 0, c.addressof(timeout), 1))


thread = threading.Thread(target=vector_wait)
thread.start()
queued(source, 0x82, 1)
assert requeue(source, 0x82, target, 2) == (1, 0)
assert wake(target, 2) == (1, 0)
thread.join(timeout=9)
assert not thread.is_alive() and result == [(1, 0)], result
assert wake(sibling, 0x82) == (0, 0)

result = []
thread = threading.Thread(target=lambda: result.append(wait(source, 0x82, seconds=.15)))
thread.start()
queued(source, 0x82, 1)
assert requeue(source, 0x82, target, 2) == (1, 0)
thread.join(timeout=2)
assert not thread.is_alive() and result == [(-1, errno.ETIMEDOUT)], result
assert wake(target, 2) == (0, 0)

with tempfile.TemporaryFile() as file:
    os.ftruncate(file.fileno(), 4096)
    first = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    second = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    shared = c.c_int.from_buffer(first)
    alias = c.c_int.from_buffer(second)
    shared.value = 0
    shared_address, alias_address = c.addressof(shared), c.addressof(alias)

    # Fork before creating a parent worker thread. The child later wakes the
    # parent's migrated PRIVATE waiter through another VA of the shared file.
    reader, writer = os.pipe()

    def child_waker():
        os.close(writer)
        assert os.read(reader, 1) == b'x'
        assert wake(alias_address, 2) == (1, 0)
        os.close(reader)

    pid = spawn(child_waker)
    os.close(reader)
    result = []
    thread = threading.Thread(target=lambda: result.append(wait(source, 0x82)))
    thread.start()
    queued(source, 0x82, 1)
    assert requeue(source, 0x82, shared_address, 2) == (1, 0)
    os.write(writer, b'x')
    os.close(writer)
    reap(pid)
    thread.join(timeout=9)
    assert not thread.is_alive() and result == [(0, 0)], result
    assert wake(shared_address, 2) == (0, 0)

    # An external shared waiter may move to the requeuer's private queue. Its
    # own timeout/wake bookkeeping follows its original token across the move.
    reader, writer = os.pipe()

    def child_waiter():
        os.close(reader)
        outcome = wait(alias_address, 2)
        os.write(writer, json.dumps(outcome).encode())
        os.close(writer)
        assert outcome == (0, 0), outcome

    pid = spawn(child_waiter)
    os.close(writer)
    queued(shared_address, 2, 1)
    assert requeue(shared_address, 2, target, 0x82) == (1, 0)
    assert wake(shared_address, 2) == (0, 0)
    assert wake(target, 0x82) == (1, 0)
    outcome = os.read(reader, 4096)
    os.close(reader)
    reap(pid)
    assert json.loads(outcome) == [0, 0], outcome
    assert wake(target, 0x82) == (0, 0)
    del shared, alias
    first.close()
    second.close()

print('FUTEX_REQUEUE_PASS', flush=True)
