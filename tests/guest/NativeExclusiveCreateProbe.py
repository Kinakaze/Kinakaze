"""Exclusive native creates preserve inherited ownership and path semantics."""
import errno
import fcntl
import os
import shutil
import stat
import tempfile

directory = tempfile.mkdtemp(prefix='native-exclusive-create-', dir='/var/tmp')
saved = os.getcwd()
saved_umask = os.umask(0o027)
try:
    os.mkdir(directory + '/parent')
    os.chown(directory + '/parent', 0, 45678)
    os.chmod(directory + '/parent', 0o2755)
    os.chdir(directory + '/parent')
    flags = os.O_CREAT | os.O_EXCL | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK
    fd = os.open('payload', flags, 0o666)
    details = os.fstat(fd)
    assert (stat.S_IMODE(details.st_mode), details.st_gid) == (0o640, 45678), details
    assert fcntl.fcntl(fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
    assert fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_NONBLOCK
    assert os.write(fd, b'original') == 8
    os.fsync(fd)
    os.rename('payload', 'moved')
    os.unlink('moved')
    replacement = os.open('payload', flags, 0o666)
    assert os.fstat(replacement).st_ino != details.st_ino
    os.write(replacement, b'replacement')
    os.lseek(fd, 0, os.SEEK_SET)
    assert os.read(fd, 64) == b'original'
    os.close(replacement)
    os.close(fd)
    os.symlink('missing-target', 'link')
    os.mkdir('child')
    os.mkfifo('fifo', 0o600)
    for name in ('payload', 'link', 'child', 'fifo'):
        try:
            os.open(name, flags, 0o600)
        except OSError as error:
            assert error.errno == errno.EEXIST, (name, error)
        else:
            raise AssertionError(name)
    assert not os.path.exists('missing-target')
    os.mkdir(directory + '/other')
    os.symlink('../other', 'parent-link')
    fd = os.open('parent-link/via-link', flags, 0o600)
    os.write(fd, b'followed parent')
    os.close(fd)
    with open(directory + '/other/via-link', 'rb') as stream:
        assert stream.read() == b'followed parent'
    os.chown(directory + '/parent', 0, 56789)
    fd = os.open('fresh-group', flags, 0o666)
    assert os.fstat(fd).st_gid == 56789
    os.close(fd)
    os.chmod(directory + '/parent', 0o755)
    fd = os.open('ordinary-group', flags, 0o666)
    assert os.fstat(fd).st_gid == os.getegid()
    os.close(fd)
    fd = os.open('readonly-new', flags, 0o400)
    assert os.write(fd, b'retained write access') == 21
    assert stat.S_IMODE(os.fstat(fd).st_mode) == 0o400
    os.fsync(fd)
    os.close(fd)
    parent = os.open('.', os.O_RDONLY | os.O_DIRECTORY)
    os.rename(directory + '/parent', directory + '/renamed-parent')
    os.mkdir(directory + '/parent')
    fd = os.open('relative-dirfd', flags, 0o600, dir_fd=parent)
    os.write(fd, b'retained directory')
    os.close(fd)
    assert os.path.exists(directory + '/renamed-parent/relative-dirfd')
    assert not os.path.exists(directory + '/parent/relative-dirfd')
    os.close(parent)
finally:
    os.chdir(saved)
    os.umask(saved_umask)
    shutil.rmtree(directory)
print('NATIVE_EXCLUSIVE_CREATE_OK', flush=True)
