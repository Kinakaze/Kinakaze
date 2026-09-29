"""An unlinked directory stays usable through a held fd and cwd reference."""
import errno
import os
from pathlib import Path
import tempfile


with tempfile.TemporaryDirectory(prefix='directory-removal-') as temporary:
    root = Path(temporary)
    child = root / 'child'
    child.mkdir()
    original_cwd = os.open('.', os.O_RDONLY | os.O_DIRECTORY)
    held = os.open(child, os.O_RDONLY | os.O_DIRECTORY)
    try:
        inode = os.fstat(held).st_ino
        os.chdir(child)
        os.rmdir(child)
        assert not child.exists()
        assert os.stat('.').st_ino == inode
        assert os.fstat(held).st_ino == inode
        child.mkdir()
        assert child.stat().st_ino != inode
        (child / 'content').write_text('keep')
        try:
            os.rmdir(child)
        except OSError as error:
            assert error.errno == errno.ENOTEMPTY, error
        else:
            raise AssertionError('rmdir accepted a nonempty directory')
        assert (child / 'content').read_text() == 'keep'
    finally:
        os.fchdir(original_cwd)
        os.close(original_cwd)
        os.close(held)
print('DIRECTORY_REMOVAL_OK')
