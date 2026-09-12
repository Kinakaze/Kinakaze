"""Partial stream reads retain data; draining and refilling rearm EPOLLET."""
import array
import fcntl
import select
import socket
import termios

for blocking in (True, False):
    left, right=socket.socketpair()
    epoll=select.epoll()
    try:
        right.setblocking(blocking)
        epoll.register(right, select.EPOLLIN | select.EPOLLET)
        count=array.array('i', [0])
        for iteration in range(64):
            payload=bytes([iteration])*8192
            left.sendall(payload)
            events=epoll.poll(1)
            assert any(fd==right.fileno() and flags & select.EPOLLIN for fd,flags in events), events
            assert right.recv(123)==payload[:123]
            fcntl.ioctl(right, termios.FIONREAD, count, True)
            assert count[0]==8192-123, count
            assert right.recv(79, socket.MSG_PEEK)==payload[:79]
            fcntl.ioctl(right, termios.FIONREAD, count, True)
            assert count[0]==8192-123, count
            assert epoll.poll(0)==[]
            assert right.recv(16384)==payload[123:]
            fcntl.ioctl(right, termios.FIONREAD, count, True)
            assert count[0]==0, count
            assert epoll.poll(0)==[]
        left.shutdown(socket.SHUT_WR)
        assert right.recv(16)==b''
    finally:
        epoll.close();left.close();right.close()
print('UnixReadinessProbe: PASS',flush=True)
