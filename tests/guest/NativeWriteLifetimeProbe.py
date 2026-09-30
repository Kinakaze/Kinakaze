"""Checked native writers retain inode ownership across dup, fork and exec."""
import errno
import os
from pathlib import Path
import sys
import tempfile


with tempfile.TemporaryDirectory(prefix='native-writer-', dir='/var/tmp') as name:
    path = Path(name) / 'file'
    for access in (os.O_WRONLY, os.O_RDWR):
        path.write_bytes(b'original')
        reader = os.open(path, os.O_RDONLY)
        writer = os.open(path, access)
        alias = os.dup(writer)
        os.set_inheritable(alias, True)
        gate, release = os.pipe()
        child = os.fork()
        if child == 0:
            try:
                os.close(release)
                os.close(writer)
                os.close(reader)
                assert os.read(gate, 1) == b'x'
                os.close(gate)
                os.execv(sys.executable, [sys.executable, '-c',
                    'import os,sys; fd=int(sys.argv[1]); '
                    'assert os.write(fd,b"updated!")==8; '
                    'assert os.pwrite(fd,b"X",2)==1; os.close(fd)', str(alias)])
            except BaseException:
                os._exit(1)
        os.close(gate)
        os.close(writer)
        os.close(alias)
        renamed = path.with_name('renamed')
        os.rename(path, renamed)
        os.unlink(renamed)
        path.write_bytes(b'unrelated replacement')
        os.write(release, b'x')
        os.close(release)
        assert os.waitpid(child, 0) == (child, 0)
        assert os.read(reader, 8) == b'upXated!'
        assert path.read_bytes() == b'unrelated replacement'
        # Reopening through proc gets fresh access and a fresh open description.
        reopened = os.open('/proc/self/fd/' + str(reader), os.O_WRONLY)
        assert os.write(reopened, b'proc') == 4
        os.dup2(reader, reopened)
        try:
            os.write(reopened, b'forbidden')
        except OSError as error:
            assert error.errno == errno.EBADF, error
        else:
            raise AssertionError('readonly replacement accepted a write')
        os.close(reopened)
        os.lseek(reader, 0, os.SEEK_SET)
        assert os.read(reader, 8) == b'procted!'
        os.close(reader)
print('NATIVE_WRITE_LIFETIME_OK', flush=True)
