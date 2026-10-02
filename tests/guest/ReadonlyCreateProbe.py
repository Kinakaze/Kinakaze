"""A newly created readonly inode still permits writes through its creating open."""

import os
from pathlib import Path
import pwd
import subprocess
import sys
import tempfile


child = r'''
import errno
import fcntl
import os
import sys

directory, uid, gid = sys.argv[1:]
os.setgroups([])
os.setgid(int(gid))
os.setuid(int(uid))
for suffix in ('plain', './dot', 'sub/../parent'):
    path = directory + '/' + suffix
    descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_CLOEXEC, 0o444)
    try:
        assert os.fstat(descriptor).st_mode & 0o777 == 0o444
        assert fcntl.fcntl(descriptor, fcntl.F_GETFL) & os.O_ACCMODE == os.O_WRONLY
        assert fcntl.fcntl(descriptor, fcntl.F_GETFD) & fcntl.FD_CLOEXEC
        assert os.write(descriptor, b'object') == 6
    finally:
        os.close(descriptor)
    with open(path, 'rb') as stream:
        assert stream.read() == b'object'
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_TRUNC)
    except OSError as error:
        assert error.errno == errno.EACCES, error
    else:
        os.close(descriptor)
        raise AssertionError('existing readonly inode reopened for write')
os.chmod(directory, 0o500)
try:
    os.open(directory + '/./forbidden', os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
except OSError as error:
    assert error.errno == errno.EACCES, error
else:
    raise AssertionError('created file in unwritable parent')
finally:
    os.chmod(directory, 0o755)
print('READONLY_CREATE_DOT_PARENT_FD_ACCESS_OK')
'''

account = pwd.getpwnam('nobody')
with tempfile.TemporaryDirectory(prefix='readonly-create-') as directory:
    os.chown(directory, account.pw_uid, account.pw_gid)
    os.chmod(directory, 0o755)
    (Path(directory) / 'sub').mkdir()
    subprocess.run([sys.executable, '-c', child, directory, str(account.pw_uid),
                    str(account.pw_gid)], check=True, timeout=20)
