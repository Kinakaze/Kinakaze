"""Unix peer and per-message identities, including fork/exec and SCM_RIGHTS."""
import array
import errno
import os
import socket
import struct
import sys
import tempfile

SOL = socket.SOL_SOCKET
CRED = socket.SCM_CREDENTIALS
PASS = socket.SO_PASSCRED
SPACE = socket.CMSG_SPACE(12)
identity = (os.getpid(), os.getuid(), os.getgid())
with socket.socket(socket.AF_UNIX) as idle:
    assert struct.unpack('iII', idle.getsockopt(SOL, socket.SO_PEERCRED, 12)) == (0, 0xffffffff, 0xffffffff)


def creds(channel, size=64, flags=0, control=SPACE):
    payload, messages, result, _ = channel.recvmsg(size, control, flags)
    values = [struct.unpack('iII', data) for level, kind, data in messages
              if (level, kind) == (SOL, CRED) and len(data) == 12]
    assert len(values) == 1 and not result & socket.MSG_CTRUNC, (messages, result)
    return payload, values[0]


def explicit(channel, value, payload=b'E'):
    return channel.sendmsg([payload], [(SOL, CRED, struct.pack('iII', *value))])


def waited(child):
    assert os.waitpid(child, 0) == (child, 0)


for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM, socket.SOCK_SEQPACKET):
    left, right = socket.socketpair(type=kind)
    assert struct.unpack('iII', left.getsockopt(SOL, socket.SO_PEERCRED, 12)) == identity
    assert left.getsockopt(SOL, socket.SO_PEERCRED, 4) == struct.pack('i', identity[0])
    assert len(left.getsockopt(SOL, PASS, 1)) == 1
    assert left.getsockopt(SOL, PASS) == 0
    left.setsockopt(SOL, PASS, 1)
    alias = socket.socket(fileno=os.dup(left.fileno()))
    assert alias.getsockopt(SOL, PASS) == 1
    alias.setsockopt(SOL, PASS, 0)
    assert left.getsockopt(SOL, PASS) == 0
    alias.close()
    left.setsockopt(SOL, PASS, 1)
    os.write(right.fileno(), b'ABC')
    if kind == socket.SOCK_STREAM:
        assert creds(left, 1, socket.MSG_PEEK) == (b'A', identity)
        assert creds(left, 1) == (b'A', identity)
        assert creds(left) == (b'BC', identity)
    else:
        assert creds(left, 64, socket.MSG_PEEK) == (b'ABC', identity)
        assert creds(left) == (b'ABC', identity)
        right.send(b'')
        assert creds(left) == (b'', identity)
    assert explicit(right, identity) == 1
    assert creds(left) == (b'E', identity)
    # Controls that don't fit are truncated without consuming another payload.
    for capacity in (0, 15, 16, 20, 27, 28, 31, 32):
        right.send(b'T')
        data, controls, flags, _ = left.recvmsg(1, capacity)
        assert data == b'T' and bool(flags & socket.MSG_CTRUNC) == (capacity < 28)
        if capacity >= 28:
            assert struct.unpack('iII', controls[0][2]) == identity
    left.close()
    right.close()
print('credentials payload, peek, datagrams and truncation OK', flush=True)

# Same-identity stream writes may merge; distinct senders must remain separate.
left, right = socket.socketpair()
left.setsockopt(SOL, PASS, 1)
right.sendall(b'parent')
child = os.fork()
if child == 0:
    left.close()
    right.sendall(b'child')
    os._exit(0)
waited(child)
for _ in range(2):
    target = bytearray(b'!' * 64)
    copied, controls, flags, _ = left.recvmsg_into([memoryview(target)[8:56]], SPACE, socket.MSG_PEEK)
    assert copied == len(b'parent') and target[8:8+copied] == b'parent'
    assert target[:8] == b'!' * 8 and target[8+copied:] == b'!' * (56-copied)
    assert not flags & socket.MSG_CTRUNC and len(controls) == 1
    assert struct.unpack('iII', controls[0][2]) == identity
assert creds(left) == (b'parent', identity)
assert creds(left) == (b'child', (child, identity[1], identity[2]))
assert struct.unpack('iII', left.getsockopt(SOL, socket.SO_PEERCRED, 12)) == identity

# Capture is on demand. A sender may request capture before a receiver opts in.
left.setsockopt(SOL, PASS, 0)
right.setsockopt(SOL, PASS, 1)
right.sendall(b'captured')
left.setsockopt(SOL, PASS, 1)
assert creds(left) == (b'captured', identity)
left.setsockopt(SOL, PASS, 0)
right.setsockopt(SOL, PASS, 0)
right.sendall(b'unattributed')
left.setsockopt(SOL, PASS, 1)
assert creds(left) == (b'unattributed', (0, 65534, 65534))

# Socket option and queued identity survive exec without a host PID leaking out.
child = os.fork()
if child == 0:
    left.close()
    os.set_inheritable(right.fileno(), True)
    code = 'import os,socket; s=socket.socket(fileno=int(os.sys.argv[1])); s.sendall(b"exec"); s.close()'
    os.execv(sys.executable, [sys.executable, '-c', code, str(right.fileno())])
waited(child)
assert creds(left) == (b'exec', (child, identity[1], identity[2]))
left.close()
right.close()
print('credentials fork, sender exit and exec OK', flush=True)

# Credentials precede rights and consume control capacity before fd installation.
left, right = socket.socketpair()
left.setsockopt(SOL, PASS, 1)
with tempfile.TemporaryFile() as file:
    file.write(b'rights-with-identity')
    file.flush()
    file.seek(0)
    for capacity in (0, 28, 32, 48, 52, 56):
        right.sendmsg([b'R'], [(SOL, socket.SCM_RIGHTS, array.array('i', [file.fileno()]))])
        payload, controls, flags, _ = left.recvmsg(1, capacity, socket.MSG_CMSG_CLOEXEC)
        assert payload == b'R' and bool(flags & socket.MSG_CTRUNC) == (capacity < 52)
        descriptors = [array.array('i', value) for level, kind, value in controls if kind == socket.SCM_RIGHTS]
        assert len(descriptors) == (1 if capacity >= 52 else 0), controls
        if descriptors:
            fd, = descriptors[0]
            assert not os.get_inheritable(fd)
            assert os.read(fd, 64) == b'rights-with-identity'
            os.lseek(fd, 0, os.SEEK_SET)
            os.close(fd)
left.close()
right.close()
print('credentials and rights combined OK', flush=True)

if os.geteuid() == 0:
    left, right = socket.socketpair()
    left.setsockopt(SOL, PASS, 1)
    child = os.fork()
    if child == 0:
        left.close()
        os.setresgid(41001, 41002, 41003)
        os.setresuid(40001, 40002, 40003)
        for forged in ((os.getpid(), 0, 41001), (os.getpid(), 40001, 0), (identity[0], 40001, 41001)):
            try:
                explicit(right, forged)
            except OSError as error:
                assert error.errno == errno.EPERM, error
            else:
                raise AssertionError(('forged credentials accepted', forged))
        explicit(right, (os.getpid(), 40003, 41003), b'S')
        os.write(right.fileno(), b'A')
        os._exit(0)
    waited(child)
    assert creds(left) == (b'S', (child, 40003, 41003))
    assert creds(left) == (b'A', (child, 40001, 41001))
    left.close()
    right.close()

    # A connection retains effective IDs from connect even after client exit.
    listener = socket.socket(socket.AF_UNIX)
    address = '\0credential-probe-' + str(os.getpid())
    listener.bind(address)
    listener.setsockopt(SOL, PASS, 1)
    listener.listen(2)
    assert struct.unpack('iII', listener.getsockopt(SOL, socket.SO_PEERCRED, 12)) == identity
    child = os.fork()
    if child == 0:
        listener.close()
        os.setresgid(41001, 41002, 41003)
        os.setresuid(40001, 40002, 40003)
        client = socket.socket(socket.AF_UNIX)
        client.connect(address)
        assert struct.unpack('iII', client.getsockopt(SOL, socket.SO_PEERCRED, 12)) == identity
        client.sendall(b'C')
        client.close()
        os._exit(0)
    waited(child)
    accepted, _ = listener.accept()
    assert accepted.getsockopt(SOL, PASS) == 1
    assert struct.unpack('iII', accepted.getsockopt(SOL, socket.SO_PEERCRED, 12)) == (child, 40002, 41002)
    assert creds(accepted) == (b'C', (child, 40001, 41001))
    accepted.close()
    listener.close()
print('UNIX_CREDENTIALS_LIFETIME_OK')
