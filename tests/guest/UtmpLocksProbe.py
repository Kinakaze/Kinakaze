"""Real utmp records, byte-range locks and shared file-position regressions."""
import ctypes as C
import array
import errno
import json
import os
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import traceback

lib = C.CDLL(None, use_errno=True)


class Lock(C.Structure):
    _fields_ = [('type', C.c_short), ('whence', C.c_short),
                ('start', C.c_longlong), ('length', C.c_longlong), ('pid', C.c_int)]


assert C.sizeof(Lock) == 32
lib.fcntl.argtypes = [C.c_int, C.c_int, C.POINTER(Lock)]
lib.fcntl.restype = C.c_int
lib.syscall.restype = C.c_long
lib.utmpxname.argtypes = [C.c_char_p]
lib.pututxline.argtypes = [C.c_void_p]
lib.pututxline.restype = C.c_void_p
lib.updwtmpx.argtypes = [C.c_char_p, C.c_void_p]
lib.getutxent.restype = C.c_void_p


def lock(fd, command, kind=1, whence=0, start=0, length=0, pid=0, error=None, raw=False):
    request = Lock(kind, whence, start, length, pid)
    C.set_errno(0)
    result = (lib.syscall(72, fd, command, C.byref(request)) if raw else
              lib.fcntl(fd, command, C.byref(request)))
    actual = C.get_errno() if result < 0 else None
    assert actual == error, (command, kind, whence, start, length, pid, actual, error)
    return request


def wait(pid):
    child, status = os.waitpid(pid, 0)
    assert child == pid and status == 0, (pid, status)


def receive(fd, expected=b'R'):
    assert select.select([fd], [], [], 10)[0], 'child did not reply'
    actual = os.read(fd, len(expected))
    assert actual == expected, (actual, expected)


def child_body(fn):
    try:
        fn()
    except BaseException:
        traceback.print_exc()
        os._exit(1)
    os._exit(0)


def records(directory):
    fd, path = tempfile.mkstemp(prefix='utmp-lock-probe-', dir=directory)
    os.write(fd, b'\0' * 1152)
    return fd, path


def matrix(directory):
    fd, path = records(directory)
    try:
        for raw in [False, True]:
            for command in [5, 6, 7, 36, 37, 38]:
                for whence, start, length in [(0, 0, 0), (0, 384, 384), (1, -384, 384),
                                              (1, 384, -384), (2, -384, 384), (2, 0, -384)]:
                    os.lseek(fd, 384, 0)
                    lock(fd, command, whence=whence, start=start, length=length, raw=raw)
                    lock(fd, 37 if command >= 36 else 6, kind=2)
                    assert os.lseek(fd, 0, 1) == 384
        for command in [36, 37, 38]:
            lock(fd, command, pid=1, error=errno.EINVAL)
        for command in [6, 37]:
            lock(fd, command, start=-1, error=errno.EINVAL)
            lock(fd, command, start=0, length=-1, error=errno.EINVAL)
            lock(fd, command, whence=3, error=errno.EINVAL)
            lock(fd, command, start=2**63-1, length=2, error=errno.EOVERFLOW)
        assert os.lseek(fd, 384, os.SEEK_DATA) == 384
        assert os.lseek(fd, 384, os.SEEK_HOLE) == 1152
        for whence in [os.SEEK_DATA, os.SEEK_HOLE]:
            for offset, expected in [(-1, errno.EINVAL), (1152, errno.ENXIO), (4096, errno.ENXIO)]:
                try:
                    os.lseek(fd, offset, whence)
                    raise AssertionError('invalid seek succeeded')
                except OSError as exc:
                    assert exc.errno == expected, (exc.errno, expected)
                assert os.lseek(fd, 0, 1) == 1152
    finally:
        os.close(fd)
        os.unlink(path)


def posix_ranges(directory):
    fd, path = records(directory)
    try:
        os.lseek(fd, 768, 0)
        lock(fd, 6, whence=1, start=0, length=-384)
        # Query from another Linux process: return the absolute range and PID.
        parent = os.getpid()
        pid = os.fork()
        if pid == 0:
            def query():
                request = lock(fd, 5)
                assert (request.type, request.whence, request.start, request.length, request.pid) == (1, 0, 384, 384, parent)
                lock(fd, 6, start=384, length=1, error=errno.EAGAIN)
                # Child file-position changes must be visible to its parent.
                os.lseek(fd, 100, 0)
            child_body(query)
        wait(pid)
        assert os.lseek(fd, 0, 1) == 100
        lock(fd, 6, kind=2)
        os.lseek(fd, 1152, 0)
        lock(fd, 6, whence=2, start=0, length=-384)
        # POSIX rule: closing ANY descriptor for this inode releases our locks.
        os.close(os.open(path, os.O_RDWR))
        pid = os.fork()
        if pid == 0:
            child_body(lambda: lock(fd, 6))
        wait(pid)
    finally:
        os.close(fd)
        os.unlink(path)


def ofd_lifetime(directory):
    fd, path = records(directory)
    alias = os.dup(fd)
    other = os.open(path, os.O_RDWR)
    try:
        lock(fd, 37, length=1152)
        assert lock(alias, 36).type == 2
        for command in [5, 36]:
            request = lock(other, command)
            assert (request.type, request.start, request.length, request.pid) == (1, 0, 1152, -1)
        lock(other, 6, error=errno.EAGAIN)
        lock(other, 37, error=errno.EAGAIN)
        lock(alias, 37, kind=2, start=384, length=384)
        assert lock(other, 36, start=384, length=384).type == 2
        request = lock(other, 36, start=768, length=384)
        assert (request.start, request.length) == (768, 384)
        lock(alias, 37, length=1152)
        os.close(other)
        other = -1
        ready_r, ready_w = os.pipe()
        release_r, release_w = os.pipe()
        pid = os.fork()
        if pid == 0:
            def hold():
                os.close(ready_r)
                os.close(release_w)
                os.close(fd)
                assert lock(alias, 36).type == 2
                os.write(ready_w, b'R')
                receive(release_r)
                os.close(alias)
            child_body(hold)
        os.close(ready_w)
        os.close(release_r)
        receive(ready_r)
        os.close(fd)
        fd = -1
        os.close(alias)
        alias = -1
        other = os.open(path, os.O_RDWR)
        assert lock(other, 36).pid == -1
        os.write(release_w, b'R')
        wait(pid)
        os.close(ready_r)
        os.close(release_w)
        assert lock(other, 36).type == 2
    finally:
        for item in [fd, alias, other]:
            if item >= 0:
                os.close(item)
        os.unlink(path)


def ofd_exec(directory):
    fd, path = records(directory)
    try:
        lock(fd, 37)
        with subprocess.Popen([sys.executable, __file__, '--inherited', str(fd)], pass_fds=(fd,),
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE) as child:
            receive(child.stdout.fileno())
            os.close(fd)
            fd = -1
            other = os.open(path, os.O_RDWR)
            try:
                assert lock(other, 36).pid == -1
                child.stdin.write(b'R')
                child.stdin.flush()
                assert child.wait(timeout=10) == 0
                assert lock(other, 36).type == 2
            finally:
                os.close(other)
    finally:
        if fd >= 0:
            os.close(fd)
        os.unlink(path)


def ofd_holder_death(directory):
    fd, path = records(directory)
    ready_r, ready_w = os.pipe()
    pid = os.fork()
    if pid == 0:
        def hold():
            os.close(ready_r)
            separate = os.open(path, os.O_RDWR)
            lock(separate, 37)
            os.write(ready_w, b'R')
            signal.pause()
        child_body(hold)
    os.close(ready_w)
    try:
        receive(ready_r)
        assert lock(fd, 36).pid == -1
        # A separate waiter must wake after abrupt last-holder death.
        waiter = os.fork()
        if waiter == 0:
            child_body(lambda: lock(fd, 38))
        os.kill(pid, signal.SIGKILL)
        killed, status = os.waitpid(pid, 0)
        assert killed == pid and os.WIFSIGNALED(status)
        wait(waiter)
    finally:
        os.close(ready_r)
        os.close(fd)
        os.unlink(path)


def ofd_rights(directory):
    fd, path = records(directory)
    sender, receiver = socket.socketpair()
    lock(fd, 37)
    pid = os.fork()
    if pid == 0:
        def transferred():
            sender.close()
            os.close(fd)
            data, controls, flags, address = receiver.recvmsg(1, socket.CMSG_SPACE(4))
            assert data == b'F' and len(controls) == 1
            level, kind, payload = controls[0]
            assert (level, kind) == (socket.SOL_SOCKET, socket.SCM_RIGHTS)
            received = array.array('i')
            received.frombytes(payload[:4])
            assert lock(received[0], 36).type == 2
            receiver.sendall(b'R')
            assert receiver.recv(1) == b'R'
            os.close(received[0])
            receiver.close()
        child_body(transferred)
    receiver.close()
    other = -1
    try:
        sender.sendmsg([b'F'], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
        assert sender.recv(1) == b'R'
        os.close(fd)
        fd = -1
        other = os.open(path, os.O_RDWR)
        assert lock(other, 36).pid == -1
        sender.sendall(b'R')
        wait(pid)
        assert lock(other, 36).type == 2
    finally:
        sender.close()
        for item in [fd, other]:
            if item >= 0:
                os.close(item)
        os.unlink(path)


def record(identity, pid=1234):
    data = bytearray(384)
    struct.pack_into('h', data, 0, 7)
    struct.pack_into('i', data, 4, pid)
    data[8:13] = b'pts/1'
    data[40:44] = identity.encode().ljust(4, b'\0')
    data[44:49] = b'probe'
    return C.create_string_buffer(bytes(data), 384)


def utmp_database(directory):
    fd, path = tempfile.mkstemp(prefix='utmp-db-probe-', dir=directory)
    wtmp = path + '.wtmp'
    ready_r, ready_w = os.pipe()
    done_r, done_w = os.pipe()
    lock(fd, 6)
    pid = os.fork()
    if pid == 0:
        def write_locked():
            os.close(ready_r)
            os.close(done_r)
            os.close(fd)
            assert lib.utmpxname(path.encode()) == 0
            os.write(ready_w, b'R')
            assert lib.pututxline(record('hold')), C.get_errno()
            os.write(done_w, b'R')
        child_body(write_locked)
    os.close(ready_w)
    os.close(done_w)
    try:
        receive(ready_r)
        premature = bool(select.select([done_r], [], [], .2)[0])
        lock(fd, 6, kind=2)
        receive(done_r)
        wait(pid)
        assert not premature, 'pututxline bypassed the external fcntl lock'
        children = []
        for worker in range(4):
            pid = os.fork()
            if pid == 0:
                def append():
                    os.close(fd)
                    assert lib.utmpxname(path.encode()) == 0
                    for index in range(8):
                        item = record('%d%02d' % (worker, index), 2000 + worker)
                        assert lib.pututxline(item), C.get_errno()
                        C.set_errno(0)
                        lib.updwtmpx(wtmp.encode(), item)
                        assert C.get_errno() == 0, C.get_errno()
                child_body(append)
            children.append(pid)
        for pid in children:
            wait(pid)
        assert lib.utmpxname(path.encode()) == 0
        assert lib.pututxline(record('hold', 9999)), C.get_errno()
        lib.setutxent()
        observed = {}
        while True:
            pointer = lib.getutxent()
            if not pointer:
                break
            data = C.string_at(pointer, 384)
            identity = data[40:44]
            assert identity not in observed
            observed[identity] = struct.unpack_from('i', data, 4)[0]
        lib.endutxent()
        assert len(observed) == 33, len(observed)
        assert observed[b'hold'] == 9999
        assert os.stat(path).st_size == 33 * 384
        assert os.stat(wtmp).st_size == 32 * 384
        # Also exercise installed readers against these private fixture files.
        for command in [('/usr/bin/utmpdump', path), ('/usr/bin/who', path),
                        ('/usr/bin/last', '-f', wtmp, '-n', '3')]:
            if os.path.exists(command[0]):
                output = subprocess.run(command, capture_output=True, text=True, timeout=10)
                assert output.returncode == 0, (command, output.stderr)
                assert 'probe' in output.stdout, (command, output.stdout)
                print(json.dumps(dict(reader=command[0], directory=directory, passed=True)), flush=True)
    finally:
        for item in [fd, ready_r, done_r]:
            os.close(item)
        os.unlink(path)
        if os.path.exists(wtmp):
            os.unlink(wtmp)


if len(sys.argv) > 1 and sys.argv[1] == '--inherited':
    fd = int(sys.argv[2])
    assert lock(fd, 36).type == 2
    os.write(1, b'R')
    receive(0)
    os.close(fd)
    raise SystemExit(0)

results = []
for directory in ['/tmp', '/run', '/dev/shm']:
    for probe in [matrix, posix_ranges, ofd_lifetime, ofd_exec, ofd_holder_death, ofd_rights, utmp_database]:
        try:
            probe(directory)
            result = dict(probe=probe.__name__, directory=directory, passed=True)
        except BaseException as exc:
            result = dict(probe=probe.__name__, directory=directory, passed=False, error=repr(exc))
            traceback.print_exc()
        results.append(result)
        print(json.dumps(result), flush=True)
print(json.dumps(dict(passed=all(row['passed'] for row in results), results=results)), flush=True)
raise SystemExit(int(not all(row['passed'] for row in results)))
