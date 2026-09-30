"""Keep one guest alive while repeatedly retiring epoll, PTY and proc-fd I/O."""
import ctypes
import os
from pathlib import Path
import pty
import sys
import time


control = Path(sys.argv[1])
libc = ctypes.CDLL(None, use_errno=True)
libc.epoll_create1.argtypes = [ctypes.c_int]
libc.epoll_create1.restype = ctypes.c_int
libc.syscall.restype = ctypes.c_long
events = ctypes.create_string_buffer(12)
mask = ctypes.c_uint64(0)
timeout = (ctypes.c_long * 2)(0, 0)
initial_fds = len(os.listdir('/proc/self/fd'))


def cycle(count):
    for index in range(count):
        descriptor = libc.epoll_create1(0)
        assert descriptor >= 0
        try:
            assert libc.syscall(ctypes.c_long(441), ctypes.c_int(descriptor),
                                ctypes.byref(events), ctypes.c_int(1),
                                ctypes.byref(timeout), ctypes.byref(mask), ctypes.c_size_t(8)) == 0
        finally:
            os.close(descriptor)
        if index % 16 == 0:
            master, slave = pty.openpty()
            os.close(slave)
            os.close(master)
            parent = os.open(control, os.O_DIRECTORY | os.O_CLOEXEC)
            try:
                path = '/proc/self/fd/{}/scratch'.format(parent)
                with open(path, 'wb') as stream:
                    stream.write(b'closed')
                os.unlink(path)
            finally:
                os.close(parent)
    assert len(os.listdir('/proc/self/fd')) == initial_fds


def wait(name):
    deadline = time.monotonic() + 60
    while not (control / name).exists():
        assert time.monotonic() < deadline, name
        time.sleep(.01)


cycle(128)
print('RESOURCE_WARM_READY', flush=True)
for phase in (1, 2):
    wait('go-' + str(phase))
    cycle(1024)
    print('RESOURCE_PHASE_{}_READY'.format(phase), flush=True)
wait('stop')
print('AGENT_RESOURCE_LIFETIMES_OK', flush=True)
