"""Bounded descriptor workloads with correctness assertions and guest timings."""
import json
import os
import select
import sys
import time
import tempfile

mode = sys.argv[1]
iterations = int(sys.argv[2]) if len(sys.argv) > 2 else 5000
assert 0 < iterations <= 1_000_000

if mode in ('select', 'poll'):
    reader, writer = os.pipe()
    os.write(writer, b'x')
    if mode == 'select':
        operation = lambda: select.select([reader], [], [], 0) == ([reader], [], [])
    else:
        poll = select.poll()
        poll.register(reader, select.POLLIN)
        operation = lambda: poll.poll(0) == [(reader, select.POLLIN)]
    start = time.perf_counter_ns()
    for _ in range(iterations):
        assert operation()
    elapsed = time.perf_counter_ns() - start
    assert os.read(reader, 1) == b'x'
    os.close(reader)
    os.close(writer)
elif mode in ('epoll-ready', 'epoll-empty'):
    fd = os.eventfd(int(mode == 'epoll-ready'), os.EFD_NONBLOCK)
    epoll = select.epoll()
    epoll.register(fd, select.EPOLLIN)
    expected = [(fd, select.EPOLLIN)] if mode == 'epoll-ready' else []
    start = time.perf_counter_ns()
    for _ in range(iterations):
        assert epoll.poll(0) == expected
    elapsed = time.perf_counter_ns() - start
    epoll.close()
    os.close(fd)
elif mode in ('pipe-churn', 'eventfd-churn'):
    start = time.perf_counter_ns()
    for _ in range(iterations):
        if mode == 'pipe-churn':
            reader, writer = os.pipe()
            os.write(writer, b'x')
            assert os.read(reader, 1) == b'x'
            os.close(reader)
            os.close(writer)
        else:
            fd = os.eventfd(1, os.EFD_NONBLOCK)
            assert os.eventfd_read(fd) == 1
            os.close(fd)
    elapsed = time.perf_counter_ns() - start
elif mode == 'fstatvfs':
    with tempfile.TemporaryDirectory(prefix='fstatvfs-perf-', dir='/var/tmp') as directory:
        with open(directory + '/file', 'w') as stream:
            expected = os.fstatvfs(stream.fileno())
            start = time.perf_counter_ns()
            for _ in range(iterations):
                value = os.fstatvfs(stream.fileno())
                assert value.f_bsize == expected.f_bsize and value.f_fsid == expected.f_fsid
            elapsed = time.perf_counter_ns() - start
elif mode == 'tmpfs-io':
    import ctypes as c
    libc = c.CDLL(None, use_errno=True)
    libc.mount.argtypes = [c.c_char_p, c.c_char_p, c.c_char_p, c.c_ulong, c.c_void_p]
    libc.umount.argtypes = [c.c_char_p]
    with tempfile.TemporaryDirectory(prefix='tmpfs-perf-') as directory:
        target = os.fsencode(directory)
        assert libc.mount(b'tmpfs', target, b'tmpfs', 0, b'size=4m') == 0, c.get_errno()
        try:
            fd = os.open(directory + '/file', os.O_CREAT | os.O_RDWR, 0o600)
            alias = os.dup(fd)
            try:
                os.write(fd, b'x')
                start = time.perf_counter_ns()
                for _ in range(iterations):
                    assert os.lseek(alias, 0, os.SEEK_SET) == 0
                    assert os.read(fd, 1) == b'x'
                    assert os.lseek(alias, 0, os.SEEK_CUR) == 1
                    assert os.fstat(fd).st_size == 1
                elapsed = time.perf_counter_ns() - start
            finally:
                os.close(alias)
                os.close(fd)
        finally:
            assert libc.umount(target) == 0, c.get_errno()
else:
    raise ValueError(mode)

print('PERF_OK ' + json.dumps(dict(mode=mode, iterations=iterations,
    elapsed_ms=elapsed/1e6, ns_per_operation=elapsed/iterations)), flush=True)
print('PERF_END', flush=True)
