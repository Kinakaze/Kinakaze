"""Check SIGCHLD payloads, wait/reap independence, and pending signals across exec."""
import ctypes
import os
import select
import signal
import struct
import sys

libc = ctypes.CDLL(None, use_errno=True)
libc.signalfd.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
libc.signalfd.restype = ctypes.c_int


def receive(fd, pid, code, status):
    assert select.select([fd], [], [], 5)[0], 'SIGCHLD did not become readable'
    data = os.read(fd, 128)
    assert len(data) == 128
    observed = tuple(struct.unpack_from('I', data, offset)[0] for offset in (0, 8, 12, 16, 40))
    expected = (signal.SIGCHLD, code, pid, os.getuid(), status)
    assert observed == expected, (observed, expected)


if len(sys.argv) > 1:
    fd, pid = map(int, sys.argv[1:])
    receive(fd, pid, 1, 37)
    assert os.waitpid(pid, 0) == (pid, 37 << 8)
    os.close(fd)
    print('CHILD_SIGNAL_EXIT_STOP_CONTINUE_REAP_EXEC_OK', flush=True)
    sys.exit(0)

signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGCHLD})
mask = (ctypes.c_uint64 * 16)()
mask[0] = 1 << (signal.SIGCHLD - 1)
fd = libc.signalfd(-1, ctypes.byref(mask), 0)
assert fd >= 0, ctypes.get_errno()
os.set_inheritable(fd, True)

pid = os.fork()
if pid == 0:
    os._exit(23)
assert select.select([fd], [], [], 5)[0]
# Reaping must not erase the already queued sender and exit status.
assert os.waitpid(pid, 0) == (pid, 23 << 8)
receive(fd, pid, 1, 23)

pid = os.fork()
if pid == 0:
    while True:
        signal.pause()
try:
    os.kill(pid, signal.SIGSTOP)
    receive(fd, pid, 5, signal.SIGSTOP)
    assert os.WIFSTOPPED(os.waitpid(pid, os.WUNTRACED)[1])
    os.kill(pid, signal.SIGCONT)
    receive(fd, pid, 6, signal.SIGCONT)
    assert os.WIFCONTINUED(os.waitpid(pid, os.WCONTINUED)[1])
    os.kill(pid, signal.SIGTERM)
    receive(fd, pid, 2, signal.SIGTERM)
    assert os.WTERMSIG(os.waitpid(pid, 0)[1]) == signal.SIGTERM
    pid = 0
finally:
    if pid:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)

pid = os.fork()
if pid == 0:
    os._exit(37)
assert select.select([fd], [], [], 5)[0]
os.execv(sys.executable, [sys.executable, os.path.abspath(__file__), str(fd), str(pid)])
