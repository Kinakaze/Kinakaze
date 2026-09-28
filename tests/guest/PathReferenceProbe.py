"""O_PATH references keep inode identity without performing a data open."""
import errno
import ctypes
import fcntl
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile


def rejects(error, action):
    try:
        result = action()
    except OSError as exception:
        assert exception.errno == error, exception
    else:
        if isinstance(result, int):
            os.close(result)
        raise AssertionError('operation unexpectedly succeeded')


def exercise(base):
    with tempfile.TemporaryDirectory(prefix='path-reference-', dir=base) as name:
        directory = Path(name)
        file = directory / 'file'
        file.write_bytes(b'original')
        file.chmod(0)
        link = directory / 'link'
        link.symlink_to('file')
        # create/exclusive/truncate/nonblocking/noatime/access are all ignored.
        flags = os.O_PATH | os.O_CLOEXEC | os.O_CREAT | os.O_EXCL | os.O_TRUNC | os.O_RDWR
        flags |= os.O_APPEND | os.O_NONBLOCK | os.O_NOATIME
        fd = os.open(file, flags, 0o777)
        parent = os.open(directory, os.O_PATH | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            original = os.fstat(fd)
            assert stat.S_ISREG(original.st_mode) and original.st_size == 8
            assert stat.S_IMODE(original.st_mode) == 0
            assert fcntl.fcntl(fd, fcntl.F_GETFL) == os.O_PATH
            assert fcntl.fcntl(fd, fcntl.F_GETFD) == fcntl.FD_CLOEXEC
            rejects(errno.EBADF, lambda: os.read(fd, 1))
            rejects(errno.EBADF, lambda: os.write(fd, b'changed'))
            rejects(errno.ENOENT, lambda: os.open(directory / 'missing', flags, 0o777))
            assert not (directory / 'missing').exists()
            rejects(errno.ENOTDIR, lambda: os.open(file, os.O_PATH | os.O_DIRECTORY))
            for extra, symbolic in ((0, False), (os.O_NOFOLLOW, True)):
                reference = os.open(link, os.O_PATH | extra)
                try:
                    assert stat.S_ISLNK(os.fstat(reference).st_mode) == symbolic
                    if symbolic:
                        assert os.readlink('', dir_fd=reference) == 'file'
                    else:
                        assert os.fstat(reference).st_ino == original.st_ino
                finally:
                    os.close(reference)
            link.unlink()
            link.symlink_to('missing')
            reference = os.open(link, os.O_PATH | os.O_NOFOLLOW)
            os.close(reference)
            rejects(errno.ENOENT, lambda: os.open(link, os.O_PATH))
            moved = directory / 'moved'
            file.rename(moved)
            moved.unlink()
            file.write_bytes(b'replacement')
            assert os.fstat(fd).st_ino == original.st_ino
            assert os.fstat(fd).st_nlink == 0
            reference = os.open('file', os.O_PATH | os.O_NOFOLLOW, dir_fd=parent)
            try:
                assert os.fstat(reference).st_ino != original.st_ino
                assert os.fstat(reference).st_size == 11
            finally:
                os.close(reference)
            code = ('import os,sys; s=os.fstat(int(sys.argv[1])); '
                    'assert (s.st_ino,s.st_size,s.st_nlink)==(int(sys.argv[2]),8,0)')
            subprocess.run([sys.executable, '-c', code, str(fd), str(original.st_ino)],
                           pass_fds=(fd,), check=True, timeout=10)
        finally:
            os.close(parent)
            os.close(fd)
        for name in (r'system-goal\x2dtemplate.slice', r'..\outside', r'C:\Windows', r'\\server\share'):
            entry = directory / name
            entry.write_bytes(name.encode())
            assert name in os.listdir(directory)
            symbolic = directory / 'escaped-link'
            symbolic.symlink_to(name)
            try:
                assert symbolic.read_bytes() == name.encode()
                reference = os.open(entry, os.O_PATH)
                try:
                    assert os.fstat(reference).st_ino == entry.stat().st_ino
                finally:
                    os.close(reference)
            finally:
                symbolic.unlink()
        # Exec first initializes a native cwd, then restores the guest fs state.
        # A tmpfs cwd must replace that native directory reference as well.
        subprocess.run([sys.executable, '-c',
            'import os,sys; from pathlib import Path; '
            'assert os.getcwd()==sys.argv[1], (os.getcwd(),sys.argv[1]); '
            'Path("cwd-state").write_text("relative")', str(directory)],
            cwd=directory, check=True, timeout=10)
        assert (directory / 'cwd-state').read_text() == 'relative'


for backend in ('/tmp', '/dev/shm'):
    exercise(backend)
    print(json.dumps(dict(backend=backend, status='passed')), flush=True)
libc = ctypes.CDLL('libc.so.6', use_errno=True)
with tempfile.TemporaryDirectory(prefix='path-reference-mount-') as mounted:
    assert libc.mount(b'tmpfs', os.fsencode(mounted), b'tmpfs', 0, b'size=4m') == 0, ctypes.get_errno()
    try:
        exercise(mounted)
        print(json.dumps(dict(backend='mounted-tmpfs', status='passed')), flush=True)
    finally:
        assert libc.umount(os.fsencode(mounted)) == 0, ctypes.get_errno()
print('PATH_REFERENCE_OK', flush=True)
