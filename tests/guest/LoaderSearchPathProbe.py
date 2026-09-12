"""Dynamic-loader metadata must keep Linux search paths in the guest namespace."""
import time

started = time.monotonic()
import ctypes as c

import_ms = (time.monotonic() - started) * 1000
loader = c.CDLL("libdl.so.2")
library = c.CDLL("libffi.so.8")
loader.dlinfo.argtypes = [c.c_void_p, c.c_int, c.c_void_p]
loader.dlinfo.restype = c.c_int


class Header(c.Structure):
    _fields_ = [("size", c.c_size_t), ("count", c.c_uint), ("padding", c.c_uint)]


class SearchPath(c.Structure):
    _fields_ = [("name", c.c_char_p), ("flags", c.c_uint), ("padding", c.c_uint)]


header = Header()
assert loader.dlinfo(library._handle, 5, c.byref(header)) == 0
assert 0 < header.count < 1000 and c.sizeof(Header) <= header.size < 1024 * 1024
storage = c.create_string_buffer(header.size)
c.memmove(storage, c.byref(header), c.sizeof(header))
assert loader.dlinfo(library._handle, 4, storage) == 0
records = (SearchPath * header.count).from_address(c.addressof(storage) + c.sizeof(Header))
paths = [record.name.decode() for record in records]
for expected in ("/usr/lib", "/lib", "/usr/lib/x86_64-linux-gnu", "/lib/x86_64-linux-gnu"):
    assert expected in paths, (expected, paths)
assert all("\\" not in path for path in paths), paths
assert library.ffi_prep_cif
print("LOADER_SEARCH_PATHS_OK", paths, "ctypes_import_ms", round(import_ms, 2), flush=True)
