"""Event-driven Unix stream waits: timeout, data, peer death and mixed AFD sets."""
import json
import os
import select
import signal
import socket
import time


for mixed in (False, True):
    reader, writer = socket.socketpair()
    reader.setblocking(False)
    poller = select.epoll()
    poller.register(reader, select.EPOLLIN)
    inet = socket.socket(socket.AF_INET, socket.SOCK_DGRAM) if mixed else None
    if inet:
        inet.bind(('127.0.0.1', 0))
        poller.register(inet, select.EPOLLIN)
    try:
        started = time.monotonic()
        cpu = time.process_time()
        assert poller.poll(.3) == []
        elapsed = time.monotonic() - started
        assert elapsed >= .25, elapsed
        print(json.dumps(dict(mixed=mixed, idle_seconds=elapsed,
                              cpu_seconds=time.process_time()-cpu)), flush=True)
        child = os.fork()
        if child == 0:
            reader.close()
            if inet:
                inet.close()
            time.sleep(.15)
            writer.sendall(b'wakeup')
            time.sleep(30)
            os._exit(1)
        writer.close()
        try:
            ready = poller.poll(3)
            assert any(fd == reader.fileno() and mask & select.EPOLLIN for fd, mask in ready), ready
            assert reader.recv(64) == b'wakeup'
            assert poller.poll(.05) == []
        finally:
            os.kill(child, signal.SIGKILL)
            os.waitpid(child, 0)
        ready = poller.poll(3)
        assert any(fd == reader.fileno() and mask & (select.EPOLLIN | select.EPOLLHUP) for fd, mask in ready), ready
        assert reader.recv(64) == b''
    finally:
        poller.close()
        reader.close()
        writer.close()
        if inet:
            inet.close()
print('UNIX_EPOLL_WAKE_OK')
