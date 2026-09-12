"""Qt's XCB reader must not consume CEF's Xlib PropertyNotify replies."""
import ctypes as C
import os
import time

P, I, U = C.c_void_p, C.c_int, C.c_ulong


def bind(lib, name, result, *arguments):
    function = getattr(lib, name)
    function.restype, function.argtypes = result, arguments
    return function


x = C.CDLL('libX11.so.6')
b = C.CDLL('libxcb.so.1')
bridge = C.CDLL('libX11-xcb.so.1')
open_display = bind(x, 'XOpenDisplay', P, P)
displays = [open_display(None) for _ in range(3)]
assert all(displays) and len(set(displays)) == 3
gtk, cef, qt = displays
create = bind(x, 'XCreateSimpleWindow', U, P, U, I, I, C.c_uint, C.c_uint, C.c_uint, U, U)
select = bind(x, 'XSelectInput', I, P, U, C.c_long)
atom = bind(x, 'XInternAtom', U, P, C.c_char_p, I)(cef, b'KINAKAZE_CEF_TIMESTAMP_PROBE', 0)
change = bind(x, 'XChangeProperty', I, P, U, U, U, I, I, P, I)
check = bind(x, 'XCheckTypedWindowEvent', I, P, U, I, P)
free = bind(x, 'XFree', I, P)
cef_window, qt_window = [create(d, 1, 0, 0, 1, 1, 0, 0, 0) for d in [cef, qt]]
assert cef_window and qt_window
select(cef, cef_window, 1 << 22)
select(gtk, cef_window, 1 << 22)
select(qt, qt_window, 1 << 22)
bind(bridge, 'XSetEventQueueOwner', None, P, I)(qt, 1)
connection = bind(bridge, 'XGetXCBConnection', P, P)(qt)
cef_connection = bind(bridge, 'XGetXCBConnection', P, P)(cef)
assert cef_connection != connection
class Cookie(C.Structure):
    _fields_ = [('sequence', C.c_uint)]
mask = C.c_uint(1 << 22)
# CEF uses XCB to select events, then Xlib to receive the timestamp reply.
bind(b, 'xcb_change_window_attributes', Cookie, P, C.c_uint, C.c_uint, P)(
    cef_connection, cef_window, 1 << 11, C.byref(mask))
poll = bind(b, 'xcb_poll_for_event', P, P)
getfd = bind(b, 'xcb_get_file_descriptor', I, P)
xfd = bind(x, 'XConnectionNumber', I, P)
assert getfd(connection) == xfd(qt) and getfd(connection) != xfd(cef)

for _ in range(10):
    # Appending zero bytes is the server timestamp round trip used by CEF.
    assert change(cef, cef_window, atom, 31, 8, 2, None, 0)
    assert change(qt, qt_window, atom, 31, 8, 2, None, 0)
    properties = []
    for _ in range(32):
        event = poll(connection)
        if not event:
            break
        if C.c_ubyte.from_address(event).value & 127 == 28 and C.c_uint.from_address(event + 8).value == atom:
            properties.append(C.c_uint.from_address(event + 4).value)
        free(event)
    assert properties == [qt_window], ('XCB reader stole another display event', properties)
    for display in [cef, gtk]:
        event = (C.c_long * 24)()
        assert check(display, cef_window, 28, event), 'Xlib PropertyNotify was lost'
        assert event[3] == display and event[4] == cef_window

Predicate = C.CFUNCTYPE(I, P, P, P)
@Predicate
def matches(display, event, window):
    fields = C.cast(event, C.POINTER(C.c_long))
    return C.c_int.from_address(event).value == 28 and fields[4] == cef_window

change(cef, cef_window, atom, 31, 8, 2, None, 0)
event = (C.c_long * 24)()
assert bind(x, 'XIfEvent', I, P, P, Predicate, P)(cef, event, matches, None) == 0
assert event[4] == cef_window
assert check(gtk, cef_window, 28, event)

# Native WM_PAINT must reach both frontends. Let the XCB reader pump first;
# previously it swallowed the Xlib window's expose along with its own.
select(cef, cef_window, 1 << 15)
select(gtk, cef_window, 0)
select(qt, qt_window, 1 << 15)
resize = bind(x, 'XResizeWindow', I, P, U, C.c_uint, C.c_uint)
map_window = bind(x, 'XMapWindow', I, P, U)
for display, window in [(cef, cef_window), (qt, qt_window)]:
    resize(display, window, 64, 64)
    # Discard the synthetic resize exposure before mapping generates WM_PAINT.
    while check(display, window, 12, event):
        pass
    map_window(display, window)
time.sleep(0.1)
exposed = set()
for _ in range(128):
    native = poll(connection)
    if not native:
        break
    if C.c_ubyte.from_address(native).value & 127 == 12:
        exposed.add(C.c_uint.from_address(native + 4).value)
    free(native)
assert cef_window not in exposed, ('XCB stole Xlib expose', exposed)
assert qt_window in exposed, ('XCB expose missing', exposed)
assert check(cef, cef_window, 12, event), 'Native Xlib expose missing'

# CEF embeds its Xlib window inside a Qt-created XCB resource name.
parent_id = bind(b, 'xcb_generate_id', C.c_uint, P)(connection)
bind(b, 'xcb_create_window', Cookie, P, C.c_ubyte, C.c_uint, C.c_uint,
     C.c_short, C.c_short, C.c_ushort, C.c_ushort, C.c_ushort,
     C.c_ushort, C.c_uint, C.c_uint, P)(connection, 24, parent_id, 1,
                                     0, 0, 200, 150, 0, 1, 1, 0, None)
reparent = bind(x, 'XReparentWindow', I, P, U, U, I, I)
assert reparent(cef, cef_window, parent_id, 7, 9), 'Xlib cannot embed in an XCB parent'
attributes = (C.c_long * 32)()
assert bind(x, 'XGetWindowAttributes', I, P, U, P)(cef, parent_id, attributes)
query = bind(b, 'xcb_query_tree', Cookie, P, C.c_uint)
reply = bind(b, 'xcb_query_tree_reply', P, P, Cookie, P)
answer = reply(connection, query(connection, cef_window), None)
assert answer and C.c_uint.from_address(answer + 12).value == parent_id
free(answer)

# The per-display owner and descriptor survive the native fork handoff.
child = os.fork()
if child == 0:
    assert getfd(connection) == xfd(qt) and getfd(connection) != xfd(cef)
    assert bind(x, 'XGetWindowAttributes', I, P, U, P)(cef, parent_id, attributes)
    os._exit(0)
assert os.waitpid(child, 0)[1] == 0
destroy = bind(x, 'XDestroyWindow', I, P, U)
destroy(cef, cef_window)
destroy(qt, qt_window)
bind(b, 'xcb_destroy_window', Cookie, P, C.c_uint)(connection, parent_id)
close = bind(x, 'XCloseDisplay', I, P)
for display in reversed(displays):
    close(display)
print('MIXED_XLIB_XCB_OK', flush=True)
