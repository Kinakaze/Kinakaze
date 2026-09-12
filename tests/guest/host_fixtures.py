"""Bounded host-state guards for probes that exercise desktop control APIs."""
from contextlib import contextmanager
import ctypes


@contextmanager
def pointer_control():
    user = ctypes.WinDLL('user32', use_last_error=True)
    spi = user.SystemParametersInfoW
    spi.argtypes = [ctypes.c_uint, ctypes.c_uint, ctypes.c_void_p, ctypes.c_uint]
    spi.restype = ctypes.c_int
    saved = (ctypes.c_int * 3)()
    if not spi(3, 0, saved, 0):  # SPI_GETMOUSE
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        yield
    finally:
        # Restore both native thresholds, not just the single X11 threshold.
        if not spi(4, 0, saved, 0):  # SPI_SETMOUSE; no registry persistence
            raise ctypes.WinError(ctypes.get_last_error())
