"""Check real libc getsubopt pointer and mutation semantics, including unknowns."""
import ctypes as c

libc = c.CDLL(None)
getsubopt = libc.getsubopt
getsubopt.argtypes = [c.POINTER(c.c_void_p), c.POINTER(c.c_char_p), c.POINTER(c.c_void_p)]
getsubopt.restype = c.c_int
tokens = (c.c_char_p * 4)(b"size", b"flag", b"", None)
source = c.create_string_buffer(b"size=42,flag,unknown=a=b,size=,sizeish=x,,flag,")
cursor = c.c_void_p(c.addressof(source))
value = c.c_void_p(123)
expected = [(0, b"42"), (1, None), (-1, b"unknown=a=b"), (0, b""),
            (-1, b"sizeish=x"), (2, None), (1, None)]
for index, text in expected:
    before = cursor.value
    assert getsubopt(c.byref(cursor), tokens, c.byref(value)) == index
    assert cursor.value > before
    assert (c.string_at(value.value) if value.value else None) == text
exhausted = cursor.value
value.value = 123
assert getsubopt(c.byref(cursor), tokens, c.byref(value)) == -1
assert cursor.value == exhausted and value.value == 123
assert b"size=42\x00flag\x00unknown=a=b\x00" in source.raw
# No getopt global state is shared between independent callers.
second = c.create_string_buffer(b"flag")
other = c.c_void_p(c.addressof(second))
assert getsubopt(c.byref(other), tokens, c.byref(value)) == 1 and not value.value
print("GETSUBOPT_MATCH_UNKNOWN_EMPTY_CURSOR_OK")
