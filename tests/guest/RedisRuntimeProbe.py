"""Exercise real Redis transactions, Lua, forked persistence and restart over AF_UNIX."""
import os
import pathlib
import socket
import subprocess
import tempfile
import time


class Redis:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.settimeout(10)
        try:
            self.socket.connect(path)
            self.stream = self.socket.makefile('rb')
        except BaseException:
            self.socket.close()
            raise

    def close(self):
        self.stream.close()
        self.socket.close()

    def send(self, *arguments):
        values = [value if isinstance(value, bytes) else str(value).encode() for value in arguments]
        frame = b'*%d\r\n' % len(values)
        frame += b''.join(b'$%d\r\n' % len(value) + value + b'\r\n' for value in values)
        self.socket.sendall(frame)

    def read(self):
        line = self.stream.readline(65536)
        if not line.endswith(b'\r\n'):
            raise RuntimeError('truncated Redis reply')
        kind, body = line[:1], line[1:-2]
        if kind == b'+':
            return body
        if kind == b'-':
            raise RuntimeError(body.decode(errors='replace'))
        if kind == b':':
            return int(body)
        if kind in (b'$', b'*'):
            length = int(body)
            if length == -1:
                return None
            if not 0 <= length <= 8 * 1024 * 1024:
                raise RuntimeError('unbounded Redis reply')
            if kind == b'*':
                return [self.read() for _ in range(length)]
            payload = self.stream.read(length + 2)
            if len(payload) != length + 2 or not payload.endswith(b'\r\n'):
                raise RuntimeError('truncated Redis bulk reply')
            return payload[:-2]
        raise RuntimeError('unknown Redis reply')

    def call(self, *arguments):
        self.send(*arguments)
        return self.read()


def main():
    with tempfile.TemporaryDirectory(prefix='redis-agent-') as directory:
        log_path = pathlib.Path(directory, 'redis.log')
        address = os.path.join(directory, 'redis.sock')
        server = client = second = None
        log = log_path.open('ab', buffering=0)

        def start():
            nonlocal server
            server = subprocess.Popen([
                '/usr/bin/redis-server', '--port', '0', '--unixsocket', address,
                '--dir', directory, '--save', '', '--appendonly', 'yes', '--appendfsync', 'always',
            ], stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if server.poll() is not None:
                    raise RuntimeError(f'Redis exited before listen: {server.returncode}')
                try:
                    connection = Redis(address)
                except (ConnectionError, FileNotFoundError):
                    time.sleep(0.05)
                    continue
                try:
                    assert connection.call('PING') == b'PONG'
                    return connection
                except BaseException:
                    connection.close()
                    raise
            raise TimeoutError('Redis did not listen')

        def persistence_done(field, status):
            deadline = time.monotonic() + 25
            while time.monotonic() < deadline:
                info = dict(line.split(b':', 1) for line in client.call('INFO', 'persistence').splitlines()
                            if b':' in line)
                if info[field] == b'0':
                    assert info[status] == b'ok', info
                    return
                time.sleep(0.05)
            raise TimeoutError('Redis persistence child did not complete')

        try:
            client = start()
            assert client.call('SET', 'counter', 40) == b'OK'
            assert client.call('MULTI') == b'OK'
            assert client.call('INCR', 'counter') == b'QUEUED'
            assert client.call('INCR', 'counter') == b'QUEUED'
            assert client.call('EXEC') == [41, 42]
            second = Redis(address)
            assert client.call('WATCH', 'counter') == b'OK'
            assert second.call('INCR', 'counter') == 43
            assert client.call('MULTI') == b'OK'
            assert client.call('SET', 'counter', 0) == b'QUEUED'
            assert client.call('EXEC') is None
            second.close()
            second = None
            assert client.call('GET', 'counter') == b'43'
            assert client.call('EVAL', 'return {redis.call("INCR", KEYS[1]), math.ceil(1.25)}', 1, 'counter') == [44, 2]
            assert client.call('HSET', 'hash', 'field', 'value') == 1
            assert client.call('HGET', 'hash', 'field') == b'value'
            assert client.call('ZADD', 'sorted', '1.25', 'a', '2.5', 'b') == 2
            assert client.call('ZRANGE', 'sorted', 0, -1) == [b'a', b'b']
            payload = bytes(range(256)) * 4096
            assert client.call('SET', 'binary', payload) == b'OK'
            assert client.call('GET', 'binary') == payload
            print('REDIS_TRANSACTIONS_LUA_UNIX_OK', flush=True)
            assert client.call('BGSAVE') == b'Background saving started'
            persistence_done(b'rdb_bgsave_in_progress', b'rdb_last_bgsave_status')
            assert pathlib.Path(directory, 'dump.rdb').stat().st_size > 0
            assert client.call('BGREWRITEAOF') == b'Background append only file rewriting started'
            persistence_done(b'aof_rewrite_in_progress', b'aof_last_bgrewrite_status')
            print('REDIS_FORK_PERSISTENCE_OK', flush=True)
            client.send('SHUTDOWN', 'SAVE')
            assert server.wait(timeout=20) == 0
            client.close()
            client = start()
            assert client.call('GET', 'counter') == b'44'
            assert client.call('GET', 'binary') == payload
            assert client.call('HGET', 'hash', 'field') == b'value'
            client.send('SHUTDOWN', 'NOSAVE')
            assert server.wait(timeout=20) == 0
            print('REDIS_RUNTIME_OK', flush=True)
        except BaseException:
            print(log_path.read_text(errors='replace')[-12000:], flush=True)
            raise
        finally:
            for connection in (second, client):
                if connection:
                    connection.close()
            if server and server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
            log.close()


main()
