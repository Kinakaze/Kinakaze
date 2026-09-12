"""GDK must catch errors from geometry queries within the current X request."""
import ctypes as c

p, i, u = c.c_void_p, c.c_int, c.c_ulong
gdk, x = c.CDLL('libgdk-3.so.0'), c.CDLL('libX11.so.6')

def bind(lib, name, result, *args):
    function = getattr(lib, name)
    function.restype, function.argtypes = result, args
    return function

assert bind(gdk, 'gdk_init_check', i, p, p)(None, None)
display = bind(gdk, 'gdk_display_get_default', p)()
xdisplay = bind(gdk, 'gdk_x11_display_get_xdisplay', p, p)(display)
push = bind(gdk, 'gdk_x11_display_error_trap_push', None, p)
pop = bind(gdk, 'gdk_x11_display_error_trap_pop', i, p)
next_request = bind(x, 'XNextRequest', u, p)
geometry = bind(x, 'XGetGeometry', i, p, u, p, p, p, p, p, p, p)
for drawable, expected in ((1, 0), (0, 9)):
    push(display)
    serial = next_request(xdisplay)
    root, xpos, ypos = u(), i(), i()
    width, height, border, depth = (c.c_uint() for _ in range(4))
    result = geometry(xdisplay, drawable, c.byref(root), c.byref(xpos), c.byref(ypos),
                      c.byref(width), c.byref(height), c.byref(border), c.byref(depth))
    # Flushing older requests may run GDK's async round-trip callbacks, which
    # can issue additional requests. This query still needs a fresh serial.
    assert next_request(xdisplay) > serial, (drawable, serial, next_request(xdisplay))
    assert pop(display) == expected
    assert bool(result) == (expected == 0)
push(display)
serial = next_request(xdisplay)
attributes = c.create_string_buffer(256)
assert bind(x, 'XGetWindowAttributes', i, p, u, p)(xdisplay, 0, attributes) == 0
assert next_request(xdisplay) > serial and pop(display) == 3
# Mutter asks XGetEventData for core events as well. Their union tail is not
# a cookie payload; a nonzero tail must never be dereferenced or freed.
class Cookie(c.Structure):
    _fields_ = [('kind', i), ('serial', u), ('send', i), ('display', p),
                ('extension', i), ('evtype', i), ('cookie', c.c_uint), ('data', p)]
cookie = Cookie(kind=2, serial=123, display=xdisplay, data=1)
get_data = bind(x, 'XGetEventData', i, p, p)
free_data = bind(x, 'XFreeEventData', None, p, p)
assert get_data(xdisplay, c.byref(cookie)) == 0
free_data(xdisplay, c.byref(cookie))
assert cookie.data == 1
payload = Cookie()
cookie.kind, cookie.data = 35, c.addressof(payload)
assert get_data(xdisplay, c.byref(cookie)) == 1 and payload.serial == 123
print('GDK_GEOMETRY_REQUEST_ERROR_TRAP_OK', flush=True)
