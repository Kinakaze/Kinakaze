"""Late initial-exec TLS: existing/new threads, relocated data and fork."""
import ctypes
import os
from pathlib import Path
import sys
import threading

directory = Path(sys.argv[1])
ready = threading.Barrier(5)
loaded = threading.Event()
errors = []
addresses = []
library = None

def load(name):
    result = ctypes.CDLL(str(directory / name))
    result.tls_check.argtypes = [ctypes.c_int, ctypes.c_int]
    result.tls_check.restype = ctypes.c_int
    result.tls_address.restype = ctypes.c_void_p
    return result

def check(lib):
    assert lib.tls_check(37, 42) == 0
    address = lib.tls_address()
    assert lib.tls_check(42, 51) == 0
    assert lib.tls_address() == address
    return address

def worker():
    ready.wait(timeout=20)
    if not loaded.wait(timeout=20):
        return
    try:
        addresses.append(check(library))
    except BaseException as error:
        errors.append(repr(error))

threads = [threading.Thread(target=worker) for _ in range(4)]
for thread in threads:
    thread.start()
ready.wait(timeout=20)
try:
    library = load('late.so')
    main_address = check(library)
finally:
    loaded.set()
for thread in threads:
    thread.join(timeout=20)
assert not errors, errors
assert len(set([main_address, *addresses])) == 5
assert library.tls_check(51, 52) == 0

result = []
def fresh():
    result.append(check(library))
thread = threading.Thread(target=fresh)
thread.start()
thread.join(timeout=20)
assert len(result) == 1

pid = os.fork()
if pid == 0:
    try:
        assert library.tls_address() == main_address
        assert library.tls_check(52, 53) == 0
        check(load('child.so'))
        result.clear()
        thread = threading.Thread(target=fresh)
        thread.start()
        thread.join(timeout=20)
        assert len(result) == 1
        os._exit(0)
    except BaseException:
        import traceback
        traceback.print_exc()
        os._exit(1)
assert os.waitpid(pid, 0) == (pid, 0)
assert library.tls_check(52, 54) == 0
print('LATE_STATIC_TLS_THREADS_FORK_OK', flush=True)

if len(sys.argv) > 2:
    omp = ctypes.CDLL(sys.argv[2])
    callback_type = ctypes.CFUNCTYPE(None, ctypes.c_void_p)
    omp.GOMP_parallel.argtypes = [callback_type, ctypes.c_void_p, ctypes.c_uint, ctypes.c_uint]
    omp.GOMP_parallel.restype = None
    omp.omp_set_dynamic.argtypes = [ctypes.c_int]
    omp.omp_set_dynamic(0)
    ids = []

    @callback_type
    def parallel_body(unused):
        ids.append((omp.omp_get_thread_num(), omp.omp_get_num_threads()))

    for _ in range(2):
        ids.clear()
        omp.GOMP_parallel(parallel_body, None, 4, 0)
        assert sorted(ids) == [(i, 4) for i in range(4)], ids
    print('LIBGOMP_PARALLEL_OK', flush=True)
