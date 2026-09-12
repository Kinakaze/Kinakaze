"""shadow/passwd/group ABI, invalid records, long lines and fortified fgets."""
import ctypes as c
import errno
import fcntl
import os
import tempfile

lib = c.CDLL('libc.so.6', use_errno=True)
p, z, i = c.c_void_p, c.c_size_t, c.c_int

def bind(name, result, *args):
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args
    return fn

class Shadow(c.Structure):
    _fields_ = [('name', p), ('password', p), ('last', c.c_long), ('minimum', c.c_long),
                ('maximum', c.c_long), ('warn', c.c_long), ('inactive', c.c_long),
                ('expire', c.c_long), ('flag', c.c_ulong)]

class Passwd(c.Structure):
    _fields_ = [('name', c.c_char_p), ('password', c.c_char_p), ('uid', c.c_uint),
                ('gid', c.c_uint), ('gecos', c.c_char_p), ('directory', c.c_char_p),
                ('shell', c.c_char_p)]

class Group(c.Structure):
    _fields_ = [('name', c.c_char_p), ('password', c.c_char_p), ('gid', c.c_uint),
                ('members', c.POINTER(c.c_char_p))]

parse = bind('sgetspent', c.POINTER(Shadow), c.c_char_p)
read_shadow = bind('fgetspent', c.POINTER(Shadow), p)
open_file = bind('fopen', p, c.c_char_p, c.c_char_p)
close = bind('fclose', i, p)
checked = bind('__fgets_chk', p, p, z, i, p)
ferror = bind('ferror', i, p)

def snapshot(entry):
    assert entry
    s = entry.contents
    return (c.string_at(s.name), c.string_at(s.password) if s.password else None,
            s.last, s.minimum, s.maximum, s.warn, s.inactive, s.expire, s.flag)

source = b'alice:!locked:123:0:99999:7:::42\nignored'
assert snapshot(parse(source)) == (b'alice', b'!locked', 123, 0, 99999, 7, -1, -1, 42)
assert snapshot(parse(b'alice:!:1:2:3')) == (b'alice', b'!', 1, 2, 3, -1, -1, -1, 2**64-1)
assert snapshot(parse(b'+netgroup')) == (b'+netgroup', None, 0, 0, 0, -1, -1, -1, 2**64-1)
assert snapshot(parse(b'bob:!:::::::')) == (b'bob', b'!', -1, -1, -1, -1, -1, -1, 2**64-1)
for invalid in [b'broken', b'a:b:bad:0:1', b'a:b:1:2:3:4:5:6:7:8', b'a:b:1:2:']:
    c.set_errno(123)
    assert not parse(invalid), invalid
    assert c.get_errno() == 123
long_password = b'x' * 20000
entry = parse(b'long:' + long_password + b':1:2:3:4:5:6:7')
assert snapshot(entry)[1] == long_password
pid = os.fork()
if pid == 0:
    try:
        assert snapshot(entry)[1] == long_password
    except BaseException:
        os._exit(1)
    os._exit(0)
assert os.waitpid(pid, 0) == (pid, 0)

with tempfile.TemporaryDirectory(prefix='account-files-') as directory:
    path = os.fsencode(directory + '/records')
    with open(path, 'wb') as stream:
        stream.write(b'# comment\n \t\ninvalid record\n  user:' + long_password + b':1:2:3:4:5:6:7\nlast:!:7:8:9')
    file = open_file(path, b'r')
    assert file
    assert snapshot(read_shadow(file)) == (b'user', long_password, 1, 2, 3, 4, 5, 6, 7)
    assert snapshot(read_shadow(file)) == (b'last', b'!', 7, 8, 9, -1, -1, -1, 2**64-1)
    c.set_errno(0)
    assert not read_shadow(file) and c.get_errno() == errno.ENOENT
    assert close(file) == 0

    def output(name, record, expected, error=None):
        file = open_file(path, b'w')
        assert file
        fn = bind(name, i, p, p)
        c.set_errno(0)
        result = fn(c.byref(record), file)
        if error is None:
            assert result == 0, (name, result, c.get_errno())
        else:
            assert result == -1 and c.get_errno() == error, (name, result, c.get_errno())
        assert close(file) == 0
        with open(path, 'rb') as stream:
            assert stream.read() == expected, name

    password = Passwd(b'alice', b'x', 123, 456, b'A:B\nC', b'/home/alice', b'/bin/sh')
    output('putpwent', password, b'alice:x:123:456:A B C:/home/alice:/bin/sh\n')
    password.name = b'+alice'
    output('putpwent', password, b'+alice:x:::A B C:/home/alice:/bin/sh\n')
    password.name = b'bad:name'
    output('putpwent', password, b'', errno.EINVAL)
    members = (c.c_char_p * 3)(b'alice', b'bob', None)
    group = Group(b'workers', b'x', 321, members)
    output('putgrent', group, b'workers:x:321:alice,bob\n')
    group.name = b'-workers'
    output('putgrent', group, b'-workers:x::alice,bob\n')
    members[1] = b'bad,member'
    output('putgrent', group, b'', errno.EINVAL)
    shadow = parse(b'alice:!:123:0:99999:7:::').contents
    output('putspent', shadow, b'alice:!:123:0:99999:7:::\n')
    bad = c.create_string_buffer(b'bad\nname')
    shadow.name = c.addressof(bad)
    output('putspent', shadow, b'', errno.EINVAL)

    password.name = b'alice'
    members[1] = b'bob'
    shadow = parse(b'alice:!:123:0:99999:7:::').contents
    shadow.flag = 2**63 + 17
    # glibc formats sp_flag through signed long despite its unsigned field type.
    output('putspent', shadow, b'alice:!:123:0:99999:7:::-9223372036854775791\n')
    fdopen = bind('fdopen', p, i, c.c_char_p)
    for name, record in [('putpwent', password), ('putgrent', group), ('putspent', shadow)]:
        fn = bind(name, i, p, p)
        for file in [open_file(path, b'r'), fdopen(os.open(path, os.O_RDWR), b'r')]:
            assert file
            c.set_errno(0)
            assert fn(c.byref(record), file) == -1 and c.get_errno() == errno.EBADF, name
            assert ferror(file), name
            # The stream's access mode must survive fork as well as its buffers.
            pid = os.fork()
            if pid == 0:
                c.set_errno(0)
                result = fn(c.byref(record), file)
                os._exit(0 if result == -1 and c.get_errno() == errno.EBADF else 1)
            assert os.waitpid(pid, 0) == (pid, 0)
            assert close(file) == 0
    fd = os.open(path, os.O_RDONLY)
    assert not fdopen(fd, b'w') and c.get_errno() == errno.EINVAL
    os.close(fd)
    file = fdopen(os.open(path, os.O_RDWR), b'w')
    assert file
    assert not read_shadow(file) and c.get_errno() == errno.EBADF and ferror(file)
    assert close(file) == 0
    with open(path, 'wb') as stream:
        stream.write(b'prefix\n')
    fd = os.open(path, os.O_RDWR)
    file = fdopen(fd, b'a')
    assert file and fcntl.fcntl(fd, fcntl.F_GETFL) & os.O_APPEND
    assert os.lseek(fd, 0, os.SEEK_CUR) == 7
    assert bind('putpwent', i, p, p)(c.byref(password), file) == 0
    assert close(file) == 0
    with open(path, 'rb') as stream:
        assert stream.read() == b'prefix\nalice:x:123:456:A B C:/home/alice:/bin/sh\n'
    file = open_file(path, b'r')
    assert bind('freopen', p, c.c_char_p, c.c_char_p, p)(path, b'w', file) == file
    assert bind('putpwent', i, p, p)(c.byref(password), file) == 0
    assert close(file) == 0

    with open(path, 'wb') as stream:
        stream.write(b'ok\ntail')
    file = open_file(path, b'r')
    buffer = c.create_string_buffer(b'?' * 8)
    # A large requested count is safe when the actual short line fits.
    assert checked(buffer, 4, 100, file) == c.addressof(buffer)
    assert buffer.raw[:5] == b'ok\n\0?'
    assert checked(buffer, 8, 4, file) == c.addressof(buffer)
    assert buffer.value == b'tai'
    assert checked(buffer, 8, 4, file) == c.addressof(buffer) and buffer.value == b'l'
    assert not checked(buffer, 8, 4, file)
    assert close(file) == 0
    file = open_file(path, b'r')
    before = buffer.raw
    assert not checked(buffer, 8, 0, file) and buffer.raw == before
    assert not checked(buffer, 8, 1, file) and buffer.raw == before
    assert close(file) == 0
    pid = os.fork()
    if pid == 0:
        file = open_file(path, b'r')
        checked(buffer, 2, 100, file)
        os._exit(99)
    _, status = os.waitpid(pid, 0)
    assert os.WIFSIGNALED(status) and os.WTERMSIG(status) == 6, status

    # Partial nonblocking input must survive EAGAIN and leave the error flag set.
    r, w = os.pipe2(os.O_NONBLOCK)
    os.write(w, b'xy')
    file = bind('fdopen', p, i, c.c_char_p)(r, b'r')
    assert checked(buffer, 8, 8, file) == c.addressof(buffer) and buffer.value == b'xy'
    assert c.get_errno() == errno.EAGAIN and ferror(file)
    os.write(w, b'z\n')
    assert checked(buffer, 8, 8, file) == c.addressof(buffer) and buffer.value == b'z\n'
    assert ferror(file), 'previous sticky error was lost'
    assert close(file) == 0
    os.close(w)
print('ACCOUNT_FILES_PARSE_WRITE_FORTIFY_FORK_OK')
