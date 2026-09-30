"""Memory advice used by Node native addons and Bun's sparse heap allocator."""
import ctypes as c
import errno
import os
from pathlib import Path
import sys
import time

lib = c.CDLL(None, use_errno=True)
lib.mmap.argtypes = [c.c_void_p, c.c_size_t, c.c_int, c.c_int, c.c_int, c.c_long]
lib.mmap.restype = c.c_void_p
for name in ('msync', 'madvise', 'mprotect'):
    getattr(lib, name).argtypes = [c.c_void_p, c.c_size_t, c.c_int]
lib.munmap.argtypes = [c.c_void_p, c.c_size_t]
lib.mincore.argtypes = [c.c_void_p, c.c_size_t, c.c_void_p]
lib.malloc.argtypes = [c.c_size_t]
lib.malloc.restype = c.c_void_p
lib.free.argtypes = [c.c_void_p]
page = os.sysconf('SC_PAGESIZE')

def check(value):
    assert value == 0, (value, c.get_errno())

heap = c.create_string_buffer(b'heap probe')
heap_page = c.addressof(heap) & -page
check(lib.msync(heap_page, 1, 1))  # NARB's actual MS_ASYNC readability probe
check(lib.msync(heap_page, 1, 4))
native_heap = lib.malloc(page)
assert native_heap
try:
    check(lib.msync(native_heap & -page, 1, 1))
finally:
    lib.free(native_heap)
check(lib.msync(c.cast(lib.msync, c.c_void_p).value & -page, 1, 1))
assert lib.msync(heap_page + 1, 1, 1) == -1 and c.get_errno() == errno.EINVAL
assert lib.msync(heap_page, 1, 5) == -1 and c.get_errno() == errno.EINVAL
print('MSYNC_HEAP_OK', flush=True)

if len(sys.argv) > 1:
    # The host runner sets a 4 GiB Job limit. Anonymous sections previously
    # bypassed that limit and could exhaust system commit without failing.
    oversized = lib.mmap(None, 8 * 1024**3, 3, 0x4022, -1, 0)
    if oversized != c.c_void_p(-1).value:
        check(lib.munmap(oversized, 8 * 1024**3))
        raise AssertionError('8 GiB mapping bypassed the 4 GiB Job limit')
    assert c.get_errno() == errno.ENOMEM, c.get_errno()
    print('ANONYMOUS_JOB_LIMIT_OK', flush=True)

length = 512 * 1024 * 1024
address = lib.mmap(None, length, 3, 0x4022, -1, 0)
assert address not in (None, c.c_void_p(-1).value)
try:
    for offset in (0, page, length - page):
        c.c_ubyte.from_address(address + offset).value = 123
    check(lib.madvise(address, length, 4))
    # DONTNEED must evict rather than materialize untouched pages. mincore is
    # checked before reading the resulting zeros back into the working set.
    residency = (c.c_ubyte * (length // page))()
    check(lib.mincore(address, length, residency))
    resident = sum(value & 1 for value in residency) * page
    assert resident < 16 * 1024 * 1024, resident
    # Reuse one live mapping: retained section backings must be retired on
    # repeated discards, and old touched pages must not accumulate residency.
    for iteration in range(12):
        c.c_ubyte.from_address(address + iteration * page).value = 123
        check(lib.madvise(address, length, 4))
        check(lib.mincore(address, length, residency))
        assert sum(value & 1 for value in residency) * page < 16 * 1024 * 1024
    if len(sys.argv) > 1:
        control = Path(sys.argv[1])
        (control / 'go-0').exists()  # Warm pathname lookup before measuring handles.
        for phase in range(4):
            time.sleep(.05)  # Warm the same native timer path used by the barrier.
            print('MEMORY_PHASE_{}_READY'.format(phase), flush=True)
            deadline = time.monotonic() + 30
            while not (control / ('go-' + str(phase))).exists():
                assert time.monotonic() < deadline
                time.sleep(.01)
            for iteration in range(16):
                c.c_ubyte.from_address(address + iteration * page).value = 123
                check(lib.madvise(address, length, 4))
    for offset in (0, page, length - page):
        assert c.c_ubyte.from_address(address + offset).value == 0
    c.c_ubyte.from_address(address).value = 41
    c.c_ubyte.from_address(address + 2 * page).value = 42
    c.c_ubyte.from_address(address + page).value = 43
    check(lib.mprotect(address + page, page, 1))
    check(lib.madvise(address + page, page, 4))
    assert c.c_ubyte.from_address(address + page).value == 0
    assert c.c_ubyte.from_address(address).value == 41
    assert c.c_ubyte.from_address(address + 2 * page).value == 42
    check(lib.msync(address + page, 1, 1))
    check(lib.mprotect(address + page, page, 3))
    check(lib.mprotect(address + 16 * 1024 * 1024, 16 * 1024 * 1024, 1))
    check(lib.madvise(address + 16 * 1024 * 1024, 16 * 1024 * 1024, 4))
    check(lib.mprotect(address + 32 * 1024 * 1024, 16 * 1024 * 1024, 0))
    check(lib.madvise(address + 32 * 1024 * 1024, 16 * 1024 * 1024, 4))
    check(lib.mprotect(address + 32 * 1024 * 1024, 16 * 1024 * 1024, 1))
    assert c.c_ubyte.from_address(address + 32 * 1024 * 1024).value == 0
    # Parent writes must stay isolated when its child discards a COW page.
    pid = os.fork()
    if pid == 0:
        try:
            check(lib.madvise(address, page, 4))
            assert c.c_ubyte.from_address(address).value == 0
            os._exit(0)
        except BaseException:
            os._exit(1)
    assert os.waitpid(pid, 0)[1] == 0
    assert c.c_ubyte.from_address(address).value == 41
    print('DONTNEED_SPARSE_PROTECTION_FORK_OK', resident, flush=True)
finally:
    check(lib.munmap(address, length))
assert lib.msync(address, 1, 1) == -1 and c.get_errno() == errno.ENOMEM
print('AGENT_MEMORY_ADVICE_OK', flush=True)
