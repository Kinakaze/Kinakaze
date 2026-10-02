"""MongoDB-observed libc/libm interfaces: values, CPU accounting and fenv state."""
import ctypes as c
import errno
import os
import subprocess
import time

libc = c.CDLL(None, use_errno=True)
libc.sysconf.argtypes = [c.c_int]
libc.sysconf.restype = c.c_long
minimum, recommended = libc.sysconf(249), libc.sysconf(250)
assert 2048 <= minimum <= recommended <= 1024 * 1024
class SignalStack(c.Structure):
    _fields_ = [('pointer', c.c_void_p), ('flags', c.c_int), ('size', c.c_size_t)]
libc.sigaltstack.argtypes = [c.POINTER(SignalStack), c.POINTER(SignalStack)]
previous = SignalStack()
memory = c.create_string_buffer(recommended)
assert libc.sigaltstack(c.byref(SignalStack(c.addressof(memory), 0, recommended)), c.byref(previous)) == 0
assert libc.sigaltstack(c.byref(previous), None) == 0
libm = c.CDLL('libm.so.6')
libc.strtouq.argtypes = [c.c_void_p, c.POINTER(c.c_void_p), c.c_int]
libc.strtouq.restype = c.c_uint64
for text, base, expected, consumed, error in [
    (b'18446744073709551615!', 10, 2**64-1, 20, 0),
    (b'18446744073709551616!', 10, 2**64-1, 20, errno.ERANGE),
    (b' -1x', 10, 2**64-1, 3, 0), (b'0xff!', 0, 255, 4, 0),
    (b'xyz', 10, 0, 0, 0)]:
    source = c.create_string_buffer(text)
    end = c.c_void_p()
    c.set_errno(0)
    assert libc.strtouq(source, c.byref(end), base) == expected
    assert end.value - c.addressof(source) == consumed and c.get_errno() == error

class Timespec(c.Structure):
    _fields_ = [('seconds', c.c_long), ('nanoseconds', c.c_long)]

libc.clock_getcpuclockid.argtypes = [c.c_int, c.POINTER(c.c_int)]
libc.clock_gettime.argtypes = [c.c_int, c.POINTER(Timespec)]
libc.clock_getres.argtypes = [c.c_int, c.POINTER(Timespec)]
def cpu(clock):
    value = Timespec()
    assert libc.clock_gettime(clock, c.byref(value)) == 0
    return value.seconds * 10**9 + value.nanoseconds
for pid in (0, os.getpid()):
    clock = c.c_int(777)
    c.set_errno(999)
    assert libc.clock_getcpuclockid(pid, c.byref(clock)) == 0 and c.get_errno() == 999
    assert clock.value == ((~pid) << 3) | 2
    assert libc.clock_getres(clock, None) == 0
    before = cpu(clock)
    stop = time.monotonic() + 0.15
    while time.monotonic() < stop:
        sum(range(1000))
    assert cpu(clock) > before
clock = c.c_int(777)
assert libc.clock_getcpuclockid(99999999, c.byref(clock)) == errno.ESRCH and clock.value == 777
with subprocess.Popen(['/bin/sleep', '10']) as child:
    try:
        assert libc.clock_getcpuclockid(child.pid, c.byref(clock)) == 0
        assert cpu(clock) >= 0
    finally:
        child.terminate(); child.wait(timeout=5)

libm.fegetexceptflag.argtypes = [c.POINTER(c.c_uint16), c.c_int]
libm.fesetexceptflag.argtypes = [c.POINTER(c.c_uint16), c.c_int]
saved = c.create_string_buffer(32)
assert libm.fegetenv(saved) == 0
try:
    libm.fedisableexcept(0x3d)
    assert libm.feclearexcept(0x3d) == 0
    assert libm.fesetround(0x800) == 0
    flags = c.c_uint16(0x25)
    assert libm.fesetexceptflag(c.byref(flags), 0x3d) == 0
    captured = c.c_uint16(0xffff)
    assert libm.fegetexceptflag(c.byref(captured), 0x05) == 0 and captured.value == 0x05
    zero = c.c_uint16(0)
    assert libm.fesetexceptflag(c.byref(zero), 0x04) == 0
    assert libm.fetestexcept(0x3d) == 0x21 and libm.fegetround() == 0x800
    assert libm.feclearexcept(0x3d) == 0
    # Setting a saved divide-by-zero flag with traps enabled must not raise.
    libm.feenableexcept(0x04)
    flags.value = 0x04
    assert libm.fesetexceptflag(c.byref(flags), 0x04) == 0
    assert libm.fegetexceptflag(c.byref(captured), 0x04) == 0 and captured.value == 0x04
    libm.fedisableexcept(0x3d)
finally:
    assert libm.fesetenv(saved) == 0
print('DATABASE_ABI_QUAD_CPU_CLOCK_EXCEPTION_FLAGS_OK')
