"""Core clipping changes actual pixels and retains copied region/bitmap data."""
import ctypes as c

x = c.CDLL('libX11.so.6')
p, u, i = c.c_void_p, c.c_ulong, c.c_int
def bind(name, result, *args):
    f = getattr(x, name); f.restype, f.argtypes = result, args; return f

class Rect(c.Structure):
    _fields_ = [('x', c.c_short), ('y', c.c_short), ('width', c.c_ushort), ('height', c.c_ushort)]

d = bind('XOpenDisplay', p, c.c_char_p)(None)
assert d
pixmap = bind('XCreatePixmap', u, p, u, c.c_uint, c.c_uint, c.c_uint)(d, 1, 8, 8, 24)
gc = bind('XCreateGC', p, p, u, u, p)(d, pixmap, 0, None)
foreground = bind('XSetForeground', i, p, p, u)
fill = bind('XFillRectangle', i, p, u, p, i, i, c.c_uint, c.c_uint)
get_image = bind('XGetImage', p, p, u, i, i, c.c_uint, c.c_uint, u, i)
get_pixel = bind('XGetPixel', u, p, i, i)
free_image = bind('XDestroyImage', i, p)
mask = bind('XSetClipMask', i, p, p, u)
rectangles = bind('XSetClipRectangles', i, p, p, i, i, p, i, i)
def check(allowed, color):
    sample = get_image(d, pixmap, 0, 0, 8, 8, 0xffffffff, 2)
    assert sample
    try:
        for y in range(8):
            for x in range(8):
                actual = get_pixel(sample, x, y) & 0xffffff
                expected = color if allowed(x, y) else 0
                assert actual == expected, (x, y, hex(actual), hex(expected))
    finally:
        free_image(sample)
def clear():
    mask(d, gc, 0)
    foreground(d, gc, 0)
    fill(d, pixmap, gc, 0, 0, 8, 8)
try:
    clear()
    region = bind('XCreateRegion', p)()
    rect = Rect(1, 1, 2, 3)
    bind('XUnionRectWithRegion', i, p, p, p)(c.byref(rect), region, region)
    bind('XSetRegion', i, p, p, p)(d, gc, region)
    bind('XDestroyRegion', i, p)(region)
    bind('XSetClipOrigin', i, p, p, i, i)(d, gc, 2, 1)
    foreground(d, gc, 0x123456)
    fill(d, pixmap, gc, 0, 0, 8, 8)
    check(lambda x, y: 3 <= x < 5 and 2 <= y < 5, 0x123456)
    clear()
    rectangles(d, gc, 0, 0, None, 0, 0)
    foreground(d, gc, 0xffffff)
    fill(d, pixmap, gc, 0, 0, 8, 8)
    bind('XDrawPoint', i, p, u, p, i, i)(d, pixmap, gc, 1, 1)
    check(lambda x, y: False, 0)
    clear()
    bits = c.create_string_buffer(bytes([0b00000101, 0b00001010]))
    bitmap = bind('XCreateBitmapFromData', u, p, u, p, c.c_uint, c.c_uint)(d, pixmap, bits, 4, 2)
    mask(d, gc, bitmap)
    bind('XFreePixmap', i, p, u)(d, bitmap)
    bind('XSetClipOrigin', i, p, p, i, i)(d, gc, 0, 0)
    foreground(d, gc, 0x00ff00)
    fill(d, pixmap, gc, 0, 0, 8, 8)
    check(lambda x, y: (x, y) in ((0, 0), (2, 0), (1, 1), (3, 1)), 0x00ff00)
    print('XREGION_RECTANGLE_BITMAP_CLIPPING_OK')
finally:
    bind('XFreeGC', i, p, p)(d, gc)
    bind('XFreePixmap', i, p, u)(d, pixmap)
    bind('XCloseDisplay', i, p)(d)
