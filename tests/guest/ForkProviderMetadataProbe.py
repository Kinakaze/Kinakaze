"""Nested fork before lookup, then versioned native lookup and introspection."""
import ctypes
import os

libc = ctypes.CDLL(None)
libm = ctypes.CDLL('libm.so.6')
libm.cos.argtypes = [ctypes.c_double]
libm.cos.restype = ctypes.c_double
assert libm.cos(0.0) == 1.0

libc.dlvsym.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p]
libc.dlvsym.restype = ctypes.c_void_p
libc.dlerror.restype = ctypes.c_char_p

class DlInfo(ctypes.Structure):
    _fields_ = [('name', ctypes.c_char_p), ('base', ctypes.c_void_p),
                ('symbol', ctypes.c_char_p), ('address', ctypes.c_void_p)]

libc.dladdr.argtypes = [ctypes.c_void_p, ctypes.POINTER(DlInfo)]
libc.dladdr.restype = ctypes.c_int
callback_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_size_t,
                               ctypes.c_void_p)
libc.dl_iterate_phdr.argtypes = [callback_type, ctypes.c_void_p]
libc.dl_iterate_phdr.restype = ctypes.c_int

def inspect():
    address = libc.dlvsym(libm._handle, b'cos', b'GLIBC_2.2.5')
    assert address, libc.dlerror()
    assert ctypes.CFUNCTYPE(ctypes.c_double, ctypes.c_double)(address)(0.0) == 1.0
    assert not libc.dlvsym(libm._handle, b'cos', b'NO_SUCH_VERSION')
    assert libc.dlerror()
    info = DlInfo()
    assert libc.dladdr(address, ctypes.byref(info)) == 1
    assert info.base and info.name
    seen = []
    @callback_type
    def visit(record, size, unused):
        seen.append((bool(record), size))
        return 0
    assert libc.dl_iterate_phdr(visit, None) == 0
    assert seen and all(record and size > 0 for record, size in seen)
    # Reopening an inherited facade keeps its handle lifetime and symbol table.
    reopened = ctypes.CDLL('libm.so.6')
    reopened.sin.argtypes = [ctypes.c_double]
    reopened.sin.restype = ctypes.c_double
    assert reopened.sin(0.0) == 0.0

def join(child):
    assert child > 0
    assert os.waitpid(child, 0) == (child, 0)

child = os.fork()
if child == 0:
    try:
        grandchild = os.fork()
        if grandchild == 0:
            inspect()
            os._exit(0)
        join(grandchild)
        inspect()
        os._exit(0)
    except BaseException:
        import traceback
        traceback.print_exc()
        os._exit(1)
join(child)
inspect()
print('FORK_PROVIDER_METADATA_OK', flush=True)
