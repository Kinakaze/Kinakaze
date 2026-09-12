"""XCB display parsing ownership and unsupported quota error reporting."""
import ctypes as C
import errno
import os

libc = C.CDLL(None, use_errno=True)
xcb = C.CDLL('libxcb.so.1')
xcb.xcb_parse_display.argtypes = [C.c_char_p, C.POINTER(C.c_void_p), C.POINTER(C.c_int), C.POINTER(C.c_int)]
libc.free.argtypes = [C.c_void_p]
os.environ['DISPLAY'] = ':19.3'
for name, expected in [(b':0', (b'', 0, 0)), (b'host:12.4', (b'host', 12, 4)),
                       (b'[::1]:7', (b'[::1]', 7, 0)), (None, (b'', 19, 3)), (b'', (b'', 19, 3))]:
    host, display, screen = C.c_void_p(), C.c_int(-1), C.c_int(-1)
    assert xcb.xcb_parse_display(name, C.byref(host), C.byref(display), C.byref(screen)) == 1
    try:
        assert (C.string_at(host), display.value, screen.value) == expected
    finally:
        libc.free(host)
for name in (b'host', b':', b':-1', b':0.', b':1.2.3', b':2147483648'):
    host, display, screen = C.c_void_p(123), C.c_int(12), C.c_int(34)
    assert xcb.xcb_parse_display(name, C.byref(host), C.byref(display), C.byref(screen)) == 0
    assert (host.value, display.value, screen.value) == (123, 12, 34)
libc.quotactl.argtypes = [C.c_int, C.c_char_p, C.c_int, C.c_void_p]
buffer = C.create_string_buffer(b'unchanged')
C.set_errno(0)
assert libc.quotactl(0, b'/', 0, buffer) == -1 and C.get_errno() == errno.ENOSYS
assert buffer.value == b'unchanged'
print('StartupInterfacesProbe: PASS', flush=True)
