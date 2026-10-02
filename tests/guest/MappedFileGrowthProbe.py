"""A retained read-only MAP_SHARED VMA follows pwrite growth through another fd."""
import ctypes as c
import os
from pathlib import Path

libc = c.CDLL(None, use_errno=True)
libc.mmap.argtypes = [c.c_void_p, c.c_size_t, c.c_int, c.c_int, c.c_int, c.c_long]
libc.mmap.restype = c.c_void_p
libc.munmap.argtypes = [c.c_void_p, c.c_size_t]
libc.mprotect.argtypes = [c.c_void_p, c.c_size_t, c.c_int]
path = Path('mapped-growth.db')
page = os.sysconf('SC_PAGESIZE')
for initial_length, protection in ((page, 1), (page + 17, 1), (page, 0)):
    with path.open('wb') as file:
        file.write(b'initial'.ljust(initial_length, b'\0'))
    reader = os.open(path, os.O_RDONLY)
    address = libc.mmap(None, 4 * page, protection, 1, reader, 0)
    assert address != c.c_void_p(-1).value, c.get_errno()
    os.close(reader)
    try:
        writer = os.open(path, os.O_RDWR)
        try:
            assert os.pwrite(writer, b'LMDB_GROWTH_OK', 2 * page + 10) == 14
            if protection == 0:
                assert libc.mprotect(address, 3 * page, 1) == 0, c.get_errno()
            assert c.string_at(address + 2 * page + 10, 14) == b'LMDB_GROWTH_OK'
            assert c.string_at(address + page, 8) == b'\0' * 8
            assert c.string_at(address, 7) == b'initial'
        finally:
            os.close(writer)
    finally:
        assert libc.munmap(address, 4 * page) == 0
        path.unlink()
print('MAPPED_FILE_PWRITE_GROWTH_RETAINED_INODE_OK')
