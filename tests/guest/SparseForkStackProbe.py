import ctypes
from pathlib import Path

fixture = ctypes.CDLL(str(Path(__file__).with_suffix('.so')))
result = fixture.probe()
assert result == 0, result
print('SPARSE_DENSE_PROTECTED_FORK_STACK_OK', flush=True)
