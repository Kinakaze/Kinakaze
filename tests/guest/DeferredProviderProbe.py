"""Late native dlopen/dlsym/dladdr, NOLOAD and fork must preserve their semantics."""
import ctypes as c
import os
import sys


dl = c.CDLL('libdl.so.2')
dl.dlopen.argtypes, dl.dlopen.restype = [c.c_char_p, c.c_int], c.c_void_p
dl.dlsym.argtypes, dl.dlsym.restype = [c.c_void_p, c.c_char_p], c.c_void_p
dl.dlvsym.argtypes, dl.dlvsym.restype = [c.c_void_p, c.c_char_p, c.c_char_p], c.c_void_p
dl.dlclose.argtypes, dl.dlclose.restype = [c.c_void_p], c.c_int


class Info(c.Structure):
    _fields_ = [('path', c.c_char_p), ('base', c.c_void_p),
                ('name', c.c_char_p), ('address', c.c_void_p)]


dl.dladdr.argtypes, dl.dladdr.restype = [c.c_void_p, c.POINTER(Info)], c.c_int


def describe(address):
    info = Info()
    assert dl.dladdr(address, c.byref(info)) == 1
    assert info.path and info.base and info.name and info.address == address
    return info.path, info.name


os.write(2, b'DEFERRED_BEFORE_NOLOAD\n')
assert not dl.dlopen(b'libvulkan.so.1', 2 | 4)
os.write(2, b'DEFERRED_AFTER_NOLOAD\n')
handle = dl.dlopen(b'libvulkan.so.1', 2)
assert handle
os.write(2, b'DEFERRED_AFTER_DLOPEN\n')
symbol = dl.dlsym(handle, b'vkEnumerateInstanceVersion')
assert symbol and not dl.dlsym(handle, b'kinakaze_nonexistent_symbol')
os.write(2, b'DEFERRED_AFTER_DLSYM\n')
assert describe(symbol)[1] == b'vkEnumerateInstanceVersion'
resident = dl.dlopen(b'libvulkan.so.1', 2 | 4)
assert resident and dl.dlsym(resident, b'vkEnumerateInstanceVersion') == symbol
assert dl.dlclose(resident) == 0

# Exact ABI versions keep their original visibility and resolution behavior.
math = dl.dlopen(b'libm.so.6', 2)
assert math
cos = dl.dlvsym(math, b'cos', b'GLIBC_2.2.5')
assert cos == dl.dlsym(math, b'cos') and cos
assert not dl.dlvsym(math, b'cos', b'NONEXISTENT_1.0')
assert c.CFUNCTYPE(c.c_double, c.c_double)(cos)(0.0) == 1.0

sys.stdout.flush()
sys.stderr.flush()
pid = os.fork()
if pid == 0:
    try:
        assert dl.dlsym(handle, b'vkEnumerateInstanceVersion') == symbol
        assert describe(symbol)[1] == b'vkEnumerateInstanceVersion'
        assert c.CFUNCTYPE(c.c_double, c.c_double)(cos)(0.0) == 1.0
        assert not dl.dlopen(b'libXrandr.so.2', 2 | 4)
        late = dl.dlopen(b'libXrandr.so.2', 2)
        assert late
        assert dl.dlsym(late, b'XRRGetScreenResources')
        assert dl.dlclose(late) == 0
    except BaseException:
        import traceback
        traceback.print_exc()
        os._exit(1)
    os._exit(0)
assert os.waitpid(pid, 0) == (pid, 0)
assert not dl.dlopen(b'libXrandr.so.2', 2 | 4), 'child loading leaked into parent'
assert dl.dlclose(math) == 0
assert dl.dlclose(handle) == 0
assert not dl.dlopen(b'libvulkan.so.1', 2 | 4)
reopened = dl.dlopen(b'libvulkan.so.1', 2)
assert reopened and dl.dlsym(reopened, b'vkEnumerateInstanceVersion') == symbol
assert dl.dlclose(reopened) == 0
print('DEFERRED_PROVIDER_OK')
