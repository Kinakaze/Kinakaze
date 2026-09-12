"""Netgroup recursion/enumeration and caller-owned shadow record parsing."""
import ctypes as c
import errno
from pathlib import Path

lib = c.CDLL('libc.so.6', use_errno=True)
p, i = c.c_void_p, c.c_int
def bind(name, result, *args):
    f = getattr(lib, name)
    f.restype, f.argtypes = result, args
    return f

path = Path('/etc/netgroup')
original = path.read_bytes() if path.exists() else None
try:
    path.write_bytes((original or b'') + b'\nkinakaze_probe (host,alice,domain) kinakaze_nested\nkinakaze_nested (,bob,) kinakaze_probe\n')
    begin = bind('setnetgrent', i, c.c_char_p)
    get = bind('getnetgrent', i, c.POINTER(p), c.POINTER(p), c.POINTER(p))
    get_r = bind('getnetgrent_r', i, c.POINTER(p), c.POINTER(p), c.POINTER(p), p, c.c_size_t)
    a, b, d = p(), p(), p()
    assert begin(b'kinakaze_probe') == 1
    buffer = c.create_string_buffer(128)
    assert get_r(c.byref(a), c.byref(b), c.byref(d), buffer, 1) == 0
    assert c.get_errno() == errno.ERANGE
    assert get_r(c.byref(a), c.byref(b), c.byref(d), buffer, len(buffer)) == 1
    assert (c.string_at(a), c.string_at(b), c.string_at(d)) == (b'host', b'alice', b'domain')
    for value in (a, b, d):
        assert c.addressof(buffer) <= value.value < c.addressof(buffer) + len(buffer)
    assert get(c.byref(a), c.byref(b), c.byref(d)) == 1
    assert not a.value and c.string_at(b) == b'bob' and not d.value
    assert get(c.byref(a), c.byref(b), c.byref(d)) == 0, 'nested cycle did not terminate'
    bind('endnetgrent', None)()
finally:
    if original is None:
        path.unlink()
    else:
        path.write_bytes(original)

class Shadow(c.Structure):
    _fields_ = [('name', p), ('password', p), ('last', c.c_long), ('minimum', c.c_long),
                ('maximum', c.c_long), ('warning', c.c_long), ('inactive', c.c_long),
                ('expires', c.c_long), ('flag', c.c_ulong)]
path = Path('/tmp/kinakaze-shadow-probe')
path.write_bytes(b'# fixture, not a real credential\ninvalid\nprobe:!:20000:1:90:7:::0\nother:*:::::::\n')
f = bind('fopen', p, c.c_char_p, c.c_char_p)(str(path).encode(), b'r')
assert f
try:
    entry, result = Shadow(), p()
    buffer = c.create_string_buffer(128)
    read = bind('fgetspent_r', i, p, c.POINTER(Shadow), p, c.c_size_t, c.POINTER(p))
    assert read(f, c.byref(entry), buffer, 1, c.byref(result)) == errno.ERANGE
    assert not result.value
    assert read(f, c.byref(entry), buffer, len(buffer), c.byref(result)) == 0
    assert result.value == c.addressof(entry)
    assert c.string_at(entry.name) == b'probe' and c.string_at(entry.password) == b'!'
    assert (entry.last, entry.minimum, entry.maximum, entry.warning, entry.inactive, entry.expires, entry.flag) == (20000, 1, 90, 7, -1, -1, 0)
    assert read(f, c.byref(entry), buffer, len(buffer), c.byref(result)) == 0
    assert c.string_at(entry.name) == b'other' and entry.last == -1
    assert entry.flag == (1 << 64) - 1
    assert read(f, c.byref(entry), buffer, len(buffer), c.byref(result)) == errno.ENOENT
    assert not result.value
finally:
    bind('fclose', i, p)(f)
    path.unlink()
print('DESKTOP_NETGROUP_SHADOW_STREAM_OK')
