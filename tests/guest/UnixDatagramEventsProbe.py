"""Named datagram blocking waits across fork, capacity, shutdown and signals."""
import contextlib
import ctypes
import errno
import os
import signal
import socket
import tempfile
import threading
import time


@contextlib.contextmanager
def pair(pathname=False):
    with tempfile.TemporaryDirectory(prefix='dgram-events-', dir='.') as directory:
        names = [os.path.abspath(directory + '/' + side) if pathname else '\0' + directory + side
                 for side in ('left', 'right')]
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as left, socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as right:
            left.bind(names[0]); right.bind(names[1])
            left.connect(names[1]); right.connect(names[0])
            yield left, right


def forked(action):
    child = os.fork()
    if child == 0:
        try:
            action()
            os._exit(0)
        except BaseException:
            os._exit(90)
    return child


def waited(child):
    assert os.waitpid(child, 0) == (child, 0)


def fill(sender, payload=b''):
    count = 0
    while True:
        try:
            assert sender.send(payload, socket.MSG_DONTWAIT) == len(payload)
            count += 1
            assert count <= 1024
        except BlockingIOError:
            return count


def cross_process_receive(pathname):
    with pair(pathname) as (left, right):
        def send():
            right.close()
            time.sleep(.05)
            left.send(b'after-fork')
        child = forked(send)
        assert right.recv(64) == b'after-fork'
        waited(child)


def full_queue_send(pathname):
    with pair(pathname) as (left, right):
        count = fill(left)
        assert count > 0
        def drain():
            left.close()
            time.sleep(.05)
            assert right.recv(1) == b''
        child = forked(drain)
        assert left.send(b'new') == 3
        waited(child)
        for _ in range(count-1):
            assert right.recv(1) == b''
        assert right.recv(3) == b'new'


def byte_capacity_send():
    with pair() as (left, right):
        payload = b'x' * 16384
        count = fill(left, payload)
        def drain():
            left.close()
            time.sleep(.05)
            assert right.recv(len(payload)) == payload
        child = forked(drain)
        assert left.send(payload) == len(payload)
        waited(child)
        for _ in range(count):
            assert right.recv(len(payload)) == payload


def shutdown_waits():
    for receiving in (True, False):
        with pair() as (left, right):
            if not receiving:
                fill(left)
            results = []
            def operation():
                try:
                    results.append(right.recv(1) if receiving else left.send(b'x'))
                except OSError as error:
                    results.append(error.errno)
            worker = threading.Thread(target=operation, daemon=True)
            worker.start()
            time.sleep(.05)
            (right if receiving else left).shutdown(socket.SHUT_RD if receiving else socket.SHUT_WR)
            worker.join(4)
            assert not worker.is_alive(), results
            assert results == ([b''] if receiving else [errno.EPIPE]), results


def competing_readers():
    with pair() as (left, right):
        results = []
        workers = [threading.Thread(target=lambda: results.append(right.recv(1)), daemon=True)
                   for _ in range(4)]
        for worker in workers: worker.start()
        time.sleep(.05)
        for byte in b'abcd': left.send(bytes([byte]))
        for worker in workers: worker.join(4)
        assert all(not worker.is_alive() for worker in workers)
        assert sorted(results) == [bytes([byte]) for byte in b'abcd'], results


def interrupted(receiving, restart):
    libc = ctypes.CDLL(None, use_errno=True)
    libc.recv.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
    libc.send.argtypes = libc.recv.argtypes
    libc.recv.restype = libc.send.restype = ctypes.c_ssize_t
    calls = []
    previous = signal.signal(signal.SIGUSR1, lambda signum, frame: calls.append(signum))
    signal.siginterrupt(signal.SIGUSR1, not restart)
    try:
        with pair() as (left, right):
            count = 0 if receiving else fill(left)
            parent = os.getpid()
            def wake():
                time.sleep(.05)
                os.kill(parent, signal.SIGUSR1)
                time.sleep(.05)
                if receiving:
                    left.send(b'z')
                else:
                    assert right.recv(1) == b''
            child = forked(wake)
            buffer = ctypes.create_string_buffer(b'z')
            result = (libc.recv if receiving else libc.send)((right if receiving else left).fileno(), buffer, 1, 0)
            error = ctypes.get_errno()
            assert calls == [signal.SIGUSR1], calls
            assert (result == 1) if restart else (result == -1 and error == errno.EINTR), (result, error, restart)
            waited(child)
            if receiving and not restart:
                assert right.recv(1) == b'z'
            if not receiving:
                for _ in range(count-1): assert right.recv(1) == b''
                if restart: assert right.recv(1) == b'z'
    finally:
        signal.signal(signal.SIGUSR1, previous)


def main():
    for pathname in (False, True):
        cross_process_receive(pathname)
        full_queue_send(pathname)
    byte_capacity_send()
    shutdown_waits()
    competing_readers()
    for receiving in (True, False):
        for restart in (False, True): interrupted(receiving, restart)
    print('UNIX_DATAGRAM_EVENTS_OK', flush=True)


if __name__ == '__main__':
    main()
