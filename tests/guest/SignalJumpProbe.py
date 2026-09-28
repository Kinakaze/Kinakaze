import ctypes
from pathlib import Path
import sys

probe = ctypes.CDLL(str(Path(__file__).with_suffix('.so'))).probe
probe.argtypes = [ctypes.c_int]
probe.restype = ctypes.c_int
for alternate in (list(map(int, sys.argv[1:])) or [0, 1, 2, 3, 4, 5, 0]):
    result = probe(alternate)
    assert result == 0, (alternate, result)
print('SIGNAL_JUMP_OK', flush=True)
