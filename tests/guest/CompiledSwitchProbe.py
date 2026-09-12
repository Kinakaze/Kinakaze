"""Copied switch indices must retain every reachable guest GS branch."""
import ctypes as C
from pathlib import Path

library = C.CDLL(str(Path(__file__).with_suffix('.so')))
result = library.probe()
assert result == 0, ('copied switch index / guest GS', result)
print('COMPILED_SWITCH_GS_OK', flush=True)
