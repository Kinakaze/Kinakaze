"""Exclusive payload creation preserves inheritance, permissions and mount gates."""
import ctypes
import errno
import fcntl
import os
import stat
import tempfile
import traceback

flags = os.O_CREAT | os.O_EXCL | os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW


def fails(code, operation):
    try:
        fd = operation()
    except OSError as error:
        assert error.errno == code, (error.errno, code)
    else:
        os.close(fd)
        raise AssertionError(('expected errno', code))


with tempfile.TemporaryDirectory(prefix='exclusive-create-', dir='/var/tmp') as directory:
    parent = directory + '/parent'
    os.mkdir(parent)
    os.chown(parent, 0, 1234)
    os.chmod(parent, 0o2770)
    path = parent + '/payload'
    fd = os.open(path, flags, 0)
    initial = os.fstat(fd)
    assert stat.S_ISREG(initial.st_mode) and initial.st_mode & 0o7777 == 0
    assert initial.st_gid == 1234 and initial.st_uid == os.geteuid()
    assert fcntl.fcntl(fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
    os.write(fd, b'original')
    os.fsync(fd)
    os.fchmod(fd, 0o640)
    duplicate = os.dup(fd)
    os.rename(path, path + '.old')
    os.unlink(path + '.old')
    replacement = os.open(path, flags, 0o600)
    os.write(replacement, b'replacement')
    os.close(replacement)
    assert os.pread(duplicate, 64, 0) == b'original'
    assert os.fstat(duplicate).st_ino == initial.st_ino
    os.close(duplicate)
    os.close(fd)
    os.mkfifo(parent + '/fifo')
    os.symlink('missing-target', parent + '/link')
    os.mkdir(parent + '/directory')
    for name in ('payload', 'fifo', 'link', 'directory'):
        fails(errno.EEXIST, lambda name=name: os.open(parent + '/' + name, flags, 0o777))
    assert open(path, 'rb').read() == b'replacement'
    assert not os.path.exists(parent + '/missing-target')

    os.symlink('parent', directory + '/parent-link')
    fd = os.open(directory + '/parent-link/followed', flags, 0o600)
    os.close(fd)
    assert os.path.isfile(parent + '/followed')
    retained = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
    os.rename(parent, parent + '.moved')
    os.mkdir(parent)
    fd = os.open('by-dirfd', flags, 0o600, dir_fd=retained)
    os.close(fd)
    os.close(retained)
    assert os.path.isfile(parent + '.moved/by-dirfd')
    assert not os.path.exists(parent + '/by-dirfd')
    saved = os.getcwd()
    try:
        os.chdir(parent)
        fd = os.open('./relative', flags, 0o600)
        os.close(fd)
        assert os.path.isfile('relative')
    finally:
        os.chdir(saved)

    reader, writer = os.pipe()
    children = []
    race = parent + '/race'
    for _ in range(4):
        pid = os.fork()
        if pid == 0:
            os.close(writer)
            assert os.read(reader, 1) == b'x'
            os.close(reader)
            try:
                fd = os.open(race, flags, 0o600)
            except OSError as error:
                os._exit(17 if error.errno == errno.EEXIST else 1)
            os.write(fd, str(os.getpid()).encode())
            os.close(fd)
            os._exit(0)
        children.append(pid)
    os.close(reader)
    os.write(writer, b'xxxx')
    os.close(writer)
    results = [os.waitpid(pid, 0)[1] for pid in children]
    assert results.count(0) == 1 and results.count(17 << 8) == 3, results
    assert int(open(race).read()) in children

    child = os.fork()
    if child == 0:
        try:
            os.setgroups([])
            os.setgid(65534)
            os.setuid(65534)
            fails(errno.EACCES, lambda: os.open(parent + '/unprivileged', flags, 0o600))
        except BaseException:
            traceback.print_exc()
            os._exit(1)
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)
    assert not os.path.exists(parent + '/unprivileged')

    # Both write and search rights on the selected parent are required. These
    # cases also exercise the ordinary fallback with owner and group grants.
    os.chmod(directory, 0o755)
    cases = [(0, 0, 0o555, [], False), (0, 0, 0o666, [], False),
             (65534, 0, 0o700, [], True), (0, 1234, 0o2770, [1234], True),
             (0, 1234, 0o750, [1234], False), (0, 0, 0o733, [], True)]
    for index, (uid, gid, mode, groups, allowed) in enumerate(cases):
        selected = directory + '/permission-' + str(index)
        os.mkdir(selected)
        with open(selected + '/existing', 'wb') as stream:
            stream.write(b'unchanged')
        os.chown(selected, uid, gid)
        os.chmod(selected, mode)
        child = os.fork()
        if child == 0:
            try:
                os.setgroups(groups)
                os.setgid(65534)
                os.setuid(65534)
                if allowed:
                    fd = os.open(selected + '/payload', flags, 0o600)
                    assert os.fstat(fd).st_uid == 65534
                    os.close(fd)
                else:
                    fails(errno.EACCES, lambda: os.open(selected + '/payload', flags, 0o600))
                # Existing leaves need no parent write permission, but the
                # parent must still be searchable before EEXIST is visible.
                fails(errno.EACCES if mode == 0o666 else errno.EEXIST,
                      lambda: os.open(selected + '/existing', flags, 0o600))
            except BaseException:
                traceback.print_exc()
                os._exit(1)
            os._exit(0)
        assert os.waitpid(child, 0) == (child, 0), index
        assert os.path.exists(selected + '/payload') == allowed, index
        assert open(selected + '/existing', 'rb').read() == b'unchanged'

    source, target = directory + '/source', directory + '/target'
    os.mkdir(source)
    os.mkdir(target)
    child = os.fork()
    if child == 0:
        libc = ctypes.CDLL(None, use_errno=True)
        assert libc.unshare(0x20000) == 0, ctypes.get_errno()
        assert libc.mount(os.fsencode(source), os.fsencode(target), None, 4096, None) == 0, ctypes.get_errno()
        try:
            assert libc.mount(None, os.fsencode(target), None, 4096 | 32 | 1, None) == 0, ctypes.get_errno()
            fails(errno.EROFS, lambda: os.open(target + '/readonly', flags, 0o600))
        finally:
            assert libc.umount2(os.fsencode(target), 0) == 0, ctypes.get_errno()
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)
    assert not os.path.exists(source + '/readonly')

print('NATIVE_EXCLUSIVE_CREATE_OK', flush=True)
