"""Run inside the Linux guest: agent-oriented standard-library behavior."""
import asyncio
import bz2
import concurrent.futures
import hashlib
import io
import json
import lzma
import math
import os
import selectors
import socket
import sqlite3
import ssl
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile
import zlib


assert math.copysign(2.5, -0.0) == -2.5
assert struct.pack('>d', math.copysign(0.0, -1.0)) == b'\x80' + b'\0' * 7
assert os.sysconf('SC_PAGESIZE') > 0
assert socket.getprotobyname('tcp') == 6
assert socket.getprotobyname('udp') == 17
try:
    socket.getprotobyname('kinakaze-unknown-protocol')
except OSError:
    pass
else:
    raise AssertionError('unknown protocol must not resolve to TCP')
payload = ('agent Python 中文\n' * 100).encode()
for module in (bz2, lzma, zlib):
    assert module.decompress(module.compress(payload)) == payload
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    expected = hashlib.sha256(payload).hexdigest()
    assert list(pool.map(lambda _: hashlib.sha256(payload).hexdigest(), range(32))) == [expected] * 32
reader, writer = socket.socketpair()
with reader, writer, selectors.DefaultSelector() as selector:
    reader.setblocking(False)
    selector.register(reader, selectors.EVENT_READ)
    writer.sendall(b'selector')
    assert selector.select(2) and reader.recv(8) == b'selector'

# Exercise the actual OpenSSL extension and CA parsing without an external site.
context = ssl.create_default_context()
assert context.get_ciphers() and context.cert_store_stats()['x509_ca'] > 0
incoming, outgoing = ssl.MemoryBIO(), ssl.MemoryBIO()
tls = context.wrap_bio(incoming, outgoing, server_side=False, server_hostname='agent.test')
try:
    tls.do_handshake()
except ssl.SSLWantReadError:
    hello = outgoing.read()
    assert hello.startswith(b'\x16\x03') and len(hello) > 100
else:
    raise AssertionError('a TLS client must wait for its peer')

with tempfile.TemporaryDirectory(prefix='python-agent-') as directory:
    data_path = os.path.join(directory, 'data')
    with open(data_path, 'wb+') as stream:
        stream.write(b'abcdefgh')
        stream.flush()
        fd = stream.fileno()
        os.lseek(fd, 6, os.SEEK_SET)
        assert os.pwritev(fd, [b'12', b'34'], 1) == 4
        assert os.lseek(fd, 0, os.SEEK_CUR) == 6
        left, right = bytearray(2), bytearray(3)
        assert os.preadv(fd, [left, right], 3) == 5
        assert left == b'34' and right == b'fgh'
        assert os.lseek(fd, 0, os.SEEK_CUR) == 6
    with sqlite3.connect(os.path.join(directory, 'database')) as database:
        database.execute('create table tools(name text unique)')
        database.execute('insert into tools values(?)', ('agent',))
        database.commit()
        database.execute('insert into tools values(?)', ('rollback',))
        database.rollback()
        assert database.execute('select name from tools').fetchall() == [('agent',)]
        assert database.execute('pragma integrity_check').fetchone()[0] == 'ok'
    database.close()
    archive = os.path.join(directory, 'archive.zip')
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as output:
        output.writestr('payload.txt', payload)
    with zipfile.ZipFile(archive) as source:
        assert source.read('payload.txt') == payload
    tar_bytes = io.BytesIO()
    with tarfile.open(fileobj=tar_bytes, mode='w:gz') as output:
        info = tarfile.TarInfo('payload.txt')
        info.size = len(payload)
        output.addfile(info, io.BytesIO(payload))
    tar_bytes.seek(0)
    with tarfile.open(fileobj=tar_bytes, mode='r:gz') as source:
        assert source.extractfile('payload.txt').read() == payload
    child = subprocess.run(
        [sys.executable, '-c', "import json,os; print(json.dumps([os.getcwd(),os.environ['AGENT_PROBE']]))"],
        cwd=directory, env=dict(os.environ, AGENT_PROBE='中文'), capture_output=True, text=True, check=True,
    )
    assert json.loads(child.stdout) == [directory, '中文']


# Restoring an inherited mount membership after dropping IDs is not setns.
# The actual setns syscall must still enforce CAP_SYS_ADMIN/CAP_SYS_CHROOT.
def dropped_identity():
    os.setgroups([])
    os.setgid(32001)
    os.setuid(32001)

namespace_fd = os.open('/proc/self/ns/mnt', os.O_RDONLY)
try:
    child = subprocess.run(
        [sys.executable, '-c', """import ctypes,errno,os,sys
assert os.getresuid()==(32001,32001,32001)
assert os.getresgid()==(32001,32001,32001) and os.getgroups()==[]
libc=ctypes.CDLL(None,use_errno=True)
assert libc.setns(int(sys.argv[1]),0)==-1 and ctypes.get_errno()==errno.EPERM
pid=os.fork()
if pid==0:
    os.execl('/bin/busybox','busybox','true')
assert os.waitpid(pid,0)[1]==0
print('NONROOT_EXEC_FORK_OK')
""", str(namespace_fd)],
        preexec_fn=dropped_identity, pass_fds=(namespace_fd,), capture_output=True, text=True, timeout=25,
    )
    assert child.returncode == 0, (child.returncode, child.stdout, child.stderr)
    assert 'NONROOT_EXEC_FORK_OK' in child.stdout
finally:
    os.close(namespace_fd)


async def asynchronous_io():
    async def echo(reader, writer):
        try:
            writer.write(await reader.readexactly(5))
            await writer.drain()
        finally:
            writer.close()
            await writer.wait_closed()

    server = await asyncio.start_server(echo, '127.0.0.1', 0)
    async with server:
        address = server.sockets[0].getsockname()
        reader, writer = await asyncio.open_connection(*address)
        writer.write(b'async')
        await writer.drain()
        assert await asyncio.wait_for(reader.readexactly(5), 5) == b'async'
        writer.close()
        await writer.wait_closed()
    child = await asyncio.create_subprocess_exec('/bin/busybox', 'echo', 'async-child', stdout=asyncio.subprocess.PIPE)
    output, _ = await asyncio.wait_for(child.communicate(), 5)
    assert child.returncode == 0 and output.strip() == b'async-child'


asyncio.run(asynchronous_io())
print('PYTHON_RUNTIME_OK', flush=True)
