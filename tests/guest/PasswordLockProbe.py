"""Verify real process locks, inherited libc state, wakeup and the 15-second timeout."""
import ctypes as c
import errno
import fcntl
import os
import select
import time

lib = c.CDLL('libc.so.6', use_errno=True)
lock, unlock = lib.lckpwdf, lib.ulckpwdf
lock.argtypes = unlock.argtypes = []
lock.restype = unlock.restype = c.c_int

class Mask(c.Structure):
    _fields_ = [('bits', c.c_ulong * 16)]
class Action(c.Structure):
    _fields_ = [('handler', c.c_void_p), ('mask', Mask), ('flags', c.c_int),
                ('padding', c.c_int), ('restorer', c.c_void_p)]

lib.sigaction.argtypes = [c.c_int, c.POINTER(Action), c.POINTER(Action)]
lib.sigprocmask.argtypes = [c.c_int, c.POINTER(Mask), c.POINTER(Mask)]
old_action, old_mask = Action(), Mask()
action = Action(handler=1)  # Caller ignores and blocks SIGALRM.
action.mask.bits[0] = 1 << 9
mask = Mask()
mask.bits[0] = 1 << 13
assert lib.sigaction(14, c.byref(action), c.byref(old_action)) == 0
assert lib.sigprocmask(0, c.byref(mask), c.byref(old_mask)) == 0
assert unlock() == -1
assert lock() == 0
assert lock() == -1
after_action, after_mask = Action(), Mask()
assert lib.sigaction(14, None, c.byref(after_action)) == 0
assert lib.sigprocmask(2, None, c.byref(after_mask)) == 0
assert after_action.handler == 1 and after_action.mask.bits[0] == action.mask.bits[0]
assert after_mask.bits[0] & (1 << 13)
assert os.stat('/etc/.pwd.lock').st_mode & 0o777 == 0o600

r, w = os.pipe()
pid = os.fork()
if pid == 0:
    try:
        os.close(r)
        assert lock() == -1, 'fork lost the libc lock descriptor'
        assert unlock() == 0
        fd = os.open('/etc/.pwd.lock', os.O_WRONLY)
        try:
            fcntl.lockf(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as exc:
            assert exc.errno in (errno.EACCES, errno.EAGAIN)
        else:
            raise AssertionError('child close released the parent lock')
        os.close(fd)
        os.write(w, b'R')
        assert lock() == 0
        os.write(w, b'A')
        assert unlock() == 0
    except BaseException:
        import traceback
        traceback.print_exc()
        os._exit(1)
    os._exit(0)
os.close(w)
assert os.read(r, 1) == b'R'
assert not select.select([r], [], [], 0.2)[0], 'contended lock did not wait'
assert unlock() == 0
assert os.read(r, 1) == b'A'
assert os.waitpid(pid, 0) == (pid, 0)
os.close(r)

assert lock() == 0
pid = os.fork()
if pid == 0:
    try:
        assert unlock() == 0
        start = time.monotonic()
        assert lock() == -1
        elapsed = time.monotonic() - start
        assert c.get_errno() == errno.EINTR, c.get_errno()
        assert 14 <= elapsed < 22, elapsed
        assert unlock() == -1, 'failed lock leaked its descriptor'
        assert lib.sigaction(14, None, c.byref(after_action)) == 0 and after_action.handler == 1
        assert lib.sigprocmask(2, None, c.byref(after_mask)) == 0 and after_mask.bits[0] & (1 << 13)
    except BaseException:
        import traceback
        traceback.print_exc()
        os._exit(1)
    os._exit(0)
assert os.waitpid(pid, 0) == (pid, 0)
assert unlock() == 0
assert lib.sigaction(14, c.byref(old_action), None) == 0
assert lib.sigprocmask(2, c.byref(old_mask), None) == 0
print('PASSWORD_LOCK_PROCESS_FORK_WAKE_TIMEOUT_SIGNAL_RESTORE_OK')
