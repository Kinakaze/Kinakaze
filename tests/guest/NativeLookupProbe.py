"""Native negative lookup, hosted links and synthetic configuration after chroot."""
import errno
import os
from pathlib import Path
import stat
import tempfile


def absent(action):
    try:
        action()
    except OSError as error:
        assert error.errno == errno.ENOENT, error
    else:
        raise AssertionError('missing leaf unexpectedly succeeded')


with tempfile.TemporaryDirectory(prefix='native-lookup-', dir='/var/tmp') as name:
    root = Path(name)
    directory = root / 'one/two/three/real'
    directory.mkdir(parents=True)
    missing = directory / 'absent'
    for _ in range(16):
        absent(lambda: os.stat(missing))
        absent(lambda: os.lstat(missing))
        absent(lambda: os.open(missing, os.O_RDONLY))
        absent(lambda: os.unlink(missing))
    os.symlink('real', directory.parent / 'link')
    alias = directory.parent / 'link/new'
    fd = os.open(alias, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o640)
    try:
        os.write(fd, b'content')
    finally:
        os.close(fd)
    assert (directory / 'new').read_bytes() == b'content'
    with (directory / 'new').open('rb') as original:
        os.rename(alias, directory.parent / 'link/renamed')
        os.unlink(directory.parent / 'link/renamed')
        assert original.read() == b'content'
    absent(lambda: os.unlink(directory.parent / 'link/renamed'))
    os.unlink(directory.parent / 'link')
    assert directory.is_dir()
    (directory / 'empty').mkdir()
    os.rmdir(directory / 'empty')
    (root / 'etc').mkdir()
    child = os.fork()
    if child == 0:
        try:
            os.chroot(root)
            os.chdir('/')
            assert b'localhost' in Path('/etc/hosts').read_bytes()
            for synthetic in ('/etc/hosts', '/etc/resolv.conf', '/etc/environment'):
                assert stat.S_ISREG(os.stat(synthetic).st_mode)
                assert stat.S_ISREG(os.lstat(synthetic).st_mode)
            absent(lambda: os.open('/etc/not-synthetic', os.O_RDONLY))
        except BaseException:
            os._exit(1)
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)
    assert not (root / 'etc/hosts').exists()
print('NATIVE_NEGATIVE_LOOKUP_SYMLINK_CHROOT_OK')
