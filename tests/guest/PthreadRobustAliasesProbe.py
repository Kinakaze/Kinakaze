"""Regenerating exports preserves GNU robust mutex aliases and exact versions."""
import ctypes

loader = ctypes.CDLL("libdl.so.2")
loader.dlvsym.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p]
loader.dlvsym.restype = ctypes.c_void_p
aliases = {
    "pthread_mutex_consistent_np": "pthread_mutex_consistent",
    "pthread_mutexattr_getrobust_np": "pthread_mutexattr_getrobust",
    "pthread_mutexattr_setrobust_np": "pthread_mutexattr_setrobust",
}
for soname, version in (("libc.so.6", b"GLIBC_2.34"),
                        ("libpthread.so.0", b"GLIBC_2.12")):
    library = ctypes.CDLL(soname)
    for alias, original in aliases.items():
        expected = ctypes.cast(getattr(library, original), ctypes.c_void_p).value
        assert ctypes.cast(getattr(library, alias), ctypes.c_void_p).value == expected
        assert loader.dlvsym(library._handle, alias.encode(), b"GLIBC_2.4") == expected
        assert loader.dlvsym(library._handle, alias.encode(), b"GLIBC_999.0") is None
    expected = ctypes.cast(library.pthread_mutexattr_getrobust, ctypes.c_void_p).value
    assert loader.dlvsym(library._handle, b"pthread_mutexattr_getrobust", version) == expected
print("PTHREAD_ROBUST_ALIASES_OK", flush=True)
