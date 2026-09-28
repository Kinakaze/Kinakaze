"""Verify shared listener admission, worker death, close and connection timing."""
import errno
import json
import os
import select
import signal
import socket
import sys
import time


def name(suffix):
    return '\0listener-custody-' + str(os.getpid()) + '-' + suffix


def rebind():
    for _ in range(100):
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(name('rebind')); listener.listen(0)


def blocked_close(crash=False):
    listener = socket.socket(socket.AF_UNIX)
    address = name('crash' if crash else 'close')
    listener.bind(address); listener.listen(0)
    release, stop = os.pipe()
    server = os.fork()
    if server == 0:
        os.close(stop)
        os.read(release, 1)
        listener.close()
        os._exit(0)
    os.close(release); listener.close()
    read, write = os.pipe()
    client = None
    try:
        with socket.socket(socket.AF_UNIX) as filler:
            filler.connect(address)
            client = os.fork()
            if client == 0:
                os.close(read); os.close(stop); filler.close()
                signal.alarm(6)
                with socket.socket(socket.AF_UNIX) as blocked:
                    os.write(write, b'R')
                    try:
                        blocked.connect(address)
                        result = 0
                    except OSError as error: result = error.errno
                os.write(write, str(result).encode())
                os._exit(0)
            os.close(write); write = -1
            assert os.read(read, 1) == b'R'
            assert not select.select([read], [], [], .1)[0], 'full backlog did not block'
            if crash: os.kill(server, signal.SIGKILL)
            else: os.write(stop, b'!')
            os.waitpid(server, 0); server = None
            assert select.select([read], [], [], 4)[0], 'last close did not wake admission'
            assert os.read(read, 32) == str(errno.ECONNREFUSED).encode()
            assert os.waitpid(client, 0)[1] == 0; client = None
    finally:
        os.close(stop); os.close(read)
        if write >= 0: os.close(write)
        for pid in (client, server):
            if pid is not None:
                try: os.kill(pid, signal.SIGKILL)
                except ProcessLookupError: pass
                os.waitpid(pid, 0)


def competing_acceptors():
    with socket.socket(socket.AF_UNIX) as listener:
        address = name('acceptors'); listener.bind(address); listener.listen(4); listener.setblocking(False)
        result, output = os.pipe()
        workers = []
        complete = False
        try:
            for _ in range(2):
                start, go = os.pipe()
                pid = os.fork()
                if pid == 0:
                    os.close(go); os.close(result)
                    for _, inherited in workers: os.close(inherited)
                    while os.read(start, 1):
                        try:
                            connection, _ = listener.accept(); connection.close(); value = b'A'
                        except BlockingIOError: value = b'E'
                        os.write(output, value)
                    os._exit(0)
                os.close(start); workers.append((pid, go))
            for _ in range(30):
                with socket.socket(socket.AF_UNIX) as client:
                    client.connect(address)
                    for _, go in workers: os.write(go, b'!')
                    observed = b''
                    while len(observed) < 2:
                        assert select.select([result], [], [], 3)[0], observed
                        observed += os.read(result, 2 - len(observed))
                    assert sorted(observed) == [65, 69], observed
            complete = True
        finally:
            for pid, go in workers:
                os.close(go)
                if not complete:
                    try: os.kill(pid, signal.SIGKILL)
                    except ProcessLookupError: pass
                _, status = os.waitpid(pid, 0)
                if complete: assert status == 0
            os.close(result); os.close(output)


def benchmark(count=500):
    with socket.socket(socket.AF_UNIX) as listener:
        address = name('bench'); listener.bind(address); listener.listen(4)
        began = time.monotonic()
        for _ in range(count):
            with socket.socket(socket.AF_UNIX) as client:
                client.connect(address)
                connection, _ = listener.accept()
                with connection:
                    client.sendall(b'ping'); assert connection.recv(4) == b'ping'
        print(json.dumps(dict(connections=count, elapsed_ms=(time.monotonic() - began)*1000)), flush=True)


def main():
    failed = 0
    for name, action in [('immediate-rebind', rebind), ('fork-competing-acceptors', competing_acceptors),
                         ('full-backlog-last-close', blocked_close), ('full-backlog-worker-death', lambda: blocked_close(True))]:
        began = time.monotonic()
        try:
            action(); row = dict(name=name, status='passed')
        except Exception as error:
            failed += 1; row = dict(name=name, status='failed', error=repr(error))
        row['elapsed_ms'] = (time.monotonic() - began) * 1000
        print(json.dumps(row), flush=True)
    return int(bool(failed))


if __name__ == '__main__':
    if '--benchmark' in sys.argv: benchmark()
    else: raise SystemExit(main())
