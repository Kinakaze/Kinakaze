"""Cgroup idle polling must retain membership, deletion and inotify events."""
import ctypes
import os
import select
import struct

libc = ctypes.CDLL('libc.so.6', use_errno=True)
group = '/sys/fs/cgroup/kinakaze-poll-probe-' + str(os.getpid())
os.mkdir(group)
fd = os.open(group + '/cgroup.events', os.O_RDONLY | os.O_CLOEXEC)
notify = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
assert notify >= 0, ctypes.get_errno()
watch = libc.inotify_add_watch(notify, (group + '/cgroup.events').encode(), 2)
assert watch >= 0, ctypes.get_errno()
inner, outer = select.epoll(), select.epoll()
inner.register(fd, select.EPOLLPRI)
outer.register(inner, select.EPOLLIN)
commands_r, commands_w = os.pipe()
child = None


def read_events():
    os.lseek(fd, 0, os.SEEK_SET)
    return os.read(fd, 4096)


def changed(populated):
    assert outer.poll(3), 'nested epoll lost membership notification'
    assert inner.poll(0)[0][1] & select.EPOLLPRI
    assert b'populated ' + str(populated).encode() in read_events()
    assert not inner.poll(0), 'reading did not acknowledge cgroup event'
    data = os.read(notify, 4096)
    assert struct.unpack_from('iI', data) == (watch, 2), data


try:
    assert b'populated 0' in read_events()
    assert not outer.poll(.05)
    child = os.fork()
    if child == 0:
        try:
            os.close(commands_w)
            assert os.read(commands_r, 1) == b'j'
            with open(group + '/cgroup.procs', 'w') as target:
                target.write(str(os.getpid()))
            assert os.read(commands_r, 1) == b'x'
            os._exit(0)
        except BaseException:
            os._exit(1)
    os.close(commands_r)
    commands_r = -1
    os.write(commands_w, b'j')
    changed(1)
    assert not outer.poll(.05)
    os.write(commands_w, b'x')
    assert os.waitpid(child, 0)[1] == 0
    child = None
    changed(0)
    os.rmdir(group)
    os.mkdir(group)
    # A same-name replacement cannot retarget an open fd to a new generation.
    assert inner.poll(1)[0][1] & select.EPOLLPRI
    with open(group + '/cgroup.events') as replacement:
        assert 'populated 0' in replacement.read()
    print('CGROUP_POLL_OK', flush=True)
finally:
    if child:
        try:
            os.kill(child, 9)
            os.waitpid(child, 0)
        except ProcessLookupError:
            pass
    inner.close()
    outer.close()
    for descriptor in (fd, notify, commands_r, commands_w):
        if descriptor >= 0:
            os.close(descriptor)
    os.rmdir(group)
