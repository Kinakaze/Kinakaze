"""Agent cleanup through proc directory aliases retains Linux unlink semantics."""
import errno
import os
from pathlib import Path
import tempfile


for base in ('/tmp', '/dev/shm'):
    print('PROC_FD_UNLINK_BACKEND', base, flush=True)
    with tempfile.TemporaryDirectory(prefix='proc-fd-unlink-', dir=base) as temporary:
        root = Path(temporary)
        original = root / 'original'
        original.mkdir()
        descriptor = os.open(original, os.O_PATH | os.O_DIRECTORY)
        try:
            moved = root / 'moved'
            original.rename(moved)
            original.mkdir()
            alias = Path(f'/proc/self/fd/{descriptor}')
            (moved / 'target').write_text('preserved')
            (moved / 'link').symlink_to('target')
            os.unlink(alias / 'link')
            assert (moved / 'target').read_text() == 'preserved'
            assert not (moved / 'link').is_symlink()
            (original / 'target').write_text('decoy')
            opened = os.open(moved / 'target', os.O_RDONLY)
            try:
                os.unlink(alias / 'target')
                assert not (moved / 'target').exists()
                assert os.read(opened, 100) == b'preserved'
            finally:
                os.close(opened)
            assert (original / 'target').read_text() == 'decoy'
            (moved / 'empty').mkdir()
            os.rmdir(alias / 'empty')
            assert not (moved / 'empty').exists()
        finally:
            os.close(descriptor)
        try:
            os.unlink(alias / 'missing')
        except OSError as error:
            assert error.errno == errno.ENOENT, error
        else:
            raise AssertionError('closed alias accepted')

print('PROC_FD_UNLINK_RENAME_OPEN_SYMLINK_OK', flush=True)
