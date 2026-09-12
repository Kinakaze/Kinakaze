"""Measure eventfd notification latency in the waits used by GUI event loops."""
import json
import os
import select
import socket
import statistics
import threading
import time

results = []
for kind in ('poll', 'mixed_poll', 'epoll'):
    fd = os.eventfd(0, os.EFD_NONBLOCK)
    left, right = socket.socketpair()
    waiter = select.epoll() if kind == 'epoll' else select.poll()
    waiter.register(fd, select.POLLIN)
    if kind == 'mixed_poll':
        waiter.register(left, select.POLLIN)
    samples = []
    try:
        for _ in range(30):
            written = []
            def write():
                time.sleep(.002)
                written.append(time.perf_counter())
                os.eventfd_write(fd, 1)
            thread = threading.Thread(target=write)
            thread.start()
            ready = waiter.poll(1 if kind == 'epoll' else 1000)
            received = time.perf_counter()
            thread.join()
            assert (fd, select.POLLIN) in ready, ready
            assert waiter.poll(0), 'wait consumed eventfd readiness'
            assert os.eventfd_read(fd) == 1
            assert not waiter.poll(0), 'stale readiness after read'
            samples.append((received - written[0]) * 1000)
        results.append({'kind': kind, 'median_ms': round(statistics.median(samples), 3),
                        'p95_ms': round(sorted(samples)[28], 3)})
    finally:
        if kind == 'epoll':
            waiter.close()
        os.close(fd)
        left.close()
        right.close()
print(json.dumps(results), flush=True)
print('EVENTFD_WAKE_READINESS_OK', flush=True)
