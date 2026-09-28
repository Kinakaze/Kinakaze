"""Resolve native roots, changing ancestors and links across mount boundaries."""
import ctypes
import errno
import json
import os
from pathlib import Path
import stat
import tempfile
import traceback


libc = ctypes.CDLL(None, use_errno=True)


def mount(source, target, kind=None, flags=0):
    result = libc.mount(os.fsencode(source), os.fsencode(target),
                        os.fsencode(kind) if kind else None, flags, None)
    assert result == 0, (source, str(target), ctypes.get_errno())


def check(name, action):
    action()
    print(json.dumps(dict(name=name, status='passed')), flush=True)


with tempfile.TemporaryDirectory(prefix='root-resolution-', dir='/tmp') as name:
    root = Path(name)
    native = root / 'native'
    native.mkdir()
    (native / 'value').write_text('original')
    entry = root / 'entry'
    entry.symlink_to('native')

    def changing_ancestor():
        assert (entry / 'value').read_text() == 'original'
        native.rename(root / 'old')
        native.mkdir()
        (native / 'value').write_text('replacement')
        assert (entry / 'value').read_text() == 'replacement'
        assert (root / 'old/value').read_text() == 'original'
        assert os.readlink(entry) == 'native'
        try:
            (entry / 'missing/value').read_text()
        except FileNotFoundError:
            pass
        else:
            raise AssertionError('missing ancestor was accepted')
    check('replacement-and-unfollowed-link', changing_ancestor)

    bound = root / 'bound'
    bound.mkdir()
    mount(native, bound, flags=4096)
    try:
        def bound_paths():
            (root / 'bound-link').symlink_to('bound')
            assert (root / 'bound-link/value').read_text() == 'replacement'
            assert (bound / '../old/value').read_text() == 'original'
            assert (bound / 'value').stat().st_ino == (native / 'value').stat().st_ino
        check('bind-mount-and-parent', bound_paths)
    finally:
        assert libc.umount(os.fsencode(bound)) == 0, ctypes.get_errno()

    memory = root / 'memory'
    memory.mkdir()
    mount('tmpfs', memory, 'tmpfs')
    try:
        (memory / 'value').write_text('memory')
        (root / 'memory-link').symlink_to(memory)
        (memory / 'native-link').symlink_to(native)

        def cross_mount_links():
            assert (root / 'memory-link/value').read_text() == 'memory'
            assert (memory / 'native-link/value').read_text() == 'replacement'
            assert (memory / '../native/value').read_text() == 'replacement'
            assert (root / 'memory-link/value').stat().st_ino == (memory / 'value').stat().st_ino
            assert (root / 'memory-link/value').lstat().st_ino == (memory / 'value').stat().st_ino
            assert stat.S_ISLNK((root / 'memory-link/native-link').lstat().st_mode)
            assert stat.S_ISDIR((root / 'memory-link/native-link').stat().st_mode)
            (root / 'cycle').symlink_to(memory / 'cycle')
            (memory / 'cycle').symlink_to(root / 'cycle')
            for operation in (lambda: (root / 'cycle').stat(),
                              lambda: os.open(root / 'cycle', os.O_RDONLY)):
                try:
                    operation()
                except OSError as error:
                    assert error.errno == errno.ELOOP, error
                else:
                    raise AssertionError('cross-mount symlink cycle was followed')
        check('native-tmpfs-links-in-both-directions', cross_mount_links)

        def confined_root():
            child = os.fork()
            if child == 0:
                try:
                    os.chroot(root)
                    os.chdir('/')
                    assert Path('/native/value').read_text() == 'replacement'
                    assert Path('/memory/value').read_text() == 'memory'
                    assert Path('/entry/value').read_text() == 'replacement'
                    try:
                        Path('/memory-link/value').read_text()
                    except OSError as error:
                        assert error.errno == errno.ENOENT, error
                    else:
                        raise AssertionError('absolute link escaped chroot')
                    os._exit(0)
                except BaseException:
                    traceback.print_exc()
                    os._exit(91)
            assert os.waitpid(child, 0)[1] == 0
        check('relocated-root-and-absolute-link-confinement', confined_root)
    finally:
        assert libc.umount(os.fsencode(memory)) == 0, ctypes.get_errno()

print('ROOT_RESOLUTION_OK', flush=True)
