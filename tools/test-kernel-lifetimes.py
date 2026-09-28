"""Exercise kernel catalogues and last-close semantics in real Debian workers."""
import argparse
import json
import time
import uuid
from pathlib import Path

from init_pool import InitPool


def ready(pool, offset, marker):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        with pool.lock:
            if marker in pool.buffers[0][offset:]:
                return
        time.sleep(.01)
    raise AssertionError(f'worker never printed {marker!r}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--case', choices=['all', 'locks'], default='all')
    args = parser.parse_args()
    rows = []
    target = '/tmp/kernel-lifetimes-' + uuid.uuid4().hex

    def run(pool, name, script, marker='LIFETIME_OK'):
        row = pool.run(['/usr/bin/python3', '-c', script], expect=[marker])
        row['name'] = name
        rows.append(row)
        assert row['status'] == 'passed', row

    with InitPool(args.root, args.dist, args.output / 'left', size=1, timeout=180) as left:
        with InitPool(args.root, args.dist, args.output / 'right', size=1, timeout=180) as right:
            with left.lock:
                offset = len(left.buffers[0])
            owner = left.launch(['/usr/bin/python3', '-c', f'''
import fcntl, os, time
with open({target!r}, 'a+b') as file:
    fcntl.lockf(file, fcntl.LOCK_EX)
    print('LOCK_OWNER_READY', flush=True)
    while not os.path.exists({(target + '.stop')!r}):
        time.sleep(.01)
'''])
            ready(left, offset, b'LOCK_OWNER_READY')
            blocked = f'''
import errno, fcntl
with open({target!r}, 'r+b') as file:
    try:
        fcntl.lockf(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as error:
        assert error.errno in (errno.EAGAIN, errno.EACCES), error
    else:
        raise AssertionError('a live owner lost its record lock')
print('LIFETIME_OK')
'''
            acquired = f'''
import fcntl
with open({target!r}, 'r+b') as file:
    fcntl.lockf(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
print('LIFETIME_OK')
'''
            run(left, 'same kernel sees the live record lock', blocked)
            run(right, 'independent kernel has its own lock table', acquired)
            run(left, 'other init cannot sweep our live lock', blocked)
            run(left, 'release lock owner', f"open({(target + '.stop')!r}, 'w').close(); print('LIFETIME_OK')")
            assert owner.wait() == 0
            run(left, 'record lock released on owner exit', acquired)
            run(left, 'remove lock fixtures', f"import os; os.unlink({target!r}); os.unlink({(target + '.stop')!r}); print('LIFETIME_OK')")

        if args.case == 'all':
            semaphore_name = '/kinakaze-lifetime-' + uuid.uuid4().hex
            named = f'''
import ctypes as c, errno, os
libc = c.CDLL('libc.so.6', use_errno=True)
libc.sem_open.argtypes = [c.c_char_p, c.c_int, c.c_uint, c.c_uint]
libc.sem_open.restype = c.c_void_p
libc.sem_close.argtypes = [c.c_void_p]
libc.sem_unlink.argtypes = [c.c_char_p]
libc.sem_getvalue.argtypes = [c.c_void_p, c.POINTER(c.c_int)]
name = {semaphore_name.encode()!r}
'''
            run(left, 'POSIX semaphore creator closes and exits', named + '''
sem = libc.sem_open(name, os.O_CREAT | os.O_EXCL, 0o600, 7)
assert sem, c.get_errno()
assert libc.sem_close(sem) == 0, c.get_errno()
print('LIFETIME_OK')
''')
            run(left, 'fresh worker retains POSIX tokens and unlinks with an open mapping', named + '''
sem = libc.sem_open(name, 0, 0, 0)
assert sem, c.get_errno()
assert libc.sem_unlink(name) == 0, c.get_errno()
value = c.c_int()
assert libc.sem_getvalue(sem, c.byref(value)) == 0 and value.value == 7
assert libc.sem_close(sem) == 0, c.get_errno()
print('LIFETIME_OK')
''')
            run(left, 'removed POSIX name stays absent in the next worker', named + '''
assert not libc.sem_open(name, 0, 0, 0) and c.get_errno() == errno.ENOENT
print('LIFETIME_OK')
''')
            key = uuid.uuid4().int % 0x3fffffff + 1
            ipc = f'''
import ctypes as c
libc = c.CDLL('libc.so.6', use_errno=True)
libc.shmget.argtypes = [c.c_int, c.c_size_t, c.c_int]
libc.shmat.argtypes = [c.c_int, c.c_void_p, c.c_int]
libc.shmat.restype = c.c_void_p
libc.shmdt.argtypes = [c.c_void_p]
libc.shmctl.argtypes = [c.c_int, c.c_int, c.c_void_p]
libc.semget.argtypes = [c.c_int, c.c_int, c.c_int]
libc.semctl.argtypes = [c.c_int, c.c_int, c.c_int, c.c_size_t]
key = {key}
def check(value):
    assert value != -1, c.get_errno()
    return value
def attach(segment):
    address = libc.shmat(segment, None, 0)
    assert address not in (None, c.c_void_p(-1).value), c.get_errno()
    return address
'''
            run(left, 'named SysV objects survive their creator', ipc + '''
segment = check(libc.shmget(key, 4096, 0o3600))
address = attach(segment)
c.memmove(address, b'kept-by-init', 12)
check(libc.shmdt(address))
semaphore = check(libc.semget(key, 1, 0o3600))
check(libc.semctl(semaphore, 0, 16, 7))
print('LIFETIME_OK')
''')
            time.sleep(1.1)
            run(left, 'fresh worker reads IPC and removes it with attachments alive', ipc + '''
segment = check(libc.shmget(key, 4096, 0))
address = attach(segment)
assert c.string_at(address, 12) == b'kept-by-init'
semaphore = check(libc.semget(key, 1, 0))
assert check(libc.semctl(semaphore, 0, 12, 0)) == 7
check(libc.shmctl(segment, 0, None))
assert c.string_at(address, 12) == b'kept-by-init'
check(libc.shmdt(address))
check(libc.semctl(semaphore, 0, 0, 0))
print('LIFETIME_OK')
''')
            run(left, 'removed IPC keys can be reused immediately', ipc + '''
segment = check(libc.shmget(key, 4096, 0o3600))
address = attach(segment)
assert c.string_at(address, 12) == bytes(12)
check(libc.shmctl(segment, 0, None))
check(libc.shmdt(address))
semaphore = check(libc.semget(key, 1, 0o3600))
assert check(libc.semctl(semaphore, 0, 12, 0)) == 0
check(libc.semctl(semaphore, 0, 0, 0))
print('LIFETIME_OK')
''')
            run(left, 'PTY limit writer exits', f'''
from pathlib import Path
limit = Path('/proc/sys/kernel/pty/max')
old = int(limit.read_text())
Path({target!r}).write_text(str(old))
limit.write_text(str(old - 1))
print('LIFETIME_OK')
''')
            run(left, 'PTY limit survives worker replacement', f'''
from pathlib import Path
limit = Path('/proc/sys/kernel/pty/max')
old = int(Path({target!r}).read_text())
assert int(limit.read_text()) == old - 1
limit.write_text(str(old))
Path({target!r}).unlink()
print('LIFETIME_OK')
''')
            probes = Path(__file__).resolve().parent.parent / 'tests' / 'guest'
            for name, marker in [
                ('UnixRightsProbe.py', 'UNIX_RIGHTS_LIFETIME_OK'),
                ('UnixBlockingProbe.py', 'UnixBlockingProbe: PASS'),
            ]:
                run(left, name, (probes / name).read_text(encoding='utf-8'), marker)
    result = dict(passed=True, checks=rows)
    (args.output / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(dict(passed=True, checks=[row['name'] for row in rows]), indent=2))


if __name__ == '__main__':
    main()
