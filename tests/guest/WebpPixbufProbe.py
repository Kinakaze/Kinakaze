"""Decode real WebP files and incremental streams through GdkPixbuf's loader."""
import ctypes as c
from pathlib import Path
import tempfile

p, i, u = c.c_void_p, c.c_int, c.c_uint
pixbuf = c.CDLL('libgdk_pixbuf-2.0.so.0')
webp = c.CDLL('libwebp.so.7')
objects = c.CDLL('libgobject-2.0.so.0')
glib = c.CDLL('libglib-2.0.so.0')

def bind(lib, name, result, *args):
    function = getattr(lib, name)
    function.restype, function.argtypes = result, args
    return function

unref = bind(objects, 'g_object_unref', None, p)
free_error = bind(glib, 'g_error_free', None, p)
width = bind(pixbuf, 'gdk_pixbuf_get_width', i, p)
height = bind(pixbuf, 'gdk_pixbuf_get_height', i, p)
channels = bind(pixbuf, 'gdk_pixbuf_get_n_channels', i, p)
stride = bind(pixbuf, 'gdk_pixbuf_get_rowstride', i, p)
pixels = bind(pixbuf, 'gdk_pixbuf_get_pixels', p, p)
load = bind(pixbuf, 'gdk_pixbuf_new_from_file', p, c.c_char_p, p)

def packed(image):
    address, row = pixels(image), width(image) * channels(image)
    return b''.join(c.string_at(address + y * stride(image), row) for y in range(height(image)))

# Nonzero alpha retains meaningful RGB in every pixel, including translucent ones.
rgba = bytes((220, 40, 20, 255, 20, 190, 40, 128, 10, 40, 240, 64, 90, 80, 70, 255))
encoded = p()
size = bind(webp, 'WebPEncodeLosslessRGBA', c.c_size_t, p, i, i, i, p)(rgba, 2, 2, 8, c.byref(encoded))
assert size and encoded.value
data = c.string_at(encoded, size)
bind(webp, 'WebPFree', None, p)(encoded)
with tempfile.TemporaryDirectory(prefix='kinakaze-webp-') as directory:
    path = Path(directory) / 'alpha.webp'
    path.write_bytes(data)
    error = p()
    image = load(str(path).encode(), c.byref(error))
    assert image and not error.value, 'WebP loader failed file decoding'
    try:
        assert (width(image), height(image), channels(image)) == (2, 2, 4)
        assert packed(image) == rgba, packed(image)
    finally:
        unref(image)

    loader = bind(pixbuf, 'gdk_pixbuf_loader_new', p)()
    write = bind(pixbuf, 'gdk_pixbuf_loader_write', i, p, p, c.c_size_t, p)
    close = bind(pixbuf, 'gdk_pixbuf_loader_close', i, p, p)
    try:
        for offset in range(0, len(data), 7):
            chunk = data[offset:offset + 7]
            assert write(loader, chunk, len(chunk), c.byref(error)) and not error.value
        assert close(loader, c.byref(error)) and not error.value
        image = bind(pixbuf, 'gdk_pixbuf_loader_get_pixbuf', p, p)(loader)
        assert image and packed(image) == rgba
    finally:
        unref(loader)

    # Lossy opaque WebP is decoded to the same RGB as libwebp's reference path.
    size = bind(webp, 'WebPEncodeRGB', c.c_size_t, p, i, i, i, c.c_float, p)(bytes((60, 120, 180)) * 64, 8, 8, 24, 80, c.byref(encoded))
    assert size and encoded.value
    data = c.string_at(encoded, size)
    bind(webp, 'WebPFree', None, p)(encoded)
    path.write_bytes(data)
    image = load(str(path).encode(), c.byref(error))
    reference_width, reference_height = i(), i()
    reference = bind(webp, 'WebPDecodeRGB', p, p, c.c_size_t, p, p)(data, len(data), c.byref(reference_width), c.byref(reference_height))
    assert image and reference and not error.value
    try:
        assert (width(image), height(image)) == (8, 8)
        decoded = packed(image)
        if channels(image) == 4:
            assert decoded[3::4] == bytes([255]) * 64
            decoded = b''.join(decoded[n:n + 3] for n in range(0, len(decoded), 4))
        assert decoded == c.string_at(reference, 8 * 8 * 3)
    finally:
        unref(image)
        bind(webp, 'WebPFree', None, p)(reference)

    path.write_bytes(data[:20])
    image = load(str(path).encode(), c.byref(error))
    assert not image and error.value, 'Truncated WebP must fail with GError'
    free_error(error)

wallpaper = b'/usr/share/backgrounds/gnome/adwaita-l.webp'
error = p()
image = load(wallpaper, c.byref(error))
assert image and not error.value, 'GNOME default wallpaper failed decoding'
try:
    assert width(image) >= 1920 and height(image) >= 1080
    print('WEBP_PIXBUF_FILE_STREAM_ALPHA_LOSSY_WALLPAPER_OK', width(image), height(image), flush=True)
finally:
    unref(image)
