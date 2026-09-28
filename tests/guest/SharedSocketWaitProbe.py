"""Forked blocking socket waiters must retain independent readiness requests."""
import os
import select
import signal
import socket
import time


def check(kind):
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(16)
    address = listener.getsockname()
    peer = None
    source = listener
    if kind == 'recv':
        peer = socket.create_connection(address)
        source, _ = listener.accept()
        listener.close()
    result_read, result_write = os.pipe()
    workers = []
    completed = False

    def read_result():
        assert select.select([result_read], [], [], 2)[0], (kind, 'lost readiness notification')
        return os.read(result_read, 1)

    try:
        for _ in range(2):
            command, release = os.pipe()
            pid = os.fork()
            if pid == 0:
                os.close(release)
                os.close(result_read)
                for _, inherited in workers:
                    os.close(inherited)
                if peer is not None:
                    peer.close()
                while os.read(command, 1):
                    os.write(result_write, b'R')
                    if kind == 'accept':
                        connection, _ = source.accept()
                        connection.close()
                    else:
                        assert source.recv(1) == b'!'
                    os.write(result_write, b'A')
                os._exit(0)
            os.close(command)
            workers.append((pid, release))
        for _ in range(20):
            for _, release in workers:
                os.write(release, b'!')
            assert read_result() == read_result() == b'R'
            # Both guests have entered the call. Give native waits time to arm;
            # the second arrival must wake the other process independently.
            time.sleep(.01)
            for _ in range(2):
                if kind == 'accept':
                    with socket.create_connection(address):
                        assert read_result() == b'A'
                else:
                    peer.sendall(b'!')
                    assert read_result() == b'A'
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
        source.close()
        if peer is not None:
            peer.close()


check('accept')
check('recv')
print('SHARED_SOCKET_BLOCKING_ACCEPT_RECV_OK', flush=True)
