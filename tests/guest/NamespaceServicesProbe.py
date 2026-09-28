"""Process clone, virtual/native mount crossings, and credential-style moves."""
import ctypes
import errno
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import traceback

libc = ctypes.CDLL(None, use_errno=True)
libc.syscall.restype = ctypes.c_long
original = Path('/proc/self/mountinfo').read_text()


def mount(source, target, filesystem, flags, data=None):
    def encode(value):
        return os.fsencode(value) if value is not None else None
    assert libc.mount(encode(source), encode(target), encode(filesystem),
                      ctypes.c_ulong(flags), encode(data)) == 0, (target, flags, ctypes.get_errno())


with tempfile.TemporaryDirectory(prefix='namespace-services-') as directory:
    # This is the libc syscall entry, not a trapped machine instruction. A
    # stackless process clone must not require a signal-return register frame.
    child = libc.syscall(56, 0x20000 | signal.SIGCHLD, 0, 0, 0, 0)
    assert child >= 0, ctypes.get_errno()
    if child == 0:
        try:
            mount(None, '/dev', None, (1 << 19) | 16384)
            mount('tmpfs', directory, 'tmpfs', 0, 'size=8m')
            alias = directory + '/root'
            os.mkdir(alias)
            mount('/', alias, None, 4096 | 16384)
            fd = os.open('/', os.O_PATH | os.O_DIRECTORY)
            try:
                for part in (alias + '/proc/sys/kernel/domainname').strip('/').split('/'):
                    following = os.open(part, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
                    os.close(fd)
                    fd = following
                assert os.fstat(fd).st_dev == os.stat('/proc/sys/kernel/domainname').st_dev
            finally:
                os.close(fd)
            with open(alias + '/etc/os-release') as file:
                assert file.read() == Path('/etc/os-release').read_text()
            hostname = alias + '/proc/sys/kernel/hostname'
            mount(hostname, hostname, None, 4096 | 16384)
            fd = os.open(hostname, os.O_PATH | os.O_CLOEXEC)
            try:
                descriptor = '/proc/self/fd/' + str(fd)
                assert os.readlink(descriptor) == hostname
                mount(None, descriptor, None, 4096 | 32 | 1)
                try:
                    os.close(os.open(hostname, os.O_WRONLY))
                    raise AssertionError('read-only proc bind accepted a write open')
                except OSError as error:
                    assert error.errno == errno.EROFS, error
            finally:
                os.close(fd)
            staging, credentials = directory + '/staging', directory + '/credentials'
            os.mkdir(staging)
            os.mkdir(credentials)
            mount('tmpfs', staging, 'tmpfs', 2 | 4 | 8, 'size=1m,mode=0700')
            Path(staging + '/secret').write_text('credential')
            mount(None, staging, None, 32 | 1 | 2 | 4 | 8)
            mount(staging, credentials, None, 8192)
            assert Path(credentials + '/secret').read_text() == 'credential'
            try:
                Path(credentials + '/secret').write_text('changed')
                raise AssertionError('read-only credential mount accepted a write')
            except OSError as error:
                assert error.errno == errno.EROFS, error
            os.chdir(alias)
            mount('.', '/', None, 8192)
            assert os.getcwd() == '/'
            assert subprocess.check_output(['/bin/pwd'], text=True, timeout=10).strip() == '/'
            os.chroot('.')
            os.chdir('/')
            mount(None, '/', None, (1 << 18) | 16384)
            assert Path('/proc/self/mountinfo').read_text()
            os._exit(0)
        except BaseException:
            traceback.print_exc()
            os._exit(1)
    assert os.waitpid(child, 0) == (child, 0)
    assert Path('/proc/self/mountinfo').read_text() == original
    assert list(Path(directory).iterdir()) == []

print('NAMESPACE_SERVICES_OK', flush=True)
