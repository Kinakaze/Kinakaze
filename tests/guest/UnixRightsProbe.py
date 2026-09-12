"""SCM_RIGHTS preserves open descriptions across sender exit and socket transfer."""
import array
import os
import socket
import tempfile


def send(channel, fd):
    assert channel.sendmsg([b'R'], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))]) == 1


def receive(channel):
    payload, controls, flags, address = channel.recvmsg(1, socket.CMSG_SPACE(4), socket.MSG_CMSG_CLOEXEC)
    assert payload == b'R' and not (flags & socket.MSG_CTRUNC)
    assert len(controls) == 1 and controls[0][:2] == (socket.SOL_SOCKET, socket.SCM_RIGHTS)
    descriptors = array.array('i')
    descriptors.frombytes(controls[0][2])
    assert len(descriptors) == 1 and not os.get_inheritable(descriptors[0])
    return descriptors[0]


with tempfile.TemporaryFile() as file:
    file.write(b'0123456789')
    file.flush()
    file.seek(3)
    reader, writer = socket.socketpair()
    child = os.fork()
    if child == 0:
        reader.close()
        send(writer, file.fileno())
        writer.close()
        os._exit(0)
    writer.close()
    assert os.waitpid(child, 0) == (child, 0)
    fd = receive(reader)
    assert os.read(fd, 4) == b'3456'
    assert os.lseek(file.fileno(), 0, os.SEEK_CUR) == 7
    os.close(fd)
    reader.close()

# nginx distributes worker channel endpoints, not only ordinary files.
control, child_control = socket.socketpair()
peer, endpoint = socket.socketpair()
child = os.fork()
if child == 0:
    control.close()
    endpoint.close()
    peer.close()
    transferred = socket.socket(fileno=receive(child_control))
    transferred.sendall(b'endpoint transferred')
    transferred.close()
    child_control.close()
    os._exit(0)
child_control.close()
send(control, endpoint.fileno())
endpoint.close()
assert peer.recv(64) == b'endpoint transferred'
assert os.waitpid(child, 0) == (child, 0)
control.close()
peer.close()

# A peek stops after the rights-bearing send even when ordinary later data is
# already queued. No bytes past that returned boundary may be overwritten.
with tempfile.TemporaryFile() as file:
    file.write(b'peeked-rights'); file.flush(); file.seek(0)
    reader, writer = socket.socketpair()
    try:
        writer.sendall(b'prefix')
        send(writer, file.fileno())
        writer.sendall(b'tail')
        for _ in range(2):
            target = bytearray(b'!' * 64)
            count, controls, flags, _ = reader.recvmsg_into(
                [memoryview(target)[8:56]], socket.CMSG_SPACE(4), socket.MSG_PEEK | socket.MSG_CMSG_CLOEXEC)
            assert count == 7 and target[8:15] == b'prefixR'
            assert target[:8] == b'!' * 8 and target[15:] == b'!' * 49
            assert not flags & socket.MSG_CTRUNC and len(controls) == 1
            descriptor, = array.array('i', controls[0][2])
            assert not os.get_inheritable(descriptor)
            assert os.lseek(descriptor, 0, os.SEEK_CUR) == 0
            os.close(descriptor)
        payload, controls, flags, _ = reader.recvmsg(64, socket.CMSG_SPACE(4))
        assert payload == b'prefixR' and not flags & socket.MSG_CTRUNC
        descriptor, = array.array('i', controls[0][2])
        try:
            assert os.read(descriptor, 64) == b'peeked-rights'
        finally:
            os.close(descriptor)
        assert reader.recv(64) == b'tail'
    finally:
        reader.close(); writer.close()
print('UNIX_RIGHTS_LIFETIME_OK')
