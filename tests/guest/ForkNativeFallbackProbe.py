"""An unaudited participant must retain ordinary fork behavior with the pool on."""
import ctypes
import os

library = ctypes.CDLL('libX11.so.6')
assert library._handle
read_end, write_end = os.pipe()
os.write(write_end, b'ordinary-fork')
child = os.fork()
if child == 0:
    assert os.read(read_end, 13) == b'ordinary-fork'
    nested = os.fork()
    if nested == 0:
        os._exit(0)
    assert os.waitpid(nested, 0) == (nested, 0)
    os._exit(0)
assert os.waitpid(child, 0) == (child, 0)
os.close(read_end)
os.close(write_end)
print('FORK_NATIVE_FALLBACK_OK', flush=True)
