import ctypes as C
import math

for name in ('libc.so.6', 'libm.so.6'):
    library = C.CDLL(name)
    library.isinf.argtypes, library.isinf.restype = [C.c_double], C.c_int
    library.isnan.argtypes, library.isnan.restype = [C.c_double], C.c_int
    assert library.isinf(math.inf) == 1 and library.isinf(-math.inf) == -1
    assert library.isinf(0) == library.isinf(math.nan) == 0
    assert library.isnan(math.nan) != 0 and library.isnan(math.inf) == 0
libc = C.CDLL('libc.so.6')
libc.__strndup.argtypes, libc.__strndup.restype = [C.c_char_p, C.c_size_t], C.c_void_p
libc.free.argtypes = [C.c_void_p]
for data, length, expected in ((b'hello', 2, b'he'), (b'hello', 8, b'hello'), (b'hello', 0, b'')):
    pointer = libc.__strndup(data, length)
    assert pointer and C.string_at(pointer) == expected
    libc.free(pointer)
print('COMMON_LIBC_ALIASES_OK', flush=True)
