"""Exercise the descriptor limit and closefrom ABI used by Erlang."""
import ctypes
import errno
import os
import subprocess
import sys

libc = ctypes.CDLL(None, use_errno=True)
libc.closefrom.argtypes = [ctypes.c_int]
libc.closefrom.restype = None
if len(sys.argv) > 1:
    descriptor = os.open('/dev/null', os.O_RDONLY)
    for target in (199, 200, 201):
        os.dup2(descriptor, target)
    ctypes.set_errno(errno.EDOM)
    libc.closefrom(200)
    assert ctypes.get_errno() == errno.EDOM
    os.fstat(199)
    for target in (200, 201):
        try:
            os.fstat(target)
        except OSError as error:
            assert error.errno == errno.EBADF
        else:
            raise AssertionError('descriptor remained open')
    libc.closefrom(-1)
    try:
        os.fstat(0)
    except OSError as error:
        os._exit(0 if error.errno == errno.EBADF else 1)
    os._exit(1)

limit = os.sysconf('SC_IOV_MAX')
assert limit == 1024
descriptor = os.open('/dev/null', os.O_WRONLY)
try:
    assert os.writev(descriptor, [b'x'] * limit) == limit
    try:
        os.writev(descriptor, [b'x'] * (limit + 1))
    except OSError as error:
        assert error.errno == errno.EINVAL
    else:
        raise AssertionError('writev accepted too many entries')
finally:
    os.close(descriptor)
subprocess.run([sys.executable, *sys.orig_argv[1:], 'child'], check=True, timeout=20)
print('RUNTIME_IOV_MAX_CLOSEFROM_BOUNDARY_ERRNO_OK')
