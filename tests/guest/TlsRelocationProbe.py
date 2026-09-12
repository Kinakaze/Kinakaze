"""MPFR's TLS constant-cache callbacks require relocated .tdata pointers."""
import ctypes as c
from concurrent.futures import ThreadPoolExecutor
import math

m = c.CDLL('libmpfr.so.6')
class Number(c.Structure):
    _fields_ = [('precision', c.c_long), ('sign', c.c_int), ('exponent', c.c_long), ('data', c.c_void_p)]
for name, result, args in (
    ('mpfr_init2', None, [c.POINTER(Number), c.c_ulong]),
    ('mpfr_clear', None, [c.POINTER(Number)]),
    ('mpfr_const_pi', c.c_int, [c.POINTER(Number), c.c_int]),
    ('mpfr_const_log2', c.c_int, [c.POINTER(Number), c.c_int]),
    ('mpfr_get_d', c.c_double, [c.POINTER(Number), c.c_int]),
    ('mpfr_free_cache', None, []),
):
    fn = getattr(m, name); fn.restype = result; fn.argtypes = args
def constants(index):
    number = Number()
    m.mpfr_init2(c.byref(number), 64 + index * 32)
    try:
        for _ in range(3):
            m.mpfr_const_pi(c.byref(number), 0)
            assert abs(m.mpfr_get_d(c.byref(number), 0) - math.pi) < 1e-15
            m.mpfr_const_log2(c.byref(number), 0)
            assert abs(m.mpfr_get_d(c.byref(number), 0) - math.log(2)) < 1e-15
    finally:
        m.mpfr_clear(c.byref(number)); m.mpfr_free_cache()
    return index
constants(0)
with ThreadPoolExecutor(max_workers=4) as executor:
    assert list(executor.map(constants, range(8))) == list(range(8))
constants(2)
print('ELF_TLS_RELOCATED_CALLBACKS_THREADS_OK', flush=True)
