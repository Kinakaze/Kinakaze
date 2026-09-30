"""Check wchar counts and abort-before-write for fortified wide copies."""
import ctypes
import os
import signal

libc = ctypes.CDLL(None)
copy = libc.__wmemcpy_chk
copy.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t]
copy.restype = ctypes.c_void_p
source = (ctypes.c_int32 * 4)(0x4E2D, 0x6587, 0x1F600, 0)
target = (ctypes.c_int32 * 6)(*[123] * 6)
destination = ctypes.addressof(target) + ctypes.sizeof(ctypes.c_int32)
assert copy(destination, source, 4, 4) == destination
assert list(target) == [123, 0x4E2D, 0x6587, 0x1F600, 0, 123]
assert copy(None, None, 0, 0) is None
child = os.fork()
if child == 0:
    copy(destination, source, 4, 3)
    os._exit(1)
_, status = os.waitpid(child, 0)
assert os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGABRT, status
print('WIDE_MEMORY_FORTIFY_OK', flush=True)
