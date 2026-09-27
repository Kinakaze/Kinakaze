"""Exercise file overlays, partial unmap, and private bytes before/after fork."""
import ctypes as c
import os
import sys
import tempfile

libc = c.CDLL('libc.so.6', use_errno=True)
libc.mmap.argtypes = [c.c_void_p, c.c_size_t, c.c_int, c.c_int, c.c_int, c.c_longlong]
libc.mmap.restype = c.c_void_p
libc.munmap.argtypes = [c.c_void_p, c.c_size_t]
libc.mprotect.argtypes = [c.c_void_p, c.c_size_t, c.c_int]
page = os.sysconf('SC_PAGE_SIZE')
length = int(sys.argv[1]) if len(sys.argv) > 1 else 8 * 1024 * 1024
window = 1024 * 1024 + page
data = bytes((index * 37 + 11) % 251 for index in range(1024 * 1024 + 37))
rounded = (len(data) + page - 1) // page * page

def byte(address):
    return c.c_ubyte.from_address(address)

def checked_map(address, size, flags, fd=-1, offset=0):
    result = libc.mmap(address, size, 3, flags, fd, offset)
    assert result is not None and result != c.c_void_p(-1).value, c.get_errno()
    return result

def fork_check(check):
    pid = os.fork()
    if pid == 0:
        try:
            check()
        except BaseException:
            os._exit(98)
        os._exit(29)
    assert os.waitpid(pid, 0)[1] == 29 << 8

for fork_first in (False, True):
    address = checked_map(None, length, 0x22)
    try:
        byte(address).value = 0x41
        byte(address + length - 1).value = 0x63
        if fork_first:
            fork_check(lambda: setattr(byte(address), 'value', 0x99))
            assert byte(address).value == 0x41
        # In the second iteration this is a private dirty page in an already
        # forked snapshot; preserving just the original section would lose it.
        byte(address).value = 0x75
        fd, path = tempfile.mkstemp(prefix='memory-snapshot-')
        try:
            with os.fdopen(os.dup(fd), 'wb') as stream:
                stream.write(b'\0' * page + data + bytes(rounded - len(data)))
            os.close(fd)
            fd = os.open(path, os.O_RDONLY)
            result = checked_map(address + window, len(data), 0x12, fd, page)
            assert result == address + window
        finally:
            os.close(fd)
            os.unlink(path)
        assert c.string_at(address + window, len(data)) == data
        assert c.string_at(address + window + len(data), rounded - len(data)) == bytes(rounded - len(data))
        assert byte(address).value == 0x75
        assert byte(address + length - 1).value == 0x63

        def child_check():
            assert byte(address).value == 0x75
            assert c.string_at(address + window, len(data)) == data
            byte(address).value = 0x19
            byte(address + window).value = 0xa5
            byte(address + length - 1).value = 0x20

        fork_check(child_check)
        assert byte(address).value == 0x75
        assert byte(address + window).value == data[0]
        assert byte(address + length - 1).value == 0x63
        assert libc.mprotect(address + window, rounded, 1) == 0
        assert libc.mprotect(address + window, rounded, 3) == 0
        byte(address + window).value = 0xef
        gap = address + length - 3 * page
        byte(gap - 1).value = 0x72
        byte(gap + page).value = 0x83
        assert libc.munmap(gap, page) == 0
        assert byte(gap - 1).value == 0x72
        assert byte(gap + page).value == 0x83
    finally:
        assert libc.munmap(address, length) == 0

print('MEMORY_SNAPSHOT_OK', flush=True)
