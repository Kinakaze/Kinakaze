"""Real GTK CSD window; the host drags its title bar and selects entry text."""
import ctypes as c
import json
import os

gtk, gdk = (c.CDLL(name) for name in ('libgtk-3.so.0', 'libgdk-3.so.0'))
p, i = c.c_void_p, c.c_int
def bind(lib, name, result, *args):
    f = getattr(lib, name); f.restype, f.argtypes = result, args; return f
assert bind(gtk, 'gtk_init_check', i, p, p)(None, None)
window = bind(gtk, 'gtk_window_new', p, i)(0)
header = bind(gtk, 'gtk_header_bar_new', p)()
bind(gtk, 'gtk_header_bar_set_title', None, p, c.c_char_p)(header, b'Kinakaze drag and selection probe')
bind(gtk, 'gtk_header_bar_set_show_close_button', None, p, i)(header, 1)
bind(gtk, 'gtk_window_set_titlebar', None, p, p)(window, header)
bind(gtk, 'gtk_window_set_title', None, p, c.c_char_p)(window, b'Kinakaze drag and selection probe')
bind(gtk, 'gtk_window_set_default_size', None, p, i, i)(window, 640, 340)
entry = bind(gtk, 'gtk_entry_new', p)()
bind(gtk, 'gtk_entry_set_text', None, p, c.c_char_p)(entry, b'KINAKAZE mouse drag selection across this text must work')
bind(gtk, 'gtk_widget_set_margin_start', None, p, i)(entry, 20)
bind(gtk, 'gtk_widget_set_margin_end', None, p, i)(entry, 20)
box = bind(gtk, 'gtk_box_new', p, i, i)(1, 8)
bind(gtk, 'gtk_container_add', None, p, p)(window, box)
bind(gtk, 'gtk_box_pack_start', None, p, p, i, i, c.c_uint)(box, entry, 0, 0, 0)
keyboard_entry = bind(gtk, 'gtk_entry_new', p)()
bind(gtk, 'gtk_box_pack_start', None, p, p, i, i, c.c_uint)(box, keyboard_entry, 0, 0, 0)
scroll = bind(gtk, 'gtk_scrolled_window_new', p, p, p)(None, None)
label = bind(gtk, 'gtk_label_new', p, c.c_char_p)(b'\n'.join(b'Wheel scrolling row %d' % n for n in range(100)))
bind(gtk, 'gtk_widget_set_size_request', None, p, i, i)(label, 1800, -1)
bind(gtk, 'gtk_container_add', None, p, p)(scroll, label)
bind(gtk, 'gtk_box_pack_start', None, p, p, i, i, c.c_uint)(box, scroll, 1, 1, 0)
adjustment = bind(gtk, 'gtk_scrolled_window_get_vadjustment', p, p)(scroll)
horizontal = bind(gtk, 'gtk_scrolled_window_get_hadjustment', p, p)(scroll)
drawn = []
draw_callback = c.CFUNCTYPE(i, p, p, p)
@draw_callback
def on_draw(widget, context, data):
    drawn.append(True)
    return 0
objects = c.CDLL('libgobject-2.0.so.0')
bind(objects, 'g_signal_connect_data', c.c_ulong, p, c.c_char_p, p, p, p, i)(window, b'draw', on_draw, None, None, 0)
bind(gtk, 'gtk_widget_show_all', None, p)(window)
iterate = bind(gtk, 'gtk_main_iteration_do', i, i)
# The first frame in each header-bar state (backdrop, focused) renders icons
# and fonts, which takes seconds under emulation.  Run the loop by hand and
# print QUIET twice a second while it has stayed responsive for a full second
# since the last slow iteration; the host waits for a fresh QUIET after each
# action before injecting input.
import time
def run_loop(until):
    """Iterate until `until(quiet)` holds; `quiet` is a full responsive second."""
    settled, reported = time.monotonic(), 0.0
    while True:
        started = time.monotonic()
        iterate(0)
        if time.monotonic() - started > 0.05:
            print('SLOW %.3f' % (time.monotonic() - started), flush=True)
            settled = time.monotonic()
        quiet = time.monotonic() - settled >= 1.0
        if quiet and time.monotonic() - reported >= 0.5:
            print('QUIET', flush=True)
            reported = time.monotonic()
        if until(quiet):
            return
        time.sleep(0.01)
run_loop(lambda quiet: bool(drawn) and quiet)
# A process that forks to launch a command must not display another copy of
# this window. The host observes the whole fork interval before READY.
child = os.fork()
if child == 0:
    time.sleep(0.4)
    os._exit(0)
assert os.waitpid(child, 0) == (child, 0)
native = bind(gtk, 'gtk_widget_get_window', p, p)(window)
xid = bind(gdk, 'gdk_x11_window_get_xid', c.c_ulong, p)(native)
ex, ey = i(), i()
assert bind(gtk, 'gtk_widget_translate_coordinates', i, p, p, i, i, p, p)(entry, window, 0, 0, c.byref(ex), c.byref(ey))
height = bind(gtk, 'gtk_widget_get_allocated_height', i, p)(entry)
bind(gtk, 'gtk_editable_select_region', None, p, i, i)(entry, 0, 0)
initial_start, initial_end = i(), i()
assert not bind(gtk, 'gtk_editable_get_selection_bounds', i, p, p, p)(entry, c.byref(initial_start), c.byref(initial_end))
hx, hy = i(), i()
bind(gtk, 'gtk_widget_translate_coordinates', i, p, p, i, i, p, p)(header, window, 0, 0, c.byref(hx), c.byref(hy))
hh = bind(gtk, 'gtk_widget_get_allocated_height', i, p)(header)
sx, sy = i(), i()
bind(gtk, 'gtk_widget_translate_coordinates', i, p, p, i, i, p, p)(scroll, window, 0, 0, c.byref(sx), c.byref(sy))
kx, ky = i(), i()
bind(gtk, 'gtk_widget_translate_coordinates', i, p, p, i, i, p, p)(keyboard_entry, window, 0, 0, c.byref(kx), c.byref(ky))
print(json.dumps(dict(hwnd=xid, entry=[ex.value, ey.value, height], header=[hx.value,hy.value,hh], scroll=[sx.value+80,sy.value+60], keyboard=[kx.value+30,ky.value+16])), flush=True)
if '--trace-input' in __import__('sys').argv:
    class Cookie(c.Structure):
        _fields_ = [('kind', i), ('serial', c.c_ulong), ('send', i), ('display', p),
                    ('extension', i), ('evtype', i), ('cookie', c.c_uint), ('data', p)]
    class Device(c.Structure):
        _fields_ = [('kind', i), ('serial', c.c_ulong), ('send', i), ('display', p),
                    ('extension', i), ('evtype', i), ('time', c.c_ulong), ('device', i),
                    ('source', i), ('detail', i), ('root', c.c_ulong), ('event', c.c_ulong),
                    ('child', c.c_ulong), ('root_x', c.c_double), ('root_y', c.c_double),
                    ('x', c.c_double), ('y', c.c_double)]
    filter_callback = c.CFUNCTYPE(i, p, p, p)
    @filter_callback
    def raw_event(raw, event, data):
        cookie = c.cast(raw, c.POINTER(Cookie)).contents
        if cookie.kind == 35 and cookie.data and cookie.evtype in (4, 5, 6, 7, 8):
            device = c.cast(cookie.data, c.POINTER(Device)).contents
            print('XI', cookie.evtype, cookie.serial, device.serial, hex(device.event),
                  device.detail, device.x, device.y, flush=True)
        return 0
    bind(gdk, 'gdk_window_add_filter', None, p, p, p)(None, raw_event, None)
    event_callback = c.CFUNCTYPE(None, p, p)
    @event_callback
    def dispatch(event, data):
        kind = bind(gdk,'gdk_event_get_event_type',i,p)(event)
        if kind in (3,4,5,6,7,31):
            xval,yval=c.c_double(),c.c_double(); state=c.c_uint()
            bind(gdk,'gdk_event_get_coords',i,p,p,p)(event,c.byref(xval),c.byref(yval))
            bind(gdk,'gdk_event_get_state',i,p,p)(event,c.byref(state))
            print('INPUT',kind,xval.value,yval.value,hex(state.value),flush=True)
        bind(gtk,'gtk_main_do_event',None,p)(event)
    bind(gdk,'gdk_event_handler_set',None,p,p,p)(dispatch,None,None)
selection = []
context_menus = []
menu_actions = []
menu_callbacks = []
popup_callback = c.CFUNCTYPE(None, p, p, p)
@popup_callback
def populate_popup(widget, menu, data):
    context_menus.append(True)
    item = bind(gtk, 'gtk_menu_item_new_with_label', p, c.c_char_p)(b'Coordinate probe action')
    bind(gtk, 'gtk_menu_shell_append', None, p, p)(menu, item)
    bind(gtk, 'gtk_widget_show', None, p)(item)
    action_callback = c.CFUNCTYPE(None, p, p)
    @action_callback
    def activate(widget, data):
        menu_actions.append(True)
    reported = []
    @draw_callback
    def menu_draw(widget, context, data):
        if reported: return 0
        top = bind(gtk, 'gtk_widget_get_toplevel', p, p)(widget)
        native = bind(gtk, 'gtk_widget_get_window', p, p)(top)
        if not native: return 0
        mx, my = i(), i()
        if not bind(gtk, 'gtk_widget_translate_coordinates', i, p, p, i, i, p, p)(item, top, 0, 0, c.byref(mx), c.byref(my)):
            return 0
        width = bind(gtk, 'gtk_widget_get_allocated_width', i, p)(item)
        height = bind(gtk, 'gtk_widget_get_allocated_height', i, p)(item)
        if width <= 1 or height <= 1: return 0
        reported.append(True)
        hwnd = bind(gdk, 'gdk_x11_window_get_xid', c.c_ulong, p)(native)
        print(json.dumps(dict(menu=hwnd, action=[mx.value+width//2, my.value+height//2])), flush=True)
        return 0
    menu_callbacks.extend((activate, menu_draw))
    connect = bind(objects, 'g_signal_connect_data', c.c_ulong, p, c.c_char_p, p, p, p, i)
    connect(item, b'activate', activate, None, None, 0)
    connect(menu, b'draw', menu_draw, None, None, 0)
bind(objects, 'g_signal_connect_data', c.c_ulong, p, c.c_char_p, p, p, p, i)(entry, b'populate-popup', populate_popup, None, None, 0)
scroll_values = []
keyboard_values = []
callback = c.CFUNCTYPE(i, p, p, p)
@callback
def close(widget, event, data):
    start, end = i(), i()
    selected = bind(gtk, 'gtk_editable_get_selection_bounds', i, p, p, p)(entry, c.byref(start), c.byref(end))
    selection.append((selected, start.value, end.value))
    scroll_values.append((bind(gtk, 'gtk_adjustment_get_value', c.c_double, p)(adjustment),
                          bind(gtk, 'gtk_adjustment_get_value', c.c_double, p)(horizontal)))
    keyboard_values.append(bind(gtk, 'gtk_entry_get_text', c.c_char_p, p)(keyboard_entry))
    return 0
bind(objects, 'g_signal_connect_data', c.c_ulong, p, c.c_char_p, p, p, p, i)(window, b'delete-event', close, None, None, 0)
run_loop(lambda quiet: bool(selection))
assert selection and selection[0][0] and selection[0][2] - selection[0][1] >= 8, selection
assert all(value > 0 for value in scroll_values[0]), ('Wheel did not scroll GTK content', scroll_values)
assert keyboard_values == [b'Aa&|<>!_'], keyboard_values
assert context_menus, 'Right click did not open the GTK context menu'
assert menu_actions, 'The visible menu action did not receive its click'
print('GTK_POPUP_ANCHOR_AND_ACTION_OK', flush=True)
print('GTK_CONTEXT_MENU_OK', flush=True)
print('GTK_WHEEL_SCROLL_OK', scroll_values[0], flush=True)
print('GTK_KEYBOARD_MODIFIERS_OK', keyboard_values, flush=True)
print('GTK_CSD_NATIVE_DRAG_TEXT_SELECTION_OK', selection, flush=True)
