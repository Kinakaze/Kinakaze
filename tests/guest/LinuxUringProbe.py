"""Exercise the real Linux syscalls and mmap SQ/CQ ABI, including async CQEs."""
import ctypes as C
import errno
import os
import select
import signal
import tempfile
import threading
import time

lib = C.CDLL(None, use_errno=True)
lib.syscall.restype = C.c_long
lib.mmap.restype = C.c_void_p
lib.mmap.argtypes = [C.c_void_p, C.c_size_t, C.c_int, C.c_int, C.c_int, C.c_longlong]
lib.munmap.argtypes = [C.c_void_p, C.c_size_t]


class Offsets(C.Structure):
    _fields_ = [(name, C.c_uint32) for name in ('head', 'tail', 'mask', 'entries', 'flags', 'extra', 'array', 'reserved')] + [('user', C.c_uint64)]


class Params(C.Structure):
    _fields_ = [(name, C.c_uint32) for name in ('sq_entries', 'cq_entries', 'flags', 'cpu', 'idle', 'features', 'wq_fd')] + [('reserved', C.c_uint32 * 3), ('sq', Offsets), ('cq', Offsets)]


class SQE(C.Structure):
    _fields_ = [('opcode', C.c_uint8), ('flags', C.c_uint8), ('ioprio', C.c_uint16),
                ('fd', C.c_int32), ('offset', C.c_uint64), ('address', C.c_uint64),
                ('length', C.c_uint32), ('op_flags', C.c_uint32), ('cookie', C.c_uint64),
                ('index', C.c_uint16), ('personality', C.c_uint16), ('splice_fd', C.c_int32),
                ('padding', C.c_uint64 * 2)]


def syscall(number, *args):
    return lib.syscall(C.c_long(number), *(C.c_uint64(a) for a in (*args, *([0] * (6 - len(args))))))


def checked(value):
    assert value >= 0, (value, C.get_errno())
    return value


def word(address):
    return C.c_uint32.from_address(address)


assert C.sizeof(Params) == 120 and C.sizeof(SQE) == 64
assert syscall(425, 4, 0) == -1 and C.get_errno() == errno.EFAULT
invalid = Params(flags=0x80000000)
assert syscall(425, 4, C.addressof(invalid)) == -1 and C.get_errno() == errno.EINVAL
params = Params()
fd = checked(syscall(425, 4, C.addressof(params)))
maps = []
try:
    assert params.sq_entries == 4 and params.cq_entries >= 4 and params.features & 1
    rings_length = max(params.sq.array + params.sq_entries * 4, params.cq.extra + params.cq_entries * 16)
    def mapping(length, offset):
        address = lib.mmap(None, length, 3, 1, fd, offset)
        assert address != C.c_void_p(-1).value, C.get_errno()
        maps.append((address, length))
        return address
    rings = mapping(rings_length, 0)
    alias = mapping(rings_length, 0x08000000)
    sqes = mapping(params.sq_entries * 64, 0x10000000)
    assert word(rings + params.sq.mask).value == params.sq_entries - 1
    assert word(alias + params.cq.entries).value == params.cq_entries
    probe = C.create_string_buffer(16 + 256 * 8)
    checked(syscall(427, fd, 8, C.addressof(probe), 256))
    assert probe.raw[16 + 0 * 8 + 2] & 1
    assert probe.raw[16 + 22 * 8 + 2] & 1

    def queue(opcode=0, file=-1, address=0, length=0, offset=0, cookie=0, flags=0, op_flags=0):
        tail = word(rings + params.sq.tail).value
        index = tail & (params.sq_entries - 1)
        entry = SQE(opcode=opcode, fd=file, address=address, length=length, offset=offset, cookie=cookie,
                    flags=flags, op_flags=op_flags)
        C.memmove(sqes + index * 64, C.byref(entry), 64)
        word(rings + params.sq.array + index * 4).value = index
        word(rings + params.sq.tail).value = (tail + 1) & 0xffffffff

    def available():
        return (word(rings + params.cq.tail).value - word(rings + params.cq.head).value) & 0xffffffff

    def take():
        assert available()
        head = word(rings + params.cq.head).value
        address = rings + params.cq.extra + (head & (params.cq_entries - 1)) * 16
        cookie = C.c_uint64.from_address(address).value
        result = C.c_int32.from_address(address + 8).value
        word(rings + params.cq.head).value = (head + 1) & 0xffffffff
        assert word(alias + params.cq.head).value == (head + 1) & 0xffffffff
        return cookie, result

    for index in range(40):
        queue(cookie=index)
        assert checked(syscall(426, fd, 1, 1, 1)) == 1
        assert take() == (index, 0)
    with tempfile.TemporaryFile() as file:
        payload = bytes(range(256)) * 16
        source = C.create_string_buffer(payload)
        queue(23, file.fileno(), C.addressof(source), len(payload), cookie=100)
        assert checked(syscall(426, fd, 1, 0, 0)) == 1
        # The CQ must advance without another syscall into the ring.
        deadline = time.monotonic() + 5
        while not available():
            assert time.monotonic() < deadline, 'async CQ publication stalled'
            time.sleep(.001)
        assert take() == (100, len(payload))
        destination = C.create_string_buffer(len(payload))
        queue(22, file.fileno(), C.addressof(destination), len(payload), cookie=101)
        checked(syscall(426, fd, 1, 1, 1))
        assert take() == (101, len(payload)) and destination.raw == payload
        # Real vectored read, including EOF in the last vector.
        buffers = [C.create_string_buffer(3072), C.create_string_buffer(3072)]
        vectors = (C.c_uint64 * 4)(C.addressof(buffers[0]), 3072, C.addressof(buffers[1]), 3072)
        queue(1, file.fileno(), C.addressof(vectors), 2, cookie=102)
        checked(syscall(426, fd, 1, 1, 1))
        assert take() == (102, len(payload))
        assert buffers[0].raw + buffers[1].raw[:1024] == payload
        queue(22, file.fileno(), 1, 64, cookie=103)
        checked(syscall(426, fd, 1, 1, 1))
        assert take() == (103, -errno.EFAULT)
        queue(3, file.fileno(), cookie=106)
        checked(syscall(426, fd, 1, 1, 1))
        assert take() == (106, 0)
        queue(3, file.fileno(), cookie=107, op_flags=1)
        checked(syscall(426, fd, 1, 1, 1))
        assert take() == (107, 0)
    queue(22, -1, 0, 0, cookie=104)
    checked(syscall(426, fd, 1, 1, 1))
    assert take() == (104, -errno.EBADF)
    waited = []
    waiter = threading.Thread(target=lambda: waited.append(syscall(426, fd, 0, 1, 1)))
    waiter.start()
    time.sleep(.05)
    queue(cookie=105)
    checked(syscall(426, fd, 1, 0, 0))
    waiter.join(5)
    assert not waiter.is_alive() and waited == [0]
    assert take() == (105, 0)
    with tempfile.TemporaryFile() as fixed:
        fixed.write(b'fixed-file-retained')
        fixed.flush()
        files = (C.c_int * 1)(fixed.fileno())
        assert checked(syscall(427, fd, 2, C.addressof(files), 1)) == 0
        assert syscall(427, fd, 2, C.addressof(files), 1) == -1 and C.get_errno() == errno.EBUSY
    # All original descriptors have closed; the registered open remains owned.
    destination = C.create_string_buffer(19)
    queue(22, 0, C.addressof(destination), 19, cookie=108, flags=1)
    checked(syscall(426, fd, 1, 1, 1))
    assert take() == (108, 19) and destination.raw == b'fixed-file-retained'
    files[0] = -1
    update = (C.c_uint64 * 2)(0, C.addressof(files))
    assert checked(syscall(427, fd, 6, C.addressof(update), 1)) == 1
    queue(22, 0, C.addressof(destination), 19, cookie=109, flags=1)
    checked(syscall(426, fd, 1, 1, 1))
    assert take() == (109, -errno.EBADF)
    checked(syscall(427, fd, 3, 0, 0))
    queue(14, address=0x123456789, cookie=110)
    checked(syscall(426, fd, 1, 1, 1))
    assert take() == (110, -errno.ENOENT)
    # Cancellation races real kernel I/O. Both documented outcomes must leave
    # exactly one original CQE and retain its buffer until native retirement.
    with tempfile.TemporaryFile() as cancel_file:
        cancel_file.truncate(4 * 1024 * 1024)
        target = C.create_string_buffer(b'Z' * (4 * 1024 * 1024))
        for attempt in range(5):
            C.memset(C.addressof(target), 90, 4 * 1024 * 1024)
            cookie = 300 + attempt * 2
            queue(22, cancel_file.fileno(), C.addressof(target), 4 * 1024 * 1024, cookie=cookie)
            queue(14, address=cookie, cookie=cookie+1)
            checked(syscall(426, fd, 2, 2, 1))
            results = dict([take(), take()])
            assert results[cookie+1] in (0, -errno.ENOENT), results
            assert results[cookie] in (-errno.ECANCELED, 4 * 1024 * 1024), results
            if results[cookie] == -errno.ECANCELED:
                assert target.raw[:4 * 1024 * 1024] == b'Z' * (4 * 1024 * 1024)
            else:
                assert target.raw[:4 * 1024 * 1024] == b'\0' * (4 * 1024 * 1024)
    event = os.eventfd(0, os.EFD_NONBLOCK)
    try:
        event_fd = C.c_int(event)
        checked(syscall(427, fd, 4, C.addressof(event_fd), 1))
        queue(cookie=111)
        checked(syscall(426, fd, 1, 1, 1))
        assert take() == (111, 0)
        assert int.from_bytes(os.read(event, 8), 'little') >= 1
        checked(syscall(427, fd, 5, 0, 0))
    finally:
        os.close(event)
    with select.epoll() as ep:
        ep.register(fd, select.EPOLLIN)
        assert ep.poll(.02) == []
        def publish():
            time.sleep(.05)
            queue(cookie=112)
            checked(syscall(426, fd, 1, 0, 0))
        thread = threading.Thread(target=publish)
        thread.start()
        assert ep.poll(2) == [(fd, select.EPOLLIN)]
        thread.join(3)
        assert not thread.is_alive()
        assert take() == (112, 0)
        assert ep.poll(.01) == []
    observed = []
    old_handler = signal.signal(signal.SIGUSR1, lambda *_: observed.append(True))
    old_mask = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR1})
    try:
        temporary_mask = C.c_uint64(0)
        def interrupt_wait():
            time.sleep(.05)
            os.kill(os.getpid(), signal.SIGUSR1)
        sender = threading.Thread(target=interrupt_wait)
        sender.start()
        assert syscall(426, fd, 0, 1, 1, C.addressof(temporary_mask), 8) == -1
        assert C.get_errno() == errno.EINTR and observed == [True]
        sender.join(3)
        assert signal.SIGUSR1 in signal.pthread_sigmask(signal.SIG_BLOCK, set())
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)
        signal.signal(signal.SIGUSR1, old_handler)
    # CQ overflow retains completions and resumes publication after head advances.
    for index in range(params.cq_entries + 4):
        queue(cookie=200 + index)
        checked(syscall(426, fd, 1, 0, 0))
    for index in range(params.cq_entries + 4):
        checked(syscall(426, fd, 0, 1, 1))
        assert take() == (200 + index, 0)
    print('LINUX_URING_MMAP_ASYNC_RW_VECTORS_OVERFLOW_WAKE_OK', flush=True)
finally:
    os.close(fd)
    for address, length in maps:
        assert lib.munmap(address, length) == 0, C.get_errno()
