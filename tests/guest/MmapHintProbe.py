"""Chromium page hints must not enlarge the underlying Windows placeholder."""
import ctypes as C
import os

libc = C.CDLL('libc.so.6', use_errno=True)
mmap = libc.mmap
mmap.argtypes = [C.c_void_p, C.c_size_t, C.c_int, C.c_int, C.c_int, C.c_longlong]
mmap.restype = C.c_void_p
unmap = libc.munmap
unmap.argtypes = [C.c_void_p, C.c_size_t]
unmap.restype = C.c_int
protect = libc.mprotect
protect.argtypes = [C.c_void_p, C.c_size_t, C.c_int]
protect.restype = C.c_int
failed = C.c_void_p(-1).value
for offset in range(0, 65536, 4096):
    hint, size = 0x32c076ac0000 + offset, 0x40000
    memory = mmap(hint, size, 3, 0x22, -1, 0)
    assert memory != failed, (hex(hint), C.get_errno())
    assert memory % 4096 == 0
    C.memset(memory, 0x6b, size)
    assert protect(memory + 4096, 4096, 0) == 0
    assert protect(memory + 4096, 4096, 3) == 0
    assert C.c_ubyte.from_address(memory + 4096).value == 0x6b
    # A colliding hint is advisory and must leave the original mapping intact.
    other = mmap(memory + 4096, size, 3, 0x22, -1, 0)
    assert other != failed and (other + size <= memory or other >= memory + size)
    assert C.c_ubyte.from_address(memory).value == 0x6b
    assert unmap(other, size) == 0
    if offset == 32768:
        child = os.fork()
        if child == 0:
            assert C.c_ubyte.from_address(memory + size - 1).value == 0x6b
            C.memset(memory, 0x31, size)
            assert unmap(memory, size) == 0
            os._exit(0)
        assert os.waitpid(child, 0)[1] == 0
        assert C.c_ubyte.from_address(memory + size - 1).value == 0x6b
    assert unmap(memory, size) == 0
print('MMAP_HINT_OK', flush=True)
