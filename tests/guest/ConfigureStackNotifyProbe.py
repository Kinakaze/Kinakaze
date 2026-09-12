"""Geometry and pure restacks report the X11 sibling below the changed window."""
import ctypes as C
import time

x = C.CDLL('libX11.so.6')
p, u, i = C.c_void_p, C.c_ulong, C.c_int
def bind(name, result, *args):
    fn = getattr(x, name)
    fn.restype, fn.argtypes = result, args
    return fn
open_display = bind('XOpenDisplay', p, p)
create = bind('XCreateSimpleWindow', u, p, u, i, i, C.c_uint, C.c_uint, C.c_uint, u, u)
select = bind('XSelectInput', i, p, u, C.c_long)
map_window = bind('XMapWindow', i, p, u)
move = bind('XMoveWindow', i, p, u, i, i)
raise_window = bind('XRaiseWindow', i, p, u)
lower_window = bind('XLowerWindow', i, p, u)
pending = bind('XPending', i, p)
next_event = bind('XNextEvent', i, p, p)
query = bind('XQueryTree', i, p, u, p, p, p, p)
free = bind('XFree', i, p)
destroy = bind('XDestroyWindow', i, p, u)

class Configure(C.Structure):
    _fields_ = [('kind', i), ('serial', u), ('sent', i), ('display', p),
                ('event', u), ('window', u), ('x', i), ('y', i),
                ('width', i), ('height', i), ('border', i), ('above', u), ('override', i)]

d = open_display(None)
windows = [create(d, 1, 30 + n*10, 30, 90, 70, 0, 0, 0) for n in range(3)]
event = (u * 24)()
try:
    for window in windows:
        select(d, window, 1 << 17)
        map_window(d, window)
    while pending(d):
        next_event(d, event)
    for index, operation in enumerate(('move', 'raise', 'lower', 'raise')):
        window = windows[1]
        if operation == 'move':
            move(d, window, 80, 90)
        elif operation == 'raise':
            raise_window(d, window)
        else:
            lower_window(d, window)
        found = None
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            while pending(d):
                next_event(d, event)
                notice = C.cast(event, C.POINTER(Configure)).contents
                if notice.kind == 22 and notice.window == window:
                    found = notice.above
            if found is not None:
                break
            time.sleep(.01)
        root, parent, children, count = u(), u(), C.POINTER(u)(), C.c_uint()
        assert query(d, 1, C.byref(root), C.byref(parent), C.byref(children), C.byref(count))
        try:
            order = list(children[:count.value])
            position = order.index(window)
            expected = order[position-1] if position else 0
            print(operation,'above',found,'expected',expected,'order',order,flush=True)
            assert found == expected, (operation, found, expected, order)
        finally:
            free(children)
finally:
    for window in windows:
        destroy(d, window)
print('CONFIGURE_STACK_NOTIFY_OK', flush=True)
