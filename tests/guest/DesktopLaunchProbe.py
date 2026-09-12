"""Run inside a GNOME session and verify its real WM adopts a foreign client."""
import ctypes as c
import json
from pathlib import Path
import time
from gi.repository import Gio, GLib

x = c.CDLL('libX11.so.6')
p, u, i = c.c_void_p, c.c_ulong, c.c_int

def bind(name, result, *args):
    fn = getattr(x, name); fn.restype = result; fn.argtypes = args
    return fn

d = bind('XOpenDisplay', p, p)(None)
intern = bind('XInternAtom', u, p, c.c_char_p, i)
get = bind('XGetWindowProperty', i, p, u, u, c.c_long, c.c_long, i, u, p, p, p, p, p)
free = bind('XFree', i, p)
pending = bind('XPending', i, p)
next_event = bind('XNextEvent', i, p, p)
sync = bind('XSync', i, p, i)
class Attributes(c.Structure):
    _fields_ = [(name, kind) for names, kind in [
        ('x y width height border depth', i), ('visual', p), ('root', u),
        ('window_class bit_gravity win_gravity backing_store', i),
        ('backing_planes backing_pixel', u), ('save_under', i), ('colormap', u),
        ('map_installed map_state', i), ('all_event_masks your_event_mask do_not_propagate_mask', c.c_long),
        ('override_redirect', i), ('screen', p)] for name in names.split()]

def attributes(window):
    value = Attributes()
    result = bind('XGetWindowAttributes', i, p, u, p)(d, window, c.byref(value))
    return {name: getattr(value, name) for name in
            ('map_state', 'all_event_masks', 'your_event_mask', 'override_redirect')} if result else None

def request(name, window, *values):
    event = (u * 24)()
    event[0], event[3], event[4], event[5], event[6] = 33, d, window, intern(d, name.encode(), 0), 32
    for index, value in enumerate(values): event[7 + index] = value
    assert bind('XSendEvent', i, p, u, i, c.c_long, p)(d, 1, 0, (1 << 19) | (1 << 20), event)
    sync(d, 0)

def wait_property(window, name, expected):
    deadline = time.monotonic() + 5
    while property(window, name) != expected:
        assert time.monotonic() < deadline, (name, expected, property(window, name))
        time.sleep(.02)

def property(window, name):
    kind, fmt, count, after, data = u(), i(), u(), u(), p()
    get(d, window, intern(d, name.encode(), 0), 0, 65536, 0, 0,
        c.byref(kind), c.byref(fmt), c.byref(count), c.byref(after), c.byref(data))
    try:
        if not data.value: return None
        if fmt.value == 32: return list(c.cast(data, c.POINTER(u))[:count.value])
        return c.string_at(data, count.value).decode(errors='replace')
    finally:
        if data.value: free(data)


import os
rows=[]
for desktop in ('org.gnome.Calculator.desktop', 'org.gnome.gedit.desktop'):
    previous=set(property(1, '_NET_CLIENT_LIST') or [])
    app=Gio.DesktopAppInfo.new(desktop);assert app
    started=time.monotonic()
    app.launch([], None)
    launch=time.monotonic()-started
    while time.monotonic()-started < 45:
        new=set(property(1, '_NET_CLIENT_LIST') or [])-previous
        ready=[w for w in new if (attributes(w) or {}).get('map_state') == 2]
        if ready:break
        while GLib.MainContext.default().iteration(False):pass
        time.sleep(.01)
    assert ready, (desktop, 'no managed application window within 45 seconds')
    rows.append(dict(app=desktop,launch_ms=round(launch*1000,2),mapped_ms=round((time.monotonic()-started)*1000,2),windows=ready))
    print(rows[-1],flush=True)
    Path('/tmp/desktop-launch-probe.json').write_text(json.dumps(rows,indent=2))
    time.sleep(1)
