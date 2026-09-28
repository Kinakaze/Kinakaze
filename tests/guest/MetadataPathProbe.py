"""Pinned metadata lookup with links, rename, mount crossings and fork/exec."""
import ctypes as c
import errno
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile

FLAGS = os.O_PATH | os.O_NOFOLLOW
results = []

def check(name, action):
    try:
        action()
        row = dict(name=name, status='passed')
    except Exception as error:
        row = dict(name=name, status='failed', error=str(error))
    results.append(row)
    print(json.dumps(row), flush=True)

def fails(expected, action):
    try:
        value = action()
    except OSError as error:
        assert error.errno == expected, (expected, error)
    else:
        os.close(value)
        raise AssertionError('unexpected open success')

with tempfile.TemporaryDirectory(prefix='metadata-path-') as temporary:
    base = Path(temporary)
    original = base / 'original'
    original.mkdir()
    (original / 'file').write_text('original')
    for name in ('package:amd64', 'trailing.', 'space ', '中文'):
        (original / name).write_text(name)
    (original / 'dir').mkdir()
    os.symlink('file', original / 'link')
    os.symlink('dir', original / 'dirlink')
    os.symlink('absent', original / 'dangling')
    os.symlink('loop', original / 'loop')
    os.mkfifo(original / 'fifo')
    parent = os.open(original, os.O_PATH | os.O_DIRECTORY)
    def types():
        for name in ('file', 'dir', 'link', 'dirlink', 'dangling', 'loop', 'fifo',
                     'package:amd64', 'trailing.', 'space ', '中文'):
            descriptor = os.open(name, FLAGS, dir_fd=parent)
            try:
                got = os.fstat(descriptor)
                expected = (original / name).lstat()
                assert (got.st_ino, stat.S_IFMT(got.st_mode)) == (expected.st_ino, stat.S_IFMT(expected.st_mode)), name
                if stat.S_ISLNK(expected.st_mode):
                    assert os.readlink('', dir_fd=descriptor) == os.readlink(original / name)
            finally:
                os.close(descriptor)
        for name in ('file', 'link', 'dirlink', 'dangling', 'loop', 'fifo'):
            fails(errno.ENOTDIR, lambda: os.open(name, FLAGS | os.O_DIRECTORY, dir_fd=parent))
        fails(errno.ENOENT, lambda: os.open('missing', FLAGS, dir_fd=parent))
    check('file-types-and-nofollow', types)
    def ignored():
        descriptor = os.open('file', FLAGS | os.O_WRONLY | os.O_TRUNC | os.O_CREAT, dir_fd=parent)
        os.close(descriptor)
        assert (original / 'file').read_text() == 'original'
        fails(errno.ENOENT, lambda: os.open('missing', FLAGS | os.O_CREAT, dir_fd=parent))
    check('path-only-ignores-write-and-create', ignored)
    moved = base / 'moved'
    original.rename(moved)
    original.mkdir()
    (original / 'file').write_text('replacement')
    def renamed():
        descriptor = os.open('file', FLAGS, dir_fd=parent)
        try:
            assert os.fstat(descriptor).st_ino == (moved / 'file').stat().st_ino
            assert os.fstat(descriptor).st_ino != (original / 'file').stat().st_ino
        finally:
            os.close(descriptor)
    check('renamed-parent-pins-original-directory', renamed)
    def inherited():
        code = "import os,sys; f=os.open('file',os.O_PATH|os.O_NOFOLLOW,dir_fd=int(sys.argv[1])); print(os.fstat(f).st_ino); os.close(f)"
        value = subprocess.check_output([sys.executable, '-c', code, str(parent)], pass_fds=(parent,), timeout=10)
        assert int(value) == (moved / 'file').stat().st_ino
    check('exec-retains-parent-identity', inherited)
    libc = c.CDLL(None, use_errno=True)
    libc.mount.argtypes = [c.c_char_p, c.c_char_p, c.c_char_p, c.c_ulong, c.c_void_p]
    libc.umount2.argtypes = [c.c_char_p, c.c_int]
    assert libc.unshare(0x20000) == 0, c.get_errno()
    os.close(parent)
    parent = os.open(moved, os.O_PATH | os.O_DIRECTORY)
    def mounted(provider):
        target = moved / provider
        target.mkdir()
        encoded = os.fsencode(target)
        assert libc.mount(provider.encode(), encoded, provider.encode(), 0, None) == 0, c.get_errno()
        try:
            directory = os.open(provider, FLAGS | os.O_DIRECTORY, dir_fd=parent)
            try:
                if provider == 'tmpfs':
                    (target / 'file').write_text('mounted')
                    node = os.open('file', FLAGS, dir_fd=directory)
                    assert os.fstat(node).st_ino == (target / 'file').stat().st_ino
                    os.close(node)
                else:
                    for name in ('.', 'self', 'sys/kernel/hostname'):
                        relative = os.stat(name, dir_fd=directory, follow_symlinks=False)
                        absolute = os.stat(target / name, follow_symlinks=False)
                        assert (relative.st_dev, relative.st_ino, relative.st_mode) == (
                            absolute.st_dev, absolute.st_ino, absolute.st_mode), (name, relative, absolute)
                        output = c.create_string_buffer(256)
                        assert libc.statx(directory, name.encode(), 0x100 | 0x800,
                                          1, output) == 0, (name, c.get_errno())
                    link = os.open('self', FLAGS, dir_fd=directory)
                    assert stat.S_ISLNK(os.fstat(link).st_mode)
                    assert os.readlink('', dir_fd=link) == str(os.getpid())
                    os.close(link)
            finally:
                os.close(directory)
        finally:
            assert libc.umount2(encoded, 0) == 0, c.get_errno()
    check('tmpfs-mount-crossing', lambda: mounted('tmpfs'))
    check('proc-mount-crossing', lambda: mounted('proc'))
    def proc_bind():
        target = moved / 'proc-hostname'
        target.touch()
        encoded = os.fsencode(target)
        assert libc.mount(b'/proc/sys/kernel/hostname', encoded, None, 4096, None) == 0
        descriptor = os.open(target, os.O_PATH | os.O_CLOEXEC)
        try:
            magic = '/proc/self/fd/' + str(descriptor)
            assert os.readlink(magic) == str(target), os.readlink(magic)
            assert os.fstat(descriptor).st_ino == os.stat('/proc/sys/kernel/hostname').st_ino
            assert libc.mount(None, magic.encode(), None, 4096 | 32 | 1, None) == 0, c.get_errno()
            assert os.statvfs(target).f_flag & os.ST_RDONLY
            assert libc.umount2(encoded, 2) == 0, c.get_errno()
            assert os.readlink(magic) == str(target)
        finally:
            os.close(descriptor)
            libc.umount2(encoded, 2)
    check('proc-file-bind-retains-root-and-remount-target', proc_bind)
    def bind():
        target = moved / 'bind'
        target.mkdir()
        source = base / 'source'
        source.mkdir()
        (source / 'file').write_text('bind')
        encoded = os.fsencode(target)
        assert libc.mount(os.fsencode(source), encoded, None, 4096, None) == 0, c.get_errno()
        try:
            assert libc.mount(None, encoded, None, 4096 | 32 | 1, None) == 0, c.get_errno()
            directory = os.open('bind', FLAGS | os.O_DIRECTORY, dir_fd=parent)
            try:
                node = os.open('file', FLAGS, dir_fd=directory)
                try:
                    assert os.fstat(node).st_ino == (source / 'file').stat().st_ino
                    fails(errno.EROFS, lambda: os.open('/proc/self/fd/' + str(node), os.O_WRONLY))
                finally:
                    os.close(node)
            finally:
                os.close(directory)
        finally:
            assert libc.umount2(encoded, 0) == 0, c.get_errno()
    check('bind-attachment-retains-read-only-policy', bind)
    def rooted():
        child = os.fork()
        if not child:
            try:
                expected = (moved / 'file').stat().st_ino
                os.chroot(base)
                os.chdir('/')
                node = os.open('file', FLAGS, dir_fd=parent)
                assert os.fstat(node).st_ino == expected
                assert os.readlink('/proc/self/fd/' + str(node)) == '/moved/file'
                os.close(node)
                os._exit(0)
            except BaseException:
                os._exit(1)
        assert os.waitpid(child, 0) == (child, 0)
    check('chroot-retains-namespace-coordinates', rooted)
    os.close(parent)

assert all(row['status'] == 'passed' for row in results), results
print('METADATA_PATH_OK', flush=True)
