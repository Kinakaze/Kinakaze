import array
import ctypes as c
import errno
import os
import select
import socket
import struct
import sys
import time
import threading

lib = c.CDLL(None, use_errno=True)
lib.inotify_init1.argtypes = [c.c_int]
lib.inotify_add_watch.argtypes = [c.c_int, c.c_char_p, c.c_uint]
lib.inotify_rm_watch.argtypes = [c.c_int, c.c_int]
mode, path = sys.argv[1:3]

def create():
    fd = lib.inotify_init1(os.O_NONBLOCK)
    assert fd >= 0, c.get_errno()
    wd = lib.inotify_add_watch(fd, path.encode(), 0x100)
    assert wd >= 0, c.get_errno()
    return fd, wd

def receive(fd, wd, name):
    deadline = time.monotonic() + 3
    while True:
        try:
            data = os.read(fd, 4096)
            break
        except BlockingIOError:
            assert time.monotonic() < deadline, 'watch did not deliver ' + name
            time.sleep(.01)
    offset = 0
    while offset < len(data):
        event_wd, mask, cookie, length = struct.unpack_from('iIII', data, offset)
        filename = data[offset + 16:offset + 16 + length].rstrip(b'\0')
        if event_wd == wd and filename == name.encode() and mask & 0x100:
            return
        offset += 16 + length
    raise AssertionError(data)

if mode in ('inotify-last-close', 'inotify-crash-owner'):
    os.makedirs(path)
    descriptors = [create()[0] for _ in range(48)]
    print('WATCH_BATCH_READY', flush=True)
    while not os.path.exists(path + '/close'):
        time.sleep(.01)
    for fd in descriptors:
        os.close(fd)
    print('WATCH_BATCH_CLOSED', flush=True)
    while not os.path.exists(path + '/stop'):
        time.sleep(.01)
    os.unlink(path + '/close')
    os.unlink(path + '/stop')
    os.rmdir(path)
elif mode.startswith('inotify'):
    os.makedirs(path, exist_ok=True)
    if mode == 'inotify-memory':
        lib.mount.argtypes = [c.c_char_p, c.c_char_p, c.c_char_p, c.c_ulong, c.c_void_p]
        assert lib.mount(b'tmpfs', path.encode(), b'tmpfs', 0, None) == 0, c.get_errno()
    if mode == 'inotify-rights':
        parent, child_socket = socket.socketpair()
        child = os.fork()
        if child == 0:
            parent.close()
            fd, wd = create()
            child_socket.sendmsg([str(wd).encode()],
                [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
            os._exit(0)
        child_socket.close()
        assert os.waitpid(child, 0) == (child, 0)
        # The creator has exited before the change and before FD import.
        open(path + '/after-exit', 'w').close()
        data, controls, _, _ = parent.recvmsg(32, socket.CMSG_SPACE(4))
        fd, = array.array('i', controls[0][2])
        receive(fd, int(data), 'after-exit')
        os.close(fd)
        parent.close()
        os.unlink(path + '/after-exit')
    elif mode == 'inotify-exec':
        fd, wd = create()
        os.set_inheritable(fd, True)
        child = os.fork()
        if child == 0:
            os.execv('/usr/bin/python3', ['/usr/bin/python3', '-c', sys.argv[3],
                'watch-after-exec', path, str(fd), str(wd)])
        os.close(fd)
        assert os.waitpid(child, 0) == (child, 0)
        os.unlink(path + '/exec-created')
    else:
        fd, wd = create()
        duplicate = os.dup(fd)
        os.close(fd)
        child = os.fork()
        if child == 0:
            open(path + '/child-created', 'w').close()
            receive(duplicate, wd, 'child-created')
            assert lib.inotify_rm_watch(duplicate, wd) == 0, c.get_errno()
            os._exit(0)
        assert os.waitpid(child, 0) == (child, 0)
        data = os.read(duplicate, 4096)
        assert len(data) == 16 and struct.unpack('iIII', data) == (wd, 0x8000, 0, 0), data
        assert lib.inotify_rm_watch(duplicate, wd) == -1 and c.get_errno() == errno.EINVAL
        os.close(duplicate)
        os.unlink(path + '/child-created')
    if mode == 'inotify-memory':
        lib.umount2.argtypes = [c.c_char_p, c.c_int]
        assert lib.umount2(path.encode(), 0) == 0, c.get_errno()
    os.rmdir(path)
elif mode == 'watch-after-exec':
    fd, wd = map(int, sys.argv[3:5])
    open(path + '/exec-created', 'w').close()
    receive(fd, wd, 'exec-created')
    os.close(fd)
elif mode == 'timer-rights':
    class Timespec(c.Structure):
        _fields_ = [('seconds', c.c_long), ('nanoseconds', c.c_long)]
    class TimerSpec(c.Structure):
        _fields_ = [('interval', Timespec), ('value', Timespec)]
    lib.timerfd_create.argtypes = [c.c_int, c.c_int]
    lib.timerfd_settime.argtypes = [c.c_int, c.c_int, c.POINTER(TimerSpec), c.c_void_p]
    parent, child_socket = socket.socketpair()
    child = os.fork()
    if child == 0:
        parent.close()
        fd = lib.timerfd_create(0, os.O_NONBLOCK)
        assert fd >= 0, c.get_errno()
        setting = TimerSpec(Timespec(), Timespec(int(time.time()) + 3600, 0))
        assert lib.timerfd_settime(fd, 3, c.byref(setting), None) == 0, c.get_errno()
        child_socket.sendmsg([b'timer'],
            [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
        os._exit(0)
    child_socket.close()
    assert os.waitpid(child, 0) == (child, 0)
    print('TIMER_CLOCK_READY', flush=True)
    while not os.path.exists(path):
        time.sleep(.01)
    data, controls, _, _ = parent.recvmsg(32, socket.CMSG_SPACE(4))
    fd, = array.array('i', controls[0][2])
    try:
        os.read(fd, 8)
    except OSError as error:
        assert error.errno == errno.ECANCELED, error
    else:
        raise AssertionError('clock cancellation was lost while timer was queued')
    os.close(fd)
    parent.close()
    os.unlink(path)
elif mode.startswith('epoll-remote-'):
    kind = mode.removeprefix('epoll-remote-')
    sock = None
    if kind == 'eventfd':
        fd = os.eventfd(1, os.EFD_NONBLOCK)
    elif kind == 'inotify':
        os.makedirs(path)
        fd, wd = create()
        open(path + '/ready', 'w').close()
    elif kind == 'netlink':
        sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW | socket.SOCK_NONBLOCK, 0)
        sock.bind((0, 0))
        sock.sendto(struct.pack('<IHHII', 32, 0x7777, 1, 32, 0) + bytes(16), (0, 0))
        fd = sock.fileno()
    else:
        raise AssertionError(kind)
    ep = select.epoll()
    ep.register(fd, select.EPOLLIN)
    upstream, downstream = socket.socketpair()
    child = os.fork()
    if child == 0:
        upstream.close()
        if sock is not None:
            sock.close()
        else:
            os.close(fd)
        assert ep.poll(2) == [(fd, select.EPOLLIN)], 'local close lost remote target'
        assert ep.poll(0) == [(fd, select.EPOLLIN)]
        downstream.sendall(b'ready')
        assert downstream.recv(16) == b'closed'
        assert ep.poll(0) == [], 'watch retained the last target reference'
        os._exit(0)
    downstream.close()
    assert upstream.recv(16) == b'ready'
    if sock is not None:
        sock.close()
    else:
        os.close(fd)
    upstream.sendall(b'closed')
    assert os.waitpid(child, 0) == (child, 0)
    upstream.close()
    ep.close()
    if kind == 'inotify':
        os.unlink(path + '/ready')
        os.rmdir(path)
elif mode == 'epoll-only-rights':
    parent, child_socket = socket.socketpair()
    child = os.fork()
    if child == 0:
        parent.close()
        counter = os.eventfd(1, os.EFD_NONBLOCK)
        ep = select.epoll()
        ep.register(counter, select.EPOLLIN)
        child_socket.sendmsg([str(counter).encode()],
            [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [ep.fileno()]))])
        assert child_socket.recv(16) == b'close'
        os.close(counter)
        child_socket.sendall(b'closed')
        os._exit(0)
    child_socket.close()
    data, controls, _, _ = parent.recvmsg(32, socket.CMSG_SPACE(4))
    epfd, = array.array('i', controls[0][2])
    ep = select.epoll.fromfd(epfd)
    assert ep.poll(0) == [(int(data), select.EPOLLIN)]
    parent.sendall(b'close')
    assert parent.recv(16) == b'closed'
    assert os.waitpid(child, 0) == (child, 0)
    assert ep.poll(0) == []
    ep.close()
    parent.close()
elif mode == 'epoll-threaded-fork':
    counter = os.eventfd(1, os.EFD_NONBLOCK)
    ep = select.epoll()
    ep.register(counter, select.EPOLLIN | select.EPOLLONESHOT)
    stopping = threading.Event()
    def update_interest():
        while not stopping.is_set():
            ep.modify(counter, select.EPOLLIN | select.EPOLLONESHOT)
    writer = threading.Thread(target=update_interest)
    writer.start()
    try:
        for _ in range(6):
            child = os.fork()
            if child == 0:
                assert len(ep.poll(0)) <= 1
                os._exit(0)
            assert os.waitpid(child, 0) == (child, 0)
    finally:
        stopping.set()
        writer.join()
    ep.close()
    os.close(counter)
elif mode == 'epoll-race':
    counter = os.eventfd(1, os.EFD_NONBLOCK)
    ep = select.epoll()
    ep.register(counter, select.EPOLLIN | select.EPOLLONESHOT)
    start_r, start_w = os.pipe()
    out_r, out_w = os.pipe()
    children = []
    for _ in range(6):
        child = os.fork()
        if child == 0:
            os.close(start_w)
            os.close(out_r)
            assert os.read(start_r, 1) == b'x'
            os.write(out_w, bytes([len(ep.poll(0))]))
            os._exit(0)
        children.append(child)
    os.close(start_r)
    os.close(out_w)
    os.write(start_w, b'x' * len(children))
    os.close(start_w)
    results = bytearray()
    while len(results) < len(children):
        results.extend(os.read(out_r, len(children)))
    for child in children:
        assert os.waitpid(child, 0) == (child, 0)
    assert sum(results) == 1, results
    os.close(out_r)
    os.close(counter)
    ep.close()
elif mode == 'epoll-rights':
    parent, child_socket = socket.socketpair()
    child = os.fork()
    if child == 0:
        parent.close()
        counter = os.eventfd(1, os.EFD_NONBLOCK)
        ep = select.epoll()
        ep.register(counter, select.EPOLLIN | select.EPOLLONESHOT)
        child_socket.sendmsg([str(counter).encode()],
            [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [ep.fileno(), counter]))])
        os._exit(0)
    child_socket.close()
    assert os.waitpid(child, 0) == (child, 0)
    data, controls, _, _ = parent.recvmsg(32, socket.CMSG_SPACE(8))
    epfd, counter = array.array('i', controls[0][2])
    ep = select.epoll.fromfd(epfd)
    assert ep.poll(0) == [(int(data), select.EPOLLIN)]
    assert ep.poll(0) == []
    os.close(counter)
    ep.close()
    parent.close()
elif mode == 'epoll-shared':
    counter = os.eventfd(1, os.EFD_NONBLOCK)
    ep = select.epoll()
    ep.register(counter, select.EPOLLIN | select.EPOLLONESHOT)
    child = os.fork()
    if child == 0:
        assert ep.poll(0) == [(counter, select.EPOLLIN)]
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)
    assert ep.poll(0) == [], 'fork copied one-shot delivery state'
    child = os.fork()
    if child == 0:
        ep.modify(counter, select.EPOLLIN | select.EPOLLONESHOT)
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)
    assert ep.poll(0) == [(counter, select.EPOLLIN)], 'child rearm was lost'
    child = os.fork()
    if child == 0:
        ep.unregister(counter)
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)
    try:
        ep.modify(counter, select.EPOLLIN)
    except FileNotFoundError:
        pass
    else:
        raise AssertionError('child deletion was lost')
    ep.close()
    os.close(counter)
else:
    raise AssertionError(mode)
print('PASS_' + mode, flush=True)
