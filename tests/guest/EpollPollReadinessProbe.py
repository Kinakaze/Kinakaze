"""poll(epollfd) must not spin or consume edge/one-shot notifications."""
import os
import select
import socket
import threading
import time

def tcp_pair():
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0)); listener.listen(1)
    writer = socket.socket(); writer.connect(listener.getsockname())
    reader, _ = listener.accept(); listener.close()
    return reader, writer

for factory, flags in [(factory, flags) for factory in (socket.socketpair, tcp_pair)
                       for flags in (0, select.EPOLLET, select.EPOLLONESHOT,
                                     select.EPOLLET | select.EPOLLONESHOT)]:
    reader, writer = factory()
    ep = select.epoll()
    poll = select.poll()
    poll.register(ep.fileno(), select.POLLIN | select.POLLOUT)
    try:
        assert poll.poll(0) == [], 'empty epoll reported readable/writable'
        ep.register(reader, select.EPOLLIN | flags)
        start = time.monotonic()
        assert poll.poll(30) == [], 'idle registered epoll reported ready'
        assert time.monotonic() - start >= .025
        writer.send(b'a')
        for _ in range(3):
            assert poll.poll(1000) == [(ep.fileno(), select.POLLIN)]
        assert ep.poll(0) == [(reader.fileno(), select.EPOLLIN)]
        if flags:
            assert poll.poll(0) == [], 'poll consumed or repeated an edge/one-shot'
        assert reader.recv(1) == b'a'
        if flags & select.EPOLLONESHOT:
            ep.modify(reader, select.EPOLLIN | flags)
        assert not poll.poll(0)
        timer = threading.Timer(.05, writer.send, args=(b'b',))
        timer.start()
        assert poll.poll(1000) == [(ep.fileno(), select.POLLIN)]
        timer.join()
        assert ep.poll(0) == [(reader.fileno(), select.EPOLLIN)]
        assert reader.recv(1) == b'b'
    finally:
        reader.close(); writer.close(); ep.close()
print('EPOLL_POLL_NONCONSUMING_EDGE_ONESHOT_WAKE_OK', flush=True)
