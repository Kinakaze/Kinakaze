"""Check legacy epoll_create size validation and eventfd interoperability."""
import ctypes
import errno
import fcntl
import os
import select


libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long


def create(size):
    ctypes.set_errno(0)
    return libc.syscall(ctypes.c_long(213), ctypes.c_long(size))


for invalid in (0, -1, -(2**31)):
    assert create(invalid) == -1
    assert ctypes.get_errno() == errno.EINVAL

for size in (1, 2**31 - 1):
    descriptor = create(size)
    assert descriptor >= 0, ctypes.get_errno()
    counter = None
    poller = None
    try:
        assert fcntl.fcntl(descriptor, fcntl.F_GETFD) & fcntl.FD_CLOEXEC == 0
        poller = select.epoll.fromfd(descriptor)
        counter = os.eventfd(0, os.EFD_NONBLOCK)
        poller.register(counter, select.EPOLLIN)
        assert poller.poll(0) == []
        os.eventfd_write(counter, 7)
        assert poller.poll(1) == [(counter, select.EPOLLIN)]
        assert os.eventfd_read(counter) == 7
        assert poller.poll(0) == []
    finally:
        if counter is not None:
            os.close(counter)
        if poller is not None:
            poller.close()
        else:
            os.close(descriptor)

print('RAW_EPOLL_CREATE_OK', flush=True)
