import ctypes
from pathlib import Path

probe = ctypes.CDLL(str(Path(__file__).with_suffix('.so'))).probe
probe.argtypes = []
probe.restype = ctypes.c_int
result = probe()
assert result == 0, result
print('CONCURRENT_VFORK_OK', flush=True)
