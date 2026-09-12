"""GObject plugins reopen libraries already loaded from a private RUNPATH."""
import ctypes
import os

path = "/usr/lib/x86_64-linux-gnu/eog/libeog.so"
original = ctypes.CDLL(path, mode=os.RTLD_LOCAL | os.RTLD_NOW)
reopened = ctypes.CDLL("libeog.so", mode=os.RTLD_NOLOAD | os.RTLD_NOW)
symbol = "eog_window_activatable_activate"
assert ctypes.cast(getattr(original, symbol), ctypes.c_void_p).value == ctypes.cast(
    getattr(reopened, symbol), ctypes.c_void_p
).value
try:
    ctypes.CDLL("/tmp/no-such-eog-directory/libeog.so", mode=os.RTLD_NOLOAD | os.RTLD_NOW)
except OSError:
    pass
else:
    raise AssertionError("an explicit missing pathname reused an unrelated object")
print("DLOPEN_RESIDENT_SONAME_AND_EXPLICIT_PATH_OK", flush=True)
