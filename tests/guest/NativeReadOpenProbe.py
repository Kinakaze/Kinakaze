"""Read-only native opens retain Linux descriptor and fallback semantics."""
import errno
import fcntl
import os
import tempfile

def fails(code, call):
    try:
        value = call()
    except OSError as error:
        assert error.errno == code, (error.errno, code)
    else:
        if isinstance(value, int):
            os.close(value)
        raise AssertionError(('expected error', code))

with tempfile.TemporaryDirectory(prefix='native-read-open-', dir='/var/tmp') as directory:
    path = directory + '/payload'
    with open(path, 'wb') as stream:
        stream.write(b'original')
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC | os.O_NOFOLLOW)
    alias = os.dup(fd)
    try:
        assert fcntl.fcntl(fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
        assert fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_NONBLOCK
        assert os.read(fd, 3) == b'ori'
        assert os.read(alias, 2) == b'gi'
        assert os.pread(fd, 8, 0) == b'original'
        os.rename(path, directory + '/old')
        os.unlink(directory + '/old')
        with open(path, 'wb') as stream:
            stream.write(b'replaced')
        assert os.pread(alias, 8, 0) == b'original'
    finally:
        os.close(alias)
        os.close(fd)
    os.symlink('payload', directory + '/link')
    with open(directory + '/link', 'rb') as stream:
        assert stream.read() == b'replaced'
    fails(errno.ELOOP, lambda: os.open(directory + '/link', os.O_RDONLY | os.O_NOFOLLOW))
    fails(errno.ENOTDIR, lambda: os.open(path + '/', os.O_RDONLY))
    fails(errno.ENOTDIR, lambda: os.open(path + '/.', os.O_RDONLY))
    fails(errno.ENOTDIR, lambda: os.open(path, os.O_RDONLY | os.O_DIRECTORY))
    os.mkdir(directory + '/sub')
    os.mkdir(directory + '/sub/deep')
    with open(directory + '/sub/payload', 'wb') as stream:
        stream.write(b'via-link-parent')
    os.symlink('sub/deep', directory + '/directory-link')
    fd = os.open(directory + '/directory-link/', os.O_RDONLY | os.O_NOFOLLOW)
    os.close(fd)
    with open(directory + '/directory-link/../payload', 'rb') as stream:
        assert stream.read() == b'via-link-parent'
    os.mkfifo(directory + '/fifo', 0o600)
    fd = os.open(directory + '/fifo', os.O_RDONLY | os.O_NONBLOCK)
    assert os.read(fd, 1) == b''
    os.close(fd)
    fd = os.open(directory, os.O_RDONLY)
    os.close(fd)
print('NATIVE_READ_OPEN_OK', flush=True)
