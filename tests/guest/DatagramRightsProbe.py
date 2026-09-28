"""Exercise queued AF_UNIX rights across close, exit, peek and resource pressure."""
import array
import contextlib
import ctypes
import errno
import fcntl
import json
import os
import select
import signal
import socket
import struct
import tempfile
import time


@contextlib.contextmanager
def endpoints(pathname=False):
    with tempfile.TemporaryDirectory(prefix='dgram-rights-', dir='.') as directory:
        address = os.path.abspath(directory + '/socket') if pathname else '\0' + directory + str(os.getpid())
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as receiver, socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as sender:
            receiver.bind(address)
            receiver.settimeout(4)
            yield sender, receiver, address


def send(sender, address, fds, data=b'x'):
    return sender.sendmsg([data], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', fds))], 0, address)


def receive(receiver, size=4096, control=4096, flags=0):
    data, ancillary, out_flags, source = receiver.recvmsg(size, control, flags)
    descriptors = []
    for level, kind, values in ancillary:
        if (level, kind) == (socket.SOL_SOCKET, socket.SCM_RIGHTS):
            values = values[:len(values)//4*4]
            descriptors.extend(array.array('i', values))
    return data, descriptors, out_flags, ancillary


@contextlib.contextmanager
def received(receiver, **kwargs):
    result = receive(receiver, **kwargs)
    try:
        yield result
    finally:
        for descriptor in result[1]:
            os.close(descriptor)


def file_transfer(pathname=False, empty=False):
    with endpoints(pathname) as (sender, receiver, address), tempfile.TemporaryFile(dir='.') as original:
        original.write(b'abcdef')
        original.flush()
        original.seek(2)
        send(sender, address, [original.fileno()], b'' if empty else b'packet')
        with received(receiver) as (data, fds, flags, _):
            assert data == (b'' if empty else b'packet') and flags == 0 and len(fds) == 1, (data, fds, flags)
            assert os.read(fds[0], 2) == b'cd'
            assert original.tell() == 4
            assert not fcntl.fcntl(fds[0], fcntl.F_GETFD) & fcntl.FD_CLOEXEC


def sender_exit():
    with endpoints() as (sender, receiver, address):
        pid = os.fork()
        if pid == 0:
            try:
                receiver.close()
                with tempfile.TemporaryFile(dir='.') as original:
                    original.write(b'after-exit'); original.flush(); original.seek(0)
                    send(sender, address, [original.fileno()])
                os._exit(0)
            except BaseException:
                os._exit(90)
        assert os.waitpid(pid, 0)[1] == 0
        with received(receiver, flags=socket.MSG_CMSG_CLOEXEC) as (_, fds, flags, _):
            assert len(fds) == 1 and not flags & socket.MSG_CTRUNC, (fds, flags)
            assert os.read(fds[0], 32) == b'after-exit'
            assert fcntl.fcntl(fds[0], fcntl.F_GETFD) & fcntl.FD_CLOEXEC


def peek():
    with endpoints() as (sender, receiver, address), tempfile.TemporaryFile(dir='.') as original:
        original.write(b'abc'); original.flush(); original.seek(0)
        send(sender, address, [original.fileno()], b'long')
        for expected, flags in [(b'a', socket.MSG_PEEK), (b'b', socket.MSG_PEEK), (b'c', 0)]:
            with received(receiver, size=2, flags=flags) as (data, fds, out_flags, _):
                assert data == b'lo' and out_flags & socket.MSG_TRUNC
                assert len(fds) == 1 and os.read(fds[0], 1) == expected, fds
        receiver.setblocking(False)
        try: receiver.recv(1)
        except BlockingIOError: pass
        else: raise AssertionError('peek duplicated the queued message')


def assert_eof(fd):
    assert select.select([fd], [], [], 4)[0], 'queued writer leaked'
    assert os.read(fd, 1) == b'', 'writer still open'


def discard(mode):
    with endpoints() as (sender, receiver, address):
        read, write = os.pipe()
        try:
            send(sender, address, [write]); os.close(write); write = -1
            assert not select.select([read], [], [], 0)[0]
            if mode == 'close': receiver.close()
            elif mode == 'shutdown': receiver.shutdown(socket.SHUT_RD)
            else:
                data, fds, flags, _ = receive(receiver, control=0)
                assert data == b'x' and not fds and flags & socket.MSG_CTRUNC
            assert_eof(read)
        finally:
            os.close(read)
            if write >= 0: os.close(write)


def control_truncation():
    with endpoints() as (sender, receiver, address), tempfile.TemporaryFile(dir='.') as original:
        send(sender, address, [original.fileno()] * 4)
        with received(receiver, control=socket.CMSG_LEN(4)) as (_, fds, flags, _):
            assert len(fds) == 1 and flags & socket.MSG_CTRUNC, (fds, flags)


def credentials():
    with endpoints() as (sender, receiver, address), tempfile.TemporaryFile(dir='.') as original:
        receiver.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
        send(sender, address, [original.fileno()])
        with received(receiver) as (_, fds, flags, ancillary):
            assert len(fds) == 1 and flags == 0, (fds, flags, ancillary)
            cred = [struct.unpack('3i', value) for level, kind, value in ancillary if kind == socket.SCM_CREDENTIALS]
            assert cred == [(os.getpid(), os.getuid(), os.getgid())], cred


def invalid_send():
    with endpoints() as (sender, receiver, address), tempfile.TemporaryFile(dir='.') as original:
        try: send(sender, address, [original.fileno(), 100000])
        except OSError as error: assert error.errno == errno.EBADF, error
        else: raise AssertionError('invalid rights accepted')
        receiver.setblocking(False)
        try: receiver.recv(1)
        except BlockingIOError: pass
        else: raise AssertionError('failed send published payload')


def eventfd():
    with endpoints() as (sender, receiver, address):
        fd = os.eventfd(7)
        try: send(sender, address, [fd])
        finally: os.close(fd)
        with received(receiver) as (_, fds, flags, _):
            assert len(fds) == 1 and flags == 0, (fds, flags)
            assert os.eventfd_read(fds[0]) == 7


def socket_transfer(kind):
    with endpoints() as (sender, receiver, address):
        if kind == 'unix-pair':
            source, other = socket.socketpair()
        elif kind == 'udp':
            source, other = socket.socket(socket.AF_INET, socket.SOCK_DGRAM), socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            source.bind(('127.0.0.1', 0)); other.bind(('127.0.0.1', 0))
        else:
            source = socket.socket(socket.AF_UNIX)
            source.bind('\0fdstore-listener-' + str(os.getpid()))
            source.listen(4)
            other = socket.socket(socket.AF_UNIX)
        with source, other:
            saved = source.getsockname()
            send(sender, address, [source.fileno()])
            source.close()
            with received(receiver) as (_, fds, flags, _):
                assert len(fds) == 1 and flags == 0, (fds, flags)
                with socket.socket(fileno=os.dup(fds[0])) as copy:
                    copy.settimeout(3)
                    if kind == 'udp': other.sendto(b'hello', saved)
                    elif kind == 'unix-listener':
                        other.connect(saved); other.sendall(b'hello')
                        connection, _ = copy.accept()
                        with connection: assert connection.recv(5) == b'hello'
                        return
                    else: other.sendall(b'hello')
                    assert copy.recv(5) == b'hello'


def socket_cycle(mutual=False):
    with endpoints() as (sender, receiver, address):
        read, write = os.pipe()
        try:
            if mutual:
                with endpoints() as (other_sender, other, other_address):
                    send(sender, address, [other.fileno(), write])
                    send(other_sender, other_address, [receiver.fileno()])
                    receiver.close(); other.close()
            else:
                send(sender, address, [receiver.fileno(), write])
                receiver.close()
            os.close(write); write = -1
            assert_eof(read)
        finally:
            os.close(read)
            if write >= 0: os.close(write)


def queued_listener():
    with endpoints() as (sender, receiver, address):
        source = socket.socket(socket.AF_UNIX)
        name = '\0queued-listener-' + str(os.getpid())
        source.bind(name); source.listen(4)
        with source, socket.socket(socket.AF_UNIX) as client:
            client.settimeout(3)
            client.connect(name); client.sendall(b'before')
            send(sender, address, [source.fileno()])
            source.close()
            with received(receiver) as (_, fds, flags, _):
                assert len(fds) == 1 and flags == 0, (fds, flags)
                with socket.socket(fileno=os.dup(fds[0])) as listener:
                    connection, _ = listener.accept()
                    with connection: assert connection.recv(6) == b'before'


def listener_sender_exit():
    with endpoints() as (sender, receiver, address):
        name = '\0exited-listener-' + str(os.getpid())
        pid = os.fork()
        if pid == 0:
            try:
                receiver.close()
                with socket.socket(socket.AF_UNIX) as listener:
                    listener.bind(name); listener.listen(4)
                    send(sender, address, [listener.fileno()])
                os._exit(0)
            except BaseException: os._exit(91)
        assert os.waitpid(pid, 0)[1] == 0
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(3); client.connect(name); client.sendall(b'exit')
            with received(receiver) as (_, fds, flags, _):
                assert len(fds) == 1 and flags == 0, (fds, flags)
                with socket.socket(fileno=os.dup(fds[0])) as listener:
                    connection, _ = listener.accept()
                    with connection: assert connection.recv(4) == b'exit'


def descriptor_pressure():
    import resource
    with endpoints() as (sender, receiver, address), tempfile.TemporaryFile(dir='.') as original:
        send(sender, address, [original.fileno()] * 4)
        old = resource.getrlimit(resource.RLIMIT_NOFILE)
        opened = []
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (64, old[1]))
            while True:
                try: opened.append(os.dup(original.fileno()))
                except OSError as error:
                    assert error.errno == errno.EMFILE
                    break
            os.close(opened.pop())
            with received(receiver) as (_, fds, flags, _):
                assert len(fds) == 1 and flags & socket.MSG_CTRUNC, (fds, flags)
        finally:
            resource.setrlimit(resource.RLIMIT_NOFILE, old)
            for fd in opened: os.close(fd)


def terminal():
    import tty
    with endpoints() as (sender, receiver, address):
        master, slave = os.openpty()
        try:
            tty.setraw(slave)
            send(sender, address, [slave]); os.close(slave); slave = -1
            with received(receiver) as (_, fds, flags, _):
                assert len(fds) == 1 and flags == 0 and os.isatty(fds[0]), (fds, flags)
                os.write(fds[0], b'terminal')
                assert select.select([master], [], [], 3)[0]
                assert os.read(master, 8) == b'terminal'
        finally:
            os.close(master)
            if slave >= 0: os.close(slave)


def fifo():
    with endpoints() as (sender, receiver, address), tempfile.TemporaryDirectory(dir='.') as directory:
        path = directory + '/fifo'; os.mkfifo(path)
        read = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
        write = os.open(path, os.O_WRONLY | os.O_NONBLOCK)
        try:
            send(sender, address, [write]); os.close(write); write = -1
            assert not select.select([read], [], [], 0)[0]
            with received(receiver) as (_, fds, flags, _):
                assert len(fds) == 1 and flags == 0, (fds, flags)
                os.write(fds[0], b'fifo'); assert os.read(read, 4) == b'fifo'
            assert_eof(read)
        finally:
            os.close(read)
            if write >= 0: os.close(write)


def epoll():
    with endpoints() as (sender, receiver, address):
        event = os.eventfd(1)
        try:
            with select.epoll() as poll:
                poll.register(event, select.EPOLLIN)
                send(sender, address, [poll.fileno()])
            with received(receiver) as (_, fds, flags, _):
                assert len(fds) == 1 and flags == 0, (fds, flags)
                with select.epoll.fromfd(os.dup(fds[0])) as copy:
                    assert copy.poll(1) == [(event, select.EPOLLIN)]
                    assert os.eventfd_read(event) == 1
                    assert copy.poll(0) == []
        finally: os.close(event)


def synthetic():
    with endpoints() as (sender, receiver, address):
        source = os.open('/proc/cpuinfo', os.O_RDONLY)
        try: send(sender, address, [source])
        finally: os.close(source)
        with received(receiver) as (_, fds, flags, _):
            assert len(fds) == 1 and flags == 0, (fds, flags)
            assert b'processor' in os.read(fds[0], 4096)


def bound_datagram():
    with endpoints() as (sender, receiver, address):
        source = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        name = '\0retained-datagram-' + str(os.getpid())
        with source:
            source.bind(name); sender.sendto(b'queued', name)
            send(sender, address, [source.fileno()]); source.close()
            with received(receiver) as (_, fds, flags, _):
                assert len(fds) == 1 and flags == 0, (fds, flags)
                with socket.socket(fileno=os.dup(fds[0])) as copy:
                    copy.settimeout(3); assert copy.recv(6) == b'queued'
                    sender.sendto(b'after', name); assert copy.recv(5) == b'after'


def bulk_rights():
    with endpoints() as (sender, receiver, address), tempfile.TemporaryFile(dir='.') as original:
        identity = os.fstat(original.fileno()).st_ino
        send(sender, address, [original.fileno()] * 253)
        with received(receiver) as (_, fds, flags, _):
            assert len(fds) == 253 and flags == 0, (len(fds), flags)
            assert all(os.fstat(fd).st_ino == identity for fd in fds)
        try: send(sender, address, [original.fileno()] * 254)
        except OSError as error: assert error.errno == errno.EINVAL, error
        else: raise AssertionError('accepted more than SCM_MAX_FD')


def main():
    cases = [('abstract-file-offset', file_transfer), ('pathname-file-offset', lambda: file_transfer(True)),
             ('zero-payload-rights', lambda: file_transfer(empty=True)), ('sender-exit-cloexec', sender_exit),
             ('repeated-peek-and-payload-truncation', peek), ('control-truncation', control_truncation),
             ('credentials-and-rights', credentials), ('invalid-send-atomicity', invalid_send),
             ('discard-without-control', lambda: discard('read')), ('shutdown-discards-rights', lambda: discard('shutdown')),
             ('last-close-releases-rights', lambda: discard('close')), ('eventfd-after-close', eventfd),
             ('unix-pair-after-close', lambda: socket_transfer('unix-pair')), ('udp-after-close', lambda: socket_transfer('udp')),
             ('unix-listener-after-close', lambda: socket_transfer('unix-listener')),
             ('queued-listener-before-transfer', queued_listener), ('listener-sender-exit', listener_sender_exit),
             ('partial-receive-under-emfile', descriptor_pressure),
             ('pty-slave-liveness', terminal), ('fifo-writer-liveness', fifo), ('epoll-after-close', epoll),
             ('synthetic-proc-file-after-close', synthetic), ('bound-datagram-queue-after-close', bound_datagram),
             ('maximum-rights-and-shared-transfer', bulk_rights),
             ('queued-self-cycle', socket_cycle), ('queued-mutual-cycle', lambda: socket_cycle(True))]
    results = []
    for name, action in cases:
        began = time.monotonic()
        try:
            action()
            row = dict(name=name, status='passed')
        except Exception as error:
            row = dict(name=name, status='failed', error=repr(error))
        row['elapsed_ms'] = (time.monotonic() - began) * 1000
        results.append(row)
        print(json.dumps(row), flush=True)
    print(json.dumps(dict(passed=sum(row['status'] == 'passed' for row in results), total=len(results))))
    return int(any(row['status'] != 'passed' for row in results))


if __name__ == '__main__':
    raise SystemExit(main())
