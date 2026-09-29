"""Mixed TCP/PTY waits wake on real data and terminal hangup without polling."""
import json
import os
import select
import socket
import threading
import time
import tty

master, slave = os.openpty()
tty.setraw(slave)
listener = socket.socket()
listener.bind(('127.0.0.1', 0))
listener.listen()
client = socket.create_connection(listener.getsockname())
server, _ = listener.accept()
try:
    for backend in ('poll', 'epoll'):
        poller = select.poll() if backend == 'poll' else select.epoll()
        try:
            poller.register(master, select.POLLIN)
            poller.register(server.fileno(), select.POLLIN)
            wait = lambda timeout: poller.poll(timeout * 1000 if backend == 'poll' else timeout)
            cpu = time.process_time()
            begin = time.monotonic()
            assert wait(.25) == []
            print(json.dumps(dict(backend=backend, idle_cpu_ms=(time.process_time()-cpu)*1000,
                                  wall_ms=(time.monotonic()-begin)*1000)), flush=True)
            for fd, send, receive, value in (
                (master, lambda: os.write(slave, b'pty'), lambda: os.read(master, 3), b'pty'),
                (server.fileno(), lambda: client.sendall(b'tcp'), lambda: server.recv(3), b'tcp'),
            ):
                def publish():
                    time.sleep(.05)
                    send()
                thread = threading.Thread(target=publish)
                thread.start()
                assert wait(3) == [(fd, select.POLLIN)]
                assert receive() == value
                thread.join(3)
                assert not thread.is_alive()
                assert wait(.01) == []
        finally:
            if backend == 'epoll':
                poller.close()
    poller = select.poll()
    poller.register(master, select.POLLIN)
    poller.register(server, select.POLLIN)
    os.close(slave)
    slave = -1
    events = dict(poller.poll(2000))
    assert events.get(master, 0) & select.POLLHUP, events
finally:
    os.close(master)
    if slave >= 0:
        os.close(slave)
    server.close()
    client.close()
    listener.close()
print('PTY_TCP_NATIVE_WAKE_HANGUP_OK', flush=True)
