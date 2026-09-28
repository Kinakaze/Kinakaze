"""Exercise real shared tmpfs pages, descriptor lifetime and fork/COW transport."""
import ctypes
import errno
import mmap
import os
from pathlib import Path
import subprocess
import sys
import tempfile

PAGE = mmap.PAGESIZE
if len(sys.argv) > 1 and sys.argv[1] == 'child':
    fd = int(sys.argv[2])
    with mmap.mmap(fd, PAGE * 3, flags=mmap.MAP_SHARED) as view:
        assert view[:6] == b'parent'
        view[PAGE:PAGE + 5] = b'child'
    raise SystemExit(0)

libc = ctypes.CDLL(None, use_errno=True)
with tempfile.TemporaryDirectory(prefix='tmpfs-map-') as directory:
    encoded = os.fsencode(directory)
    assert libc.mount(b'tmpfs', encoded, b'tmpfs', 0, b'size=16m') == 0, ctypes.get_errno()
    try:
        path = directory + '/file'
        fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
        os.posix_fallocate(fd, 0, PAGE * 3)
        assert os.fstat(fd).st_size == PAGE * 3
        assert os.fstat(fd).st_blocks == PAGE * 3 // 512
        print('TMPFS_MAPPING_ALLOCATED', flush=True)
        with mmap.mmap(fd, PAGE * 3, flags=mmap.MAP_SHARED) as first:
            first[:6] = b'parent'
            assert os.pread(fd, 6, 0) == b'parent'
            other = os.open(path, os.O_RDWR)
            with mmap.mmap(other, PAGE, flags=mmap.MAP_SHARED, offset=PAGE) as second:
                second[:6] = b'offset'
                assert first[PAGE:PAGE + 6] == b'offset'
                os.pwrite(other, b'write', 2 * PAGE)
                assert first[2 * PAGE:2 * PAGE + 5] == b'write'
                first.flush()
            os.close(other)
            print('TMPFS_MAPPING_SHARED_VIEWS_OK', flush=True)
            subprocess.run([sys.executable, __file__, 'child', str(fd)], pass_fds=(fd,), check=True, timeout=15)
            assert first[PAGE:PAGE + 5] == b'child'
            print('TMPFS_MAPPING_EXEC_CHILD_OK', flush=True)
            with mmap.mmap(fd, PAGE * 3, flags=mmap.MAP_PRIVATE) as private:
                private[:7] = b'private'
                assert first[:6] == b'parent'
                child = os.fork()
                if child == 0:
                    assert private[:7] == b'private'
                    private[:5] = b'local'
                    first[2 * PAGE:2 * PAGE + 4] = b'fork'
                    os._exit(0)
                assert os.waitpid(child, 0) == (child, 0)
                assert private[:7] == b'private'
                assert first[2 * PAGE:2 * PAGE + 4] == b'fork'
            print('TMPFS_MAPPING_PRIVATE_FORK_OK', flush=True)
            os.unlink(path)
            os.close(fd)
            # Force inode collection and fd reuse while the old inode is mapped.
            replacement = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
            os.write(replacement, b'replacement')
            assert first[:6] == b'parent'
            first[:6] = b'pinned'
            assert os.pread(replacement, 11, 0) == b'replacement'
            os.close(replacement)
            print('TMPFS_MAPPING_UNLINK_PIN_OK', flush=True)
        os.unlink(path)
        print('TMPFS_MAPPING_UNMAPPED_OK', flush=True)
        fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
        os.ftruncate(fd, PAGE)
        os.close(fd)
        fd = os.open(path, os.O_RDONLY)
        try:
            mmap.mmap(fd, PAGE, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
            raise AssertionError('writable shared map accepted on read-only fd')
        except OSError as error:
            assert error.errno == errno.EACCES, error
        with mmap.mmap(fd, PAGE, flags=mmap.MAP_PRIVATE) as private:
            private[:4] = b'copy'
            assert os.pread(fd, 4, 0) == bytes(4)
        print('TMPFS_MAPPING_READONLY_PRIVATE_OK', flush=True)
        # An already-open descriptor must consult the current attachment policy.
        assert libc.mount(None, encoded, None, 32 | 8, None) == 0, ctypes.get_errno()
        print('TMPFS_MAPPING_NOEXEC_REMOUNT_OK', flush=True)
        try:
            mmap.mmap(fd, PAGE, flags=mmap.MAP_PRIVATE, prot=mmap.PROT_READ | mmap.PROT_EXEC)
            raise AssertionError('executable mapping accepted after noexec remount')
        except OSError as error:
            assert error.errno == errno.EPERM, error
        os.close(fd)
        print('TMPFS_MAPPING_OK', flush=True)
    finally:
        assert libc.umount2(encoded, 2) == 0, ctypes.get_errno()
