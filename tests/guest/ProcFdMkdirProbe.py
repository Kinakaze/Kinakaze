"""Directory creation through proc fd aliases follows the live directory."""
import errno
import os
from pathlib import Path
import tempfile


for base in ('/tmp', '/dev/shm'):
    print('PROC_FD_BACKEND', base, flush=True)
    with tempfile.TemporaryDirectory(prefix='proc-fd-mkdir-', dir=base) as temporary:
        root = Path(temporary)
        original = root / 'original'
        original.mkdir()
        descriptor = os.open(original, os.O_PATH | os.O_DIRECTORY)
        try:
            moved = root / 'moved'
            original.rename(moved)
            original.mkdir()
            alias = Path(f'/proc/self/fd/{descriptor}')
            os.mkdir(alias / '-agent', 0o700)
            assert (moved / '-agent').is_dir(), (base, os.readlink(alias), list(original.iterdir()), list(moved.iterdir()))
            assert not (original / '-agent').exists()
            assert (moved / '-agent').stat().st_mode & 0o777 == 0o700
            (alias / '-agent' / 'nested').mkdir()
            assert (moved / '-agent' / 'nested').is_dir()
            filename = alias / '-agent' / 'nested' / 'output'
            with open(filename, 'x') as stream:
                stream.write('agent-output')
            with open(filename) as stream:
                assert stream.read() == 'agent-output'
            assert filename.stat().st_size == len('agent-output')
            assert (moved / '-agent' / 'nested' / 'output').read_text() == 'agent-output'
            if base == '/tmp':
                os.rename(filename, alias / '-agent' / 'nested' / 'renamed')
                assert (moved / '-agent' / 'nested' / 'renamed').read_text() == 'agent-output'
                os.rename(alias / '-agent' / 'nested' / 'renamed', filename)
            (moved / '-agent' / 'link').symlink_to('nested/output')
            try:
                os.open(alias / '-agent' / 'link', os.O_RDONLY | os.O_NOFOLLOW)
            except OSError as error:
                assert error.errno == errno.ELOOP, error
            else:
                raise AssertionError('final symlink followed despite O_NOFOLLOW')
            try:
                os.mkdir(alias / '-agent')
            except OSError as error:
                assert error.errno == errno.EEXIST, error
            else:
                raise AssertionError('existing directory accepted')
        finally:
            os.close(descriptor)
        for suffix in ('closed', 'tmp/closed'):
            try:
                os.mkdir(f'/proc/self/fd/{descriptor}/{suffix}')
            except OSError as error:
                assert error.errno == errno.ENOENT, error
            else:
                raise AssertionError('closed directory descriptor accepted')

print('PROC_FD_MKDIR_LIVE_DIRECTORY_OK', flush=True)
