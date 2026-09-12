"""Read a live redirected surface through the real libXcomposite client."""
import ctypes as c

lib = c.CDLL('libX11.so.6')
comp = c.CDLL('libXcomposite.so.1')
def bind(lib, name, result, *args):
    fn = getattr(lib, name); fn.restype, fn.argtypes = result, args; return fn
p, u, i = c.c_void_p, c.c_ulong, c.c_int
open_display = bind(lib, 'XOpenDisplay', p, c.c_char_p)
close = bind(lib, 'XCloseDisplay', i, p)
create = bind(lib, 'XCreateSimpleWindow', u, p, u, i, i, c.c_uint, c.c_uint, c.c_uint, u, u)
destroy = bind(lib, 'XDestroyWindow', i, p, u)
map_window = bind(lib, 'XMapWindow', i, p, u)
sync = bind(lib, 'XSync', i, p, i)
gc_new = bind(lib, 'XCreateGC', p, p, u, u, p)
gc_free = bind(lib, 'XFreeGC', i, p, p)
foreground = bind(lib, 'XSetForeground', i, p, p, u)
fill = bind(lib, 'XFillRectangle', i, p, u, p, i, i, c.c_uint, c.c_uint)
image = bind(lib, 'XGetImage', p, p, u, i, i, c.c_uint, c.c_uint, u, i)
pixel = bind(lib, 'XGetPixel', u, p, i, i)
image_free = bind(lib, 'XDestroyImage', i, p)
pixmap_free = bind(lib, 'XFreePixmap', i, p, u)
version = bind(comp, 'XCompositeQueryVersion', i, p, c.POINTER(i), c.POINTER(i))
redirect = bind(comp, 'XCompositeRedirectWindow', None, p, u, i)
unredirect = bind(comp, 'XCompositeUnredirectWindow', None, p, u, i)
name = bind(comp, 'XCompositeNameWindowPixmap', u, p, u)
geometry = bind(lib, 'XGetGeometry', i, p, u, p, p, p, p, p, p, p)
d = open_display(None)
w = g = a = b = 0
try:
    assert d
    major, minor = i(), i()
    assert version(d, c.byref(major), c.byref(minor))
    assert (major.value, minor.value) >= (0, 2)
    w = create(d, 1, 0, 0, 32, 24, 0, 0, 0)
    assert w
    map_window(d, w)
    redirect(d, w, 1)
    sync(d, 0)
    root, xpos, ypos = u(), i(), i()
    width, height, border, depth = (c.c_uint() for _ in range(4))
    assert geometry(d, w, c.byref(root), c.byref(xpos), c.byref(ypos),
                    c.byref(width), c.byref(height), c.byref(border), c.byref(depth))
    window_dimensions = (width.value, height.value, depth.value)
    a = name(d, w)
    b = name(d, w)
    # A synchronous core query must execute the pending extension request;
    # callers must not need an extra XSync after naming a window pixmap.
    assert geometry(d, b, c.byref(root), c.byref(xpos), c.byref(ypos),
                    c.byref(width), c.byref(height), c.byref(border), c.byref(depth))
    assert (width.value, height.value, depth.value) == window_dimensions
    assert a and b and a != b
    g = gc_new(d, w, 0, None)
    for color in (0xff0000, 0x00ff00):
        foreground(d, g, color)
        fill(d, w, g, 0, 0, 32, 24)
        sync(d, 0)
        for drawable in (a, b):
            sample = image(d, drawable, 0, 0, 1, 1, 0xffffffff, 2)
            assert sample
            try:
                assert pixel(sample, 0, 0) & 0xffffff == color
            finally:
                image_free(sample)
    unredirect(d, w, 1)
    sync(d, 0)
    destroy(d, w)
    w = 0
    sample = image(d, a, 0, 0, 1, 1, 0xffffffff, 2)
    assert sample
    assert pixel(sample, 0, 0) & 0xffffff == 0x00ff00
    image_free(sample)
    print('COMPOSITE_SHARED_PIXMAP_LIFETIME_OK', flush=True)
finally:
    if g: gc_free(d, g)
    if a: pixmap_free(d, a)
    if b: pixmap_free(d, b)
    if w: destroy(d, w)
    if d: close(d)
