import ctypes
import os
import traceback


libc = ctypes.CDLL(None, use_errno=True)
libc.malloc.argtypes = [ctypes.c_size_t]
libc.malloc.restype = ctypes.c_void_p
libc.free.argtypes = [ctypes.c_void_p]
libc.mprotect.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
libc.mprotect.restype = ctypes.c_int
length = 12 * 1024 * 1024
address = libc.malloc(length)
assert address and 0x500000000000 <= address < 0x500400000000, hex(address or 0)
ctypes.memset(address, 31, length)
data = (ctypes.c_ubyte * length).from_address(address)
protected = (address + length - 16384 + 4095) & ~4095
assert libc.mprotect(protected, 4096, 0) == 0, ctypes.get_errno()
assert libc.mprotect(protected + 4096, 4096, 1) == 0, ctypes.get_errno()
reader, writer = os.pipe()


def checked_child(action):
    child = os.fork()
    if child == 0:
        try:
            action()
        except BaseException:
            traceback.print_exc()
            os._exit(1)
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)


def old_snapshot():
    os.close(writer)
    assert os.read(reader, 1) == b'x'
    assert all(data[offset] == 31 for offset in range(0, length - 32768, 4096))
    assert libc.mprotect(protected, 4096, 3) == 0
    assert ctypes.c_ubyte.from_address(protected).value == 31
    data[0] = 233
    def grandchild():
        assert data[0] == 233
    checked_child(grandchild)


old_child = os.fork()
if old_child == 0:
    try:
        old_snapshot()
    except BaseException:
        traceback.print_exc()
        os._exit(1)
    os._exit(0)
os.close(reader)
try:
    for offset in range(0, length - 32768, 4096):
        data[offset] = 50
    for generation in range(65):
        value = generation + 50
        data[generation * 4096] = value

        def fresh_snapshot():
            assert all(data[offset] == (50 + offset // 4096 if offset // 4096 <= generation else 50)
                       for offset in range(0, length - 32768, 4096))
            assert libc.mprotect(protected, 4096, 3) == 0
            assert ctypes.c_ubyte.from_address(protected).value == 31
            assert ctypes.c_ubyte.from_address(protected + 4096).value == 31
            data[0] = 211
            if generation in (31, 63):
                def grandchild():
                    assert data[0] == 211
                    data[0] = 219
                checked_child(grandchild)
                assert data[0] == 211

        checked_child(fresh_snapshot)
        assert data[0] == 50 and data[generation * 4096] == value
    os.write(writer, b'x')
    assert os.waitpid(old_child, 0) == (old_child, 0)
    assert data[0] == 50 and data[64 * 4096] == 114
finally:
    os.close(writer)
    assert libc.mprotect(protected, 8192, 3) == 0
    libc.free(address)

print('FORK_HEAP_REFRESH_GENERATIONS_PROTECTION_NESTED_OK', flush=True)
