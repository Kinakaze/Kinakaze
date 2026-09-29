"""Exercise active epoll views while another worker mutates shared descriptors."""
import ctypes
import json
import os
import select
import struct
import tempfile
import time
import traceback


libc = ctypes.CDLL(None, use_errno=True)


class Timespec(ctypes.Structure):
    _fields_ = [('sec', ctypes.c_long), ('nsec', ctypes.c_long)]


class Itimerspec(ctypes.Structure):
    _fields_ = [('interval', Timespec), ('value', Timespec)]


libc.timerfd_create.argtypes = [ctypes.c_int, ctypes.c_int]
libc.timerfd_settime.argtypes = [ctypes.c_int, ctypes.c_int,
                               ctypes.POINTER(Itimerspec), ctypes.c_void_p]
libc.inotify_init1.argtypes = [ctypes.c_int]
libc.inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
libc.inotify_rm_watch.argtypes = [ctypes.c_int, ctypes.c_int]


def checked(value):
    if value < 0:
        raise OSError(ctypes.get_errno(), os.strerror(ctypes.get_errno()))
    return value


def child(action):
    pid = os.fork()
    if pid == 0:
        try:
            time.sleep(.15)  # Let the parent enter a wait with the old state.
            action()
        except BaseException:
            traceback.print_exc()
            os._exit(1)
        os._exit(0)
    return pid


def joined(pid):
    assert os.waitpid(pid, 0)[1] == 0


def timer_probe():
    fd = checked(libc.timerfd_create(1, os.O_NONBLOCK))
    alias = os.dup(fd)
    try:
        with select.epoll() as ep:
            ep.register(fd, select.EPOLLIN)
            # Initially disarmed. Another worker rearms and exits before expiry.
            def rearm():
                timer = Itimerspec(Timespec(), Timespec(0, 150_000_000))
                checked(libc.timerfd_settime(alias, 0, ctypes.byref(timer), None))
            for _ in range(2):
                pid = child(rearm)
                assert ep.poll(3) == [(fd, select.EPOLLIN)]
                joined(pid)
                assert struct.unpack('Q', os.read(alias, 8))[0] == 1
                assert ep.poll(.05) == []
    finally:
        os.close(alias)
        os.close(fd)


def inotify_probe():
    fd = checked(libc.inotify_init1(os.O_NONBLOCK))
    alias = os.dup(fd)
    try:
        with tempfile.TemporaryDirectory(prefix='wait-view-') as path, select.epoll() as ep:
            ep.register(fd, select.EPOLLIN)
            for index in range(2):
                filename = os.path.join(path, str(index))
                def add_and_create():
                    checked(libc.inotify_add_watch(alias, os.fsencode(path), 0x100))
                    with open(filename, 'w') as stream:
                        stream.write('created by another worker')
                pid = child(add_and_create)
                assert ep.poll(3) == [(fd, select.EPOLLIN)]
                joined(pid)
                data = os.read(alias, 4096)
                wd, mask, _, length = struct.unpack_from('iIII', data)
                assert mask & 0x100 and data[16:16 + length].rstrip(b'\0') == str(index).encode()
                assert ep.poll(.05) == []
                pid = child(lambda: checked(libc.inotify_rm_watch(alias, wd)))
                assert ep.poll(3) == [(fd, select.EPOLLIN)]
                joined(pid)
                data = os.read(alias, 4096)
                assert struct.unpack_from('iIII', data)[:2] == (wd, 0x8000)
                assert ep.poll(.05) == []
    finally:
        os.close(alias)
        os.close(fd)


timer_probe()
inotify_probe()
print(json.dumps({'passed': True, 'checks': ['shared-timer-rearm-owner-exit',
                                          'shared-inotify-add-remove-read']}))
