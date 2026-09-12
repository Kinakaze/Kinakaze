"""Composite overlay pixels, root coordinates and pointer queries must coincide."""
import ctypes as c

x = c.CDLL('libX11.so.6')
comp = c.CDLL('libXcomposite.so.1')
p, u, i = c.c_void_p, c.c_ulong, c.c_int
def bind(lib, name, result, *args):
    f = getattr(lib, name); f.restype, f.argtypes = result, args; return f

d = bind(x, 'XOpenDisplay', p, c.c_char_p)(None)
overlay = bind(comp, 'XCompositeGetOverlayWindow', u, p, u)(d, 1)
assert overlay
translate = bind(x, 'XTranslateCoordinates', i, p, u, u, i, i, p, p, p)
geometry = bind(x, 'XGetGeometry', i, p, u, p, p, p, p, p, p, p)
query = bind(x, 'XQueryPointer', i, p, u, p, p, p, p, p, p, p)
try:
    bind(x, 'XSync', i, p, i)(d, 0)
    def dimensions(window):
        root, xpos, ypos = u(), i(), i()
        width, height, border, depth = c.c_uint(), c.c_uint(), c.c_uint(), c.c_uint()
        assert geometry(d, window, c.byref(root), c.byref(xpos), c.byref(ypos),
                        c.byref(width), c.byref(height), c.byref(border), c.byref(depth))
        return width.value, height.value
    width, height = dimensions(1)
    assert dimensions(overlay) == (width, height), (dimensions(overlay), (width, height))
    for source, dest in ((overlay, 1), (1, overlay)):
        for px, py in ((0, 0), (8, 31), (width // 2, height // 2), (width - 1, height - 1)):
            ox, oy, child = i(), i(), u()
            assert translate(d, source, dest, px, py, c.byref(ox), c.byref(oy), c.byref(child))
            assert (ox.value, oy.value) == (px, py), (source, dest, px, py, ox.value, oy.value)
    root, child, rx, ry, wx, wy, state = u(), u(), i(), i(), i(), i(), c.c_uint()
    assert query(d, overlay, c.byref(root), c.byref(child), c.byref(rx), c.byref(ry),
                 c.byref(wx), c.byref(wy), c.byref(state))
    assert (rx.value, ry.value) == (wx.value, wy.value)
    print('COMPOSITE_BORDERLESS_ROOT_POINTER_COORDINATES_OK', width, height, flush=True)
finally:
    bind(comp, 'XCompositeReleaseOverlayWindow', None, p, u)(d, 1)
    bind(x, 'XCloseDisplay', i, p)(d)
