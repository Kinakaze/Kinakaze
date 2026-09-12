"""Real C allocator reuse, cross-thread frees, alignment and fork isolation."""
import ctypes as C
import os
import queue
import threading

libc = C.CDLL(None, use_errno=True)
libc.malloc.argtypes = [C.c_size_t]
libc.malloc.restype = C.c_void_p
libc.free.argtypes = [C.c_void_p]
libc.realloc.argtypes = [C.c_void_p, C.c_size_t]
libc.realloc.restype = C.c_void_p
libc.posix_memalign.argtypes = [C.POINTER(C.c_void_p), C.c_size_t, C.c_size_t]
libc.malloc_usable_size.argtypes = [C.c_void_p]
libc.malloc_usable_size.restype = C.c_size_t


def allocate(size, marker):
    pointer = libc.malloc(size)
    assert pointer and pointer % 16 == 0
    assert libc.malloc_usable_size(pointer) >= size
    C.memset(pointer, marker, size)
    return pointer, size, marker


def release(block):
    pointer, size, marker = block
    assert C.c_ubyte.from_address(pointer).value == marker
    assert C.c_ubyte.from_address(pointer + size - 1).value == marker
    libc.free(pointer)


def main():
    failures = queue.Queue()
    incoming = queue.Queue(maxsize=2)
    def recycler():
        try:
            while True:
                blocks = incoming.get()
                if blocks is None:
                    return
                for block in reversed(blocks):
                    release(block)
        except BaseException as error:
            failures.put(error)
    thread = threading.Thread(target=recycler)
    thread.start()
    try:
        for iteration in range(12):
            blocks = [allocate((16, 128, 384, 4096, 65536, 131072)[index % 6],
                               (index + iteration) % 256) for index in range(512)]
            assert len({block[0] for block in blocks}) == len(blocks)
            incoming.put(blocks, timeout=5)
    finally:
        incoming.put(None, timeout=5)
        thread.join(timeout=5)
    assert not thread.is_alive()
    if not failures.empty():
        raise failures.get()

    for alignment in (16, 64, 256, 4096):
        value = C.c_void_p()
        assert libc.posix_memalign(C.byref(value), alignment, 4097) == 0
        assert value.value % alignment == 0
        C.memset(value, 0xa7, 4097)
        libc.free(value)

    block = allocate(131072, 0x5a)
    pointer = libc.realloc(block[0], 262145)
    assert pointer and C.string_at(pointer, 131072) == b"\x5a" * 131072
    child = os.fork()
    if child == 0:
        C.memset(pointer, 0x33, 131072)
        libc.free(pointer)
        for index in range(256):
            release(allocate(384, index % 256))
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)
    assert C.string_at(pointer, 131072) == b"\x5a" * 131072
    libc.free(pointer)
    print("AllocationLifecycleProbe: PASS", flush=True)


if __name__ == "__main__":
    main()
