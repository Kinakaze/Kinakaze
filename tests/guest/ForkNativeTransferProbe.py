"""Independent native handle tables preserve Linux fork and exec state."""
import errno
import mmap
import os
from pathlib import Path
import signal
import socket
import tempfile
import traceback

with tempfile.TemporaryDirectory(prefix='fork-native-transfer-') as directory:
    path = Path(directory) / 'file'
    fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
    os.write(fd, b'abcdef' + bytes(4096 - 6))
    alias = os.dup(fd)
    os.set_inheritable(fd, True)
    os.set_inheritable(alias, True)
    cloexec = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
    os.lseek(fd, 1, os.SEEK_SET)
    private = mmap.mmap(fd, 4096, flags=mmap.MAP_PRIVATE)
    shared = mmap.mmap(fd, 4096, flags=mmap.MAP_SHARED)
    private[:8] = b'PRIVATE!'
    shared[16:22] = b'parent'
    pipe_read, pipe_write = os.pipe()
    left, right = socket.socketpair()
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.bind(('127.0.0.1', 0))
    address = udp.getsockname()
    fifo_path = Path(directory) / 'fifo'
    os.mkfifo(fifo_path)
    fifo = os.open(fifo_path, os.O_RDWR | os.O_NONBLOCK)
    counter = os.eventfd(9, os.EFD_NONBLOCK)
    os.write(pipe_write, b'pipe')
    left.sendall(b'unix')
    os.write(fifo, b'fifo')
    saved_mask = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR1})
    child = os.fork()
    if child == 0:
        try:
            assert os.read(fd, 2) == b'bc'
            assert os.lseek(alias, 0, os.SEEK_CUR) == 3
            assert os.read(pipe_read, 4) == b'pipe'
            assert right.recv(4) == b'unix'
            assert os.read(fifo, 4) == b'fifo'
            assert os.eventfd_read(counter) == 9
            os.eventfd_write(counter, 3)
            assert private[:8] == b'PRIVATE!'
            private[:8] = b'CHILD!!!'
            assert shared[16:22] == b'parent'
            shared[16:22] = b'child!'
            assert udp.getsockname() == address
            udp.sendto(b'udp', address)
            assert signal.SIGUSR1 in signal.pthread_sigmask(signal.SIG_BLOCK, set())
            os.kill(os.getpid(), signal.SIGUSR1)
            assert signal.sigwait({signal.SIGUSR1}) == signal.SIGUSR1
            grandchild = os.fork()
            if grandchild == 0:
                assert private[:8] == b'CHILD!!!'
                assert os.read(alias, 1) == b'd'
                source = '''
import errno, os, sys
retained, closed = map(int, sys.argv[1:])
assert os.read(retained, 1) == b'e'
try:
    os.fstat(closed)
except OSError as error:
    assert error.errno == errno.EBADF
else:
    raise AssertionError('CLOEXEC descriptor survived exec')
'''
                os.execl('/usr/bin/python3.11', 'python3.11', '-c', source, str(fd), str(cloexec))
            assert os.waitpid(grandchild, 0) == (grandchild, 0)
            os._exit(0)
        except BaseException:
            traceback.print_exc()
            os._exit(1)
    assert os.waitpid(child, 0) == (child, 0)
    signal.pthread_sigmask(signal.SIG_SETMASK, saved_mask)
    assert os.read(alias, 1) == b'f'
    assert private[:8] == b'PRIVATE!'
    assert shared[16:22] == b'child!'
    assert os.eventfd_read(counter) == 3
    udp.settimeout(2)
    assert udp.recv(3) == b'udp'
    private.close()
    shared.close()
    left.close()
    right.close()
    udp.close()
    for descriptor in [fd, alias, cloexec, pipe_read, pipe_write, fifo, counter]:
        os.close(descriptor)
print('FORK_NATIVE_TRANSFER_OK', flush=True)
