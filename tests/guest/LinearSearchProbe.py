"""Check search.h first-match pointers, callback order and in-place insertion."""
import ctypes as c

lib = c.CDLL('libc.so.6', use_errno=True)
compare_type = c.CFUNCTYPE(c.c_int, c.c_void_p, c.c_void_p)
calls = []

@compare_type
def compare(key, item):
    a, b = c.c_int.from_address(key).value, c.c_int.from_address(item).value
    calls.append((a, b))
    return (a > b) - (a < b)

for name in ['lfind', 'lsearch']:
    fn = getattr(lib, name)
    fn.restype = c.c_void_p
    fn.argtypes = [c.c_void_p, c.c_void_p, c.POINTER(c.c_size_t), c.c_size_t, compare_type]

array = (c.c_int * 6)(7, 3, 7, 99, 99, 99)
length, key = c.c_size_t(3), c.c_int(7)
c.set_errno(123)
assert lib.lfind(c.byref(key), array, c.byref(length), 4, compare) == c.addressof(array)
assert calls == [(7, 7)] and length.value == 3 and c.get_errno() == 123
calls.clear()
key.value = 8
assert lib.lfind(c.byref(key), array, c.byref(length), 4, compare) is None
assert calls == [(8, 7), (8, 3), (8, 7)]
assert list(array) == [7, 3, 7, 99, 99, 99] and length.value == 3
assert lib.lsearch(c.byref(key), array, c.byref(length), 4, compare) == c.addressof(array) + 12
assert length.value == 4 and list(array) == [7, 3, 7, 8, 99, 99]
assert lib.lsearch(c.byref(key), array, c.byref(length), 4, compare) == c.addressof(array) + 12
assert length.value == 4
length.value = 0
calls.clear()
assert lib.lfind(c.byref(key), array, c.byref(length), 4, compare) is None and not calls
assert lib.lsearch(c.byref(key), array, c.byref(length), 4, compare) == c.addressof(array)
assert length.value == 1 and array[0] == 8 and not calls
assert c.get_errno() == 123
print('LINEAR_SEARCH_FIRST_MATCH_INSERT_COUNT_ERRNO_OK')
