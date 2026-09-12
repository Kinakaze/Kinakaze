"""UTF-8 property decoding is independent of LC_CTYPE and owns its results."""
import ctypes as C
import locale

x = C.CDLL('libX11.so.6')
p, i, u = C.c_void_p, C.c_int, C.c_ulong
class Property(C.Structure):
    _fields_ = [('value', p), ('encoding', u), ('format', i), ('nitems', u)]

x.XOpenDisplay.argtypes = [p]
x.XOpenDisplay.restype = p
x.XInternAtom.argtypes = [p, C.c_char_p, i]
x.XInternAtom.restype = u
x.Xutf8TextPropertyToTextList.argtypes = [p, C.POINTER(Property), C.POINTER(C.POINTER(C.c_char_p)), C.POINTER(i)]
x.XFreeStringList.argtypes = [C.POINTER(C.c_char_p)]
x.XCloseDisplay.argtypes = [p]
display = x.XOpenDisplay(None)
assert display
locale.setlocale(locale.LC_CTYPE, 'C')
utf8 = x.XInternAtom(display, b'UTF8_STRING', 0)
try:
    for encoding, data, expected in [
        (utf8, '中文\0café'.encode(), ['中文', 'café']),
        (31, b'caf\xe9\0second', ['café', 'second']),
        (utf8, b'', []),
    ]:
        storage = C.create_string_buffer(data)
        prop = Property(C.addressof(storage), encoding, 8, len(data))
        output, count = C.POINTER(C.c_char_p)(), i()
        assert x.Xutf8TextPropertyToTextList(display, C.byref(prop), C.byref(output), C.byref(count)) == 0
        try:
            assert [output[n].decode() for n in range(count.value)] == expected
            storage.value = b''
            assert [output[n].decode() for n in range(count.value)] == expected, 'borrowed input buffer'
        finally:
            x.XFreeStringList(output)
    invalid = C.create_string_buffer(b'\xff')
    prop = Property(C.addressof(invalid), utf8, 8, 1)
    output, count = C.POINTER(C.c_char_p)(), i(123)
    assert x.Xutf8TextPropertyToTextList(display, C.byref(prop), C.byref(output), C.byref(count)) == -3
    assert not output and count.value == 0
finally:
    x.XCloseDisplay(display)
print('XUTF8_PROPERTY_OWNERSHIP_AND_LOCALE_OK', flush=True)
