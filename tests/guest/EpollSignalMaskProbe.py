"""epoll_pwait and raw epoll_pwait2 apply and restore their temporary mask."""
import ctypes
import errno
import os
import signal
import threading
import time


libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long
libc.epoll_create1.argtypes = [ctypes.c_int]
libc.epoll_create1.restype = ctypes.c_int
libc.epoll_pwait.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int,
                           ctypes.c_int, ctypes.c_void_p]
libc.epoll_pwait.restype = ctypes.c_int

class Timespec(ctypes.Structure):
    _fields_ = [('seconds', ctypes.c_long), ('nanoseconds', ctypes.c_long)]

fd = libc.epoll_create1(0)
assert fd >= 0
events = ctypes.create_string_buffer(12)
empty = ctypes.c_uint64(0)
timeout = Timespec(0, 20_000_000)
delivered = []
old_action = signal.signal(signal.SIGUSR1, lambda number, frame: delivered.append(number))
old_mask = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR1})

def raw(mask, size=8, epoll_fd=fd):
    ctypes.set_errno(0)
    return libc.syscall(ctypes.c_long(441), ctypes.c_int(epoll_fd), ctypes.byref(events),
                        ctypes.c_int(1), ctypes.byref(timeout), mask, ctypes.c_size_t(size))

try:
    for api in ('epoll_pwait2', 'epoll_pwait'):
        delivered.clear()
        os.kill(os.getpid(), signal.SIGUSR1)
        assert raw(None) == 0 and not delivered
        ctypes.set_errno(0)
        result = raw(ctypes.byref(empty)) if api == 'epoll_pwait2' else libc.epoll_pwait(
            fd, events, 1, 20, ctypes.byref(empty))
        assert result == -1 and ctypes.get_errno() == errno.EINTR, (api, result, ctypes.get_errno())
        assert delivered == [signal.SIGUSR1], (api, delivered)
        assert signal.SIGUSR1 in signal.pthread_sigmask(signal.SIG_BLOCK, set())
    assert raw(ctypes.byref(empty), 16) == -1 and ctypes.get_errno() == errno.EINVAL
    assert raw(ctypes.c_void_p(1)) == -1 and ctypes.get_errno() == errno.EFAULT
    assert raw(ctypes.byref(empty), epoll_fd=-1) == -1 and ctypes.get_errno() == errno.EBADF
    assert libc.epoll_pwait(fd, events, 1, 20, ctypes.c_void_p(1)) == -1
    assert ctypes.get_errno() == errno.EFAULT
    assert signal.SIGUSR1 in signal.pthread_sigmask(signal.SIG_BLOCK, set())
    # A successful timeout must restore the mask too, even with no signal.
    assert raw(ctypes.byref(empty)) == 0
    assert signal.SIGUSR1 in signal.pthread_sigmask(signal.SIG_BLOCK, set())
    # In the opposite direction, block a directed signal during the wait and
    # deliver it only after restoring the caller's unblocked mask.
    signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGUSR1})
    blocked = ctypes.c_uint64(1 << (signal.SIGUSR1 - 1))
    timeout.nanoseconds = 50_000_000
    main_thread = threading.get_ident()
    def send(kind):
        signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR1})
        if kind == 'thread':
            signal.pthread_kill(main_thread, signal.SIGUSR1)
        else:
            os.kill(os.getpid(), signal.SIGUSR1)

    for api, kind in ((api, kind) for api in ('epoll_pwait2', 'epoll_pwait')
                      for kind in ('thread', 'process')):
        delivered.clear()
        sender = threading.Timer(.005, send, args=(kind,))
        sender.start()
        started = time.monotonic()
        try:
            result = raw(ctypes.byref(blocked)) if api == 'epoll_pwait2' else libc.epoll_pwait(
                fd, events, 1, 50, ctypes.byref(blocked))
        finally:
            sender.join(timeout=5)
        assert not sender.is_alive()
        assert result == 0 and time.monotonic() - started >= .045, (api, result)
        assert delivered == [signal.SIGUSR1], (api, delivered)
        assert signal.SIGUSR1 not in signal.pthread_sigmask(signal.SIG_BLOCK, set())
finally:
    signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)
    signal.signal(signal.SIGUSR1, old_action)
    os.close(fd)

print('EPOLL_PWAIT_TEMPORARY_MASK_RESTORE_FAULT_OK', flush=True)
