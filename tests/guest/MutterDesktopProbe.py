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

report = {'status': 'waiting for Mutter', 'samples': []}
output = Path('/tmp/mutter-desktop-probe.json')
window = 0
try:
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        check = property(1, '_NET_SUPPORTING_WM_CHECK')
        name = property(check[0], '_NET_WM_NAME') if check else None
        if name and name != 'Kinakaze': break
        time.sleep(.1)
    report.update(manager=name, supporting_window=check)
    assert name and name != 'Kinakaze', report
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    while time.monotonic() < deadline:
        try:
            reply = bus.call_sync('org.gnome.SessionManager', '/org/gnome/SessionManager',
                'org.gnome.SessionManager', 'IsSessionRunning', None,
                GLib.VariantType.new('(b)'), Gio.DBusCallFlags.NONE, 1000, None)
            if reply.unpack()[0]: break
        except GLib.Error:
            pass
        time.sleep(.1)
    else:
        raise AssertionError('GNOME session did not finish starting')
    report['session_running'] = True
    report['root_attributes'] = attributes(1)
    window = bind('XCreateSimpleWindow', u, p, u, i, i, c.c_uint, c.c_uint, c.c_uint, u, u)(
        d, 1, 240, 180, 360, 240, 0, 0, 0)
    bind('XStoreName', i, p, u, c.c_char_p)(d, window, b'Mutter integration probe')
    bind('XSelectInput', i, p, u, u)(d, window, (1 << 17) | (1 << 15))
    gc = bind('XCreateGC', p, p, u, u, p)(d, window, 0, None)
    bind('XSetForeground', i, p, p, u)(d, gc, 0x3388cc)
    bind('XFillRectangle', i, p, u, p, i, i, c.c_uint, c.c_uint)(d, window, gc, 0, 0, 360, 240)
    bind('XMapWindow', i, p, u)(d, window)
    sync(d, 0)
    report['window'] = window
    mapped_at = time.monotonic()
    map_notified = False
    for _ in range(100):
        events = []
        while pending(d):
            event = (u * 24)(); next_event(d, event); events.append(event[0] & 0xffffffff)
        map_notified |= 19 in events
        sample = {'clients': property(1, '_NET_CLIENT_LIST'), 'state': property(window, 'WM_STATE'),
                  'workspace': property(window, '_NET_WM_DESKTOP'), 'events': events}
        report['samples'].append(sample)
        report['status'] = 'managed' if window in (sample['clients'] or []) else 'not managed'
        output.write_text(json.dumps(report, indent=2))
        if report['status'] == 'managed' and map_notified: break
        time.sleep(.1)
    assert report['status'] == 'managed', dict(window=window, last=sample, root=report['root_attributes'])
    assert map_notified, 'Managed client never received MapNotify'
    report['map_notified'] = True
    report['map_ms'] = round((time.monotonic() - mapped_at) * 1000, 2)
    print('MUTTER_FOREIGN_WINDOW_MANAGED ' + str(window), flush=True)
    report['desktop_count'] = property(1, '_NET_NUMBER_OF_DESKTOPS')
    desktop = property(1, '_NET_CURRENT_DESKTOP')[0]
    alternate = 1 if desktop == 0 else 0
    try:
        request('_NET_WM_DESKTOP', window, alternate, 1)
        wait_property(window, '_NET_WM_DESKTOP', [alternate])
        request('_NET_CURRENT_DESKTOP', 1, alternate, 0)
        wait_property(1, '_NET_CURRENT_DESKTOP', [alternate])
        report['workspace_move_and_switch'] = True
    finally:
        request('_NET_WM_DESKTOP', window, desktop, 1)
        request('_NET_CURRENT_DESKTOP', 1, desktop, 0)
    wait_property(window, '_NET_WM_DESKTOP', [desktop])
    wait_property(1, '_NET_CURRENT_DESKTOP', [desktop])
    # Keep the managed surface available briefly for a host screenshot, then
    # verify that destroying it also removes Mutter's client and overview entry.
    time.sleep(8)
    report['status'] = 'destroying'
    output.write_text(json.dumps(report, indent=2))
    destroyed = window
    bind('XDestroyWindow', i, p, u)(d, window)
    window = 0
    sync(d, 0)
    deadline = time.monotonic() + 5
    while destroyed in (property(1, '_NET_CLIENT_LIST') or []):
        assert time.monotonic() < deadline, 'Mutter retained the destroyed client'
        time.sleep(.05)
    report['status'] = 'passed'
    report['destroyed_client_removed'] = True
    print('MUTTER_CLIENT_LIFECYCLE_OK', flush=True)
except BaseException as error:
    report.update(status='failed', error=repr(error))
    raise
finally:
    output.write_text(json.dumps(report, indent=2))
    if window: bind('XDestroyWindow', i, p, u)(d, window)
    bind('XCloseDisplay', i, p)(d)
