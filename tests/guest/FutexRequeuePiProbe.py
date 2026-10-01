"""Raw PI requeue ownership, FIFO, signal stages and fork mapping aliases."""
import ctypes as c
import errno
import json
import mmap
import os
import tempfile
import threading
import time

libc = c.CDLL('libc.so.6', use_errno=True)
libc.syscall.restype = c.c_long
libc.syscall.argtypes = [c.c_long] + [c.c_ulonglong] * 6


def call(number, *args):
    c.set_errno(0)
    result = libc.syscall(number, *args, *([0] * (6 - len(args))))
    return result, c.get_errno()


class Timespec(c.Structure):
    _fields_ = [('seconds', c.c_longlong), ('nanoseconds', c.c_longlong)]


def deadline(realtime=False, seconds=5):
    value = (time.time() if realtime else time.monotonic()) + seconds
    return Timespec(int(value), int((value % 1) * 1_000_000_000))


def futex(source, command, private, value=0, fourth=0, target=None, comparison=0):
    return call(202, c.addressof(source), command | (128 if private else 0), value, fourth,
                c.addressof(target) if target is not None else 0, comparison)


def until(predicate):
    end = time.monotonic() + 5
    while not predicate():
        assert time.monotonic() < end, 'requeue state did not become visible'
        time.sleep(0.001)


rows = []
for private in (False, True):
    for realtime in (False, True):
        source, target = c.c_uint(7), c.c_uint(0)
        for wake in (0, 2, 0xffffffff):
            assert futex(source, 12, private, wake, 0, target, 7) == (-1, errno.EINVAL)
        assert futex(source, 12, private, 1, 0xffffffff, target, 7) == (-1, errno.EINVAL)
        assert futex(source, 12, private, 1, 0, target, 6) == (-1, errno.EAGAIN)
        assert futex(source, 12, private, 1, 0, target, 7) == (0, 0)
        zero = Timespec(0, 0)
        assert futex(source, 11 | (256 if realtime else 0), private, 7, c.addressof(zero), target) == (-1, errno.ETIMEDOUT)
        assert futex(source, 11, private, 7, 1, source) == (-1, errno.EFAULT)
        assert futex(source, 11, private, 7, 0, source) == (-1, errno.EINVAL)
        results, order, ids = [], [], []

        def waiter(index):
            limit = deadline(realtime)
            ids.append(call(186)[0])
            result = futex(source, 11 | (256 if realtime else 0), private, 7, c.addressof(limit), target)
            results.append(result)
            if result == (0, 0):
                assert target.value & 0x3fffffff == call(186)[0]
                order.append(index)
                assert futex(target, 7, private) == (0, 0)

        assert futex(target, 13, private) == (0, 0)
        threads = []
        for index in range(3):
            thread = threading.Thread(target=waiter, args=(index,))
            thread.start()
            threads.append(thread)
            until(lambda: len(ids) == index + 1 and futex(source, 1, private, 1) == (-1, errno.EINVAL))
            time.sleep(0.02)
        wrong = c.c_uint(0)
        assert futex(source, 12, private, 1, 2, wrong, 7) == (-1, errno.EINVAL)
        assert futex(source, 12, private, 1, 2, target, 7) == (3, 0)
        assert not results, 'requeue returned before target unlock'
        assert target.value & 0x80000000
        assert futex(target, 7, private) == (0, 0)
        for thread in threads:
            thread.join(6)
            assert not thread.is_alive()
        assert results == [(0, 0)] * 3 and order == [0, 1, 2], (results, order)
        assert target.value == 0
        rows.append(dict(private=private, realtime=realtime, order=order))


class Action(c.Structure):
    _fields_ = [('handler', c.c_ulonglong), ('flags', c.c_ulonglong),
                ('restorer', c.c_ulonglong), ('mask', c.c_ulonglong)]


active = None
delivered = []


@c.CFUNCTYPE(None, c.c_int)
def handler(number):
    # Pre-migration restart must re-copy this absolute deadline even when
    # SA_RESTART is absent. Post-migration must return EAGAIN without re-copy.
    active.seconds = -1
    delivered.append(number)


for private in (False, True):
    for migrated in (False, True):
        for restart in (False, True):
            source, target = c.c_uint(0), c.c_uint(0)
            assert futex(target, 13, private) == (0, 0)
            active = deadline()
            old = Action()
            action = Action(c.cast(handler, c.c_void_p).value, 0x10000000 if restart else 0, 0, 0)
            assert call(13, 12, c.addressof(action), c.addressof(old), 8) == (0, 0)
            ids, result = [], []

            def interrupted():
                ids.append(call(186)[0])
                result.append(futex(source, 11, private, 0, c.addressof(active), target))

            thread = threading.Thread(target=interrupted)
            thread.start()
            until(lambda: ids and futex(source, 1, private, 1) == (-1, errno.EINVAL))
            if migrated:
                assert futex(source, 12, private, 1, 0, target, 0) == (1, 0)
            before = len(delivered)
            assert call(234, os.getpid(), ids[0], 12) == (0, 0)
            thread.join(6)
            assert not thread.is_alive()
            assert len(delivered) == before + 1
            assert result == [(-1, errno.EAGAIN if migrated else errno.EINVAL)], (private, migrated, restart, result)
            assert futex(source, 1, private, 1) == (0, 0)
            assert futex(target, 7, private) == (0, 0)
            assert call(13, 12, c.addressof(old), 0, 8) == (0, 0)
            rows.append(dict(private=private, migrated=migrated, restart=restart, result=result))


for occupied in (False, True):
    with tempfile.TemporaryFile() as file:
        file.truncate(4096)
        first = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
        source, target = c.c_uint.from_buffer(first, 0), c.c_uint.from_buffer(first, 4)
        source.value = 7
        if occupied:
            assert futex(target, 13, False) == (0, 0)
        child = os.fork()
        if child == 0:
            assert call(186)[0] == os.getpid()
            alias = mmap.mmap(file.fileno(), 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
            a, b = c.c_uint.from_buffer(alias, 0), c.c_uint.from_buffer(alias, 4)
            assert c.addressof(a) != c.addressof(source)
            limit = deadline()
            assert futex(a, 11, False, 7, c.addressof(limit), b) == (0, 0)
            assert b.value & 0x3fffffff == os.getpid()
            assert futex(b, 7, False) == (0, 0)
            os._exit(0)
        until(lambda: futex(source, 1, False, 1) == (-1, errno.EINVAL))
        assert futex(source, 12, False, 1, 0, target, 7) == (1, 0)
        if occupied:
            assert os.waitpid(child, os.WNOHANG) == (0, 0)
            assert futex(target, 7, False) == (0, 0)
        assert os.waitpid(child, 0) == (child, 0)
        assert target.value == 0
        del source, target
        first.close()
        rows.append(dict(fork_alias=True, occupied=occupied))

print(json.dumps(rows, sort_keys=True))
print('FUTEX_REQUEUE_PI_PASS', flush=True)
