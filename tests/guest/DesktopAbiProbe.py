"""Behavior checks for APIs needed by desktop applications and their plugins."""
import ctypes as c
import os
import tempfile

lib = c.CDLL('libc.so.6', use_errno=True)
lib.snprintf.argtypes = [c.c_void_p, c.c_size_t, c.c_char_p]
lib.snprintf.restype = c.c_int
def formatted(fmt, *args):
    out = c.create_string_buffer(256)
    count = lib.snprintf(out, len(out), fmt, *args)
    assert count == len(out.value), (count, out.value)
    return out.value
assert formatted(b'%2$s %1$s %2$s', b'43.1', b'Calculator') == b'Calculator 43.1 Calculator'
assert formatted(b'%3$*1$.*2$f', c.c_int(9), c.c_int(2), c.c_double(3.25)) == b'     3.25'
assert formatted(b'%2$.*1$s/%3$d', c.c_int(3), b'abcdef', c.c_int(17)) == b'abc/17'
assert formatted(b'cost $%d; %%', c.c_int(5)) == b'cost $5; %'
assert formatted(b'%8$d:%1$d:%2$d:%3$d:%4$d:%5$d:%6$d:%7$d',
                 *[c.c_int(i) for i in range(1, 9)]) == b'8:1:2:3:4:5:6:7'
assert formatted(b'%9$.1f %8$.1f %7$.1f %6$.1f %5$.1f %4$.1f %3$.1f %2$.1f %1$.1f',
                 *[c.c_double(i + .5) for i in range(9)]) == b'8.5 7.5 6.5 5.5 4.5 3.5 2.5 1.5 0.5'
lib.parse_printf_format.argtypes = [c.c_char_p, c.c_size_t, c.POINTER(c.c_int)]
lib.parse_printf_format.restype = c.c_size_t
types = (c.c_int * 3)()
assert lib.parse_printf_format(b'%3$*1$.*2$f', 3, types) == 3
assert list(types) == [0, 0, 7], list(types)
out = c.create_string_buffer(8)
assert lib.snprintf(out, 8, b'%2$d', c.c_int(1), c.c_int(2)) == -1
assert c.get_errno() == 22

class Dirent(c.Structure):
    _fields_ = [('ino', c.c_ulonglong), ('off', c.c_longlong), ('reclen', c.c_ushort),
                ('type', c.c_ubyte), ('name', c.c_char * 256)]
lib.opendir.argtypes = [c.c_char_p]; lib.opendir.restype = c.c_void_p
lib.closedir.argtypes = [c.c_void_p]
for name in ('readdir_r', 'readdir64_r'):
    read = getattr(lib, name)
    read.argtypes = [c.c_void_p, c.POINTER(Dirent), c.POINTER(c.POINTER(Dirent))]
    with tempfile.TemporaryDirectory() as path:
        open(path + '/one.txt', 'w').close()
        directory = lib.opendir(os.fsencode(path)); assert directory
        entry, result, names = Dirent(), c.POINTER(Dirent)(), []
        try:
            while True:
                c.set_errno(123)
                assert read(directory, c.byref(entry), c.byref(result)) == 0
                assert c.get_errno() == 123
                if not result: break
                assert c.addressof(result.contents) == c.addressof(entry)
                names.append(entry.name)
            assert b'one.txt' in names, names
        finally: lib.closedir(directory)
    assert read(None, c.byref(entry), c.byref(result)) == 9 and not result

p = c.CDLL('libpulse.so.0')
for name, restype, args in (
    ('pa_mainloop_new', c.c_void_p, []), ('pa_mainloop_get_api', c.c_void_p, [c.c_void_p]),
    ('pa_context_new', c.c_void_p, [c.c_void_p, c.c_char_p]),
    ('pa_stream_new', c.c_void_p, [c.c_void_p, c.c_char_p, c.c_void_p, c.c_void_p]),
    ('pa_stream_begin_write', c.c_int, [c.c_void_p, c.POINTER(c.c_void_p), c.POINTER(c.c_size_t)]),
    ('pa_stream_cancel_write', c.c_int, [c.c_void_p]),
    ('pa_stream_unref', None, [c.c_void_p]), ('pa_context_unref', None, [c.c_void_p]),
    ('pa_mainloop_free', None, [c.c_void_p]),
):
    fn = getattr(p, name); fn.restype = restype; fn.argtypes = args
class Sample(c.Structure):
    _fields_ = [('format', c.c_int), ('rate', c.c_uint), ('channels', c.c_ubyte)]
loop = p.pa_mainloop_new(); context = p.pa_context_new(p.pa_mainloop_get_api(loop), b'ABI probe')
sample = Sample(3, 48000, 2)
stream = p.pa_stream_new(context, b'cancel', c.byref(sample), None); assert stream
try:
    for _ in range(8):
        data, length = c.c_void_p(), c.c_size_t(128)
        assert p.pa_stream_begin_write(stream, c.byref(data), c.byref(length)) == 0
        assert data.value and 0 < length.value <= 128
        c.memset(data, 0, length.value)
        assert p.pa_stream_cancel_write(stream) == 0
        assert p.pa_stream_cancel_write(stream) < 0
finally:
    p.pa_stream_unref(stream); p.pa_context_unref(context); p.pa_mainloop_free(loop)
print('DESKTOP_ABI_OK', flush=True)
