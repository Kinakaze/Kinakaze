"""Raw syscall wiring plus tmpfs sparse-file and sysfs topology boundaries."""
import ctypes as c
import errno
import os
from pathlib import Path
import struct
import tempfile

libc = c.CDLL(None, use_errno=True)
libc.syscall.restype = c.c_long
libc.syscall.argtypes = [c.c_long]
def call(number, *args):
    converted = [c.c_ulonglong(a & ((1 << 64) - 1)) if isinstance(a, int)
                 else c.c_char_p(a) if isinstance(a, bytes) else a for a in args]
    c.set_errno(0)
    return libc.syscall(number, *converted)
def failure(number, expected, *args):
    assert call(number, *args) == -1 and c.get_errno() == expected, (number, c.get_errno())
def mount(kind, path, options):
    assert libc.mount(kind, os.fsencode(path), kind, 0, options) == 0, c.get_errno()
class Iovec(c.Structure):
    _fields_ = [('base', c.c_void_p), ('length', c.c_size_t)]

with tempfile.TemporaryDirectory(prefix='fs-boundaries-') as directory:
    root = Path(directory)
    memory = root / 'memory'
    system = root / 'system'
    memory.mkdir()
    system.mkdir()
    mount(b'tmpfs', memory, b'size=1m,nr_inodes=64')
    try:
        path = memory / 'source'
        fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
        data = c.create_string_buffer(b'raw-vector')
        vector = Iovec(c.addressof(data), 10)
        assert call(296, fd, c.byref(vector), 1, 3, 0) == 10
        assert os.lseek(fd, 0, os.SEEK_CUR) == 0
        read = c.create_string_buffer(10)
        readvec = Iovec(c.addressof(read), 10)
        assert call(295, fd, c.byref(readvec), 1, 3, 0) == 10
        assert read.raw == b'raw-vector'
        assert call(327, fd, c.byref(readvec), 1, 3, 0, 0) == 10
        assert call(328, fd, c.byref(vector), 1, 3, 0, 0) == 10
        failure(327, errno.EOPNOTSUPP, fd, c.byref(readvec), 1, 0, 0, 0x80000000)
        failure(295, errno.EBADF, -1, c.byref(readvec), 1, 0, 0)
        failure(221, errno.EBADF, -1, 0, 0, 0)
        failure(221, errno.EINVAL, fd, 0, 0, 99)
        failure(221, errno.EINVAL, fd, 0, -1, 0)
        assert call(221, fd, 0, 0, 0) == 0
        # Native inode EAs are supported; tmpfs currently rejects xattrs rather
        # than placing metadata on an unrelated host pathname.
        failure(188, errno.EOPNOTSUPP, os.fsencode(path), b'user.unavailable', b'x', 1, 0)
        attribute_path = root / 'native-attributes'
        attribute_fd = os.open(attribute_path, os.O_CREAT | os.O_RDWR, 0o600)
        # All twelve xattr syscall numbers: set/get/list/remove x path/lpath/fd.
        for variant in range(3):
            target = attribute_fd if variant == 2 else os.fsencode(attribute_path)
            name = b'user.raw-boundary'
            assert call(188 + variant, target, name, b'value', 5, 1) == 0, (variant, c.get_errno())
            failure(188 + variant, errno.EEXIST, target, name, b'x', 1, 1)
            assert call(191 + variant, target, name, None, 0) == 5
            value = c.create_string_buffer(5)
            assert call(191 + variant, target, name, value, 5) == 5 and value.raw == b'value'
            failure(191 + variant, errno.ERANGE, target, name, value, 1)
            length = call(194 + variant, target, None, 0)
            names = c.create_string_buffer(length)
            assert call(194 + variant, target, names, length) == length
            assert name in names.raw.split(b'\0')
            assert call(197 + variant, target, name) == 0
            failure(191 + variant, errno.ENODATA, target, name, value, 5)
        os.close(attribute_fd)
        other = memory / 'destination'
        other.write_bytes(b'old')
        failure(316, errno.EEXIST, -100, os.fsencode(path), -100, os.fsencode(other), 1)
        assert path.exists() and other.read_bytes() == b'old'
        assert call(316, -100, os.fsencode(path), -100, os.fsencode(other), 2) == 0
        assert path.read_bytes() == b'old' and other.read_bytes()[3:] == b'raw-vector'
        # FD remains attached to the exchanged inode.
        assert os.pread(fd, 10, 3) == b'raw-vector'
        out = os.open(memory / 'copied', os.O_CREAT | os.O_RDWR, 0o600)
        offset = c.c_longlong(3)
        assert call(40, out, fd, c.byref(offset), 10) == 10 and offset.value == 13
        assert os.pread(out, 10, 0) == b'raw-vector'
        os.close(out)
        os.close(fd)

        sparse = os.open(memory / 'sparse', os.O_CREAT | os.O_RDWR, 0o600)
        os.ftruncate(sparse, 16384)
        os.pwrite(sparse, b'x', 4103)
        assert os.lseek(sparse, 0, os.SEEK_DATA) == 4096
        assert os.lseek(sparse, 4096, os.SEEK_HOLE) == 8192
        assert os.lseek(sparse, 10, os.SEEK_HOLE) == 10
        for offset in (8192, 16384):
            try:
                os.lseek(sparse, offset, os.SEEK_DATA)
            except OSError as error:
                assert error.errno == errno.ENXIO
            else:
                raise AssertionError('nonexistent data accepted')
        duplicate = os.dup(sparse)
        assert os.lseek(duplicate, 4103, os.SEEK_SET) == 4103
        for item in (sparse, duplicate):
            info = dict(line.split(':', 1) for line in Path(f'/proc/self/fdinfo/{item}').read_text().splitlines())
            assert int(info['pos']) == 4103, info
        os.close(duplicate)
        before = os.fstatvfs(sparse).f_blocks
        assert libc.mount(None, os.fsencode(memory), None, 32, b'size=4k') == -1
        assert c.get_errno() == errno.EINVAL and os.fstatvfs(sparse).f_blocks == before
        os.unlink(memory / 'sparse')
        assert os.pread(sparse, 1, 4103) == b'x'
        os.close(sparse)
        print('RAW_SYSCALL_TMPFS_BOUNDARIES_OK', flush=True)
    finally:
        assert libc.umount2(os.fsencode(memory), 2) == 0, c.get_errno()

    mount(b'sysfs', system, None)
    try:
        for cpu in (system / 'devices/system/cpu').glob('cpu[0-9]*'):
            topo = cpu / 'topology'
            assert int((topo / 'core_id').read_text()) >= 0
            assert int((topo / 'physical_package_id').read_text()) >= 0
            siblings = (topo / 'thread_siblings_list').read_text().strip()
            assert siblings
            mask = int((topo / 'thread_siblings').read_text().replace(',', ''), 16)
            assert mask & (1 << int(cpu.name[3:]))
        for node in (system / 'devices/system/node').glob('node[0-9]*'):
            assert int((node / 'cpumap').read_text().replace(',', ''), 16) > 0
        readonly = system / 'devices/system/cpu/online'
        original = readonly.read_bytes()
        try:
            readonly.write_bytes(b'0\n')
        except OSError as error:
            assert error.errno in (errno.EACCES, errno.EPERM, errno.EROFS)
        else:
            raise AssertionError('sysfs topology is writable')
        assert readonly.read_bytes() == original
        print('SYSFS_TOPOLOGY_BOUNDARIES_OK', flush=True)
    finally:
        assert libc.umount2(os.fsencode(system), 2) == 0, c.get_errno()

auxv = dict(struct.iter_unpack('<QQ', Path('/proc/self/auxv').read_bytes()))
assert all(auxv.get(tag, 0) for tag in (3, 9, 25, 31)), auxv
assert c.string_at(auxv[31]).startswith(b'/')
print('FILESYSTEM_BOUNDARIES_OK')
