import ctypes
import errno
import os
import tempfile


libc = ctypes.CDLL(None, use_errno=True)
sync_range = libc.sync_file_range
sync_range.argtypes = [ctypes.c_int, ctypes.c_longlong, ctypes.c_longlong, ctypes.c_uint]
sync_range.restype = ctypes.c_int


def synchronize(descriptor, flags, offset=0, length=0, error=None):
    ctypes.set_errno(0)
    result = sync_range(descriptor, offset, length, flags)
    if error is None:
        assert result == 0, (flags, ctypes.get_errno())
    else:
        assert result == -1 and ctypes.get_errno() == error, (result, ctypes.get_errno())


with tempfile.TemporaryDirectory(prefix='async-writeback-', dir='/var/tmp') as directory:
    path = directory + '/original'
    moved = directory + '/moved'
    descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    payload = bytes(range(256)) * 1024
    try:
        os.write(descriptor, payload)
        os.lseek(descriptor, 17, os.SEEK_SET)
        synchronize(descriptor, 2)
        os.rename(path, moved)
        with open(path, 'wb') as replacement:
            replacement.write(b'replacement')
        os.chmod(moved, 0o400)
        synchronize(descriptor, 1)
        os.fsync(descriptor)
        for flags in range(8):
            synchronize(descriptor, flags, 1, 4097)
            assert os.lseek(descriptor, 0, os.SEEK_CUR) == 17
        synchronize(descriptor, 2, -1, error=errno.EINVAL)
        synchronize(descriptor, 2, 0, -1, errno.EINVAL)
        synchronize(descriptor, 8, error=errno.EINVAL)
        reader = os.open(moved, os.O_RDONLY)
        try:
            synchronize(descriptor, 2)
            os.unlink(moved)
            synchronize(reader, 1)
            assert os.read(reader, len(payload) + 1) == payload
        finally:
            os.close(reader)
    finally:
        os.close(descriptor)
    synchronize(descriptor, 0, error=errno.EBADF)
    with open(path, 'rb') as replacement:
        assert replacement.read() == b'replacement'
    reader, writer = os.pipe()
    try:
        for flags in (0, 2, 7):
            synchronize(writer, flags, error=errno.ESPIPE)
    finally:
        os.close(reader)
        os.close(writer)

    child = os.fork()
    if child == 0:
        try:
            for index in range(64):
                descriptor = os.open(directory + '/child-' + str(index), os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
                os.write(descriptor, payload)
                synchronize(descriptor, 2)
                os.close(descriptor)
        except BaseException:
            os._exit(1)
        os._exit(0)
    assert os.waitpid(child, 0)[1] == 0
    for index in range(64):
        path = directory + '/child-' + str(index)
        descriptor = os.open(path, os.O_RDWR)
        try:
            synchronize(descriptor, 1)
            os.fsync(descriptor)
            assert os.read(descriptor, len(payload) + 1) == payload
        finally:
            os.close(descriptor)

print('ASYNC_WRITEBACK_EXIT_RENAME_UNLINK_OFFSET_ERRORS_OK')
