"""Static runtimes use SYS_fallocate directly instead of the libc export."""
import ctypes as c
import errno
import mmap
import os
import tempfile

libc = c.CDLL('libc.so.6', use_errno=True)
libc.syscall.restype = c.c_long


def allocate(fd, mode, offset, length):
    return libc.syscall(c.c_long(285), c.c_long(fd), c.c_long(mode),
                        c.c_long(offset), c.c_long(length))


with tempfile.TemporaryFile() as file:
    file.write(b'preserved')
    file.flush()
    position = os.lseek(file.fileno(), 0, os.SEEK_CUR)
    assert allocate(file.fileno(), 0, 0, 16384) == 0, c.get_errno()
    assert os.fstat(file.fileno()).st_size == 16384
    assert os.lseek(file.fileno(), 0, os.SEEK_CUR) == position
    with mmap.mmap(file.fileno(), 16384) as mapping:
        assert mapping[:9] == b'preserved'
        assert mapping[9:] == bytes(16384 - 9)
        mapping[8192:8196] = b'link'
        mapping.flush()
    assert os.pread(file.fileno(), 4, 8192) == b'link'
    assert allocate(file.fileno(), 1, 0, 32768) == 0, c.get_errno()
    assert os.fstat(file.fileno()).st_size == 16384
    for fd, offset, length, expected in [
        (-1, 0, 4096, errno.EBADF),
        (file.fileno(), -1, 4096, errno.EINVAL),
        (file.fileno(), 0, 0, errno.EINVAL),
    ]:
        assert allocate(fd, 0, offset, length) == -1
        assert c.get_errno() == expected, c.get_errno()
    assert os.fstat(file.fileno()).st_size == 16384
print('RAW_ALLOCATION_OK', flush=True)
