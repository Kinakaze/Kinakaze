"""Check a host-created junction through ordinary Linux metadata-only opens."""
import errno
import os
import stat
import sys

link, target = sys.argv[1:]
fd = os.open(link, os.O_PATH | os.O_DIRECTORY)
try:
    assert stat.S_ISDIR(os.fstat(fd).st_mode)
    assert os.fstat(fd).st_ino == os.stat(target).st_ino
    child = os.open('payload', os.O_RDONLY, dir_fd=fd)
    try:
        assert os.read(child, 32) == b'junction payload'
    finally:
        os.close(child)
finally:
    os.close(fd)
fd = os.open(link, os.O_PATH | os.O_NOFOLLOW)
try:
    assert stat.S_ISLNK(os.fstat(fd).st_mode)
    assert os.readlink('', dir_fd=fd) == target
finally:
    os.close(fd)
try:
    fd = os.open(link, os.O_PATH | os.O_NOFOLLOW | os.O_DIRECTORY)
except OSError as error:
    assert error.errno == errno.ENOTDIR, error
else:
    os.close(fd)
    raise AssertionError('a junction opened without following it is a symlink')
print('NATIVE_REPARSE_OK', flush=True)
