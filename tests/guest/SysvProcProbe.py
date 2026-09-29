"""Cross-process SysV shared-memory discovery, deferred removal and namespaces."""
import ctypes
import os
from pathlib import Path
import subprocess
import sys


lib = ctypes.CDLL(None, use_errno=True)
lib.shmget.argtypes = [ctypes.c_int, ctypes.c_size_t, ctypes.c_int]
lib.shmctl.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
lib.shmat.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
lib.shmat.restype = ctypes.c_void_p
lib.shmdt.argtypes = [ctypes.c_void_p]


def rows():
    text = Path('/proc/sysvipc/shm').read_text().splitlines()
    assert text and text[0].split()[:4] == ['key', 'shmid', 'perms', 'size']
    return {int(line.split()[1]): line.split() for line in text[1:]}


def create(key=0):
    identifier = lib.shmget(key, 4096, 0o3600 if key else 0o600)
    assert identifier >= 0, ctypes.get_errno()
    return identifier


if len(sys.argv) > 1:
    mode, identifier = sys.argv[1], int(sys.argv[2])
    if mode == 'inspect':
        assert identifier in rows()
        row = rows()[identifier]
        assert int(row[3]) == 4096 and int(row[2], 8) & 0o777 == 0o600
        output = subprocess.check_output(['ipcs', '-m', '-i', str(identifier)], text=True)
        assert 'bytes=4096' in output, output
    elif mode == 'isolate':
        assert lib.unshare(0x08000000) == 0, ctypes.get_errno()
        assert identifier not in rows()
        own = create()
        try:
            assert own in rows()
        finally:
            assert lib.shmctl(own, 0, None) == 0
    raise SystemExit(0)

assert 'sysvipc' in os.listdir('/proc')
assert 'shm' in os.listdir('/proc/sysvipc')
private = create()
key = 0x23000000 | (int.from_bytes(os.urandom(3), 'little') & 0xffffff)
keyed = create(key)
attachment = None
try:
    for identifier in (private, keyed):
        subprocess.run([sys.executable, __file__, 'inspect', str(identifier)], check=True, timeout=15)
    assert int(rows()[keyed][0]) == key
    subprocess.run([sys.executable, __file__, 'isolate', str(private)], check=True, timeout=15)
    assert private in rows() and keyed in rows()
    attachment = lib.shmat(private, None, 0)
    assert attachment not in (None, ctypes.c_void_p(-1).value), ctypes.get_errno()
    assert int(rows()[private][6]) == 1
    assert lib.shmctl(private, 0, None) == 0
    row = rows()[private]
    assert int(row[2], 8) & 0o1000 and int(row[6]) == 1
    assert lib.shmdt(attachment) == 0
    attachment = None
    assert private not in rows()
    attachment = lib.shmat(keyed, None, 0)
    assert attachment not in (None, ctypes.c_void_p(-1).value), ctypes.get_errno()
    assert lib.shmctl(keyed, 0, None) == 0
    assert int(rows()[keyed][0]) == 0
    recreated = create(key)
    try:
        assert recreated != keyed and recreated in rows()
        assert int(rows()[recreated][0]) == key
        assert lib.shmdt(attachment) == 0
        attachment = None
        assert keyed not in rows()
    finally:
        assert lib.shmctl(recreated, 0, None) == 0
finally:
    if attachment is not None:
        lib.shmdt(attachment)
    lib.shmctl(private, 0, None)
    lib.shmctl(keyed, 0, None)

print('SYSV_PROC_LIFETIME_NAMESPACE_OK', flush=True)
