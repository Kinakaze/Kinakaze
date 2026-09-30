"""Exercise Linux futex2 scalar waiting through real guest raw syscalls."""
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


def wait(address, flags, mask=8, seconds=5, clock=1):
    now = time.monotonic_ns() if clock == 1 else time.time_ns()
    end = now + int(seconds * 1_000_000_000)
    deadline = Timespec(end // 1_000_000_000, end % 1_000_000_000)
    return call(455, address, 0, mask, flags, c.addressof(deadline), clock)


def queued(address, flags):
    entries = (Entry * 2)(Entry(0, address, flags, 0), Entry(0, address, flags, 0))
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        result = call(456, c.addressof(entries), 0, 0, 0x7fffffff)
        assert result in [(0, 0), (1, 0)], result
        if result[0] == 1:
            return
        time.sleep(.001)
    raise AssertionError('scalar waiter did not enter its queue')


def join(thread, result):
    thread.join(timeout=6)
    assert not thread.is_alive() and result == [(0, 0)], result


word = c.c_int(7)
address = c.addressof(word)
past = Timespec()
invalid = Timespec(-1, 0)
for flags in [0, 1, 3, 6, 0x102, 0xffffffff]:
    assert call(455, 1, 0, 1, flags, 1, 999) == (-1, errno.EINVAL)
for flags in [2, 0x82]:
    for value, mask in [(1 << 32, 1), (0, 1 << 32)]:
        assert call(455, 1, value, mask, flags, 1, 999) == (-1, errno.EINVAL)
    assert call(455, 1, 0, 0, flags, 1, 1) == (-1, errno.EFAULT)
    assert call(455, 1, 0, 0, flags, c.addressof(invalid), 1) == (-1, errno.EINVAL)
    assert call(455, 1, 0, 0, flags, c.addressof(past), 1) == (-1, errno.EINVAL)
    assert call(455, 4, 0, 1, flags, c.addressof(past), 1) == (-1, errno.EFAULT)
    assert call(455, address, 8, 1, flags, 0, 999) == (-1, errno.EAGAIN)
    assert call(455, address, 8, 1, flags, c.addressof(past), 1) == (-1, errno.EAGAIN)
    assert call(455, address, 7, 1, flags, c.addressof(past), 1) == (-1, errno.ETIMEDOUT)
word.value = -1
assert call(455, address, 0xffffffff, 0xffffffff, 0x82, c.addressof(past), 1) == (-1, errno.ETIMEDOUT)
word.value = 0

for flags in [2, 0x82]:
    for legacy in [False, True]:
        result = []
        thread = threading.Thread(target=lambda: result.append(wait(address, flags)))
        thread.start()
        queued(address, flags)
        assert call(454, address, 0xffffffff, 1, flags ^ 0x80) == (0, 0)
        assert call(454, address, 4, 1, flags) == (0, 0)
        if legacy:
            assert call(202, address, 10 | (flags & 0x80), 1, 0, 0, 8) == (1, 0)
        else:
            assert call(454, address, 8, 1, flags) == (1, 0)
        join(thread, result)
        assert call(454, address, 0xffffffff, 1, flags) == (0, 0)

for source_flags in [2, 0x82]:
    for target_flags in [2, 0x82]:
        target = c.c_int()
        target_address = c.addressof(target)
        result = []
        thread = threading.Thread(target=lambda: result.append(wait(address, source_flags)))
        thread.start()
        queued(address, source_flags)
        entries = (Entry * 2)(Entry(0, address, source_flags, 0), Entry(17, target_address, target_flags, 0))
        assert call(456, c.addressof(entries), 0, 0, 1) == (1, 0)
        assert call(454, address, 0xffffffff, 1, source_flags) == (0, 0)
        assert call(454, target_address, 4, 1, target_flags) == (0, 0)
        assert call(454, target_address, 8, 1, target_flags) == (1, 0)
        join(thread, result)

for flags in [2, 0x82]:
    for clock in [0, 1]:
        start = time.monotonic()
        assert wait(address, flags, seconds=.025, clock=clock) == (-1, errno.ETIMEDOUT)
        assert time.monotonic() - start >= .01
        assert call(454, address, 0xffffffff, 1, flags) == (0, 0)

with tempfile.TemporaryFile() as file:
    os.ftruncate(file.fileno(), 4096)
    first = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    second = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
    first_word = c.c_int.from_buffer(first)
    alias_word = c.c_int.from_buffer(second)
    first_address, alias_address = c.addressof(first_word), c.addressof(alias_word)
    read_fd, write_fd = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(write_fd)
        assert os.read(read_fd, 1) == b'W'
        end = time.monotonic() + 5
        while time.monotonic() < end:
            result = call(454, alias_address, 8, 1, 2)
            assert result in [(0, 0), (1, 0)], result
            if result[0] == 1:
                os._exit(0)
            time.sleep(.001)
        os._exit(17)
    os.close(read_fd)
    result = []
    thread = threading.Thread(target=lambda: result.append(wait(first_address, 2)))
    thread.start()
    queued(first_address, 2)
    os.write(write_fd, b'W')
    os.close(write_fd)
    join(thread, result)
    assert os.waitpid(pid, 0) == (pid, 0)
    assert call(454, first_address, 0xffffffff, 1, 2) == (0, 0)
    del first_word, alias_word
    first.close()
    second.close()

print('FUTEX_SCALAR_PASS ' + json.dumps(dict(widths=True, timeout_order=True, masks=True,
    legacy=True, tag_isolation=True, requeue=True, clocks=True, shared_alias_fork=True)), flush=True)
