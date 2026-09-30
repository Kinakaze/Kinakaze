"""Check futex timeout copying and error precedence through real raw syscalls."""
import ctypes as c
import errno
import json
import mmap

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


def legacy(address, command, private, timeout, mask=1):
    return call(202, address, command | (128 if private else 0), 0, timeout, 0, mask)


invalid = Timespec(-1, 0)
zero = Timespec(0, 0)
word = c.c_int(0)
address = c.addressof(word)

for private in [False, True]:
    # Timed PI commands share sys_futex's copy stage even before their backend
    # is available. These assertions do not claim PI locking is implemented.
    for command in [0, 9, 6, 13, 11]:
        for bad_address in [0, 1]:
            assert legacy(bad_address, command, private, 1, 0) == (-1, errno.EFAULT)
            assert legacy(bad_address, command, private, c.addressof(invalid)) == (-1, errno.EINVAL)
    assert legacy(0, 256, private, 1) == (-1, errno.EFAULT)
    assert legacy(0, 256, private, c.addressof(invalid)) == (-1, errno.EINVAL)
    assert legacy(0, 256, private, c.addressof(zero)) == (-1, errno.ENOSYS)
    assert legacy(0, 9, private, c.addressof(zero), 0) == (-1, errno.EINVAL)
    assert legacy(0, 9, private, 1, 0) == (-1, errno.EFAULT)
    assert legacy(address, 1, private, 1) == (0, 0)
    assert legacy(address, 10, private, 1) == (0, 0)
    assert legacy(0, 63, private, 1) == (-1, errno.ENOSYS)
    assert legacy(0, 3, private, 0xffffffff) == (-1, errno.EINVAL)

    storage = (c.c_ubyte * 17)()
    unaligned = c.addressof(storage) + 1
    c.memmove(unaligned, c.addressof(zero), 16)
    flags = 0x82 if private else 2
    entry = Entry(0, address, flags, 0)
    word.value = 1
    for command in [0, 9]:
        assert legacy(address, command, private, unaligned) == (-1, errno.EAGAIN)
    assert call(455, address, 0, 1, flags, unaligned, 1) == (-1, errno.EAGAIN)
    assert call(449, c.addressof(entry), 1, 0, unaligned, 1) == (-1, errno.EAGAIN)
    word.value = 0
    for command in [0, 9]:
        assert legacy(address, command, private, unaligned) == (-1, errno.ETIMEDOUT)
    assert call(455, address, 0, 1, flags, unaligned, 1) == (-1, errno.ETIMEDOUT)
    assert call(449, c.addressof(entry), 1, 0, unaligned, 1) == (-1, errno.ETIMEDOUT)

with mmap.mmap(-1, 8192, flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS,
               prot=mmap.PROT_READ | mmap.PROT_WRITE) as pages:
    anchor = c.c_int.from_buffer(pages)
    base = c.addressof(anchor)
    timeout = base + 4096 - 8
    c.memmove(timeout, c.addressof(zero), 16)
    assert call(10, base + 4096, 4096, 0) == (0, 0)
    try:
        for private in [False, True]:
            for command in [0, 9, 6, 13, 11]:
                assert legacy(0, command, private, timeout) == (-1, errno.EFAULT)
            flags = 0x82 if private else 2
            entry = Entry(0, address, flags, 0)
            assert call(455, address, 0, 1, flags, timeout, 1) == (-1, errno.EFAULT)
            assert call(449, c.addressof(entry), 1, 0, timeout, 1) == (-1, errno.EFAULT)
    finally:
        assert call(10, base + 4096, 4096, 3) == (0, 0)
        del anchor

print('FUTEX_TIMEOUT_PASS ' + json.dumps(dict(error_order=True, unaligned=True,
    guard_page=True, private=True, unflagged=True, legacy=True, scalar=True,
    vector=True, pi_timeout_entry=True)), flush=True)
