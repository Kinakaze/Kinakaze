"""Check Render constant fills/clipping against actual pixels and time full frames."""
import ctypes as c
import json
import time

x, r = c.CDLL('libX11.so.6'), c.CDLL('libXrender.so.1')
p, u, i = c.c_void_p, c.c_ulong, c.c_int
def bind(lib, name, result, *args):
    f = getattr(lib, name); f.restype, f.argtypes = result, args; return f
class Color(c.Structure):
    _fields_ = [(n, c.c_ushort) for n in ('red', 'green', 'blue', 'alpha')]
class Rect(c.Structure):
    _fields_ = [('x', c.c_short), ('y', c.c_short), ('width', c.c_ushort), ('height', c.c_ushort)]
class Attributes(c.Structure):
    _fields_ = [('repeat', i), ('alpha_map', u), ('alpha_x_origin', i), ('alpha_y_origin', i),
                ('clip_x_origin', i), ('clip_y_origin', i), ('clip_mask', u),
                ('graphics_exposures', i), ('subwindow_mode', i), ('poly_edge', i),
                ('poly_mode', i), ('dither', u), ('component_alpha', i)]
d = bind(x, 'XOpenDisplay', p, c.c_char_p)(None)
pixmap = bind(x, 'XCreatePixmap', u, p, u, c.c_uint, c.c_uint, c.c_uint)(d, 1, 1024, 768, 32)
fmt = bind(r, 'XRenderFindStandardFormat', p, p, i)(d, 0)
picture = bind(r, 'XRenderCreatePicture', u, p, u, p, u, p)(d, pixmap, fmt, 0, None)
solid = bind(r, 'XRenderCreateSolidFill', u, p, p)
opaque = solid(d, c.byref(Color(65535, 0, 0, 65535)))
alpha = solid(d, c.byref(Color(65535, 0, 0, 32768)))
transparent = solid(d, c.byref(Color(0, 0, 0, 0)))
composite = bind(r, 'XRenderComposite', None, p, i, u, u, u, i, i, i, i, i, i, c.c_uint, c.c_uint)
clip = bind(r, 'XRenderSetPictureClipRectangles', None, p, u, i, i, p, i)
def fill(op, source):
    composite(d, op, source, 0, picture, 0, 0, 0, 0, 0, 0, 1024, 768)
def sample(px, py):
    image = bind(x, 'XGetImage', p, p, u, i, i, c.c_uint, c.c_uint, u, i)(d, pixmap, px, py, 1, 1, 0xffffffff, 2)
    assert image
    value = bind(x, 'XGetPixel', u, p, i, i)(image, 0, 0)
    bind(x, 'XDestroyImage', i, p)(image)
    return value
try:
    fill(1, alpha)
    assert sample(100, 100) == 0x80800000
    fill(3, transparent)
    assert sample(100, 100) == 0x80800000
    rectangles = (Rect * 2)(Rect(2, 2, 8, 8), Rect(6, 6, 8, 8))
    clip(d, picture, 0, 0, rectangles, 2)
    fill(3, opaque)
    assert sample(7, 7) == 0xffff0000 and sample(20, 20) == 0x80800000
    fill(0, opaque)
    assert sample(7, 7) == 0 and sample(20, 20) == 0x80800000
    change = bind(r, 'XRenderChangePicture', None, p, u, u, p)
    # Cairo cancels a rectangle clip with CPClipMask=None between widgets.
    change(d, picture, 1 << 6, c.byref(Attributes()))
    fill(0, opaque)
    assert sample(7, 7) == sample(20, 20) == 0, 'stale rectangle clip after CPClipMask=None'
    fill(1, alpha)
    rectangle = Rect(0, 0, 2, 2)
    clip(d, picture, 10, 20, c.byref(rectangle), 1)
    change(d, picture, (1 << 4) | (1 << 5), c.byref(Attributes(clip_x_origin=30, clip_y_origin=40)))
    fill(3, opaque)
    assert sample(30, 40) == 0xffff0000 and sample(10, 20) == 0x80800000, 'clip origin did not move'
    rectangles = (Rect * 1)(Rect(0, 0, 1024, 768))
    clip(d, picture, 0, 0, rectangles, 1)
    begin = time.perf_counter()
    for _ in range(20): fill(3, opaque)
    elapsed = time.perf_counter() - begin
    assert sample(1000, 700) == 0xffff0000
    print('RENDER_FILL_PIXELS_CLIP_OK', json.dumps(dict(frames=20, milliseconds=elapsed*1000)), flush=True)
finally:
    for value in (picture, opaque, alpha, transparent): bind(r, 'XRenderFreePicture', None, p, u)(d, value)
    bind(x, 'XFreePixmap', i, p, u)(d, pixmap)
    bind(x, 'XCloseDisplay', i, p)(d)
