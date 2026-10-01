"""Existing-file writes retain access, inode, namespace and mount semantics."""
import ctypes
import errno
import fcntl
import os
from pathlib import Path
import subprocess
import sys
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

with tempfile.TemporaryDirectory(prefix='native-write-open-', dir='/var/tmp') as directory:
    os.chmod(directory, 0o755)
    path = directory + '/payload'
    Path(path).write_bytes(b'original')
    fd = os.open(path, os.O_WRONLY | os.O_NONBLOCK | os.O_CLOEXEC | os.O_NOFOLLOW)
    alias = os.dup(fd)
    reader = os.open(path, os.O_RDONLY)
    try:
        assert fcntl.fcntl(fd, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
        assert fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_NONBLOCK
        assert fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_ACCMODE == os.O_WRONLY
        fails(errno.EBADF, lambda: os.read(fd, 1))
        os.chmod(path, 0o444)
        assert os.write(fd, b'new') == 3
        assert os.write(alias, b'inode') == 5
        fresh = os.open(path, os.O_WRONLY)
        os.close(fresh)
        assert os.stat(path).st_mode & 0o777 == 0o444
        os.rename(path, directory + '/old')
        os.unlink(directory + '/old')
        Path(path).write_bytes(b'replaced')
        assert os.pwrite(alias, b'pinned!!', 0) == 8
        assert os.pread(reader, 8, 0) == b'pinned!!'
        assert Path(path).read_bytes() == b'replaced'
        os.fsync(alias)
    finally:
        os.close(reader)
        os.close(alias)
        os.close(fd)
    fd = os.open(path, os.O_RDWR)
    try:
        assert os.read(fd, 3) == b'rep'
        assert os.write(fd, b'after') == 5
        assert os.pread(fd, 8, 0) == b'repafter'
    finally:
        os.close(fd)
    os.symlink('payload', directory + '/link')
    fd = os.open(directory + '/link', os.O_WRONLY)
    os.close(fd)
    fails(errno.ELOOP, lambda: os.open(directory + '/link', os.O_WRONLY | os.O_NOFOLLOW))
    fails(errno.EISDIR, lambda: os.open(directory, os.O_WRONLY))
    fails(errno.ENOTDIR, lambda: os.open(path + '/', os.O_WRONLY))
    fails(errno.EEXIST, lambda: os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL))
    os.mkfifo(directory + '/fifo', 0o600)
    fails(errno.ENXIO, lambda: os.open(directory + '/fifo', os.O_WRONLY | os.O_NONBLOCK))
    fd = os.open(path, os.O_WRONLY | os.O_APPEND)
    assert os.write(fd, b'!') == 1
    os.close(fd)
    assert Path(path).read_bytes() == b'repafter!'
    fd = os.open(path, os.O_WRONLY | os.O_TRUNC)
    os.close(fd)
    assert Path(path).read_bytes() == b''
    Path(path).write_bytes(b'protected')
    os.chmod(path, 0o600)
    subprocess.run([sys.executable, '-c', '''import errno,os,sys
os.setgroups([]); os.setgid(65534); os.setuid(65534)
for flags in (os.O_WRONLY, os.O_RDWR):
    try: fd=os.open(sys.argv[1],flags)
    except OSError as error: assert error.errno==errno.EACCES,error
    else: os.close(fd); raise AssertionError('unauthorized write open')
''', path], check=True, timeout=20)
    assert Path(path).read_bytes() == b'protected'
    # Use a private mount namespace; the original path stays writable when a
    # bind alias becomes read-only.
    subprocess.run([sys.executable, '-c', '''import ctypes,errno,os,sys
libc=ctypes.CDLL(None,use_errno=True)
assert libc.unshare(0x20000)==0,ctypes.get_errno()
source,target=sys.argv[1:]
os.mkdir(target)
def mount(source,target,flags):
    assert libc.mount(os.fsencode(source) if source else None,os.fsencode(target),None,
        ctypes.c_ulong(flags),None)==0,ctypes.get_errno()
mount(os.path.dirname(source),target,4096)
mount(None,target,4096|32|1)
for flags in (os.O_WRONLY,os.O_RDWR):
    try: fd=os.open(target+'/payload',flags)
    except OSError as error: assert error.errno==errno.EROFS,error
    else: os.close(fd); raise AssertionError('readonly mount writable')
os.close(os.open(source,os.O_WRONLY))
assert libc.umount2(os.fsencode(target),0)==0,ctypes.get_errno()
os.rmdir(target)
''', path, directory + '/alias'], check=True, timeout=20)
print('NATIVE_WRITE_OPEN_OK', flush=True)
