"""Native focus must reach GTK's XI2 event source, including focus loss."""
import ctypes as c
import time
import sys
import gi

gi.require_version("Gtk", "3.0")
gi.require_version("GdkX11", "3.0")
from gi.repository import Gtk, GdkX11

x = c.CDLL("libX11.so.6")
x.XOpenDisplay.restype = c.c_void_p
x.XSetInputFocus.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_ulong]
x.XCloseDisplay.argtypes = [c.c_void_p]
display = x.XOpenDisplay(None)
windows = []


def dispatch(duration):
    deadline = time.monotonic() + duration
    while time.monotonic() < deadline:
        Gtk.main_iteration_do(False)
        time.sleep(0.005)


try:
    for index in range(2):
        window = Gtk.Window(title="Kinakaze focus regression " + str(index))
        window.set_default_size(260, 120)
        if '--server-decorated' not in sys.argv:
            window.set_titlebar(Gtk.HeaderBar(title="Kinakaze focus regression " + str(index)))
        window.add(Gtk.Entry())
        window.show_all()
        windows.append(window)
    dispatch(2)
    for index in (0, 1, 0):
        target = windows[index].get_window().get_xid()
        result = x.XSetInputFocus(display, target, 2, 0)
        assert result == 1, ("set focus", hex(target), result)
        dispatch(0.3)
        states = [window.is_active() for window in windows]
        assert states[index] and not states[1 - index], (index, states)
    print("GTK_NATIVE_XI2_FOCUS_GAIN_LOSS_OK", flush=True)
finally:
    for window in windows:
        window.destroy()
    x.XCloseDisplay(display)
