"""Each unused bootstrap must receive the current fork's state exactly once."""
import mmap
import os
from pathlib import Path
import signal
import tempfile
import time
import traceback

original_directory = os.getcwd()
with tempfile.TemporaryDirectory(prefix='fork-prewarm-') as directory:
    shared = mmap.mmap(-1, 4096, flags=mmap.MAP_SHARED)
    private = mmap.mmap(-1, 4096, flags=mmap.MAP_PRIVATE)
    os.chdir(directory)
    for iteration in range(24):
        message = f'current-fork-{iteration:03d}'.encode()
        path = Path(f'file-{iteration}')
        path.write_bytes(b'!' + message)
        descriptor = os.open(path, os.O_RDONLY)
        alias = os.dup(descriptor)
        os.lseek(descriptor, 1, os.SEEK_SET)
        read_end, write_end = os.pipe()
        private[:len(message)] = message
        shared[:4] = b'wait'
        os.environ['FORK_PREWARM_CURRENT'] = str(iteration)
        child = os.fork()
        if child == 0:
            try:
                assert os.environ['FORK_PREWARM_CURRENT'] == str(iteration)
                assert os.getcwd() == directory
                assert private[:len(message)] == message
                assert os.read(alias, len(message)) == message
                assert os.lseek(descriptor, 0, os.SEEK_CUR) == 1 + len(message)
                private[:4] = b'kid!'
                shared[:4] = b'done'
                os.write(write_end, message)
                if iteration % 6 == 0:
                    nested = os.fork()
                    if nested == 0:
                        assert private[:4] == b'kid!'
                        os.execl('/bin/sh', 'sh', '-c', 'test "$FORK_PREWARM_CURRENT" = "$1"', 'sh', str(iteration))
                    assert os.waitpid(nested, 0) == (nested, 0)
                os._exit(0)
            except BaseException:
                traceback.print_exc()
                os._exit(1)
        os.close(write_end)
        assert os.waitpid(child, 0) == (child, 0)
        assert os.read(read_end, 64) == message
        assert os.read(read_end, 1) == b''
        assert private[:len(message)] == message
        assert shared[:4] == b'done'
        assert os.lseek(descriptor, 0, os.SEEK_CUR) == len(message) + 1
        for descriptor in [descriptor, alias, read_end]:
            os.close(descriptor)
        path.unlink()
        if iteration < 2:
            time.sleep(0.1)  # Allow initial asynchronous preparation in this probe.
    shared.close()
    private.close()
    os.chdir(original_directory)
print('FORK_PREWARM_POOL_OK', flush=True)
