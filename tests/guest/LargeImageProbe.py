"""Race demand-loading a large ELF with fork, checking exact snapshot bytes."""
import _ctypes
import ctypes
import os
import sys
import threading
import time

errors = []
started = threading.Event()

def load_images():
    try:
        started.set()
        for _ in range(12):
            library = ctypes.CDLL(sys.argv[1])
            value = library.image_value
            value.argtypes = [ctypes.c_uint]
            value.restype = ctypes.c_int
            for index, expected in [(0, 3), (1, 0), (8*1024*1024, 91), (16*1024*1024+36, 171)]:
                assert value(index) == expected
            del value
            _ctypes.dlclose(library._handle)
    except BaseException as error:
        errors.append(repr(error))

thread = threading.Thread(target=load_images)
thread.start()
started.wait()
for _ in range(6):
    time.sleep(.002)
    child = os.fork()
    if child == 0:
        os._exit(0)
    assert os.waitpid(child, 0)[1] == 0
thread.join()
assert not errors, errors
print('LARGE_IMAGE_FORK_OK', flush=True)
