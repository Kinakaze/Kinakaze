"""Procfs credentials track setresid, filesystem IDs and exec across processes."""
import ctypes as c
import errno
import os
from pathlib import Path
import subprocess
import sys

def ids(pid):
    rows = dict(line.split(':', 1) for line in Path(f'/proc/{pid}/status').read_text().splitlines())
    return tuple(tuple(map(int, rows[key].split())) for key in ('Uid', 'Gid'))

if len(sys.argv) > 1:
    if sys.argv[1] == 'child':
        os.setresgid(1201, 1202, 1203)
        os.setresuid(1101, 1102, 1103)
        libc = c.CDLL('libc.so.6')
        libc.setfsuid(1101)
        libc.setfsgid(1201)
        assert ids('self') == ((1101, 1102, 1103, 1101), (1201, 1202, 1203, 1201))
        print(os.getpid(), flush=True)
        input()
        os.execv(sys.executable, [sys.executable, __file__, 'exec'])
    assert ids('self') == ((1101, 1102, 1102, 1102), (1201, 1202, 1202, 1202))
    print(os.getpid(), flush=True)
    input()
    sys.exit(0)

assert ids('self')[0][:3] == os.getresuid()
with subprocess.Popen([sys.executable, __file__, 'child'], stdin=subprocess.PIPE,
                      stdout=subprocess.PIPE, text=True) as child:
    pid = int(child.stdout.readline())
    assert ids(pid) == ((1101, 1102, 1103, 1101), (1201, 1202, 1203, 1201))
    child.stdin.write('exec\n'); child.stdin.flush()
    assert int(child.stdout.readline()) == pid
    assert ids(pid) == ((1101, 1102, 1102, 1102), (1201, 1202, 1202, 1202))
    child.stdin.write('finish\n'); child.stdin.flush()
    assert child.wait(timeout=10) == 0
for path in ('/sys/class/drm/does-not-exist', '/sys/class/dmi/id/vendor', '/sys/not-a-cgroup'):
    try:
        os.stat(path)
    except OSError as error:
        assert error.errno == errno.ENOENT, (path, error)
    else:
        raise AssertionError('Unknown sysfs path aliased the cgroup root: ' + path)
assert 'cgroup.controllers' in os.listdir('/sys/fs/cgroup')
print('PROC_CREDENTIALS_EXEC_AND_SYSFS_OK')
