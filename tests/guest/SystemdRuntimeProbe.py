"""Guest-side regressions for systemd's libc, event and socket prerequisites."""
import ctypes as C
import errno
import os
import select
import socket
import signal
import stat
import struct
import tempfile

libc = C.CDLL('libc.so.6', use_errno=True)

# GNU basename results remain in each input, so unit/preset names can coexist.
libc.basename.argtypes = [C.c_void_p]
libc.basename.restype = C.c_void_p
first_name = C.create_string_buffer(b'/etc/systemd/system/default.target')
second_name = C.create_string_buffer(b'/lib/systemd/system/kinakaze.target')
first_base = libc.basename(first_name)
second_base = libc.basename(second_name)
assert C.string_at(first_base) == b'default.target'
assert C.string_at(second_base) == b'kinakaze.target'
assert first_base == C.addressof(first_name) + len(b'/etc/systemd/system/')
for raw in (b'', b'/', b'/trailing/'):
    value = C.create_string_buffer(raw)
    assert libc.basename(value) == C.addressof(value) + len(raw)
libc.__xpg_basename.argtypes = [C.c_void_p]
libc.__xpg_basename.restype = C.c_void_p
for raw, expected in ((b'', b'.'), (b'///', b'/'), (b'/usr/lib/', b'lib')):
    value = C.create_string_buffer(raw)
    assert C.string_at(libc.__xpg_basename(value)) == expected

# Removing inode write permission must not revoke an existing writable open.
with tempfile.NamedTemporaryFile() as writable:
    os.write(writable.fileno(), b'machine-id-placeholder')
    os.fchmod(writable.fileno(), 0o444)
    try:
        os.ftruncate(writable.fileno(), 0)
        assert os.fstat(writable.fileno()).st_size == 0
    finally:
        os.fchmod(writable.fileno(), 0o600)


def check(result):
    assert result >= 0, (result, C.get_errno())
    return result


# A child must inherit direct guest writes to libc's name globals. The buffer
# remains on guest-owned memory and must be writable there after fork.
name = C.create_string_buffer(b'/test/systemd-parent')
full = C.c_void_p.in_dll(libc, 'program_invocation_name')
short = C.c_void_p.in_dll(libc, 'program_invocation_short_name')
old = full.value, short.value
full.value, short.value = C.addressof(name), C.addressof(name) + 6
pid = os.fork()
if pid == 0:
    try:
        assert full.value == C.addressof(name)
        assert short.value == C.addressof(name) + 6
        C.memmove(full.value, b'(sd-executor)\0', 14)
        os._exit(0)
    except BaseException:
        os._exit(1)
assert os.waitpid(pid, 0)[1] == 0
assert name.value == b'/test/systemd-parent'
full.value, short.value = old

# %m consumes no argument before the following conversion.
buffer = C.create_string_buffer(128)
C.set_errno(errno.ENOENT)
check(libc.snprintf(buffer, len(buffer), b'%m:%d', 37))
assert buffer.value == b'No such file or directory:37', buffer.value

# signalfd must begin empty, return 128-byte records and consume only its mask.
selected = C.c_uint64(1 << (signal.SIGUSR1 - 1))
old_mask = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGUSR1, signal.SIGUSR2})
sfd = check(libc.signalfd(-1, C.byref(selected), os.O_NONBLOCK | os.O_CLOEXEC))
try:
    assert not select.select([sfd], [], [], 0)[0]
    os.kill(os.getpid(), signal.SIGUSR2)
    assert not select.select([sfd], [], [], 0)[0]
    os.kill(os.getpid(), signal.SIGUSR1)
    assert select.select([sfd], [], [], 1)[0]
    data = os.read(sfd, 256)
    assert len(data) == 128 and int.from_bytes(data[:4], 'little') == signal.SIGUSR1
    selected.value = 1 << (signal.SIGUSR2 - 1)
    assert libc.signalfd(sfd, C.byref(selected), 0) == sfd
    assert int.from_bytes(os.read(sfd, 128)[:4], 'little') == signal.SIGUSR2
finally:
    os.close(sfd)
    signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)

# A default-ignored but blocked SIGCHLD must reach signalfd. A signal-pump
# thread must not discard it using its own unrelated blocked mask.
old_action = signal.signal(signal.SIGCHLD, signal.SIG_DFL)
old_mask = signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGCHLD})
selected.value = 1 << (signal.SIGCHLD - 1)
sfd = check(libc.signalfd(-1, C.byref(selected), os.O_NONBLOCK | os.O_CLOEXEC))
try:
    for status in (17, 23):
        child = os.fork()
        if child == 0:
            os._exit(status)
        assert select.select([sfd], [], [], 3)[0], 'SIGCHLD lost'
        data = os.read(sfd, 128)
        assert int.from_bytes(data[:4], 'little') == signal.SIGCHLD
        assert os.waitpid(child, os.WNOHANG) == (child, status << 8)
finally:
    os.close(sfd)
    signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)
    signal.signal(signal.SIGCHLD, old_action)

assert libc.prctl(27, 0, 0, 0, 0) == 0
check(libc.prctl(28, 0, 0, 0, 0))


class Timespec(C.Structure):
    _fields_ = [('sec', C.c_int64), ('nsec', C.c_int64)]


class Itimerspec(C.Structure):
    _fields_ = [('interval', Timespec), ('value', Timespec)]


timer = check(libc.timerfd_create(0, os.O_NONBLOCK | os.O_CLOEXEC))
try:
    far_future = Itimerspec(Timespec(), Timespec(2**63 - 1, 0))
    check(libc.timerfd_settime(timer, 3, C.byref(far_future), None))
    assert not select.select([timer], [], [], 0)[0]
    try:
        os.read(timer, 8)
        raise AssertionError('far-future timer fired')
    except BlockingIOError:
        pass
finally:
    os.close(timer)

# sd-event watches O_PATH descriptors through /proc/self/fd, including links.
with tempfile.TemporaryDirectory(prefix='systemd-inotify-') as directory:
    path = directory + '/watched'
    with open(path, 'w') as f:
        f.write('one')
    link = directory + '/link'
    os.symlink('watched', link)
    target = os.open(link, os.O_PATH | os.O_NOFOLLOW)
    notify = check(libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC))
    try:
        check(libc.inotify_add_watch(notify, ('/proc/self/fd/' + str(target)).encode(), 0x804))
    finally:
        os.close(notify)
        os.close(target)
    event = C.c_void_p()
    source = C.c_void_p()
    systemd = C.CDLL('libsystemd.so.0')
    check(systemd.sd_event_new(C.byref(event)))
    try:
        check(systemd.sd_event_add_inotify(event, C.byref(source), path.encode(), 0x20c, None, None))
    finally:
        systemd.sd_event_source_unref(source)
        systemd.sd_event_unref(event)

# sd-netlink requires NETLINK_PKTINFO, plus getsockname and membership queries.
netlink = C.c_void_p()
systemd_internal = C.CDLL('/usr/lib/x86_64-linux-gnu/systemd/libsystemd-shared-252.so')
check(systemd_internal.sd_netlink_open(C.byref(netlink)))
systemd_internal.sd_netlink_unref(netlink)
with socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0) as route:
    route.settimeout(3)
    route.setsockopt(270, 3, 1)
    route.bind((0, 0))
    assert route.getsockopt(270, 3) == 1
    assert route.getsockopt(270, 9, 4) == b'\0' * 4
    request = struct.pack('IHHII', 32, 18, 0x301, 42, 0) + b'\0' * 16
    route.sendto(request, (0, 0))
    payload, ancillary, flags, source = route.recvmsg(65536, socket.CMSG_SPACE(4))
    assert payload and source == (0, 0)
    assert (270, 3, b'\0' * 4) in ancillary, ancillary

# tmpfs socket names must follow inode identity through hard links, rename,
# unlink and a replacement bind; fork/exec clients share the same endpoint.
with tempfile.TemporaryDirectory(prefix='systemd-sockets-') as directory:
    mounts = os.open('/proc/self/mountinfo', os.O_RDONLY)
    alias_mounts = os.dup(mounts)
    mounts_events = select.epoll()
    mounts_events.register(mounts, select.EPOLLPRI)
    outer_events = select.epoll()
    outer_events.register(mounts_events.fileno(), select.EPOLLIN)
    try:
        mounts_events.register(outer_events.fileno(), select.EPOLLIN)
        raise AssertionError('cyclic epoll accepted')
    except OSError as error:
        assert error.errno == errno.ELOOP, error
    assert outer_events.poll(0) == []
    assert mounts_events.poll(0) == []
    check(libc.mount(b'tmpfs', directory.encode(), b'tmpfs', 0, None))
    try:
        assert outer_events.poll(1)
        events = mounts_events.poll(0)
        assert events and events[0][1] & select.EPOLLPRI, events
        reader = os.fork()
        if reader == 0:
            try:
                os.lseek(alias_mounts, 0, os.SEEK_SET)
                assert directory.encode() in os.read(alias_mounts, 65536)
                os._exit(0)
            except BaseException:
                os._exit(1)
        assert os.waitpid(reader, 0)[1] == 0
        assert mounts_events.poll(0) == []
        assert outer_events.poll(0) == []
        path, alias = directory + '/notify', directory + '/alias'
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as server:
            server.settimeout(3)
            server.bind(path)
            assert stat.S_ISSOCK(os.stat(path).st_mode)
            os.link(path, alias)
            server.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
            with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as sender:
                sender.bind(directory + '/sender')
                sender.sendmsg([b'credentials'], [], 0, path)
                payload, controls, flags, source = server.recvmsg(64, socket.CMSG_SPACE(12))
                assert payload == b'credentials' and source == directory + '/sender'
                assert flags == 0
                assert controls[0][:2] == (socket.SOL_SOCKET, socket.SCM_CREDENTIALS)
                assert struct.unpack('3i', controls[0][2]) == (os.getpid(), os.getuid(), os.getgid())
                sender.sendto(b'', path)
                assert server.recv(4) == b''
                sender.sendto(b'oversized', path)
                assert server.recv(2, socket.MSG_PEEK) == b'ov'
                data, _, flags, _ = server.recvmsg(3, socket.CMSG_SPACE(12))
                assert data == b'ove' and flags & socket.MSG_TRUNC
            os.unlink(directory + '/sender')
            poll = select.epoll()
            poll.register(server, select.EPOLLIN | select.EPOLLET)
            try:
                assert not poll.poll(0)
                for message in (b'first-edge', b'second-edge'):
                    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as sender:
                        sender.sendto(message, path)
                    assert poll.poll(1) == [(server.fileno(), select.EPOLLIN)]
                    assert server.recv(32) == message
                    server.setblocking(False)
                    try:
                        server.recv(32, socket.MSG_DONTWAIT)
                        raise AssertionError('empty queue did not return EAGAIN')
                    except BlockingIOError:
                        pass
                    finally:
                        server.settimeout(3)
            finally:
                poll.close()
            with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as client:
                client.connect(alias)
                client.send(b'one')
                assert server.recv(32) == b'one'
                os.unlink(path)
                os.unlink(alias)
                with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as replacement:
                    replacement.settimeout(3)
                    replacement.bind(path)
                    client.send(b'old-inode')
                    assert server.recv(32) == b'old-inode'
                    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as other:
                        other.connect(path)
                        other.send(b'new-inode')
                    assert replacement.recv(32) == b'new-inode'
        os.unlink(path)
        notify = check(libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC))
        try:
            watched = check(libc.inotify_add_watch(notify, directory.encode(), 0x3c6))
            child = os.fork()
            if child == 0:
                os.execl('/usr/bin/python3', 'python3', '-c',
                         'import os,sys; p=sys.argv[1]+"/event"; '
                         'f=open(p,"w"); f.write("changed"); f.close(); '
                         'os.rename(p,p+"-renamed"); os.unlink(p+"-renamed")', directory)
            assert os.waitpid(child, 0)[1] == 0
            assert select.select([notify], [], [], 2)[0]
            data = os.read(notify, 65536)
            events = []
            while data:
                wd, mask, cookie, length = struct.unpack_from('iIII', data)
                events.append((wd, mask, cookie, data[16:16+length].split(b'\0')[0]))
                data = data[16+length:]
            assert any(wd == watched and mask & 0x100 and name == b'event' for wd, mask, _, name in events), events
            assert any(mask & 2 and name == b'event' for _, mask, _, name in events), events
            moved_from = [cookie for _, mask, cookie, name in events if mask & 0x40 and name == b'event']
            moved_to = [cookie for _, mask, cookie, name in events if mask & 0x80 and name == b'event-renamed']
            assert moved_from and moved_from == moved_to and moved_from[0] != 0, events
            assert any(mask & 0x200 and name == b'event-renamed' for _, mask, _, name in events), events
        finally:
            os.close(notify)
        # sshd enters /run/sshd after systemd mounts /run as tmpfs.
        jail = directory + '/jail'
        os.mkdir(jail)
        with open(jail + '/inside', 'w') as f:
            f.write('tmpfs-root')
        child = os.fork()
        if child == 0:
            try:
                os.chroot(jail)
                os.chdir('/')
                assert open('/inside').read() == 'tmpfs-root'
                assert open('/../../inside').read() == 'tmpfs-root'
                assert not os.path.exists('/etc/shadow')
                nested = os.fork()
                if nested == 0:
                    os._exit(0 if open('/inside').read() == 'tmpfs-root' else 1)
                assert os.waitpid(nested, 0)[1] == 0
                os._exit(0)
            except BaseException:
                import traceback
                traceback.print_exc()
                os._exit(1)
        assert os.waitpid(child, 0)[1] == 0
        os.unlink(jail + '/inside')
        os.rmdir(jail)
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
            server.settimeout(3)
            server.bind(path)
            server.listen(2)
            pid = os.fork()
            if pid == 0:
                os.execl('/usr/bin/python3', 'python3', '-c',
                         'import socket,sys; s=socket.socket(socket.AF_UNIX); '
                         's.connect(sys.argv[1]); s.sendall(b"exec-client"); s.close()', path)
            connection, _ = server.accept()
            with connection:
                assert connection.recv(32) == b'exec-client'
            assert os.waitpid(pid, 0)[1] == 0
        os.unlink(path)
    finally:
        check(libc.umount(directory.encode()))
        assert mounts_events.poll(1)
        outer_events.close()
        mounts_events.close()
        os.close(alias_mounts)
        os.close(mounts)

print('SYSTEMD_RUNTIME_PREREQUISITES_OK', flush=True)
