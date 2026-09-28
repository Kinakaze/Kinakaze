"""Extended attributes through retained descriptors and procfs magic links."""
import ctypes as c
import errno
import json
import os
from pathlib import Path
import resource
import subprocess
import sys
import tempfile
import threading
import time

libc = c.CDLL('libc.so.6', use_errno=True)
rows = []


def check(name, action):
    try:
        action()
        result = dict(name=name, status='passed')
    except Exception as error:
        result = dict(name=name, status='failed', error=repr(error))
    rows.append(result)
    print(json.dumps(result), flush=True)


def fails(code, action):
    try:
        action()
    except OSError as error:
        assert error.errno == code, error
    else:
        raise AssertionError('operation unexpectedly succeeded')


def call(result):
    assert result == 0, c.get_errno()


if sys.argv[1:] == ['--benchmark']:
    with tempfile.TemporaryDirectory(prefix='xattr-performance-') as temporary:
        path = Path(temporary) / 'file'
        path.write_bytes(b'data')
        os.setxattr(path, 'user.test', b'value')
        descriptor = os.open(path, os.O_RDONLY)
        try:
            for name, target in (('path', path), ('descriptor', descriptor)):
                started = time.perf_counter()
                for _ in range(500):
                    assert os.getxattr(target, 'user.test') == b'value'
                print(json.dumps(dict(name=name, count=500, milliseconds=(time.perf_counter()-started)*1000)), flush=True)
        finally:
            os.close(descriptor)
    sys.exit(0)


with tempfile.TemporaryDirectory(prefix='xattr-lifetime-') as temporary:
    root = Path(temporary)
    path = root / 'file'
    path.write_bytes(b'original')
    os.setxattr(path, 'user.original', b'value')
    normal = os.open(path, os.O_RDWR)
    pin = os.open(path, os.O_PATH)
    try:
        def access(alias):
            assert os.getxattr(alias, 'user.original') == b'value'
            os.setxattr(alias, 'user.roundtrip', b'one', os.XATTR_CREATE)
            fails(errno.EEXIST, lambda: os.setxattr(alias, 'user.roundtrip', b'bad', os.XATTR_CREATE))
            os.setxattr(alias, 'user.roundtrip', b'two', os.XATTR_REPLACE)
            assert os.getxattr(normal, 'user.roundtrip') == b'two'
            assert 'user.roundtrip' in os.listxattr(alias)
            os.removexattr(alias, 'user.roundtrip')
            fails(errno.ENODATA, lambda: os.getxattr(alias, 'user.roundtrip'))
        for descriptor, name in ((normal, 'normal'), (pin, 'opath')):
            for prefix, kind in (('/proc/self/fd/', 'self'),
                                 (f'/proc/{os.getpid()}/fd/', 'numeric'),
                                 ('/proc/thread-self/fd/', 'thread')):
                check(f'{kind}-{name}-roundtrip', lambda alias=prefix + str(descriptor): access(alias))
        check('ordinary-fd-roundtrip', lambda: access(normal))
        check('direct-opath-rejected', lambda: fails(errno.EBADF, lambda: os.getxattr(pin, 'user.original')))
        def nofollow():
            alias = f'/proc/self/fd/{pin}'
            fails(errno.EOPNOTSUPP, lambda: os.setxattr(alias, 'user.original', b'bad', follow_symlinks=False))
            assert os.getxattr(normal, 'user.original') == b'value'
        check('nofollow-does-not-mutate-target', nofollow)
        def relative():
            before = os.getcwd()
            os.chdir('/proc/self/fd')
            try:
                access(str(pin))
            finally:
                os.chdir(before)
        check('relative-procfd-lookup', relative)
        def alternate_proc():
            target = root / 'proc'
            target.mkdir()
            call(libc.mount(b'proc', os.fsencode(target), b'proc', 0, None))
            try:
                access(str(target / 'self/fd' / str(pin)))
            finally:
                call(libc.umount2(os.fsencode(target), 0))
        check('alternate-proc-mount', alternate_proc)
        def fd_capacity():
            previous = resource.getrlimit(resource.RLIMIT_NOFILE)
            opened = []
            resource.setrlimit(resource.RLIMIT_NOFILE, (64, previous[1]))
            try:
                while True:
                    try:
                        opened.append(os.open('/dev/null', os.O_RDONLY))
                    except OSError as error:
                        assert error.errno == errno.EMFILE
                        break
                assert os.getxattr(f'/proc/self/fd/{pin}', 'user.original') == b'value'
                assert os.getxattr(normal, 'user.original') == b'value'
            finally:
                for descriptor in opened:
                    os.close(descriptor)
                resource.setrlimit(resource.RLIMIT_NOFILE, previous)
        check('metadata-lookup-at-fd-limit', fd_capacity)
        def inherited():
            script = '''import os,sys
fd=int(sys.argv[1])
assert os.getxattr('/proc/self/fd/'+str(fd),'user.original')==b'value'
os.setxattr('/proc/self/fd/'+str(fd),'user.child',b'exec')
'''
            subprocess.run([sys.executable, '-c', script, str(pin)], pass_fds=(pin,), check=True, timeout=15)
            assert os.getxattr(normal, 'user.child') == b'exec'
        check('exec-inherits-inode-and-attributes', inherited)
        def denied():
            os.chmod(path, 0o600)
            script = '''import errno,os,sys
fd=int(sys.argv[1]); os.setgroups([]); os.setgid(65534); os.setuid(65534)
for target in (fd,'/proc/self/fd/'+str(fd)):
    try: os.getxattr(target,'user.original')
    except OSError as error: assert error.errno==errno.EACCES,error
    else: raise AssertionError('read bypassed inode permissions')
try: os.fchmod(fd,0o777)
except OSError as error: assert error.errno==errno.EPERM,error
else: raise AssertionError('chmod bypassed inode ownership')
'''
            subprocess.run([sys.executable, '-c', script, str(normal)], pass_fds=(normal,), check=True, timeout=15)
        check('retained-fd-respects-current-credentials', denied)
        def rename_unlink():
            renamed = root / 'moved'
            path.rename(renamed)
            path.write_bytes(b'replacement')
            os.setxattr(path, 'user.original', b'new-inode')
            access(f'/proc/self/fd/{pin}')
            renamed.unlink()
            access(f'/proc/self/fd/{pin}')
            os.fchmod(normal, 0o640)
            assert os.fstat(normal).st_mode & 0o777 == 0o640
            assert os.getxattr(path, 'user.original') == b'new-inode'
        check('rename-unlink-and-name-replacement', rename_unlink)
        def reuse():
            other = os.open(path, os.O_RDONLY)
            slot = os.dup(normal)
            errors = []
            def writer():
                try:
                    for _ in range(1000):
                        os.dup2(other, slot)
                        os.dup2(normal, slot)
                except Exception as error:
                    errors.append(error)
            thread = threading.Thread(target=writer)
            thread.start()
            try:
                for _ in range(500):
                    assert os.getxattr(slot, 'user.original') in (b'value', b'new-inode')
                thread.join(15)
                assert not thread.is_alive() and not errors, errors
            finally:
                thread.join()
                os.close(slot)
                os.close(other)
        check('concurrent-dup2-keeps-metadata-handle-live', reuse)
        def readonly():
            source, target = root / 'source', root / 'mount'
            source.mkdir()
            target.mkdir()
            item = source / 'file'
            item.write_bytes(b'data')
            os.setxattr(item, 'user.test', b'original')
            call(libc.mount(os.fsencode(source), os.fsencode(target), None, 4096, None))
            descriptor = None
            attached = True
            try:
                call(libc.mount(None, os.fsencode(target), None, 4096 | 32 | 1, None))
                descriptor = os.open(target / 'file', os.O_PATH)
                alias = f'/proc/self/fd/{descriptor}'
                assert os.getxattr(alias, 'user.test') == b'original'
                fails(errno.EROFS, lambda: os.setxattr(alias, 'user.test', b'bad'))
                call(libc.umount2(os.fsencode(target), 2))
                attached = False
                assert os.getxattr(alias, 'user.test') == b'original'
                fails(errno.EROFS, lambda: os.removexattr(alias, 'user.test'))
                assert os.getxattr(item, 'user.test') == b'original'
            finally:
                if descriptor is not None:
                    os.close(descriptor)
                if attached:
                    call(libc.umount2(os.fsencode(target), 0))
        check('readonly-policy-survives-lazy-detach', readonly)
    finally:
        os.close(normal)
        os.close(pin)
    check('closed-descriptor-path-is-missing', lambda:
          fails(errno.ENOENT, lambda: os.getxattr(f'/proc/self/fd/{pin}', 'user.original')))

assert all(row['status'] == 'passed' for row in rows), rows
