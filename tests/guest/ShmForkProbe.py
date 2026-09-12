"""Fork retains SysV section identity and MIT-SHM attachment bookkeeping."""
import ctypes as c
import os

lib = c.CDLL('libc.so.6', use_errno=True)
x, ext = c.CDLL('libX11.so.6'), c.CDLL('libXext.so.6')
p, i, u = c.c_void_p, c.c_int, c.c_ulong
def bind(lib, name, result, *args):
    f = getattr(lib, name); f.restype, f.argtypes = result, args; return f
class Info(c.Structure):
    _fields_ = [('shmseg', u), ('shmid', i), ('shmaddr', p), ('read_only', i)]
get = bind(lib, 'shmget', i, i, c.c_size_t, i)
attach = bind(lib, 'shmat', p, i, p, i)
detach = bind(lib, 'shmdt', i, p)
control = bind(lib, 'shmctl', i, i, i, p)
display = bind(x, 'XOpenDisplay', p, c.c_char_p)(None)
segment = get(0, 4096, 0o600)
assert segment >= 0
address = attach(segment, None, 0)
assert address not in (None, c.c_void_p(-1).value)
info = Info(0, segment, address, 0)
assert bind(ext, 'XShmAttach', i, p, p)(display, c.byref(info))
# A marked-for-removal segment must remain shared through the inherited views.
assert control(segment, 0, None) == 0
value = c.c_uint32.from_address(address)
value.value = 0x12345678
reader, writer = os.pipe()
child = os.fork()
if child == 0:
    os.close(reader)
    assert value.value == 0x12345678
    value.value = 0x89abcdef
    assert bind(ext, 'XShmDetach', i, p, p)(display, c.byref(info))
    assert detach(address) == 0
    os.write(writer, b'OK')
    os.close(writer)
    os._exit(0)
os.close(writer)
assert os.read(reader, 2) == b'OK'
os.close(reader)
assert os.waitpid(child, 0)[1] == 0
assert value.value == 0x89abcdef, hex(value.value)
assert bind(ext, 'XShmDetach', i, p, p)(display, c.byref(info))
assert detach(address) == 0
bind(x, 'XCloseDisplay', i, p)(display)
print('SYSV_MIT_SHM_FORK_SHARED_RMID_DETACH_OK', flush=True)
