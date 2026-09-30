"""Spawning commands must survive unrelated concurrent descriptor churn."""
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time


with tempfile.TemporaryDirectory(prefix='agent-fd-churn-') as temporary:
    directory = Path(temporary) / '目录 space'
    directory.mkdir()
    stopped = threading.Event()
    errors = []

    def churn():
        try:
            while not stopped.is_set():
                fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
                duplicate = os.dup(fd)
                os.close(fd)
                os.close(duplicate)
                time.sleep(.001)
        except BaseException as error:
            errors.append(repr(error))

    threads = [threading.Thread(target=churn) for _ in range(3)]
    for thread in threads:
        thread.start()
    try:
        for index in range(32):
            result = subprocess.run(['/bin/echo', 'SPAWN_OK'], cwd=directory,
                                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, timeout=10)
            assert result.returncode == 0 and result.stdout == b'SPAWN_OK\n', (index, result)
            assert not result.stderr, (index, result.stderr)
    finally:
        stopped.set()
        for thread in threads:
            thread.join(timeout=5)
    assert not errors and all(not thread.is_alive() for thread in threads), errors

print('AGENT_SPAWN_CONCURRENT_FD_CHURN_OK', flush=True)
