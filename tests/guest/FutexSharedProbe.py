"""Actual Linux processes, shared object aliases, requeue races and lifecycle."""
import ctypes as C
import errno
import os
from pathlib import Path
import signal
import sys
import threading
import time
import traceback

lib = C.CDLL(None, use_errno=True)
call = lib.syscall
call.restype = C.c_long
call.argtypes = [C.c_long, C.c_void_p, C.c_int, C.c_uint,
                 C.c_void_p, C.c_void_p, C.c_uint]
lib.mmap.restype = C.c_void_p
lib.mmap.argtypes = [C.c_void_p, C.c_size_t, C.c_int, C.c_int, C.c_int, C.c_long]
lib.munmap.argtypes = [C.c_void_p, C.c_size_t]
lib.mprotect.argtypes = [C.c_void_p, C.c_size_t, C.c_int]
P = 4096
ALL = 0x7fffffff


class Timespec(C.Structure):
    _fields_ = [('seconds', C.c_long), ('nanoseconds', C.c_long)]


def ts(seconds):
    ns = int(seconds * 1_000_000_000)
    return Timespec(ns // 1_000_000_000, ns % 1_000_000_000)


def futex(address, op, value=0, fourth=0, other=0, bits=0):
    C.set_errno(0)
    result = call(202, address, op, value,
                  C.byref(fourth) if isinstance(fourth, Timespec) else fourth, other, bits)
    return -C.get_errno() if result == -1 else result


def mapping(fd=-1, length=P, offset=0, flags=1, at=0, prot=3):
    address = lib.mmap(at, length, prot, flags | (32 if fd == -1 else 0), fd, offset)
    assert address not in (None, C.c_void_p(-1).value), C.get_errno()
    return address


def word(address):
    return C.c_int.from_address(address)


def spawn(body):
    child = os.fork()
    if child == 0:
        try:
            body()
        except BaseException:
            traceback.print_exc()
            os._exit(1)
        os._exit(0)
    return child


def reap(child):
    result = os.waitpid(child, 0)
    assert result == (child, 0), result


def queued(address, count, flags=0):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        # Same-key non-PI requeue observes the queue without waking it. Unlike
        # sleeping for an arbitrary delay, this proves every waiter is parked.
        n = futex(address, 4 | flags, 0, ALL, address, word(address).value)
        assert n >= 0, (address, n)
        if n == count:
            return
        time.sleep(.001)
    raise AssertionError(('queue size', address, count, n))


def wait(address, bits=None, seconds=10, result=0, flags=0):
    if bits is None:
        got = futex(address, flags, word(address).value, ts(seconds))
    else:
        got = futex(address, 9 | flags, word(address).value,
                    ts(time.monotonic() + seconds), bits=bits)
    assert got == result, ('wait', got, result)


def passed(name):
    print('FUTEX_SHARED_' + name + '_OK', flush=True)


def exec_wait(path):
    fd = os.open(path, os.O_RDWR)
    reserve = mapping(length=16 * P, flags=2, prot=0)
    address = mapping(fd, offset=P, flags=1 | 16, at=reserve + P)
    os.close(fd)
    C.c_uint64.from_address(address + 8).value = address
    wait(address)
    assert word(address + 4).value == 123456
    assert lib.munmap(reserve, 16 * P) == 0


if len(sys.argv) == 3 and sys.argv[1] == '--exec-wait':
    exec_wait(sys.argv[2])
    sys.exit(0)
if len(sys.argv) == 3 and sys.argv[1] == '--exec-retired':
    fd = os.open(sys.argv[2], os.O_RDWR)
    address = mapping(fd, offset=P)
    os.close(fd)
    assert futex(address, 1, ALL) == 0  # old image's sibling no longer exists
    assert lib.munmap(address, P) == 0
    sys.exit(0)

a, b = mapping(), mapping()
# Value checks, malformed arguments, relative and both absolute clock bases.
assert futex(a, 0, 1, ts(0)) == -errno.EAGAIN
assert futex(a, 0, 0, ts(0)) == -errno.ETIMEDOUT
for clockflag, now in ((0, time.monotonic), (256, time.time)):
    started = time.monotonic()
    assert futex(a, 9 | clockflag, 0, ts(now() + .03), bits=1) == -errno.ETIMEDOUT
    assert .015 <= time.monotonic() - started < 1
assert futex(a, 9, 0, ts(0), bits=0) == -errno.EINVAL
assert futex(a, 10, 1, bits=0) == -errno.EINVAL
assert futex(a + 1, 1, 1) == -errno.EINVAL
assert futex(a, 4, -1, 0, b) == -errno.EINVAL
assert futex(a, 3, 0, -1, b) == -errno.EINVAL
assert futex(a, 0, 0, Timespec(-1, 0)) == -errno.EINVAL
assert futex(a, 0, 0, 1) == -errno.EFAULT
for op in (0, 1, 3, 4, 5, 10):
    assert futex(a, op | 256, 0, 0, b, 1) == -errno.ENOSYS
passed('ARGUMENTS_CLOCKS')


def anon_waiter():
    wait(a)
    assert word(a + 4).value == 42


child = spawn(anon_waiter)
queued(a, 1)
assert futex(a, 129, ALL) == 0
word(a + 4).value = 42
assert futex(a, 1, 1) == 1
reap(child)
assert futex(a, 1, ALL) == 0
passed('ANONYMOUS_FORK_VISIBILITY')

# Legacy FUTEX_WAKE tests its signed count after selecting the first waiter.
for count in (0, 0xffffffff):
    child = spawn(lambda: wait(a))
    queued(a, 1)
    assert futex(a, 1, count) == 1
    reap(child)
passed('LEGACY_SIGNED_WAKE_COUNTS')

# Different shared objects must not alias, even at the exact same VA after fork.
def replace_child():
    assert lib.munmap(a, P) == 0
    assert mapping(at=a, flags=1 | 16) == a
    word(b + 4).value = 1
    wait(a, seconds=.15, result=-errno.ETIMEDOUT)
    word(b + 4).value = 2


child = spawn(replace_child)
while word(b + 4).value == 0:
    time.sleep(.001)
while word(b + 4).value != 2:
    assert futex(a, 1, ALL) == 0
    time.sleep(.001)
reap(child)
assert futex(a, 1, ALL) == 0
passed('NEW_BACKING_AT_SAME_ADDRESS')

children = [spawn(lambda bits=bits: wait(a, bits)) for bits in (1, 2, 2)]
queued(a, 3)
assert futex(a, 10, ALL, bits=4) == 0
assert futex(a, 10, 1, bits=1) == 1
reap(children[0])
assert futex(a, 4, 1, 1, b, 7) == -errno.EAGAIN
assert futex(a, 4, 0, 2, b, 0) == 2
assert futex(a, 1, ALL) == 0
assert futex(b, 10, ALL, bits=1) == 0
assert futex(b, 10, 1, bits=2) == 1
assert futex(b, 1, ALL) == 1
for child in children[1:]:
    reap(child)
passed('BITSETS_CMP_REQUEUE_COUNTS')

child = spawn(lambda: wait(a))
queued(a, 1)
assert futex(a, 3, 0, 1, b) == 1
assert futex(b, 1, 1) == 1
reap(child)
child = spawn(lambda: wait(a, seconds=.35, result=-errno.ETIMEDOUT))
queued(a, 1)
assert futex(a, 3, 0, 1, b) == 1
reap(child)
assert futex(b, 1, ALL) == 0
passed('REQUEUE_TIMEOUT_CLEANUP')

# Mixed private/shared keys in one unflagged two-address operation.
local = C.c_int(0)
local_address = C.addressof(local)
thread_errors = []
def local_wait():
    try:
        wait(local_address)
    except BaseException as error:
        thread_errors.append(error)
thread = threading.Thread(target=local_wait)
thread.start()
queued(local_address, 1)
child = spawn(lambda: os._exit(0 if futex(local_address, 1, 1) == 0 else 1))
reap(child)
queued(local_address, 1)
assert futex(local_address, 3, 0, 1, a) == 1
child = spawn(lambda: os._exit(0 if futex(a, 1, 1) == 1 else 1))
reap(child)
thread.join(5)
assert not thread.is_alive() and not thread_errors, thread_errors
passed('MIXED_PRIVATE_SHARED_REQUEUE')

# Wake-op uses the old signed word and atomically updates the shared destination.
children = [spawn(lambda address=address: wait(address)) for address in (a, b)]
queued(a, 1)
queued(b, 1)
assert futex(a, 5, 1, 1, b + 1, 7 << 12) == -errno.EINVAL
assert word(b).value == 0
assert futex(a, 5, 1, 1, b, 7 << 12) == 2
assert word(b).value == 7
for child in children:
    reap(child)
word(b).value = 0
children = [spawn(lambda: wait(a)) for _ in range(3)]
queued(a, 3)
assert futex(a, 5, 1, 1, a, 7 << 12) == 2
assert futex(a, 1, ALL) == 1
for child in children:
    reap(child)
word(a).value = 0
passed('WAKE_OP_TWO_KEYS_AND_SAME_KEY')

# A killed task must neither inflate wake counts nor consume future capacity.
for _ in range(3):
    child = spawn(lambda: wait(a))
    queued(a, 1)
    os.kill(child, signal.SIGKILL)
    assert os.waitpid(child, 0)[1] != 0
    assert futex(a, 1, ALL) == 0
passed('KILLED_WAITER_CLEANUP')

fixture = C.CDLL(str(Path(__file__).with_suffix('.so')))
fixture.install_handler.argtypes = [C.c_void_p, C.c_int]


def signal_waiter(restart, address):
    assert fixture.install_handler(address, restart) == 0
    started = time.monotonic()
    wait(address, seconds=.5, result=-errno.ETIMEDOUT if restart else -errno.EINTR)
    assert fixture.handler_calls() >= 1
    assert fixture.handler_nested_wake() == 0
    if restart:
        assert .4 <= time.monotonic() - started < 1.5


for restart in (0, 1):
    child = spawn(lambda restart=restart: signal_waiter(restart, a))
    queued(a, 1)
    assert futex(a, 3, 0, 1, b) == 1
    os.kill(child, signal.SIGUSR1)
    reap(child)
    assert futex(a, 1, ALL) == futex(b, 1, ALL) == 0
passed('SIGNAL_REENTRY_RESTART_REQUEUED_CLEANUP')

path = str(Path(__file__).with_suffix('.data'))
fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_EXCL, 0o600)
os.ftruncate(fd, 3 * P)
whole = mapping(fd, length=3 * P)
alias = mapping(fd, offset=P)
private_alias = mapping(fd, offset=P, flags=2)
os.close(fd)
assert whole + P != alias
private_results = []
thread = threading.Thread(target=lambda: private_results.append(futex(private_alias, 0, 0, ts(5))))
thread.start()
queued(private_alias, 1)
assert futex(alias, 1, ALL) == 0
assert futex(private_alias, 1, 1) == 1
thread.join(5)
assert not thread.is_alive() and private_results == [0], private_results
assert lib.munmap(private_alias, P) == 0
link = path + '.link'
os.link(path, link)
child = spawn(lambda: os.execv('/usr/bin/python3.11', ['/usr/bin/python3.11', __file__, '--exec-wait', link]))
queued(alias, 1)
assert C.c_uint64.from_address(whole + P + 8).value not in (alias, whole + P)
word(whole + P + 4).value = 123456
assert futex(whole, 1, ALL) == 0  # same inode, different page
assert futex(alias + 4, 1, ALL) == 0  # same page, different word
assert futex(whole + P, 1, 1) == 1
reap(child)


def exec_with_waiting_sibling():
    threading.Thread(target=lambda: wait(whole + P)).start()
    queued(whole + P, 1)
    os.execv('/usr/bin/python3.11', ['/usr/bin/python3.11', __file__, '--exec-retired', path])


reap(spawn(exec_with_waiting_sibling))
assert lib.mprotect(alias, P, 1) == 0
assert futex(alias, 0, 0, ts(0)) == -errno.ETIMEDOUT
assert futex(a, 5, 1, 1, alias, 7 << 12) == -errno.EFAULT
assert lib.mprotect(alias, P, 3) == 0
assert word(alias).value == 0
# Reuse a native allocation-granularity base: MAP_SHARED anonymous MAP_FIXED
# currently cannot recreate the 4 KiB-skewed file alias itself. The sleeping
# word is still the exact same VA in the replacement object.
alias_results = []
thread = threading.Thread(target=lambda: alias_results.append(futex(whole + P, 0, 0, ts(5))))
thread.start()
queued(alias, 1)
assert lib.munmap(whole, 3 * P) == 0
assert mapping(at=whole, length=3 * P, flags=1 | 16) == whole
assert futex(whole + P, 1, ALL) == 0
assert futex(alias, 1, 1) == 1
thread.join(5)
assert not thread.is_alive() and alias_results == [0], alias_results
assert lib.munmap(alias, P) == 0
assert lib.munmap(whole, 3 * P) == 0
os.unlink(link)
os.unlink(path)
passed('FILE_ALIASES_OFFSETS_CLOSED_FD_HARDLINK_EXEC')

fd = os.memfd_create('futex-shared')
os.ftruncate(fd, 2 * P)
first = mapping(fd, length=2 * P)
second = mapping(fd, offset=P)
os.close(fd)
child = spawn(lambda: wait(first + P))
queued(second, 1)
assert futex(second, 1, 1) == 1
reap(child)
assert lib.munmap(first, 2 * P) == lib.munmap(second, P) == 0
passed('MEMFD_ALIASES')

# Repeated compare/park vs store/wake races must never lose a transition.
ROUNDS = 150
def ping_pong_child():
    for i in range(1, ROUNDS + 1):
        while word(a).value < i:
            observed = word(a).value
            if observed >= i:
                break
            assert futex(a, 0, observed, ts(3)) in (0, -errno.EAGAIN)
        word(b).value = i
        assert futex(b, 1, 1) in (0, 1)
child = spawn(ping_pong_child)
for i in range(1, ROUNDS + 1):
    word(a).value = i
    assert futex(a, 1, 1) in (0, 1)
    while word(b).value < i:
        observed = word(b).value
        if observed >= i:
            break
        assert futex(b, 0, observed, ts(3)) in (0, -errno.EAGAIN)
reap(child)
assert futex(a, 1, ALL) == futex(b, 1, ALL) == 0
assert lib.munmap(a, P) == lib.munmap(b, P) == 0
passed('150_PROCESS_HANDSHAKES')
print('FUTEX_SHARED_ALL_OK', flush=True)
