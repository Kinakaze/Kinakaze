"""Exercise raw futex_waitv via guest ABI, mmap aliases and a fork child."""
import ctypes as c
import errno
import json
import mmap
import os
import tempfile
import threading
import time

libc = c.CDLL('libc.so.6', use_errno=True)
libc.syscall.restype = c.c_long
libc.syscall.argtypes = [c.c_long] + [c.c_ulonglong] * 6


def call(number, *arguments):
    c.set_errno(0)
    result = libc.syscall(number, *arguments, *([0] * (6 - len(arguments))))
    return result, c.get_errno()


class Timespec(c.Structure):
    _fields_ = [('seconds', c.c_longlong), ('nanoseconds', c.c_longlong)]


class Entry(c.Structure):
    _fields_ = [('value', c.c_ulonglong), ('address', c.c_ulonglong),
                ('flags', c.c_uint), ('reserved', c.c_uint)]


def wait(entries, seconds=5):
    end = time.monotonic_ns() + int(seconds * 1_000_000_000)
    deadline = Timespec(end // 1_000_000_000, end % 1_000_000_000)
    return call(449, c.addressof(entries), len(entries), 0, c.addressof(deadline), 1)


def wake_until(address, flags):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        selected, error = call(454, address, 0xffffffff, 1, flags)
        assert error == 0 and selected in (0, 1), (selected, error)
        if selected:
            return
        time.sleep(.001)
    raise AssertionError('vector waiter never became wakeable')


assert c.sizeof(Entry) == 24
word = c.c_int(7)
valid = Entry(7, c.addressof(word), 0x82, 0)
one = (Entry * 1)(valid)
assert call(449, 0, 1) == (-1, errno.EINVAL)
assert call(449, c.addressof(one), 129) == (-1, errno.EINVAL)
assert call(449, 1, 1) == (-1, errno.EFAULT)
for invalid in [Entry(7, c.addressof(word), 6, 0),
                Entry(7, c.addressof(word), 0x82, 1),
                Entry(1 << 32, c.addressof(word), 0x82, 0)]:
    entries = (Entry * 1)(invalid)
    assert call(449, c.addressof(entries), 1) == (-1, errno.EINVAL)
one[0].value = 8
assert call(449, c.addressof(one), 1, 0, 0, 999) == (-1, errno.EAGAIN)
one[0].value = 7
past = Timespec()
assert call(449, c.addressof(one), 1, 0, c.addressof(past), 1) == (-1, errno.ETIMEDOUT)

words = (c.c_int * 128)()
entries = (Entry * 128)(*[Entry(0, c.addressof(words) + 4 * i, 0x82 if i % 2 == 0 else 2, 0)
                         for i in range(128)])
result = []
thread = threading.Thread(target=lambda: result.append(wait(entries)))
thread.start()
wake_until(entries[127].address, 2)
thread.join(timeout=6)
assert not thread.is_alive() and result == [(127, 0)], result
for entry in entries:
    assert call(454, entry.address, 0xffffffff, 1, entry.flags) == (0, 0)

with tempfile.TemporaryFile() as file:
    os.ftruncate(file.fileno(), 4096)
    first = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    second = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    first_word = c.c_int.from_buffer(first)
    alias_word = c.c_int.from_buffer(second)
    first_word.value = 0
    assert alias_word.value == 0
    private = c.c_int()
    vector = (Entry * 2)(Entry(0, c.addressof(private), 0x82, 0),
                         Entry(0, c.addressof(first_word), 2, 0))
    result = []
    thread = threading.Thread(target=lambda: result.append(wait(vector)))
    thread.start()
    wake_until(c.addressof(alias_word), 2)
    thread.join(timeout=6)
    assert not thread.is_alive() and result == [(1, 0)], result
    assert call(454, c.addressof(first_word), 0xffffffff, 1, 2) == (0, 0)

    # Create the child after all Python worker threads have joined. The file
    # mapping retains shared identity through native fork restoration.
    reader, writer = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(reader)
        outcome = wait(vector)
        os.write(writer, json.dumps(outcome).encode())
        os.close(writer)
        os._exit(int(outcome != (1, 0)))
    os.close(writer)
    wake_until(c.addressof(alias_word), 2)
    outcome = os.read(reader, 4096)
    os.close(reader)
    waited, status = os.waitpid(pid, 0)
    assert waited == pid and status == 0 and json.loads(outcome) == [1, 0], (status, outcome)
    assert call(454, c.addressof(first_word), 0xffffffff, 1, 2) == (0, 0)
    del first_word, alias_word
    first.close()
    second.close()

print('FUTEX_VECTOR_PASS', flush=True)
