"""Path metadata preserves link, chroot, replacement and mount policy rules."""
import ctypes
import errno
import os
from pathlib import Path
import signal
import tempfile
import traceback


def denied(operation, expected):
    try:
        operation()
    except OSError as error:
        assert error.errno == expected, error
    else:
        raise AssertionError('metadata mutation unexpectedly succeeded')


with tempfile.TemporaryDirectory(prefix='native-path-metadata-', dir='/var/tmp') as directory:
    base = Path(directory)
    target = base / 'package:amd64'
    target.write_bytes(b'original')
    stamp = 1_000_000_000_000_000_000
    os.chmod(target, 0o640)
    os.chown(target, 123, 456)
    os.utime(target, ns=(stamp, stamp))
    current = target.stat()
    assert (current.st_mode & 0o777, current.st_uid, current.st_gid, current.st_mtime_ns) == (0o640, 123, 456, stamp)
    hard = base / 'hard'
    os.link(target, hard)
    os.chmod(hard, 0o644)
    assert target.stat().st_mode & 0o777 == 0o644
    link = base / 'link'
    link.symlink_to(target.name)
    os.chmod(link, 0o600)
    assert target.stat().st_mode & 0o777 == 0o600
    os.chown(link, 234, 567, follow_symlinks=False)
    os.utime(link, ns=(stamp + 1000, stamp + 1000), follow_symlinks=False)
    assert (link.lstat().st_uid, link.lstat().st_gid, link.lstat().st_mtime_ns) == (234, 567, stamp + 1000)
    assert (target.stat().st_uid, target.stat().st_gid, target.stat().st_mtime_ns) == (123, 456, stamp)
    (base / 'sub').mkdir()
    (base / 'dirlink').symlink_to('sub')
    # Dot-dot follows the directory link before selecting the destination.
    os.chmod(directory + '/dirlink/../' + target.name, 0o620)
    assert target.stat().st_mode & 0o777 == 0o620
    previous = os.getcwd()
    try:
        os.chdir(directory)
        os.chown(target.name, 345, 678)
        os.chmod('.//' + target.name, 0o604)
        os.utime(target.name, ns=(stamp + 2000, stamp + 2000))
    finally:
        os.chdir(previous)
    assert (target.stat().st_mode & 0o777, target.stat().st_uid, target.stat().st_mtime_ns) == (0o604, 345, stamp + 2000)
    denied(lambda: os.chmod(str(target) + '/', 0o777), errno.ENOTDIR)
    denied(lambda: os.chown(base / 'missing', 0, 0), errno.ENOENT)
    # A later operation must observe the replacement, while a hard link still
    # names the old inode. No mutable pathname result may survive a syscall.
    replacement = base / 'new'
    replacement.write_bytes(b'new inode')
    os.replace(replacement, target)
    os.chown(target, 456, 789)
    assert target.stat().st_uid == 456 and hard.stat().st_uid == 345

    libc = ctypes.CDLL(None, use_errno=True)
    libc.syscall.restype = ctypes.c_long
    alias = base / 'alias'
    alias.mkdir()
    child = libc.syscall(56, 0x20000 | signal.SIGCHLD, 0, 0, 0, 0)
    assert child >= 0, ctypes.get_errno()
    if child == 0:
        try:
            def mount(source, destination, flags):
                source = os.fsencode(source) if source is not None else None
                assert libc.mount(source, os.fsencode(destination), None, ctypes.c_ulong(flags), None) == 0, ctypes.get_errno()
            mount(directory, alias, 4096)
            mount(None, alias, 4096 | 32 | 1)
            readonly = alias / target.name
            for operation in (lambda: os.chmod(readonly, 0o777),
                              lambda: os.chown(readonly, 0, 0),
                              lambda: os.utime(readonly, ns=(stamp, stamp))):
                denied(operation, errno.EROFS)
            assert libc.umount2(os.fsencode(alias), 0) == 0, ctypes.get_errno()
            os.chroot(directory)
            os.chdir('/')
            os.chmod('/' + target.name, 0o642)
            assert os.stat('/' + target.name).st_mode & 0o777 == 0o642
            os._exit(0)
        except BaseException:
            traceback.print_exc()
            os._exit(1)
    assert os.waitpid(child, 0) == (child, 0)
    assert target.stat().st_mode & 0o777 == 0o642

print('NATIVE_PATH_METADATA_OK', flush=True)
