"""Native monochrome cursor snapshots must retain their XOR-only strokes."""
import ctypes as c

x = c.CDLL('libXcursor.so.1')
class Image(c.Structure):
    _fields_ = [('version', c.c_uint), ('size', c.c_uint), ('width', c.c_uint),
                ('height', c.c_uint), ('xhot', c.c_uint), ('yhot', c.c_uint),
                ('delay', c.c_uint), ('pixels', c.POINTER(c.c_uint))]
x.XcursorLibraryLoadImage.argtypes = [c.c_char_p, c.c_char_p, c.c_int]
x.XcursorLibraryLoadImage.restype = c.POINTER(Image)
x.XcursorImageDestroy.argtypes = [c.POINTER(Image)]
for name in (b'xterm', b'text', b'crosshair', b'left_ptr'):
    image = x.XcursorLibraryLoadImage(name, None, 32)
    assert image, name
    try:
        value = image.contents
        pixels = value.pixels[:value.width * value.height]
        assert sum(pixel >> 24 != 0 for pixel in pixels) > 1, name
        assert value.xhot < value.width and value.yhot < value.height
    finally:
        x.XcursorImageDestroy(image)
print('CURSOR_CONTRAST_OK', flush=True)
