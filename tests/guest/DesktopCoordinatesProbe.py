"""Report real GTK hit coordinates for a window and an owned transient."""
import ctypes as c
import json

p, i = c.c_void_p, c.c_int
gtk, gdk, obj, cairo = (c.CDLL(name) for name in
    ('libgtk-3.so.0', 'libgdk-3.so.0', 'libgobject-2.0.so.0', 'libcairo.so.2'))
def bind(lib, name, result, *args):
    fn = getattr(lib, name); fn.restype, fn.argtypes = result, args; return fn
assert bind(gtk, 'gtk_init_check', i, p, p)(None, None)
signal = bind(obj, 'g_signal_connect_data', c.c_ulong, p, c.c_char_p, p, p, p, i)
windows, callbacks = [], []
for role in ('application', 'dialog'):
    window = bind(gtk, 'gtk_window_new', p, i)(0)
    header = bind(gtk, 'gtk_header_bar_new', p)()
    title = ('Kinakaze coordinate probe ' + role).encode()
    bind(gtk, 'gtk_header_bar_set_title', None, p, c.c_char_p)(header, title)
    bind(gtk, 'gtk_window_set_titlebar', None, p, p)(window, header)
    bind(gtk, 'gtk_window_set_title', None, p, c.c_char_p)(window, title)
    bind(gtk, 'gtk_window_set_default_size', None, p, i, i)(window, 480, 330)
    if windows:
        bind(gtk, 'gtk_window_set_transient_for', None, p, p)(window, windows[0])
    area = bind(gtk, 'gtk_drawing_area_new', p)()
    bind(gtk, 'gtk_widget_add_events', None, p, i)(area, (1 << 8) | (1 << 9))
    bind(gtk, 'gtk_container_add', None, p, p)(window, area)
    callback = c.CFUNCTYPE(i, p, p, p)
    last = [None]
    @callback
    def draw(widget, context, data, window=window, role=role, last=last):
        width = bind(gtk, 'gtk_widget_get_allocated_width', i, p)(widget)
        height = bind(gtk, 'gtk_widget_get_allocated_height', i, p)(widget)
        x, y = i(), i()
        bind(gtk, 'gtk_widget_translate_coordinates', i, p, p, i, i, p, p)(widget, window, 0, 0, c.byref(x), c.byref(y))
        native = bind(gtk, 'gtk_widget_get_window', p, p)(window)
        xid = bind(gdk, 'gdk_x11_window_get_xid', c.c_ulong, p)(native)
        allocation = [x.value, y.value, width, height]
        if allocation != last[0]:
            last[0] = allocation
            print(json.dumps(dict(kind='ready', role=role, hwnd=xid, area=allocation)), flush=True)
        bind(cairo, 'cairo_set_source_rgb', None, p, c.c_double, c.c_double, c.c_double)(context, .15, .35, .55)
        bind(cairo, 'cairo_paint', None, p)(context)
        return 0
    @callback
    def press(widget, event, data, role=role):
        x, y, rx, ry = (c.c_double() for _ in range(4))
        bind(gdk, 'gdk_event_get_coords', i, p, p, p)(event, c.byref(x), c.byref(y))
        bind(gdk, 'gdk_event_get_root_coords', i, p, p, p)(event, c.byref(rx), c.byref(ry))
        print(json.dumps(dict(kind='click', role=role, local=[x.value, y.value], root=[rx.value, ry.value])), flush=True)
        return 1
    callbacks.extend((draw, press))
    signal(area, b'draw', draw, None, None, 0)
    signal(area, b'button-press-event', press, None, None, 0)
    windows.append(window)
    bind(gtk, 'gtk_widget_show_all', None, p)(window)
bind(gtk, 'gtk_main', None)()
