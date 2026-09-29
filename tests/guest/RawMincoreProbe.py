"""Exercise raw mincore and libc residency, output bounds and VMA holes."""
import ctypes
import errno
import os


libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long
libc.mmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int,
                      ctypes.c_int, ctypes.c_int, ctypes.c_long]
libc.mmap.restype = ctypes.c_void_p
libc.munmap.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
libc.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
page = os.sysconf('SC_PAGESIZE')


def raw(address, length, output):
    return libc.syscall(ctypes.c_long(27), ctypes.c_void_p(address),
                        ctypes.c_size_t(length), ctypes.c_void_p(output))


for query in (raw, libc.mincore):
    address = libc.mmap(None, 3 * page, 3, 0x22, -1, 0)
    assert address != ctypes.c_void_p(-1).value, ctypes.get_errno()
    try:
        ctypes.memset(address, 42, 3 * page)
        output = (ctypes.c_ubyte * 5)(*[0xA5] * 5)
        assert query(address, page + 1, ctypes.addressof(output) + 1) == 0, ctypes.get_errno()
        assert list(output) == [0xA5, 1, 1, 0xA5, 0xA5], list(output)
        assert query(address + 1, page, ctypes.addressof(output)) == -1
        assert ctypes.get_errno() == errno.EINVAL
        for invalid in (0, 1):
            assert query(address, page, invalid) == -1
            assert ctypes.get_errno() == errno.EFAULT
        assert libc.munmap(address + page, page) == 0
        assert query(address + page, page, ctypes.addressof(output)) == -1
        assert ctypes.get_errno() == errno.ENOMEM
        assert query(address, 3 * page, ctypes.addressof(output)) == -1
        assert ctypes.get_errno() == errno.ENOMEM
        assert ctypes.string_at(address + 2 * page, page) == bytes([42]) * page
        assert query(address + 2 * page, page, ctypes.addressof(output)) == 0
        assert output[0] == 1
    finally:
        assert libc.munmap(address, 3 * page) == 0

print('RAW_MINCORE_RESIDENCY_BOUNDS_HOLES_OK', flush=True)
