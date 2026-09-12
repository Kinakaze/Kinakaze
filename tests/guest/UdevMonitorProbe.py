"""Exercise the real libudev monitor and its passive guest-device endpoint."""
import ctypes as C
import errno
import fcntl
import os
import select
import socket


def bind(lib, name, result, *arguments):
    function = getattr(lib, name)
    function.restype, function.argtypes = result, arguments
    return function


P, I = C.c_void_p, C.c_int
udev = C.CDLL('libudev.so.1', use_errno=True)
context = bind(udev, 'udev_new', P)()
assert context
create = bind(udev, 'udev_monitor_new_from_netlink', P, P, C.c_char_p)
release = bind(udev, 'udev_monitor_unref', P, P)
enable = bind(udev, 'udev_monitor_enable_receiving', I, P)
getfd = bind(udev, 'udev_monitor_get_fd', I, P)
receive = bind(udev, 'udev_monitor_receive_device', P, P)
add_filter = bind(udev, 'udev_monitor_filter_add_match_subsystem_devtype', I, P, C.c_char_p, C.c_char_p)
set_buffer = bind(udev, 'udev_monitor_set_receive_buffer_size', I, P, I)

for channel in [b'udev', b'kernel']:
    monitor = create(context, channel)
    assert monitor, (channel, C.get_errno())
    assert add_filter(monitor, b'input', None) == 0
    assert set_buffer(monitor, 128 * 1024) >= 0, C.get_errno()
    assert enable(monitor) == 0, C.get_errno()
    fd = getfd(monitor)
    assert fd >= 0
    assert fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_NONBLOCK
    assert fcntl.fcntl(fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
    with select.epoll() as poller:
        poller.register(fd, select.EPOLLIN)
        assert poller.poll(0.01) == []
        assert not receive(monitor)
        assert C.get_errno() == errno.EAGAIN, C.get_errno()
        poller.unregister(fd)
    assert not release(monitor)
    try:
        os.fstat(fd)
        raise AssertionError('monitor did not close its descriptor')
    except OSError as error:
        assert error.errno == errno.EBADF
bind(udev, 'udev_unref', P, P)(context)

with socket.socket(socket.AF_NETLINK, socket.SOCK_RAW | socket.SOCK_NONBLOCK, 15) as monitor:
    monitor.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
    monitor.bind((0, 3))
    port, groups = monitor.getsockname()
    assert port and groups == 3
    # Netlink port IDs are local to the protocol, not shared with route sockets.
    with socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0) as route:
        route.bind((port, 0))
    duplicate = os.dup(monitor.fileno())
    with socket.socket(fileno=duplicate) as other:
        assert other.getsockname() == (port, 3)
        assert other.getsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED) == 1
    child = os.fork()
    if child == 0:
        assert monitor.getsockname() == (port, 3)
        assert monitor.getsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED) == 1
        try:
            monitor.recv(1024)
            os._exit(2)
        except BlockingIOError:
            os._exit(0)
    _, status = os.waitpid(child, 0)
    assert status == 0, status

print('UDEV_MONITOR_OK', flush=True)
