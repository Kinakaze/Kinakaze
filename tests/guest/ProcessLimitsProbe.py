"""Proc limits must describe the same retained state as the syscall ABI."""
import os
import ctypes
import errno
import resource
import subprocess


def check():
    with open('/proc/self/limits') as stream:
        header, *lines = stream.read().splitlines()
    assert len(lines) == 16, lines
    for number, line in enumerate(lines):
        # PAM distinguishes units using this Linux proc-file formatting rule.
        assert len(line) == (68 if number in (13, 14) else len(header)), repr(line)
        def value(text):
            text = text.strip()
            return resource.RLIM_INFINITY if text == 'unlimited' else int(text)
        reported = value(line[26:47]), value(line[47:68])
        actual = resource.getrlimit(number)
        assert reported == actual, (number, reported, actual)


check()
assert resource.getrlimit(resource.RLIMIT_RTPRIO) == (0, 0)
resource.setrlimit(resource.RLIMIT_RTPRIO, (0, 0))
assert os.sched_getscheduler(0) == os.SCHED_OTHER
assert os.sched_getparam(0).sched_priority == 0
assert os.sched_get_priority_min(os.SCHED_OTHER) == 0
assert os.sched_get_priority_max(os.SCHED_OTHER) == 0
assert os.sched_get_priority_min(os.SCHED_FIFO) == 1
assert os.sched_get_priority_max(os.SCHED_FIFO) == 99
os.sched_setscheduler(0, os.SCHED_OTHER, os.sched_param(0))
try:
    os.sched_setscheduler(0, os.SCHED_FIFO, os.sched_param(1))
except PermissionError:
    pass
else:
    raise AssertionError('realtime scheduling must respect the zero ceiling')
libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long
priority = ctypes.c_int(1)
assert libc.syscall(144, 0, os.SCHED_FIFO, ctypes.byref(priority)) == -1
assert ctypes.get_errno() == errno.EPERM
assert libc.sched_getparam(0, ctypes.c_void_p(1)) == -1
assert ctypes.get_errno() == errno.EFAULT
assert libc.sched_setscheduler(0, 0, ctypes.c_void_p(1)) == -1
assert ctypes.get_errno() == errno.EFAULT
for pid, expected in [(-1, errno.EINVAL), (0x7ffffff0, errno.ESRCH)]:
    assert libc.sched_getscheduler(pid) == -1
    assert ctypes.get_errno() == expected
child = os.fork()
if child == 0:
    resource.setrlimit(resource.RLIMIT_FSIZE, (4096, 8192))
    check()
    subprocess.run(['/usr/bin/python3', '-c',
                    'import resource; assert resource.getrlimit(resource.RLIMIT_FSIZE)==(4096,8192)'], check=True)
    os._exit(0)
assert os.waitpid(child, 0) == (child, 0)
check()
print('PROCESS_LIMITS_OK')
