"""Exercise the legacy GTK2/CEF interfaces from a real Linux caller."""
import ctypes as C
import math
import os
import signal

P, I, U, S = C.c_void_p, C.c_int, C.c_ulong, C.c_size_t
def bind(lib, name, result, *arguments):
    function = getattr(lib, name)
    function.restype, function.argtypes = result, arguments
    return function

libc = C.CDLL('libc.so.6', use_errno=True)
free = bind(libc, '__libc_free', None, P)
calloc = bind(libc, '__libc_calloc', P, S, S)
realloc = bind(libc, '__libc_realloc', P, P, S)
align = bind(libc, '__libc_memalign', P, S, S)
memory = calloc(17, 3)
assert memory and C.string_at(memory, 51) == bytes(51)
C.memmove(memory, b'preserve', 8)
memory = realloc(memory, 4096)
assert C.string_at(memory, 8) == b'preserve'
free(memory)
memory = align(4096, 5000)
assert memory and memory % 4096 == 0
C.memset(memory, 0x5a, 5000)
free(memory)
assert not calloc(2**63, 3), 'overflow must fail'
fcvt = bind(libc, 'fcvt', C.c_char_p, C.c_double, I, C.POINTER(I), C.POINTER(I))
for value, places, expected, point, negative in [
    (12.375, 2, b'1238', 2, 0), (-0.00125, 5, b'125', -2, 1),
    (123.0, -1, b'120', 3, 0), (-0.0, 2, b'000', 1, 1),
    (math.inf, 3, b'inf', 0, 0),
]:
    decimal, sign = I(), I()
    assert (fcvt(value, places, C.byref(decimal), C.byref(sign)), decimal.value, sign.value) == (expected, point, negative)

# Linux wchar_t is 32 bits. The va_list uses the SysV register save area.
class VaList(C.Structure):
    _fields_ = [('gp', C.c_uint), ('fp', C.c_uint), ('stack', P), ('registers', P)]
fmt = (I * 3)(ord('%'), ord('d'), 0)
registers = (C.c_ulonglong * 24)()
registers[0] = 42
arguments = VaList(0, 48, C.addressof(registers), C.addressof(registers))
buffer = (I * 8)()
checked = bind(libc, '__vswprintf_chk', I, P, S, I, S, P, P)
assert checked(buffer, 8, 1, 8, fmt, C.byref(arguments)) == 2
assert list(buffer)[:3] == [52, 50, 0]
child = os.fork()
if child == 0:
    checked(buffer, 9, 1, 8, fmt, C.byref(arguments))
    os._exit(91)
_, status = os.waitpid(child, 0)
assert os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGABRT, status

x = C.CDLL('libX11.so.6')
xi = C.CDLL('libXi.so.6')
d = bind(x, 'XOpenDisplay', P, C.c_char_p)(None)
assert d
xfree = bind(x, 'XFree', I, P)
class Device(C.Structure):
    _fields_ = [('id', U), ('kind', U), ('name', C.c_char_p), ('classes', I), ('use', I), ('info', P)]
count = I(-1)
devices = bind(xi, 'XListInputDevices', C.POINTER(Device), P, C.POINTER(I))(d, C.byref(count))
assert count.value == 2 and [(devices[n].id, devices[n].use) for n in range(2)] == [(2, 0), (3, 1)]
assert devices[0].name and devices[1].name
bind(xi, 'XFreeDeviceList', None, P)(devices)
device = bind(xi, 'XOpenDevice', P, P, U)(d, 2)
assert device
atom = bind(x, 'XInternAtom', U, P, C.c_char_p, I)(d, b'Device Enabled', 0)
kind, form, count, after, data = U(), I(), U(), U(), P()
assert bind(xi, 'XGetDeviceProperty', I, P, P, U, C.c_long, C.c_long, I, U, P, P, P, P, P)(
    d, device, atom, 0, 1, 0, 0, C.byref(kind), C.byref(form), C.byref(count), C.byref(after), C.byref(data)) == 0
assert (kind.value, form.value, count.value, after.value, C.string_at(data, 1)) == (19, 8, 1, 0, b'\1')
xfree(data)
assert bind(xi, 'XGrabDevice', I, P, P, U, I, I, P, I, I, U)(d, device, 1, 0, 0, None, 1, 1, 0) == 0
assert bind(xi, 'XUngrabDevice', I, P, P, U)(d, device, 0) == 0
assert bind(xi, 'XCloseDevice', I, P, P)(d, device) == 0
assert bind(xi, 'XSelectExtensionEvent', I, P, U, P, I)(d, 1, None, 0) == 0

class Property(C.Structure):
    _fields_ = [('value', P), ('encoding', U), ('format', I), ('nitems', U)]
words = [(I * (len(s) + 1))(*map(ord, s), 0) for s in ['中文🙂', 'second']]
pointers = (P * 2)(*[C.addressof(word) for word in words])
prop = Property()
assert bind(x, 'XwcTextListToTextProperty', I, P, P, I, I, P)(d, pointers, 2, 4, C.byref(prop)) == 0
output, count = P(), I()
assert bind(x, 'XwcTextPropertyToTextList', I, P, P, P, P)(d, C.byref(prop), C.byref(output), C.byref(count)) == 0
assert count.value == 2
returned = C.cast(output, C.POINTER(C.POINTER(I)))
assert [returned[0][i] for i in range(4)] == [*map(ord, '中文🙂'), 0]
bind(x, 'XwcFreeStringList', None, P)(output)
xfree(prop.value)

windows = (U * 2)(1, 1)
assert bind(x, 'XSetWMColormapWindows', I, P, U, P, I)(d, 1, windows, 2) == 1
output, count = P(), I()
assert bind(x, 'XGetWMColormapWindows', I, P, U, P, P)(d, 1, C.byref(output), C.byref(count)) == 1
assert count.value == 2 and list(C.cast(output, C.POINTER(U * 2)).contents) == [1, 1]
xfree(output)
colormaps = bind(x, 'XInternAtom', U, P, C.c_char_p, I)(d, b'WM_COLORMAP_WINDOWS', 0)
kind, form, count, after, data = U(), I(), U(), U(), P()
get_property = bind(x, 'XGetWindowProperty', I, P, U, U, C.c_long, C.c_long, I, U, P, P, P, P, P)
# Xlib marshals the low CARD32 bits; Chromium uses -1L for all data.
for offset, length, expected in [(0, -1, 2), (2**32 + 1, 2**32 + 1, 1)]:
    assert get_property(d, 1, colormaps, offset, length, 0, 33,
        C.byref(kind), C.byref(form), C.byref(count), C.byref(after), C.byref(data)) == 0
    assert (kind.value, form.value, count.value, after.value) == (33, 32, expected, 0)
    xfree(data)
assert bind(x, 'XSetWindowColormap', I, P, U, U)(d, 1, 1) == 1

class Rect(C.Structure):
    _fields_ = [('x', C.c_short), ('y', C.c_short), ('width', C.c_ushort), ('height', C.c_ushort)]
wide = (I * 3)(ord('A'), ord('B'), 0)
ink, logical = Rect(), Rect()
width = bind(x, 'XwcTextExtents', I, P, P, I, P, P)(None, wide, 2, C.byref(ink), C.byref(logical))
assert width > 0 and logical.width == width and logical.height > 0 and logical.y < 0
assert bind(x, 'XmbTextEscapement', I, P, C.c_char_p, I)(None, b'AB', 2) == width
pixmap = bind(x, 'XCreatePixmap', U, P, U, C.c_uint, C.c_uint, C.c_uint)(d, 1, 96, 40, 24)
gc = bind(x, 'XCreateGC', P, P, U, U, P)(d, pixmap, 0, None)
foreground = bind(x, 'XSetForeground', I, P, P, U)
foreground(d, gc, 0xffffff)
bind(x, 'XFillRectangle', I, P, U, P, I, I, C.c_uint, C.c_uint)(d, pixmap, gc, 0, 0, 96, 40)
foreground(d, gc, 0)
bind(x, 'XwcDrawString', None, P, U, P, P, I, I, P, I)(d, pixmap, None, gc, 3, 25, wide, 2)
image = bind(x, 'XGetImage', P, P, U, I, I, C.c_uint, C.c_uint, U, I)(d, pixmap, 0, 0, 96, 40, U(-1), 2)
assert image
pixel = bind(x, 'XGetPixel', U, P, I, I)
assert any(pixel(image, a, b) & 0xffffff != 0xffffff for a in range(3, 3 + width) for b in range(10, 26)), 'text must change retained pixels'
bind(x, 'XDestroyImage', I, P)(image)
# Chromium submits a stack XImage with explicit stride and bitmap_pad == 0.
class XImage(C.Structure):
    _fields_ = [(n, I) for n in ('width', 'height', 'xoffset', 'format')] + [
        ('data', P)] + [(n, I) for n in ('byte_order', 'bitmap_unit', 'bitmap_bit_order',
                                       'bitmap_pad', 'depth', 'bytes_per_line', 'bits_per_pixel')] + [
        ('red', U), ('green', U), ('blue', U), ('obdata', P), ('functions', P * 6)]
colors = (C.c_uint * 8)(0xff123456, 0xffabcdef, 0xfffedcba, 0xeeeeeeee,
                          0xff010203, 0xffa0b0c0, 0xff112233, 0xeeeeeeee)
packed = XImage(width=3, height=2, format=2, data=C.addressof(colors), bitmap_unit=8,
                depth=24, bytes_per_line=16, bits_per_pixel=32,
                red=0xff0000, green=0xff00, blue=0xff)
header = bytes(packed)
bind(x, 'XPutImage', I, P, U, P, P, I, I, I, I, C.c_uint, C.c_uint)(
    d, pixmap, gc, C.byref(packed), 0, 0, 0, 0, 3, 2)
assert bytes(packed) == header, 'XPutImage must not mutate a caller-built XImage'
image = bind(x, 'XGetImage', P, P, U, I, I, C.c_uint, C.c_uint, U, I)(
    d, pixmap, 0, 0, 3, 2, U(-1), 2)
assert [[pixel(image, a, b) & 0xffffff for a in range(3)] for b in range(2)] == [
    [0x123456, 0xabcdef, 0xfedcba], [0x010203, 0xa0b0c0, 0x112233]]
bind(x, 'XDestroyImage', I, P)(image)
bind(x, 'XFreeGC', I, P, P)(d, gc)
bind(x, 'XFreePixmap', I, P, U)(d, pixmap)
bind(x, 'XCloseDisplay', I, P)(d)

pulse = C.CDLL('libpulse.so.0')
assert bind(pulse, 'pa_stream_get_device_index', C.c_uint, P)(None) == 0xffffffff
print('NETEASE_STARTUP_ABI_OK', flush=True)
