"""Exercise proc metadata through the real guest libc and raw descriptor APIs."""
import errno
import fcntl
import os
from pathlib import Path
import stat
import subprocess
import tempfile


def fields(path):
    return dict(line.split(':', 1) for line in Path(path).read_text().splitlines())


pid = os.getpid()
for prefix in ('/proc/self', f'/proc/{pid}', f'/proc/{pid}/task/{pid}'):
    memory = [int(n) for n in Path(prefix + '/statm').read_text().split()]
    assert len(memory) == 7 and memory[0] >= memory[1] > 0, memory
    io = fields(prefix + '/io')
    assert set(io) == {'rchar', 'wchar', 'syscr', 'syscw', 'read_bytes',
                       'write_bytes', 'cancelled_write_bytes'}, io
    assert all(int(n) >= 0 for n in io.values())
    assert stat.S_ISREG(os.stat(prefix + '/statm').st_mode)
    assert os.stat(prefix + '/statm').st_size == 0
    assert {'statm', 'io', 'fdinfo', 'smaps_rollup', 'auxv'} <= set(os.listdir(prefix))
    rollup = Path(prefix + '/smaps_rollup').read_text()
    assert '[rollup]' in rollup
    assert 'Rss:' in rollup and 'Pss:' in rollup
    assert os.stat(prefix + '/smaps_rollup').st_size == 0
    auxv = Path(prefix + '/auxv').read_bytes()
    assert len(auxv) > 0 and len(auxv) % 16 == 0
    assert os.stat(prefix + '/auxv').st_size == 0

with tempfile.TemporaryFile() as stream:
    fd = stream.fileno()
    stream.write(b'abcdef')
    stream.flush()
    os.lseek(fd, 3, os.SEEK_SET)
    for prefix in ('/proc/self', f'/proc/{pid}/task/{pid}'):
        info = fields(f'{prefix}/fdinfo/{fd}')
        assert int(info['pos']) == 3, info
        assert int(info['ino']) == os.fstat(fd).st_ino
        assert int(info['flags'], 8) & os.O_CLOEXEC
        assert int(info['flags'], 8) & 3 == os.O_RDWR
        assert str(fd) in os.listdir(prefix + '/fdinfo')
    duplicate = os.dup(fd)
    os.lseek(duplicate, 1, os.SEEK_SET)
    assert int(fields(f'/proc/self/fdinfo/{fd}')['pos']) == 1
    os.close(duplicate)
    fcntl.fcntl(fd, fcntl.F_SETFD, 0)
    assert not int(fields(f'/proc/self/fdinfo/{fd}')['flags'], 8) & os.O_CLOEXEC
    assert os.read(fd, 2) == b'bc'
for operation in (os.stat, lambda p: Path(p).read_bytes()):
    try:
        operation(f'/proc/self/fdinfo/{fd}')
    except OSError as error:
        assert error.errno == errno.ENOENT, error
    else:
        raise AssertionError('closed descriptor still exists')
for path in ('/proc/self/fdinfo/-1', '/proc/self/statm/nope', '/proc/self/io/nope',
             '/proc/self/smaps_rollup/nope', '/proc/self/auxv/nope', '/proc/cmdline/nope'):
    assert not os.path.exists(path), path

# /proc/cmdline verification
assert 'cmdline' in os.listdir('/proc')
cmdline = Path('/proc/cmdline').read_text()
assert 'BOOT_IMAGE=' in cmdline or len(cmdline) > 0
assert os.stat('/proc/cmdline').st_size == 0

# /proc/sys/vm/overcommit_memory verification
assert 'vm' in os.listdir('/proc/sys')
assert 'overcommit_memory' in os.listdir('/proc/sys/vm')
orig_overcommit = Path('/proc/sys/vm/overcommit_memory').read_text().strip()
assert orig_overcommit in ('0', '1', '2')
for unsupported in ('1\n', '2\n'):
    try:
        Path('/proc/sys/vm/overcommit_memory').write_text(unsupported)
    except OSError as error:
        assert error.errno == errno.EOPNOTSUPP
    else:
        raise AssertionError('unsupported commit policy reported success')
Path('/proc/sys/vm/overcommit_memory').write_text(f'{orig_overcommit}\n')
assert Path('/proc/sys/vm/overcommit_memory').read_text().strip() == orig_overcommit

child = subprocess.Popen(['/bin/sleep', '5'])
try:
    assert len(Path(f'/proc/{child.pid}/statm').read_text().split()) == 7
    assert len(fields(f'/proc/{child.pid}/io')) == 7
    assert '[rollup]' in Path(f'/proc/{child.pid}/smaps_rollup').read_text()
    assert len(Path(f'/proc/{child.pid}/auxv').read_bytes()) % 16 == 0
finally:
    child.terminate()
    child.wait()
print('PROC_INSPECTION_OK')
