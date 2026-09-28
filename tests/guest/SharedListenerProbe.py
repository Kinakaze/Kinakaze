"""Two forked acceptors must not block after racing for one ready connection."""
import fcntl
import ctypes
import errno
import os
import select
import signal
import socket
import time

listener = socket.socket()
listener.bind(('127.0.0.1', 0))
listener.listen(16)
listener.setblocking(False)
libc = ctypes.CDLL(None, use_errno=True)
assert libc.accept4(listener.fileno(), None, None, 1) == -1
assert ctypes.get_errno() == errno.EINVAL
result_read, result_write = os.pipe()
workers = []
completed = False
try:
    for index in range(2):
        ready, release = os.pipe()
        pid = os.fork()
        if pid == 0:
            os.close(release)
            os.close(result_read)
            for _, inherited in workers:
                os.close(inherited)
            assert fcntl.fcntl(listener, fcntl.F_GETFL) & os.O_NONBLOCK
            while os.read(ready, 1):
                try:
                    connection, _ = listener.accept()
                    connection.close()
                    result = b'A'
                except BlockingIOError:
                    result = b'E'
                os.write(result_write, result)
            os._exit(0)
        os.close(ready)
        workers.append((pid, release))
    for iteration in range(100):
        with socket.create_connection(listener.getsockname()):
            for _, release in workers:
                os.write(release, b'!')
            observed = b''
            deadline = time.monotonic() + 2
            while len(observed) < 2:
                remaining = max(0, deadline - time.monotonic())
                assert select.select([result_read], [], [], remaining)[0], (
                    'nonblocking accept stalled after readiness race', iteration, observed)
                observed += os.read(result_read, 2 - len(observed))
            assert sorted(observed) == sorted(b'AE'), (iteration, observed)
    print('SHARED_LISTENER_NONBLOCK_ACCEPT_OK', flush=True)
    completed = True
finally:
    for pid, release in workers:
        os.close(release)
        if not completed:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        _, status = os.waitpid(pid, 0)
        if completed:
            assert status == 0
    os.close(result_read)
    os.close(result_write)
    listener.close()
