"""Exercise real dup/dup2/dup3/F_DUPFD ownership and replacement semantics."""
import ctypes as c
import errno
import fcntl
import json
import os
from pathlib import Path
import resource
import select
import socket
import subprocess
import sys
import tempfile
import threading
import time
import traceback

libc = c.CDLL('libc.so.6', use_errno=True)
rows = []


def checked(value):
    if value < 0:
        raise OSError(c.get_errno(), os.strerror(c.get_errno()))
    return value


def fails(code, action):
    try:
        action()
    except OSError as error:
        assert error.errno == code, error
    else:
        raise AssertionError('operation unexpectedly succeeded')


def check(name, action):
    try:
        action()
        row = dict(name=name, status='passed')
    except Exception as error:
        row = dict(name=name, status='failed', error=repr(error), traceback=traceback.format_exc())
    rows.append(row)
    print(json.dumps(row), flush=True)


with tempfile.TemporaryDirectory(prefix='duplication-') as temporary:
    root = Path(temporary)
    path = root / 'file'
    path.write_bytes(b'abcdef')

    if sys.argv[1:] == ['--benchmark']:
        source = os.open(path, os.O_RDONLY)
        target = os.open('/dev/null', os.O_RDWR)
        try:
            for name in ('dup-close', 'dup2-replace'):
                started = time.perf_counter()
                for _ in range(20000):
                    if name == 'dup-close':
                        os.close(checked(libc.dup(source)))
                    else:
                        checked(libc.dup2(source, target))
                print(json.dumps(dict(name=name, count=20000, milliseconds=(time.perf_counter()-started)*1000)), flush=True)
        finally:
            os.close(source)
            os.close(target)
        sys.exit(0)

    def file_offset():
        original = os.open(path, os.O_RDONLY)
        copy = os.dup(original)
        try:
            assert os.read(original, 2) == b'ab'
            assert os.read(copy, 2) == b'cd'
        finally:
            os.close(original)
        try:
            assert os.read(copy, 2) == b'ef'
        finally:
            os.close(copy)
    check('native-shared-offset-and-last-close', file_offset)

    def flags_and_failures():
        source = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
        target = os.open('/dev/null', os.O_RDWR)
        try:
            before = os.fstat(target).st_mode
            # CPython 3.11's negative-fd shortcut raises its existing errno
            # without calling dup2. Check the ABI directly, then a closed fd.
            fails(errno.EBADF, lambda: checked(libc.dup2(-1, target)))
            fails(errno.EBADF, lambda: os.dup2(7777, target))
            assert os.fstat(target).st_mode == before
            fails(errno.EINVAL, lambda: checked(libc.dup3(source, target, 1)))
            assert os.fstat(target).st_mode == before
            fails(errno.EINVAL, lambda: checked(libc.dup3(source, source, os.O_CLOEXEC)))
            assert checked(libc.dup2(source, source)) == source
            assert not os.get_inheritable(source)
            assert checked(libc.dup2(source, target)) == target
            assert os.get_inheritable(target)
            assert checked(libc.dup3(source, target, os.O_CLOEXEC)) == target
            assert not os.get_inheritable(target)
            fails(errno.EINVAL, lambda: fcntl.fcntl(source, fcntl.F_DUPFD, -1))
            copy = fcntl.fcntl(source, fcntl.F_DUPFD_CLOEXEC, 80)
            try:
                assert copy >= 80 and not os.get_inheritable(copy)
                assert os.read(copy, 1) == b'a'
            finally:
                os.close(copy)
        finally:
            os.close(source)
            os.close(target)
    check('flags-invalid-source-and-target-survival', flags_and_failures)

    def atomic_flags():
        source = os.open(path, os.O_RDONLY)
        target = os.dup(source)
        errors = []
        start = threading.Barrier(2)
        def replace():
            try:
                start.wait()
                for _ in range(1500):
                    checked(libc.dup3(source, target, os.O_CLOEXEC))
            except Exception as error:
                errors.append(repr(error))
        writer = threading.Thread(target=replace)
        writer.start()
        try:
            start.wait()
            for _ in range(3000):
                # ctypes releases the GIL here, so reads really overlap the
                # other native thread's dup3, rather than serializing Python.
                flags = checked(libc.fcntl(target, fcntl.F_GETFD, 0))
                assert flags & fcntl.FD_CLOEXEC, 'CLOEXEC missing during replacement'
        finally:
            writer.join()
            os.close(source)
            os.close(target)
        assert not errors, errors
    check('concurrent-dup3-publishes-cloexec-atomically', atomic_flags)

    def exhausted():
        original_limit = resource.getrlimit(resource.RLIMIT_NOFILE)
        held = []
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (64, original_limit[1]))
            source = os.open(path, os.O_RDONLY)
            held.append(source)
            while True:
                try:
                    held.append(os.open('/dev/null', os.O_RDONLY))
                except OSError as error:
                    assert error.errno == errno.EMFILE
                    break
            fails(errno.EMFILE, lambda: os.dup(source))
            assert checked(libc.dup2(source, held[-1])) == held[-1]
            assert os.read(held[-1], 3) == b'abc'
            fails(errno.EBADF, lambda: checked(libc.dup2(source, 64)))
        finally:
            for fd in held:
                os.close(fd)
            resource.setrlimit(resource.RLIMIT_NOFILE, original_limit)
    check('replacement-with-exhausted-descriptor-limit', exhausted)

    def lowered_limit():
        source = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
        limits = resource.getrlimit(resource.RLIMIT_NOFILE)
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (0, limits[1]))
            # Lowering a limit never invalidates existing descriptors. dup2's
            # no-op still succeeds; dup and F_DUPFD have different errors.
            assert checked(libc.dup2(source, source)) == source
            assert not os.get_inheritable(source)
            fails(errno.EMFILE, lambda: checked(libc.dup(source)))
            fails(errno.EINVAL, lambda: fcntl.fcntl(source, fcntl.F_DUPFD, 0))
        finally:
            resource.setrlimit(resource.RLIMIT_NOFILE, limits)
            os.close(source)
    check('lowered-limit-noop-and-dup-error-boundaries', lowered_limit)

    def opath():
        source = os.open(path, os.O_PATH)
        copy = os.dup(source)
        os.close(source)
        try:
            assert os.fstat(copy).st_size == 6
            fails(errno.EBADF, lambda: os.read(copy, 1))
        finally:
            os.close(copy)
    check('opath-preserves-restrictions', opath)

    def pipes():
        reader, writer = os.pipe()
        copy = os.dup(writer)
        os.close(writer)
        try:
            assert os.write(copy, b'pipe') == 4
            assert os.read(reader, 4) == b'pipe'
            os.close(copy)
            copy = -1
            assert os.read(reader, 1) == b''
        finally:
            os.close(reader)
            if copy >= 0:
                os.close(copy)
    check('pipe-retained-writer-and-eof', pipes)

    def fifo():
        fifo_path = root / 'fifo'
        os.mkfifo(fifo_path)
        source = os.open(fifo_path, os.O_RDWR | os.O_NONBLOCK)
        target = os.open(fifo_path, os.O_RDWR | os.O_NONBLOCK)
        try:
            os.dup2(source, target)
            os.close(source)
            source = -1
            os.write(target, b'fifo')
            assert os.read(target, 4) == b'fifo'
            with open(path, 'rb') as native:
                os.dup2(native.fileno(), target)
            assert os.read(target, 3) == b'abc'
        finally:
            os.close(target)
            if source >= 0:
                os.close(source)
    check('fifo-replace-and-retire-marker', fifo)

    def unix_pair():
        first, peer = socket.socketpair()
        target, oldpeer = socket.socketpair()
        try:
            os.dup2(first.fileno(), target.fileno())
            first.close()
            target.sendall(b'unix')
            assert peer.recv(4) == b'unix'
            oldpeer.settimeout(2)
            assert oldpeer.recv(1) == b''
        finally:
            for sock in (first, peer, target, oldpeer):
                sock.close()
    check('unix-connected-replacement', unix_pair)

    def unix_unbound():
        with socket.socket(socket.AF_UNIX) as original:
            with original.dup() as copy:
                copy.bind(str(root / 'unix'))
                assert original.getsockname() == copy.getsockname()
                original.listen(4)
                original.close()
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(2)
                    client.connect(str(root / 'unix'))
                    accepted, _ = copy.accept()
                    with accepted:
                        client.sendall(b'bind')
                        assert accepted.recv(4) == b'bind'
    check('unix-unbound-shares-bind-listen-state', unix_unbound)

    def unix_connect():
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(root / 'connect'))
            listener.listen(4)
            with socket.socket(socket.AF_UNIX) as original, original.dup() as copy:
                copy.connect(str(root / 'connect'))
                accepted, _ = listener.accept()
                with accepted:
                    assert original.getpeername() == copy.getpeername()
                    original.sendall(b'alias')
                    assert accepted.recv(5) == b'alias'
                    copy.close()
                    original.sendall(b'live')
                    assert accepted.recv(4) == b'live'
    check('unix-unbound-shares-connect-and-native-lifetime', unix_connect)

    def inet_socket(kind):
        with socket.socket(socket.AF_INET, kind) as original:
            original.bind(('127.0.0.1', 0))
            address = original.getsockname()
            if kind == socket.SOCK_STREAM:
                original.listen(4)
            with original.dup() as copy:
                original.close()
                copy.settimeout(2)
                with socket.socket(socket.AF_INET, kind) as peer:
                    peer.settimeout(2)
                    if kind == socket.SOCK_DGRAM:
                        peer.sendto(b'udp', address)
                        assert copy.recv(3) == b'udp'
                    else:
                        peer.connect(address)
                        accepted, _ = copy.accept()
                        with accepted:
                            peer.sendall(b'tcp')
                            assert accepted.recv(3) == b'tcp'
    check('tcp-provider-owned-copy', lambda: inet_socket(socket.SOCK_STREAM))
    check('udp-provider-owned-copy', lambda: inet_socket(socket.SOCK_DGRAM))

    def synthetic():
        target = os.open('/dev/null', os.O_RDONLY)
        try:
            for pathname in ('/proc/version', '/proc/sys/kernel/hostname', '/sys/fs/cgroup/cgroup.controllers'):
                source = os.open(pathname, os.O_RDONLY)
                try:
                    expected = os.read(source, 4096)
                    os.lseek(source, 0, os.SEEK_SET)
                    os.dup2(source, target)
                finally:
                    os.close(source)
                assert os.read(target, 4096) == expected, pathname
            with open(path, 'rb') as native:
                os.dup2(native.fileno(), target)
            assert os.read(target, 3) == b'abc'
        finally:
            os.close(target)
    check('synthetic-sysctl-cgroup-publication-and-replacement', synthetic)

    def events():
        source = os.eventfd(3, os.EFD_NONBLOCK)
        copy = os.dup(source)
        ep = select.epoll()
        twin = select.epoll.fromfd(os.dup(ep.fileno()))
        try:
            ep.register(copy, select.EPOLLIN)
            ep.close()
            os.close(source)
            source = -1
            assert twin.poll(1) == [(copy, select.EPOLLIN)]
            assert os.eventfd_read(copy) == 3
            assert twin.poll(0) == []
        finally:
            ep.close()
            twin.close()
            os.close(copy)
            if source >= 0:
                os.close(source)
    check('eventfd-and-epoll-shared-description', events)

    def cloexec():
        source = os.open(path, os.O_RDONLY)
        target = checked(libc.dup3(source, 90, os.O_CLOEXEC))
        try:
            child = subprocess.run(['/usr/bin/python3', '-c',
                'import os,errno\ntry: os.fstat(90)\nexcept OSError as e: assert e.errno==errno.EBADF\nelse: raise AssertionError("leaked dup3")'],
                close_fds=False, capture_output=True, timeout=10)
            assert child.returncode == 0, child.stderr
        finally:
            os.close(source)
            os.close(target)
    check('dup3-cloexec-survives-exec-snapshot', cloexec)

print(json.dumps(dict(passed=sum(r['status'] == 'passed' for r in rows), total=len(rows))), flush=True)
sys.exit(any(row['status'] != 'passed' for row in rows))
