import ctypes


libc = ctypes.CDLL(None)
libc.snprintf.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_char_p]
libc.snprintf.restype = ctypes.c_int


def check(format_string, expected, *arguments):
    buffer = ctypes.create_string_buffer(256)
    count = libc.snprintf(buffer, len(buffer), format_string, *arguments)
    assert count == len(expected) and buffer.value == expected, (format_string, count, buffer.value, expected)


for conversion in (b's', b'ls'):
    for precision in range(9):
        expected = b'(null)' if precision >= 6 else b''
        check(b'[%.*' + conversion + b']', b'[' + expected + b']',
              ctypes.c_int(precision), ctypes.c_void_p())
        check(b'[%8.*' + conversion + b']', b'[' + expected.rjust(8) + b']',
              ctypes.c_int(precision), ctypes.c_void_p())
        check(b'[%-8.*' + conversion + b']', b'[' + expected.ljust(8) + b']',
              ctypes.c_int(precision), ctypes.c_void_p())
    check(b'%.*' + conversion, b'(null)', ctypes.c_int(-1), ctypes.c_void_p())
check(b'addr="%.*s",order=%lld,time=%llu', b'addr="",order=1,time=42',
      ctypes.c_int(0), ctypes.c_void_p(), ctypes.c_longlong(1), ctypes.c_ulonglong(42))
check(b'%lld,%lld,%d', b'-1,-1,0', ctypes.c_longlong(-1), ctypes.c_longlong(-1), ctypes.c_int(0))
print('NULL_STRING_PRECISION_WIDTH_CHECKPOINT_FORMAT_OK')
