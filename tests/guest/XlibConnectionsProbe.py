"""Independent Xlib event consumers, as used by Mutter's backend and WM."""
import ctypes as c
import json
import os
import subprocess
import sys
import time

x = c.CDLL('libX11.so.6')
p, u, i = c.c_void_p, c.c_ulong, c.c_int
def api(name, result, *args):
    f = getattr(x, name); f.restype = result; f.argtypes = args; return f
open_display = api('XOpenDisplay', p, p)
close = api('XCloseDisplay', i, p)
select = api('XSelectInput', i, p, u, c.c_long)
pending = api('XPending', i, p)
next_event = api('XNextEvent', i, p, p)
fd = api('XConnectionNumber', i, p)
map_window = api('XMapWindow', i, p, u)
intern = api('XInternAtom', u, p, c.c_char_p, i)
change = api('XChangeProperty', i, p, u, u, u, i, i, p, i)

def events(display):
    result = []
    while pending(display):
        event = (u * 24)(); next_event(display, event)
        assert event[3] == display, 'event belongs to another Display'
        result.append(list(event))
    return result

a = open_display(None)
if len(sys.argv) > 1:
    window = api('XCreateSimpleWindow', u, p, u, i, i, c.c_uint, c.c_uint, c.c_uint, u, u)(
        a, 1, 100, 100, 240, 160, 0, 0, 0)
    map_window(a, window)
    print(window, flush=True)
    sys.stdin.readline()
    api('XDestroyWindow', i, p, u)(a, window)
    close(a)
    sys.exit(0)

b = open_display(None)
assert a and b and a != b and fd(a) != fd(b), 'XOpenDisplay must create independent connections'
screen = api('XDefaultScreenOfDisplay', p, p)
assert api('XDisplayOfScreen', p, p)(screen(b)) == b
select(b, 1, (1 << 19) | (1 << 20))
local = api('XCreateSimpleWindow', u, p, u, i, i, c.c_uint, c.c_uint, c.c_uint, u, u)(
    a, 1, 100, 100, 240, 160, 0, 0, 0)
map_window(a, local)
assert not events(a)
assert any(e[0] == 20 and e[5] == local for e in events(b)), 'local connection bypassed the WM'
api('XDestroyWindow', i, p, u)(a, local)
events(b)
child = subprocess.Popen(['/usr/bin/python3.11', __file__, 'producer'],
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
try:
    window = int(child.stdout.readline())
    received = []
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        assert not events(a), 'input backend stole WM events'
        received += events(b)
        if any(e[0] == 20 and e[5] == window for e in received): break
        time.sleep(.01)
    assert any(e[0] == 20 and e[5] == window for e in received), 'MapRequest missing'
    map_window(b, window)
    events(a); events(b)
    assert api('XSetInputFocus', i, p, u, i, u)(b, window, 1, 0) == 1
    focused, revert = u(), i()
    api('XGetInputFocus', i, p, p, p)(a, c.byref(focused), c.byref(revert))
    assert focused.value == window and revert.value == 1, 'foreign focus is not shared'
    atom = intern(b, b'KINAKAZE_CONNECTIONS', 0)
    select(a, window, 1 << 22); select(b, window, 1 << 22)
    change(a, window, atom, 31, 8, 0, c.c_char_p(b'one'), 3)
    assert any(e[0] == 28 and e[4] == window for e in events(a))
    assert any(e[0] == 28 and e[4] == window for e in events(b))
    # Extension request buffers and Damage notifications also belong to their
    # creating connection. Mutter's input backend must not consume WM damage.
    damage = c.CDLL('libXdamage.so.1')
    version = damage.XDamageQueryVersion; version.restype = i; version.argtypes = [p, p, p]
    create_damage = damage.XDamageCreate; create_damage.restype = u; create_damage.argtypes = [p, u, i]
    destroy_damage = damage.XDamageDestroy; destroy_damage.restype = None; destroy_damage.argtypes = [p, u]
    major, minor = i(), i()
    assert version(b, c.byref(major), c.byref(minor))
    damaged = api('XCreateSimpleWindow', u, p, u, i, i, c.c_uint, c.c_uint, c.c_uint, u, u)(a, 1, 0, 0, 240, 160, 0, 0, 0)
    gc = api('XCreateGC', p, p, u, u, p)(a, damaged, 0, None)
    api('XFillRectangle', i, p, u, p, i, i, c.c_uint, c.c_uint)(a, damaged, gc, 0, 0, 240, 160)
    api('XSync', i, p, i)(a, 0); events(a); events(b)
    watch = create_damage(b, damaged, 3); assert watch
    assert not events(a), 'input connection consumed DamageCreate or its event'
    api('XSync', i, p, i)(b, 0)
    assert not events(a), 'input connection stole the WM Damage event'
    assert any(e[0] == 96 and e[4] == damaged for e in events(b)), 'WM Damage event missing'
    destroy_damage(b, watch); api('XSync', i, p, i)(b, 0)
    api('XFreeGC', i, p, p)(a, gc); api('XDestroyWindow', i, p, u)(a, damaged)
    events(a); events(b)
    forked = os.fork()
    if forked == 0:
        try:
            assert fd(a) != fd(b)
            event = (u * 24)(); event[0] = 33; event[4] = 1
            api('XPutBackEvent', i, p, p)(b, event)
            assert not events(a) and len(events(b)) == 1
            close(b); assert fd(a) >= 0
            os._exit(0)
        except BaseException:
            import traceback; traceback.print_exc(); os._exit(74)
    assert os.waitpid(forked, 0) == (forked, 0)
    close(a); a = None
    change(b, window, atom, 31, 8, 0, c.c_char_p(b'two'), 3)
    assert any(e[0] == 28 and e[4] == window for e in events(b))
    child.stdin.write('destroy\n'); child.stdin.flush(); child.wait(timeout=5)
    print('XLIB_CONNECTIONS_EVENTS_FORK_CLOSE_OK', flush=True)
finally:
    if child.poll() is None: child.terminate(); child.wait(timeout=5)
    if a: close(a)
    close(b)
