"""C11 time is wall-clock time; unsupported bases must not modify the output."""
import ctypes as C
import time

class Timespec(C.Structure):
    _fields_ = [('sec', C.c_long), ('nsec', C.c_long)]

libc = C.CDLL('libc.so.6')
for name in ('timespec_get', 'timespec_getres'):
    function = getattr(libc, name)
    function.argtypes = [C.POINTER(Timespec), C.c_int]
    function.restype = C.c_int
    value = Timespec(123, 456)
    for base in (0, -1, 99):
        assert function(C.byref(value), base) == 0
        assert (value.sec, value.nsec) == (123, 456)
    before = time.time_ns()
    assert function(C.byref(value), 1) == 1
    after = time.time_ns()
    assert 0 <= value.nsec < 1_000_000_000
    if name == 'timespec_get':
        assert before <= value.sec * 1_000_000_000 + value.nsec <= after
    else:
        assert value.sec >= 0 and value.sec * 1_000_000_000 + value.nsec > 0
        assert function(None, 1) == 1
print('TIMESPEC_GET_OK', flush=True)
