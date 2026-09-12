"""fdopendir adopts one descriptor, including after rename, and closes it once."""
import ctypes as c
import errno
import os
import tempfile
from pathlib import Path

libc = c.CDLL('libc.so.6', use_errno=True)
libc.fdopendir.argtypes = [c.c_int];libc.fdopendir.restype = c.c_void_p
libc.opendir.argtypes = [c.c_char_p];libc.opendir.restype = c.c_void_p
libc.dirfd.argtypes = [c.c_void_p];libc.dirfd.restype = c.c_int
libc.closedir.argtypes = [c.c_void_p];libc.closedir.restype = c.c_int
libc.readdir.argtypes = [c.c_void_p];libc.readdir.restype = c.c_void_p

with tempfile.TemporaryDirectory(prefix='kinakaze-dir-adopt-') as directory:
    root = Path(directory);original = root/'original';original.mkdir();(original/'retained').touch()
    fd = os.open(original, os.O_RDONLY | os.O_DIRECTORY)
    original.rename(root/'moved');original.mkdir();(original/'replacement').touch()
    stream = libc.fdopendir(fd);assert stream, c.get_errno()
    assert libc.dirfd(stream) == fd
    names = set()
    while True:
        entry = libc.readdir(stream)
        if not entry:break
        names.add(c.string_at(entry+19).decode())
    assert names == {'.', '..', 'retained'}, names
    assert libc.closedir(stream) == 0
    try:
        os.fstat(fd)
    except OSError as error:
        assert error.errno == errno.EBADF
    else:
        os.close(fd)
        raise AssertionError('closedir leaked the adopted descriptor')
    for _ in range(30):
        stream = libc.opendir(os.fsencode(original));assert stream
        adopted = libc.dirfd(stream);assert adopted >= 0
        assert os.path.samestat(os.fstat(adopted), os.stat(original))
        assert libc.closedir(stream) == 0
    fd = os.open(original/'replacement', os.O_RDONLY)
    try:
        assert not libc.fdopendir(fd) and c.get_errno() == errno.ENOTDIR
        os.fstat(fd) # Failed adoption leaves the caller's descriptor open.
    finally:os.close(fd)
print('FDOPENDIR_PIN_ADOPT_CLOSE_AND_REUSE_OK', flush=True)
