"""Blocking AF_UNIX wakeup, shutdown and writes exceeding native pipe quota."""
import os
import errno
import signal
import socket
import threading
import time


def main():
    # Repeated cancellation followed by successful I/O must not consume a
    # previous completion event's signal or leak an unfinished operation.
    reader, writer = os.pipe()
    try:
        os.set_blocking(reader, False)
        for index in range(128):
            try:
                os.read(reader, 1)
            except BlockingIOError as error:
                assert error.errno in (errno.EAGAIN, errno.EWOULDBLOCK)
            else:
                raise AssertionError('empty nonblocking pipe read did not fail')
            marker = bytes([index])
            assert os.write(writer, marker) == 1
            assert os.read(reader, 1) == marker
    finally:
        os.close(reader)
        os.close(writer)

    # A large pending write must wake the reader before the write completes.
    # Waiting only for sender-side metadata publication would deadlock here.
    left, right = socket.socketpair()
    payload = bytes(range(256)) * 8192
    errors = []
    def send_large():
        try:
            right.sendall(payload)
            right.shutdown(socket.SHUT_WR)
        except BaseException as error:
            errors.append(error)
        finally:
            right.close()
    sender = threading.Thread(target=send_large)
    sender.start()
    received = bytearray()
    while True:
        block = left.recv(16384)
        if not block:
            break
        received.extend(block)
    sender.join(timeout=3)
    left.close()
    assert not sender.is_alive() and not errors, errors
    assert received == payload

    for local_shutdown in (False, True):
        left, right = socket.socketpair()
        results = []
        started = threading.Event()
        def receive():
            started.set()
            results.append(left.recv(16))
        reader = threading.Thread(target=receive)
        reader.start()
        assert started.wait(timeout=3)
        time.sleep(.025)
        if local_shutdown:
            left.shutdown(socket.SHUT_RD)
        else:
            right.shutdown(socket.SHUT_WR)
        reader.join(timeout=3)
        assert not reader.is_alive() and results == [b""], results
        left.close()
        right.close()

    left, right = socket.socketpair()
    child = os.fork()
    if child == 0:
        left.close()
        time.sleep(.025)
        os._exit(0)  # kernel close must wake the peer without a shared hint
    right.close()
    assert left.recv(16) == b""
    left.close()
    assert os.waitpid(child, 0) == (child, 0)

    # Interrupted native waits must retire their own request before a guest
    # handler runs. A returning handler must still permit the recv to restart.
    class InterruptedRead(Exception):
        pass

    for raises in (False, True):
        calls = []
        def handler(signum, frame):
            calls.append(signum)
            if raises:
                raise InterruptedRead()
        previous = signal.signal(signal.SIGUSR1, handler)
        left, right = socket.socketpair()
        parent = os.getpid()
        child = os.fork()
        if child == 0:
            left.close()
            assert right.recv(1) == b"s"
            time.sleep(.025)
            os.kill(parent, signal.SIGUSR1)
            time.sleep(.025)
            right.sendall(b"done")
            right.close()
            os._exit(0)
        right.close()
        try:
            left.sendall(b"s")
            if raises:
                try:
                    left.recv(4)
                except InterruptedRead:
                    pass
                else:
                    raise AssertionError("blocked recv did not run the signal handler")
            assert left.recv(4) == b"done"
            assert calls == [signal.SIGUSR1], calls
            assert os.waitpid(child, 0) == (child, 0)
        finally:
            left.close()
            signal.signal(signal.SIGUSR1, previous)
    print("UnixBlockingProbe: PASS", flush=True)


if __name__ == "__main__":
    main()
