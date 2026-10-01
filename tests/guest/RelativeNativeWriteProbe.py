"""Cwd-relative write opens preserve pinned files and directory resolution."""
import errno
import os
from pathlib import Path
import tempfile

def fails(code, path, flags):
    try:
        fd = os.open(path, flags)
    except OSError as error:
        assert error.errno == code, (path, error)
    else:
        os.close(fd)
        raise AssertionError((path, code))

saved = os.getcwd()
with tempfile.TemporaryDirectory(prefix='relative-native-write-', dir='/var/tmp') as directory:
    try:
        os.chdir(directory)
        os.mkdir('sub')
        os.mkdir('target')
        Path('payload').write_bytes(b'parent!!')
        Path('sub/payload').write_bytes(b'original')
        Path('target/payload').write_bytes(b'target!!')
        os.symlink('../target', 'sub/jump')
        os.symlink('payload', 'sub/link')
        os.chdir('sub')
        writer = os.open('payload', os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW)
        reader = os.open('payload', os.O_RDONLY)
        alias = os.dup(writer)
        os.close(writer)
        os.rename('payload', 'old')
        os.unlink('old')
        Path('payload').write_bytes(b'replacement')
        assert os.write(alias, b'retained') == 8
        assert os.pread(reader, 8, 0) == b'retained'
        os.close(reader)
        os.close(alias)
        assert Path('payload').read_bytes() == b'replacement'
        for path in ('./payload', 'link'):
            fd = os.open(path, os.O_WRONLY)
            assert os.write(fd, b'same') == 4
            os.close(fd)
        fd = os.open('jump/../payload', os.O_WRONLY)
        assert os.write(fd, b'walked') == 6
        os.close(fd)
        assert Path('../payload').read_bytes() == b'walked!!'
        assert Path('payload').read_bytes() == b'sameacement'
        fails(errno.ELOOP, 'link', os.O_WRONLY | os.O_NOFOLLOW)
        fails(errno.ENOTDIR, 'payload/', os.O_WRONLY)
        fails(errno.ENOENT, 'missing', os.O_WRONLY)
        parent = os.open('../target', os.O_RDONLY | os.O_DIRECTORY)
        os.rename('../target', '../moved-target')
        os.mkdir('../target')
        Path('../target/payload').write_bytes(b'different')
        fd = os.open('payload', os.O_WRONLY, dir_fd=parent)
        assert os.write(fd, b'pinned!!') == 8
        os.close(fd)
        os.close(parent)
        assert Path('../moved-target/payload').read_bytes() == b'pinned!!'
        assert Path('../target/payload').read_bytes() == b'different'
        os.rename(directory + '/sub', directory + '/moved-sub')
        os.mkdir(directory + '/sub')
        Path(directory + '/sub/payload').write_bytes(b'new-cwd')
        fd = os.open('payload', os.O_WRONLY)
        assert os.write(fd, b'current') == 7
        os.close(fd)
        assert Path(directory + '/moved-sub/payload').read_bytes() == b'currentment'
        assert Path(directory + '/sub/payload').read_bytes() == b'new-cwd'
    finally:
        os.chdir(saved)
print('RELATIVE_NATIVE_WRITE_OK', flush=True)
