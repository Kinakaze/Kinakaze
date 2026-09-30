import ctypes
from pathlib import Path

fixture = ctypes.CDLL(str(Path(__file__).with_suffix('.so')))
result = fixture.probe()
assert result == 0, result
print('WRITE_WATCH_KERNEL_IO_NESTED_FORK_STACK_OK', flush=True)
