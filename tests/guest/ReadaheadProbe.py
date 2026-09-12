"""File-cache hints preserve data and offsets and validate fd access/type."""
import ctypes as C
import errno
import os
import socket
import tempfile

libc = C.CDLL(None, use_errno=True)
libc.readahead.argtypes = [C.c_int, C.c_longlong, C.c_size_t]
libc.readahead.restype = C.c_ssize_t
libc.syscall.argtypes = [C.c_long]
libc.syscall.restype = C.c_long


def error(fd, offset, count, expected):
    C.set_errno(0)
    assert libc.readahead(fd, offset, count) == -1
    assert C.get_errno() == expected, C.get_errno()


with tempfile.TemporaryDirectory(prefix='readahead-') as directory:
    path = directory + '/data'
    fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
    try:
        payload = bytes(range(251)) * 1025
        assert os.write(fd, payload) == len(payload)
        os.lseek(fd, 37, os.SEEK_SET)
        for offset, count in ((0, 0), (3, 65539), (65539, 131075),
                              (len(payload) - 3, C.c_size_t(-1).value),
                              (len(payload), 4096), (2**63 - 1, 4096)):
            assert libc.readahead(fd, offset, count) == 0, C.get_errno()
            assert os.lseek(fd, 0, os.SEEK_CUR) == 37
        assert libc.syscall(187, C.c_int(fd), C.c_longlong(3), C.c_size_t(4096)) == 0
        assert os.lseek(fd, 0, os.SEEK_CUR) == 37
        assert os.pread(fd, len(payload), 0) == payload
        error(fd, -1, 1, errno.EINVAL)
        os.ftruncate(fd, 0)  # no temporary section may survive the hint
        assert libc.readahead(fd, 0, 4096) == 0
    finally:
        os.close(fd)
    writeonly = os.open(path, os.O_WRONLY)
    try:
        error(writeonly, 0, 0, errno.EBADF)
    finally:
        os.close(writeonly)
    directory_fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        error(directory_fd, 0, 0, errno.EINVAL)
    finally:
        os.close(directory_fd)
error(-1, 0, 0, errno.EBADF)
left, right = socket.socketpair()
try:
    error(left.fileno(), 0, 0, errno.EINVAL)
finally:
    left.close()
    right.close()
print('READAHEAD_FILE_CACHE_ABI_SYSCALL_OK', flush=True)
