import ctypes
import errno
import os
import stat
import struct
import tempfile


libc = ctypes.CDLL(None, use_errno=True)
sync_range = libc.sync_file_range
sync_range.argtypes = [ctypes.c_int, ctypes.c_longlong, ctypes.c_longlong, ctypes.c_uint]
sync_range.restype = ctypes.c_int


with tempfile.TemporaryDirectory(prefix='ownership-writeback-', dir='/var/tmp') as directory:
    payload = bytes(range(256)) * 1024
    for index in range(32):
        path = directory + '/' + str(index)
        descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        try:
            assert os.write(descriptor, payload) == len(payload)
            os.lseek(descriptor, 17, os.SEEK_SET)
            assert sync_range(descriptor, 0, 0, 2) == 0, ctypes.get_errno()
            os.fchown(descriptor, os.getuid(), os.getgid())
            assert os.lseek(descriptor, 0, os.SEEK_CUR) == 17
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        with open(path, 'rb') as stream:
            assert stream.read() == payload

    path = directory + '/privileges'
    moved = directory + '/moved'
    descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
    try:
        assert os.write(descriptor, payload) == len(payload)
        os.fchmod(descriptor, 0o6750)
        capability = struct.pack('<5I', 0x02000001, 1 << 10, 0, 0, 0)
        os.setxattr(descriptor, 'security.capability', capability)
        os.setxattr(descriptor, 'user.retained', b'retained')
        assert sync_range(descriptor, 0, 0, 2) == 0, ctypes.get_errno()
        os.fchown(descriptor, os.getuid(), os.getgid())
        assert stat.S_IMODE(os.fstat(descriptor).st_mode) == 0o750
        assert 'security.capability' not in os.listxattr(descriptor)
        assert os.getxattr(descriptor, 'user.retained') == b'retained'
        os.rename(path, moved)
        with open(path, 'wb') as stream:
            stream.write(b'replacement')
        os.unlink(moved)
        os.fchown(descriptor, 12345, 12345)
        assert os.fstat(descriptor).st_uid == 12345
        child = os.fork()
        if child == 0:
            try:
                os.setgroups([])
                os.setgid(12345)
                os.setuid(12345)
                os.fchown(descriptor, 12345, 12345)
                try:
                    os.fchown(descriptor, 12346, -1)
                except OSError as error:
                    assert error.errno == errno.EPERM
                else:
                    raise AssertionError('unprivileged owner change succeeded')
            except BaseException:
                os._exit(1)
            os._exit(0)
        assert os.waitpid(child, 0)[1] == 0
        os.fsync(descriptor)
        assert os.pread(descriptor, len(payload) + 1, 0) == payload
        with open(path, 'rb') as stream:
            assert stream.read() == b'replacement'
    finally:
        os.close(descriptor)

print('OWNERSHIP_WRITEBACK_PRIVILEGES_IDENTITY_PERMISSIONS_OK')
