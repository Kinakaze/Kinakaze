"""Native notification must preserve eventfd levels across consumers and fork."""
import os
import select
import socket
import threading
import time

fd = os.eventfd(0, os.EFD_NONBLOCK | os.EFD_SEMAPHORE)
alias = os.dup(fd)
os.close(fd)
fd = alias
try:
    results = []
    barrier = threading.Barrier(3)
    def waiter():
        poller = select.poll()
        poller.register(fd, select.POLLIN)
        barrier.wait()
        results.append(poller.poll(1000))
    threads = [threading.Thread(target=waiter) for _ in range(2)]
    for thread in threads:
        thread.start()
    barrier.wait()
    time.sleep(.02)
    os.eventfd_write(fd, 3)
    for thread in threads:
        # Each worker has its own bounded poll; join only after it returns.
        thread.join()
    assert len(results) == 2 and all((fd, select.POLLIN) in result for result in results), results
    poller = select.poll()
    poller.register(fd, select.POLLIN)
    for _ in range(3):
        assert poller.poll(0), 'semaphore prematurely cleared readable notification'
        assert os.eventfd_read(fd) == 1
    assert not poller.poll(0)
finally:
    os.close(fd)

fd = os.eventfd(0, os.EFD_NONBLOCK)
try:
    poller = select.poll()
    poller.register(fd, select.POLLIN)
    child = os.fork()
    if child == 0:
        time.sleep(.025)
        os.eventfd_write(fd, 7)
        os._exit(0)
    assert poller.poll(2000), 'fork writer did not wake parent'
    assert os.eventfd_read(fd) == 7
    assert os.waitpid(child, 0)[1] == 0
    os.eventfd_write(fd, (1 << 64) - 2)
    poller.modify(fd, select.POLLOUT)
    assert not poller.poll(0), 'full counter incorrectly writable'
    def drain():
        time.sleep(.02)
        assert os.eventfd_read(fd) == (1 << 64) - 2
    thread = threading.Thread(target=drain)
    thread.start()
    assert poller.poll(1000), 'reader did not wake a writable wait'
    thread.join()
    assert poller.poll(0), 'writability consumed by poll'

    # AFD sockets and eventfds must share one interruptible wait.
    server = socket.socket()
    server.bind(('127.0.0.1', 0));server.listen()
    peer = socket.create_connection(server.getsockname());accepted, _ = server.accept()
    try:
        poller.modify(fd, select.POLLIN)
        poller.register(accepted, select.POLLIN)
        thread = threading.Thread(target=lambda: (time.sleep(.02), os.eventfd_write(fd, 2)))
        thread.start()
        assert (fd, select.POLLIN) in poller.poll(1000), 'mixed AFD wait lost eventfd'
        thread.join()
        assert os.eventfd_read(fd) == 2
    finally:
        accepted.close();peer.close();server.close()
finally:
    os.close(fd)

# The timer/interrupt/source fan-in must not drop the last descriptor at 64.
fds = [os.eventfd(0, os.EFD_NONBLOCK) for _ in range(70)]
try:
    poller = select.poll()
    for fd in fds:
        poller.register(fd, select.POLLIN)
    thread = threading.Thread(target=lambda: (time.sleep(.02), os.eventfd_write(fds[-1], 1)))
    thread.start()
    assert (fds[-1], select.POLLIN) in poller.poll(1000), 'large wait dropped last eventfd'
    thread.join()
    assert os.eventfd_read(fds[-1]) == 1
finally:
    for fd in fds:
        os.close(fd)
print('EVENTFD_DUP_FORK_MULTIWAITER_SEMAPHORE_WRITABLE_AFD_OK', flush=True)
