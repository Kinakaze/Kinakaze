"""Mapping must release clients waiting for X11 VisibilityNotify (e.g. GLFW)."""
import ctypes as C

x = C.CDLL('libX11.so.6')
p, i, u, xid = C.c_void_p, C.c_int, C.c_uint, C.c_ulong


def bind(name, result, *args):
    fn = getattr(x, name)
    fn.restype, fn.argtypes = result, args
    return fn


class Visibility(C.Structure):
    _fields_ = [('type', i), ('serial', xid), ('send_event', i),
                ('display', p), ('window', xid), ('state', i)]


class Event(C.Union):
    _fields_ = [('visibility', Visibility), ('pad', C.c_long * 24)]


open_display = bind('XOpenDisplay', p, C.c_char_p)
close_display = bind('XCloseDisplay', i, p)
create = bind('XCreateSimpleWindow', xid, p, xid, i, i, u, u, u, xid, xid)
select = bind('XSelectInput', i, p, xid, C.c_long)
map_window = bind('XMapWindow', i, p, xid)
unmap = bind('XUnmapWindow', i, p, xid)
check = bind('XCheckTypedWindowEvent', i, p, xid, i, C.POINTER(Event))
destroy = bind('XDestroyWindow', i, p, xid)
displays = [open_display(None) for _ in range(3)]
assert all(displays)
d, observer, uninterested = displays
windows = []


def visibility(display, window, expected=True):
    event = Event()
    found = check(display, window, 15, C.byref(event))
    assert bool(found) == expected, ('VisibilityNotify', window, display, found, expected)
    if found:
        v = event.visibility
        assert v.display == display and v.window == window and not v.send_event
        assert v.state in (0, 1, 2)
    return event


try:
    parent = create(d, 1, 20, 20, 96, 64, 0, 0, 0)
    child = create(d, parent, 4, 4, 32, 24, 0, 0, 0)
    windows.append(parent)
    for window in (parent, child):
        select(d, window, (1 << 16) | (1 << 17))
        select(observer, window, 1 << 16)
        select(uninterested, window, 1 << 17)
    # Mapping below an unmapped ancestor is not yet a visibility transition.
    map_window(d, child)
    visibility(d, child, False)
    visibility(observer, child, False)
    map_window(d, parent)
    for window in (parent, child):
        visibility(d, window)
        visibility(observer, window)
        visibility(uninterested, window, False)
        visibility(d, window, False)
    # A repeated map is not another transition, but remapping the ancestor is.
    map_window(d, parent)
    visibility(d, parent, False)
    unmap(d, parent)
    visibility(d, parent, False)
    map_window(d, parent)
    for window in (parent, child):
        visibility(d, window)
        visibility(observer, window)
    # InputOnly windows never receive VisibilityNotify.
    input_only = bind('XCreateWindow', xid, p, xid, i, i, u, u, u, i, u, p, xid, p)(
        d, 1, 0, 0, 8, 8, 0, 0, 2, None, 0, None)
    windows.append(input_only)
    select(d, input_only, 1 << 16)
    map_window(d, input_only)
    visibility(d, input_only, False)
    # Both conversion directions must preserve the 32-byte X11 wire layout.
    encode = bind('_XEventToWire', i, p, C.POINTER(Event), p)
    decode = bind('_XWireToEvent', i, p, C.POINTER(Event), p)
    for state in (0, 1, 2):
        event, actual = Event(), Event()
        event.visibility = Visibility(15, 0x1234, 0, d, parent, state)
        wire = (C.c_ubyte * 32)()
        assert encode(d, C.byref(event), wire) == 1
        assert bytes(wire)[4:8] == parent.to_bytes(4, 'little') and wire[8] == state
        assert decode(d, C.byref(actual), wire) == 1
        v = actual.visibility
        assert (v.type, v.serial, v.display, v.window, v.state) == (15, 0x1234, d, parent, state)
finally:
    for window in reversed(windows):
        destroy(d, window)
    for display in reversed(displays):
        close_display(display)
print('X11_MAP_VISIBILITY_NOTIFY_OK', flush=True)
