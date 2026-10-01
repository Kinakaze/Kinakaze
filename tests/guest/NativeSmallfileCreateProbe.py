"""Nonexclusive native creation preserves existing-inode and fallback semantics."""
import errno
import fcntl
import os
import shutil
import stat
import tempfile
import traceback


def fails(code, path, flags):
    try:
        descriptor = os.open(path, flags, 0o666)
    except OSError as error:
        assert error.errno == code, (path, error)
    else:
        os.close(descriptor)
        raise AssertionError(('expected error', code, path))


directory = tempfile.mkdtemp(prefix='native-smallfile-create-', dir='/var/tmp')
saved, mask = os.getcwd(), os.umask(0o027)
try:
    parent = directory + '/parent'
    os.mkdir(parent)
    os.chown(parent, 0, 45678)
    os.chmod(parent, 0o2755)
    os.chdir(parent)
    flags = os.O_CREAT | os.O_RDWR | os.O_CLOEXEC | os.O_NONBLOCK
    descriptor = os.open('payload', flags | os.O_TRUNC, 0o666)
    original = os.fstat(descriptor)
    assert (stat.S_IMODE(original.st_mode), original.st_gid) == (0o640, 45678), original
    assert fcntl.fcntl(descriptor, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
    assert fcntl.fcntl(descriptor, fcntl.F_GETFL) & os.O_NONBLOCK
    os.write(descriptor, b'original')
    os.fsync(descriptor)
    os.close(descriptor)
    descriptor = os.open('payload', flags, 0o777)
    assert os.read(descriptor, 64) == b'original'
    assert stat.S_IMODE(os.fstat(descriptor).st_mode) == 0o640
    os.close(descriptor)
    descriptor = os.open('payload', flags | os.O_TRUNC, 0o777)
    assert os.fstat(descriptor).st_ino == original.st_ino
    assert os.fstat(descriptor).st_size == 0
    assert stat.S_IMODE(os.fstat(descriptor).st_mode) == 0o640
    os.write(descriptor, b'truncated')
    os.close(descriptor)
    descriptor = os.open('payload', flags | os.O_APPEND, 0o777)
    os.write(descriptor, b'+appended')
    os.close(descriptor)
    assert open('payload', 'rb').read() == b'truncated+appended'

    os.symlink('payload', 'existing-link')
    descriptor = os.open('existing-link', flags | os.O_TRUNC, 0o777)
    os.write(descriptor, b'followed')
    os.close(descriptor)
    assert open('payload', 'rb').read() == b'followed'
    assert os.path.islink('existing-link')
    fails(errno.ELOOP, 'existing-link', flags | os.O_NOFOLLOW)
    os.symlink('missing-target', 'dangling-link')
    descriptor = os.open('dangling-link', flags | os.O_TRUNC, 0o600)
    os.write(descriptor, b'new target')
    os.close(descriptor)
    assert open('missing-target', 'rb').read() == b'new target'
    os.mkdir('child')
    fails(errno.EISDIR, 'child', flags | os.O_TRUNC)
    for name in ('payload', 'existing-link', 'dangling-link', 'child'):
        fails(errno.EEXIST, name, flags | os.O_EXCL)

    descriptor = os.open('readonly-new', flags | os.O_TRUNC, 0o400)
    os.write(descriptor, b'retained access')
    assert stat.S_IMODE(os.fstat(descriptor).st_mode) == 0o400
    os.close(descriptor)
    descriptor = os.open('readonly-new', flags | os.O_TRUNC, 0o777)
    os.write(descriptor, b'root override')
    assert stat.S_IMODE(os.fstat(descriptor).st_mode) == 0o400
    os.close(descriptor)

    os.chown(parent, 0, 56789)
    descriptor = os.open('fresh-group', flags, 0o666)
    assert os.fstat(descriptor).st_gid == 56789
    os.close(descriptor)
    os.chmod(parent, 0o755)
    descriptor = os.open('ordinary-group', flags, 0o666)
    assert os.fstat(descriptor).st_gid == os.getegid()
    os.close(descriptor)
    os.mkdir(directory + '/other')
    os.symlink('../other', 'parent-link')
    descriptor = os.open('parent-link/followed', flags, 0o600)
    os.close(descriptor)
    assert os.path.isfile(directory + '/other/followed')
    retained = os.open('.', os.O_RDONLY | os.O_DIRECTORY)
    os.rename(parent, parent + '.moved')
    os.mkdir(parent)
    descriptor = os.open('by-dirfd', flags, 0o600, dir_fd=retained)
    os.close(descriptor)
    assert os.path.isfile(parent + '.moved/by-dirfd')
    assert not os.path.exists(parent + '/by-dirfd')
    os.close(retained)

    os.chdir(parent)
    os.chown(parent, 0, 45678)
    os.chmod(parent, 0o2755)
    reader, writer = os.pipe()
    children = []
    for index in range(4):
        pid = os.fork()
        if pid == 0:
            os.close(writer)
            assert os.read(reader, 1) == b'x'
            os.close(reader)
            descriptor = os.open('race', flags, 0o666)
            details = os.fstat(descriptor)
            assert (stat.S_IMODE(details.st_mode), details.st_gid) == (0o640, 45678)
            assert os.pwrite(descriptor, bytes([65 + index]), index) == 1
            os.close(descriptor)
            os._exit(0)
        children.append(pid)
    os.close(reader)
    os.write(writer, b'xxxx')
    os.close(writer)
    assert all(os.waitpid(pid, 0)[1] == 0 for pid in children)
    assert open('race', 'rb').read() == b'ABCD'

    os.chmod(directory, 0o755)
    os.chmod(parent, 0o555)
    os.chmod('race', 0o666)
    pid = os.fork()
    if pid == 0:
        try:
            os.setgroups([])
            os.setgid(65534)
            os.setuid(65534)
            fails(errno.EACCES, parent + '/denied', flags | os.O_TRUNC)
            descriptor = os.open(parent + '/race', flags | os.O_TRUNC, 0o777)
            os.write(descriptor, b'existing leaf allowed')
            os.close(descriptor)
        except BaseException:
            traceback.print_exc()
            os._exit(1)
        os._exit(0)
    assert os.waitpid(pid, 0)[1] == 0
    assert not os.path.exists('denied')
    assert open('race', 'rb').read() == b'existing leaf allowed'
    os.chmod(parent, 0o755)
finally:
    os.chdir(saved)
    os.umask(mask)
    shutil.rmtree(directory)
print('NATIVE_SMALLFILE_CREATE_OK', flush=True)
