"""Independent timing of the filesystem operations used during unpacking."""
import json
import os
from pathlib import Path
import sys
import time

root = Path(sys.argv[1])
root.mkdir()
paths = [root / str(i) for i in range(128)]
results = {}


def measure(name, action):
    start = time.monotonic_ns()
    action()
    results[name] = (time.monotonic_ns() - start) / 1e6


def create():
    for path in paths:
        with path.open('wb') as stream:
            stream.write(b'x' * 4096)


def descriptors(action):
    handles = [os.open(path, os.O_RDWR) for path in paths]
    try:
        measure(action.__name__, lambda: [action(fd) for fd in handles])
    finally:
        for fd in handles:
            os.close(fd)


def fchmod(fd):
    os.fchmod(fd, 0o640)


def fchown(fd):
    os.fchown(fd, 0, 0)


measure('create_write_close', create)
measure('stat', lambda: [os.stat(path) for path in paths])
measure('chmod', lambda: [os.chmod(path, 0o600) for path in paths])
measure('chown', lambda: [os.chown(path, 0, 0) for path in paths])
measure('utime', lambda: [os.utime(path, (1700000000, 1700000000)) for path in paths])
descriptors(fchmod)
descriptors(fchown)
descriptors(os.fstat)
descriptors(os.fsync)
measure('unlink', lambda: [path.unlink() for path in paths])
root.rmdir()
(root.parent / 'io-results.json').write_text(json.dumps(results))
print(json.dumps(results), flush=True)
