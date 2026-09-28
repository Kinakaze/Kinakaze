"""Linux empty-path extensions and shell/package permission checks."""
import ctypes as c
import errno
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile

libc = c.CDLL('libc.so.6', use_errno=True)
AT_FDCWD, AT_EMPTY_PATH, AT_NOFOLLOW = -100, 0x1000, 0x100
rows = []


def check(name, action):
    try:
        action()
        row = dict(name=name, status='passed')
    except Exception as error:
        row = dict(name=name, status='failed', error=repr(error))
    rows.append(row)
    print(json.dumps(row), flush=True)


def expect(result, error=0):
    actual = c.get_errno() if result == -1 else 0
    assert (result, actual) == (-1 if error else 0, error), (result, actual, error)


with tempfile.TemporaryDirectory(prefix='path-access-') as directory:
    path = Path(directory) / 'file'
    path.write_bytes(b'original')
    path.chmod(0o600)
    link = Path(directory) / 'link'
    link.symlink_to('file')
    fd = os.open(path, os.O_PATH)
    dirfd = os.open(directory, os.O_PATH | os.O_DIRECTORY)
    linkfd = os.open(link, os.O_PATH | os.O_NOFOLLOW)
    try:
        for shell in ('/bin/bash', '/bin/dash'):
            def shell_test(shell=shell):
                # This is the condition used by Debian's dh_installmenu scripts.
                script = ('set -e; if [ -x "$(command -v kinakaze_no_such_command)" ]; then exit 11; fi; '
                          'for flag in -e -d -r -w -x; do if [ "$flag" "" ]; then exit 12; fi; done')
                subprocess.run([shell, '-ec', script], check=True)
            check(shell + '-optional-command-guard', shell_test)
        for name in ('access', 'eaccess', 'euidaccess'):
            for mode in (0, 1, 2, 4):
                check(f'{name}-empty-{mode}', lambda name=name, mode=mode:
                      expect(getattr(libc, name)(b'', mode), errno.ENOENT))
            check(name + '-nonexecutable-file', lambda name=name:
                  expect(getattr(libc, name)(os.fsencode(path), os.X_OK), errno.EACCES))
            check(name + '-invalid-mode', lambda name=name:
                  expect(getattr(libc, name)(os.fsencode(path), 8), errno.EINVAL))
        for call in ('stat', 'lstat', 'stat64', 'lstat64'):
            check(call + '-empty', lambda call=call:
                  expect(getattr(libc, call)(b'', c.create_string_buffer(256)), errno.ENOENT))
        def stat_at(descriptor, flags, error=0, expected_inode=None):
            data = c.create_string_buffer(256)
            expect(libc.fstatat(descriptor, b'', data, flags), error)
            if not error:
                assert struct.unpack_from('Q', data.raw, 8)[0] == expected_inode
        for descriptor, name, inode in ((fd, 'file', os.fstat(fd).st_ino),
                (dirfd, 'directory', os.fstat(dirfd).st_ino),
                (linkfd, 'link', os.fstat(linkfd).st_ino),
                (AT_FDCWD, 'cwd', os.stat('.').st_ino)):
            check('fstatat-empty-denied-' + name, lambda descriptor=descriptor:
                  stat_at(descriptor, 0, errno.ENOENT))
            check('fstatat-empty-explicit-' + name, lambda descriptor=descriptor, inode=inode:
                  stat_at(descriptor, AT_EMPTY_PATH, expected_inode=inode))
            check('faccessat-empty-denied-' + name, lambda descriptor=descriptor:
                  expect(libc.faccessat(descriptor, b'', 0, 0), errno.ENOENT))
            check('faccessat-empty-explicit-' + name, lambda descriptor=descriptor:
                  expect(libc.faccessat(descriptor, b'', 0, AT_EMPTY_PATH)))
        for descriptor in (-1, 2000000):
            check('fstatat-invalid-fd-' + str(descriptor), lambda descriptor=descriptor:
                  stat_at(descriptor, AT_EMPTY_PATH, errno.EBADF))
            check('fstatat-empty-before-fd-' + str(descriptor), lambda descriptor=descriptor:
                  stat_at(descriptor, 0, errno.ENOENT))
        check('fstatat-invalid-flags', lambda: stat_at(fd, 0x200, errno.EINVAL))
        for flags, error in ((0, errno.ENOENT), (AT_EMPTY_PATH, 0),
                (AT_EMPTY_PATH | 0x2000, 0), (AT_EMPTY_PATH | 0x4000, 0),
                (AT_EMPTY_PATH | 0x6000, errno.EINVAL)):
            check('statx-empty-flags-' + str(flags), lambda flags=flags, error=error:
                  expect(libc.statx(fd, b'', flags, 0x7ff, c.create_string_buffer(256)), error))
        check('statx-reserved-mask', lambda:
              expect(libc.statx(fd, b'', AT_EMPTY_PATH, c.c_uint(0x80000000), c.create_string_buffer(256)), errno.EINVAL))
        def raw_syscalls():
            data = c.create_string_buffer(256)
            expect(libc.syscall(21, b'', os.F_OK), errno.ENOENT)
            expect(libc.syscall(262, fd, b'', data, 0), errno.ENOENT)
            expect(libc.syscall(262, fd, b'', data, AT_EMPTY_PATH))
            expect(libc.syscall(269, dirfd, b'', os.F_OK), errno.ENOENT)
            expect(libc.syscall(269, dirfd, b'file', os.F_OK, 0x7fffffff))
            expect(libc.syscall(439, AT_FDCWD, b'', os.F_OK, AT_EMPTY_PATH))
            expect(libc.syscall(332, fd, b'', AT_EMPTY_PATH, 0x7ff, data))
        check('raw-syscalls-use-the-same-path-rules', raw_syscalls)
        def nonempty_extension():
            data = c.create_string_buffer(256)
            expect(libc.fstatat(dirfd, b'file', data, AT_EMPTY_PATH))
            assert struct.unpack_from('Q', data.raw, 8)[0] == os.fstat(fd).st_ino
            expect(libc.faccessat(dirfd, b'file', os.X_OK, AT_EMPTY_PATH), errno.EACCES)
        check('empty-flag-preserves-nonempty-name', nonempty_extension)
        def ownership(descriptor):
            before = os.fstat(descriptor)
            expect(libc.fchown(descriptor, 0, 0), errno.EBADF)
            expect(libc.fchownat(descriptor, b'', 1234, 1235, 0), errno.ENOENT)
            expect(libc.fchownat(descriptor, b'', 1234, 1235, AT_EMPTY_PATH))
            changed = os.fstat(descriptor)
            assert (changed.st_uid, changed.st_gid, changed.st_ino) == (1234, 1235, before.st_ino)
            expect(libc.fchownat(descriptor, b'', before.st_uid, before.st_gid, AT_EMPTY_PATH | AT_NOFOLLOW))
            assert (os.fstat(descriptor).st_uid, os.fstat(descriptor).st_gid) == (before.st_uid, before.st_gid)
        for descriptor, name in ((fd, 'file'), (dirfd, 'directory'), (linkfd, 'symlink')):
            check('fchownat-opath-' + name, lambda descriptor=descriptor: ownership(descriptor))
        def relative_ownership():
            for name in (b'file', b'link'):
                relative = os.open(name, os.O_PATH | os.O_NOFOLLOW, dir_fd=dirfd)
                try:
                    ownership(relative)
                finally:
                    os.close(relative)
        check('fchownat-opath-relative', relative_ownership)
        def tmpfs_ownership():
            with tempfile.TemporaryDirectory(prefix='path-owner-') as tmp:
                expect(libc.mount(b'tmpfs', os.fsencode(tmp), b'tmpfs', 0, b'size=1048576'))
                try:
                    name = Path(tmp) / 'file'
                    name.write_bytes(b'tmpfs')
                    for item in (tmp, name):
                        descriptor = os.open(item, os.O_PATH | os.O_NOFOLLOW)
                        try:
                            ownership(descriptor)
                        finally:
                            os.close(descriptor)
                finally:
                    expect(libc.umount2(os.fsencode(tmp), 0))
        check('fchownat-opath-tmpfs', tmpfs_ownership)
        def readonly_ownership():
            with tempfile.TemporaryDirectory(prefix='path-readonly-') as tmp:
                source, target = Path(tmp) / 'source', Path(tmp) / 'target'
                source.mkdir()
                target.mkdir()
                expect(libc.mount(os.fsencode(source), os.fsencode(target), None, 4096, None))
                try:
                    expect(libc.mount(None, os.fsencode(target), None, 4096 | 32 | 1, None))
                    descriptor = os.open(target, os.O_PATH | os.O_DIRECTORY)
                    try:
                        expect(libc.fchownat(descriptor, b'', 1234, 1235, AT_EMPTY_PATH), errno.EROFS)
                        assert os.fstat(descriptor).st_uid == 0
                    finally:
                        os.close(descriptor)
                finally:
                    expect(libc.umount2(os.fsencode(target), 0))
        check('fchownat-opath-preserves-readonly-mount', readonly_ownership)
        def pipe_extension():
            read, write = os.pipe()
            try:
                stat_at(read, AT_EMPTY_PATH, expected_inode=os.fstat(read).st_ino)
                expect(libc.faccessat(read, b'', os.F_OK, AT_EMPTY_PATH))
            finally:
                os.close(write)
                os.close(read)
        check('empty-extension-supports-non-path-handles', pipe_extension)
        def different_ids():
            # access() uses real IDs; eaccess() uses effective IDs.
            os.seteuid(1001)
            try:
                expect(libc.access(os.fsencode(path), os.R_OK))
                expect(libc.eaccess(os.fsencode(path), os.R_OK), errno.EACCES)
                expect(libc.euidaccess(os.fsencode(path), os.R_OK), errno.EACCES)
            finally:
                os.seteuid(0)
        check('real-and-effective-access-identities', different_ids)
        def descriptor_after_unlink():
            path.unlink()
            stat_at(fd, AT_EMPTY_PATH, expected_inode=os.fstat(fd).st_ino)
            expect(libc.faccessat(fd, b'', os.R_OK, AT_EMPTY_PATH))
            ownership(fd)
            expect(libc.access(os.fsencode(path), os.F_OK), errno.ENOENT)
        check('empty-extension-retains-unlinked-inode', descriptor_after_unlink)
    finally:
        os.close(linkfd)
        os.close(dirfd)
        os.close(fd)

assert all(row['status'] == 'passed' for row in rows), rows
