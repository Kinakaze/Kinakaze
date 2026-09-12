"""Transient ownership must not change X11 parent coordinates or event routing."""
import ctypes as c

x = c.CDLL('libX11.so.6')
p, i, u, w = c.c_void_p, c.c_int, c.c_uint, c.c_ulong
def bind(name, result, *args):
    fn = getattr(x, name); fn.restype, fn.argtypes = result, args; return fn
display = bind('XOpenDisplay', p, c.c_char_p)(None)
assert display
create = bind('XCreateSimpleWindow', w, p, w, i, i, u, u, u, w, w)
owner = create(display, 1, 280, 180, 320, 240, 0, 0, 0)
dialog = create(display, 1, 730, 490, 220, 160, 0, 0, 0)
child = create(display, dialog, 17, 23, 80, 60, 0, 0, 0)
assert owner and dialog and child
assert bind('XSetTransientForHint', i, p, w, w)(display, dialog, owner)
geometry = bind('XGetGeometry', i, p, w, p, p, p, p, p, p, p)
translate = bind('XTranslateCoordinates', i, p, w, w, i, i, p, p, p)
tree = bind('XQueryTree', i, p, w, p, p, p, p)
try:
    for window, expected in ((owner, (280, 180)), (dialog, (730, 490)), (child, (17, 23))):
        root, px, py, width, height, border, depth = w(), i(), i(), u(), u(), u(), u()
        assert geometry(display, window, *map(c.byref, (root, px, py, width, height, border, depth)))
        assert (px.value, py.value) == expected, (window, (px.value, py.value), expected)
    rx, ry, beneath = i(), i(), w()
    assert translate(display, dialog, 1, 0, 0, c.byref(rx), c.byref(ry), c.byref(beneath))
    assert (rx.value, ry.value) == (730, 490)
    root, parent, children, count = w(), w(), p(), u()
    assert tree(display, dialog, *map(c.byref, (root, parent, children, count)))
    assert parent.value == 1
    bind('XFree', i, p)(children)
    class Attributes(c.Structure):
        _fields_ = [('background_pixmap', w), ('background_pixel', w), ('border_pixmap', w),
                    ('border_pixel', w), ('bit_gravity', i), ('win_gravity', i), ('backing_store', i),
                    ('backing_planes', w), ('backing_pixel', w), ('save_under', i),
                    ('event_mask', c.c_long), ('do_not_propagate_mask', c.c_long),
                    ('override_redirect', i), ('colormap', w), ('cursor', w)]
    attributes = Attributes(); attributes.override_redirect = 1
    bind('XChangeWindowAttributes', i, p, w, w, p)(display, dialog, 1 << 9, c.byref(attributes))
    bind('XMoveWindow', i, p, w, i, i)(display, dialog, 730, 490)
    bind('XMapWindow', i, p, w)(display, dialog)
    # GTK popups carry a transient owner but must retain their pointer anchor.
    assert translate(display, dialog, 1, 0, 0, c.byref(rx), c.byref(ry), c.byref(beneath))
    assert (rx.value, ry.value) == (730, 490), ('popup anchor', rx.value, ry.value)
finally:
    destroy = bind('XDestroyWindow', i, p, w)
    destroy(display, child); destroy(display, dialog); destroy(display, owner)
    bind('XCloseDisplay', i, p)(display)
print('TRANSIENT_ROOT_AND_CHILD_COORDINATES_OK', flush=True)
