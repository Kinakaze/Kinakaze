"""Legacy resolver results must own their data in the supplied bounded buffer."""
import ctypes as c
import errno
import socket

lib = c.CDLL('libc.so.6', use_errno=True)
p = c.c_void_p
class Host(c.Structure):
    _fields_ = [('name', p), ('aliases', c.POINTER(p)), ('family', c.c_int),
                ('length', c.c_int), ('addresses', c.POINTER(p))]
lookup = lib.gethostbyname2_r
lookup.argtypes = [c.c_char_p, c.c_int, c.POINTER(Host), p, c.c_size_t,
                   c.POINTER(p), c.POINTER(c.c_int)]
lookup.restype = c.c_int
buffer = c.create_string_buffer(512)
h, result, error = Host(), p(), c.c_int()
base = c.addressof(buffer) + 1  # deliberately unaligned
assert lookup(b'127.0.0.1', socket.AF_INET, c.byref(h), base, 1, c.byref(result), c.byref(error)) == errno.ERANGE
assert result.value is None and error.value == -1
assert lookup(b'127.0.0.1', socket.AF_INET, c.byref(h), base, 511, c.byref(result), c.byref(error)) == 0
assert result.value == c.addressof(h) and error.value == 0
assert h.family == socket.AF_INET and h.length == 4
assert c.string_at(h.addresses[0], 4) == b'\x7f\0\0\1'
for pointer in (h.name, c.cast(h.aliases, p).value, c.cast(h.addresses, p).value, h.addresses[0]):
    assert base <= pointer < base + 511
assert not h.aliases[0] and not h.addresses[1]
lib.gethostbyname.argtypes = [c.c_char_p]
lib.gethostbyname.restype = p
assert lib.gethostbyname(b'192.0.2.9')
assert c.string_at(h.name) == b'127.0.0.1'
assert c.string_at(h.addresses[0], 4) == b'\x7f\0\0\1', 'result retained thread-local pointers'
assert lookup(b'::1', socket.AF_INET6, c.byref(h), base, 511, c.byref(result), c.byref(error)) == 0
assert h.family == socket.AF_INET6 and h.length == 16
assert c.string_at(h.addresses[0], 16) == b'\0' * 15 + b'\1'
assert lookup(b'localhost', 999, c.byref(h), base, 511, c.byref(result), c.byref(error)) == 0
assert not result.value and error.value == 3
print('HOST_LOOKUP_REENTRANT_IPV4_IPV6_OWNERSHIP_OK')
