"""Directory open descriptions share the kernel cursor across dup/fork/exec."""
import array
import ctypes as c
import errno
import concurrent.futures
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile

libc = c.CDLL(None, use_errno=True)
libc.syscall.restype = c.c_long


def one(fd):
    for size in range(24, 281, 8):
        buffer = c.create_string_buffer(size)
        length = libc.syscall(c.c_long(217), c.c_int(fd), c.byref(buffer), c.c_size_t(size))
        if length < 0:
            if c.get_errno() == errno.EINVAL:
                continue
            raise OSError(c.get_errno(), 'getdents64')
        if not length:
            return None
        assert length == size, (length, size)
        return c.string_at(c.addressof(buffer) + 19).decode()
    raise AssertionError('no single directory entry fits NAME_MAX')


if len(sys.argv) == 3 and sys.argv[1] == 'read':
    print(one(int(sys.argv[2])), flush=True)
    sys.exit(0)

results = []
with tempfile.TemporaryDirectory(prefix='directory-cursor-') as temporary:
    directory = Path(temporary)
    for name in ('a0', 'b0', 'c0'):
        (directory / name).touch()

    def run(name, action, path=directory):
        fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
        try:
            action(fd)
            result = dict(name=name, status='passed')
        except Exception as error:
            result = dict(name=name, status='failed', error=str(error))
        finally:
            os.close(fd)
        results.append(result)
        print(json.dumps(result), flush=True)

    def duplicate(fd):
        assert one(fd) == '.'
        alias = os.dup(fd)
        try:
            assert one(alias) == '..', 'dup did not share directory cursor'
            assert one(fd) not in ('.', '..', None), 'original cursor did not advance'
        finally:
            os.close(alias)

    def seeking(fd):
        assert one(fd) == '.'
        cookie = os.lseek(fd, 0, os.SEEK_CUR)
        assert cookie > 0
        assert one(fd) == '..'
        assert os.lseek(fd, cookie, os.SEEK_SET) == cookie
        assert one(fd) == '..'
        assert os.lseek(fd, 0, os.SEEK_SET) == 0
        assert one(fd) == '.'

    def forking(fd):
        assert one(fd) == '.'
        read, write = os.pipe()
        child = os.fork()
        if not child:
            try:
                os.close(read)
                os.write(write, one(fd).encode())
            except BaseException:
                os._exit(1)
            os._exit(0)
        os.close(write)
        try:
            child_value = os.read(read, 32)
        finally:
            os.close(read)
        assert os.waitpid(child, 0) == (child, 0)
        assert child_value == b'..', child_value
        assert one(fd) not in ('.', '..', None), 'fork cursor was copied instead of shared'

    def executing(fd):
        assert one(fd) == '.'
        output = subprocess.check_output([sys.executable, __file__, 'read', str(fd)], pass_fds=(fd,), timeout=10)
        assert output.strip() == b'..', output
        assert one(fd) not in ('.', '..', None), 'exec child cursor did not propagate'

    def transferred(fd):
        assert one(fd) == '.'
        left, right = socket.socketpair()
        try:
            left.sendmsg([b'd'], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
            _, controls, _, _ = right.recvmsg(1, socket.CMSG_SPACE(4))
            received = array.array('i')
            received.frombytes(controls[0][2][:4])
            try:
                assert one(received[0]) == '..'
                assert one(fd) not in ('.', '..', None)
            finally:
                os.close(received[0])
        finally:
            left.close()
            right.close()

    def reopened(fd):
        assert one(fd) == '.'
        independent = os.open(f'/proc/self/fd/{fd}', os.O_RDONLY | os.O_DIRECTORY)
        try:
            assert one(independent) == '.'
            assert one(fd) == '..'
        finally:
            os.close(independent)

    for name, action in [('dup', duplicate), ('seek', seeking), ('fork', forking),
                         ('exec', executing), ('scm-rights', transferred), ('independent-reopen', reopened)]:
        run(name, action)
    run('proc-dup', duplicate, '/proc/self/ns')
    run('proc-seek', seeking, '/proc/self/ns')
    run('proc-fork', forking, '/proc/self/ns')
    run('proc-exec', executing, '/proc/self/ns')
    run('proc-scm-rights', transferred, '/proc/self/ns')

    def expect_error(expected, action):
        try:
            action()
        except OSError as error:
            assert error.errno == expected, (expected, error)
        else:
            raise AssertionError('operation unexpectedly succeeded')

    def bounds(fd):
        small = c.create_string_buffer(8)
        assert libc.syscall(c.c_long(217), c.c_int(fd), c.byref(small), c.c_size_t(8)) == -1
        assert c.get_errno() == errno.EINVAL
        assert one(fd) == '.', 'failed read advanced cursor'
        expect_error(errno.EINVAL, lambda: os.lseek(fd, -1, os.SEEK_SET))
        expect_error(errno.EINVAL, lambda: os.lseek(fd, 0, os.SEEK_END))
        assert one(fd) == '..'
        os.lseek(fd, (1 << 63) - 1, os.SEEK_SET)
        expect_error(errno.EINVAL, lambda: os.lseek(fd, 1, os.SEEK_CUR))
        os.lseek(fd, 1000000, os.SEEK_SET)
        assert one(fd) is None
        os.lseek(fd, 0, os.SEEK_SET)
        assert one(fd) == '.'
        path_fd = os.open(directory, os.O_PATH | os.O_DIRECTORY)
        try:
            expect_error(errno.EBADF, lambda: one(path_fd))
            expect_error(errno.EBADF, lambda: os.lseek(path_fd, 0, os.SEEK_SET))
        finally:
            os.close(path_fd)

    def legacy(fd):
        data = c.create_string_buffer(24)
        assert libc.syscall(c.c_long(78), c.c_int(fd), c.byref(data), c.c_size_t(24)) == 24
        assert c.string_at(c.addressof(data) + 18) == b'.'
        assert data.raw[23] == 4
        assert one(fd) == '..', 'legacy and wide calls have different cursors'

    def threaded(fd):
        def drain(_):
            alias = os.dup(fd)
            names = []
            try:
                while (name := one(alias)) is not None:
                    names.append(name)
                return names
            finally:
                os.close(alias)
        with concurrent.futures.ThreadPoolExecutor(4) as pool:
            names = sum(pool.map(drain, range(4)), [])
        assert len(names) == len(set(names)), names
        assert set(names) == {'.', '..', 'a0', 'b0', 'c0'}, names

    def stream(fd):
        libc.fdopendir.argtypes = [c.c_int]
        libc.fdopendir.restype = c.c_void_p
        libc.readdir.argtypes = [c.c_void_p]
        libc.readdir.restype = c.c_void_p
        libc.telldir.argtypes = [c.c_void_p]
        libc.telldir.restype = c.c_long
        libc.seekdir.argtypes = [c.c_void_p, c.c_long]
        libc.closedir.argtypes = [c.c_void_p]
        assert one(fd) == '.'
        alias = os.dup(fd)
        handle = libc.fdopendir(alias)
        assert handle, c.get_errno()
        def read():
            pointer = libc.readdir(handle)
            return c.string_at(pointer + 19).decode() if pointer else None
        try:
            assert read() == '..', 'fdopendir rewound the shared cursor'
            cookie = libc.telldir(handle)
            name = read()
            libc.seekdir(handle, cookie)
            assert read() == name
            libc.seekdir(handle, 0)
            assert read() == '.'
        finally:
            assert libc.closedir(handle) == 0

    def remote_rights(_, path=directory):
        left, right = socket.socketpair()
        # Create the description after fork, so SCM_RIGHTS alone must publish it.
        child = os.fork()
        if not child:
            try:
                left.close()
                _, controls, _, _ = right.recvmsg(1, socket.CMSG_SPACE(4))
                received = array.array('i')
                received.frombytes(controls[0][2][:4])
                right.sendall(one(received[0]).encode())
                os.close(received[0])
                right.close()
                os._exit(0)
            except BaseException:
                os._exit(1)
        right.close()
        source = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
        try:
            assert one(source) == '.'
            left.sendmsg([b'd'], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [source]))])
            left.settimeout(10)
            assert left.recv(32) == b'..'
            assert os.waitpid(child, 0) == (child, 0)
            child = 0
            assert one(source) not in ('.', '..', None), 'receiver did not advance sender cursor'
        finally:
            os.close(source)
            left.close()
            if child:
                try:
                    os.kill(child, 9)
                except ProcessLookupError:
                    pass
                os.waitpid(child, 0)

    run('remote-scm-rights', remote_rights)
    run('proc-remote-scm-rights', lambda fd: remote_rights(fd, '/proc/self/ns'))

    run('bounds-and-path-only', bounds)
    run('legacy-wide-cursor', legacy)
    run('concurrent-dup', threaded)
    run('fdopendir-seek', stream)

    # Exercise the same API over other providers, in a private mount namespace.
    assert libc.unshare(c.c_int(0x20000)) == 0, c.get_errno()
    libc.mount.argtypes = [c.c_char_p, c.c_char_p, c.c_char_p, c.c_ulong, c.c_void_p]
    libc.umount2.argtypes = [c.c_char_p, c.c_int]
    def mounted(provider, options=None):
        target = directory / provider
        target.mkdir()
        encoded = os.fsencode(target)
        assert libc.mount(provider.encode(), encoded, provider.encode(), 0, options) == 0, c.get_errno()
        try:
            for name in ('a0', 'b0', 'c0'):
                (target / name).touch()
            for name, action in [('dup', duplicate), ('seek', seeking), ('fork', forking),
                                 ('exec', executing), ('scm-rights', transferred), ('fdopendir', stream),
                                 ('remote-scm-rights', lambda fd: remote_rights(fd, target))]:
                run(provider + '-' + name, action, target)
        finally:
            assert libc.umount2(encoded, 0) == 0, c.get_errno()
    mounted('tmpfs')
    for name in ('lower', 'upper', 'work'):
        (directory / name).mkdir()
    options = ','.join(key + '=' + str(directory / name) for key, name in
                       [('lowerdir', 'lower'), ('upperdir', 'upper'), ('workdir', 'work')]).encode()
    mounted('overlay', options)



assert all(result['status'] == 'passed' for result in results), results
print('DIRECTORY_CURSOR_OK', flush=True)
