"""Exercise raw wait ABIs through the actual guest libc bridge."""
import ctypes as c
import errno
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


timeout = Timespec(0, 2_000_000)
assert call(270, 0, 0, 0, 0, c.addressof(timeout)) == (0, 0)
assert (timeout.seconds, timeout.nanoseconds) == (0, 0)
assert call(270, 0, 0, 0, 0, 1) == (-1, errno.EFAULT)
word = c.c_int(17)
address = c.addressof(word)
for _ in range(64):
    assert call(454, address, 1, 0, 0x82) == (0, 0)
    assert call(455, address, 18, 1, 0x82, 0, 999) == (-1, errno.EAGAIN)
assert call(454, address, 1 << 32, 1, 0x82) == (-1, errno.EINVAL)
past = Timespec()
assert call(455, address, 17, 1, 0x82, c.addressof(past), 0) == (-1, errno.ETIMEDOUT)

result = []


def wait():
    end = time.monotonic_ns() + 5_000_000_000
    timeout = Timespec(end // 1_000_000_000, end % 1_000_000_000)
    result.append(call(455, address, 17, 2, 0x82, c.addressof(timeout), 1))


thread = threading.Thread(target=wait)
thread.start()
deadline = time.monotonic() + 5
while time.monotonic() < deadline:
    assert call(454, address, 2, 0, 0x82) == (0, 0)
    assert call(454, address, 1, 1, 0x82) == (0, 0)
    wake, error = call(454, address, 2, 1, 0x82)
    assert error == 0
    if wake == 1:
        break
    time.sleep(.001)
else:
    raise AssertionError('waiter never became wakeable')
thread.join(timeout=6)
assert not thread.is_alive() and result == [(0, 0)], result
print('SYSCALL_WAIT_PASS', flush=True)
