"""Mount notifications wake epoll across workers and shared reads acknowledge them."""
import ctypes
import os
import select
import tempfile
import time


libc = ctypes.CDLL(None, use_errno=True)
with tempfile.TemporaryDirectory(prefix='mount-poll-') as directory:
    path = os.fsencode(directory)
    fd = os.open('/proc/self/mountinfo', os.O_RDONLY)
    duplicate = os.dup(fd)
    poller = select.epoll()
    poller.register(fd, select.EPOLLPRI | select.EPOLLERR)
    mounted = False
    try:
        assert poller.poll(.05) == []
        child = os.fork()
        if child == 0:
            time.sleep(.15)
            result = libc.mount(b'none', path, b'tmpfs', 0, b'size=1m')
            os._exit(0 if result == 0 else 1)
        try:
            ready = poller.poll(5)
        finally:
            _, status = os.waitpid(child, 0)
        assert status == 0, status
        mounted = True
        assert ready and ready[0][0] == fd, ready
        os.lseek(duplicate, 0, os.SEEK_SET)
        contents = os.read(duplicate, 65536)
        assert path in contents
        assert poller.poll(.05) == []
        assert libc.umount2(path, 0) == 0, ctypes.get_errno()
        mounted = False
        assert poller.poll(5)
        os.lseek(duplicate, 0, os.SEEK_SET)
        contents = os.read(duplicate, 65536)
        assert path not in contents
        assert poller.poll(.05) == []
    finally:
        if mounted:
            libc.umount2(path, 2)
        poller.close()
        os.close(duplicate)
        os.close(fd)
print('MOUNT_POLL_OK')
