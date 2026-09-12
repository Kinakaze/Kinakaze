"""Exercise path/fd metadata identity while names, modes and contents change."""
import json
import os
import stat
import tempfile
from pathlib import Path


def identity(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size,
            info.st_uid, info.st_gid, info.st_nlink)


with tempfile.TemporaryDirectory(prefix='metadata-stat-') as directory:
    root = Path(directory)
    source = root / '文件-🦀'
    source.write_bytes(b'original data')
    os.chmod(source, 0o640)
    with source.open('rb') as opened:
        original = os.fstat(opened.fileno())
        assert identity(os.stat(source)) == identity(original)
        assert stat.S_IMODE(original.st_mode) == 0o640
        hard = root / 'hardlink'
        os.link(source, hard)
        assert identity(os.stat(hard)) == identity(os.fstat(opened.fileno()))
        assert os.stat(hard).st_nlink == 2
        link = root / 'symlink'
        os.symlink(source.name, link)
        assert stat.S_ISLNK(os.lstat(link).st_mode)
        assert os.lstat(link).st_size == len(source.name.encode())
        assert identity(os.stat(link)) == identity(os.stat(source))
        os.chmod(hard, 0o604)
        assert stat.S_IMODE(os.fstat(opened.fileno()).st_mode) == 0o604
        moved = root / 'renamed'
        os.rename(source, moved)
        source.write_bytes(b'replacement')
        assert os.stat(source).st_ino != original.st_ino
        assert os.stat(link).st_ino == os.stat(source).st_ino
        assert os.fstat(opened.fileno()).st_ino == original.st_ino
        assert identity(os.stat(moved)) == identity(os.fstat(opened.fileno()))
        os.unlink(moved)
        os.unlink(hard)
        assert opened.read() == b'original data'
        assert os.fstat(opened.fileno()).st_nlink == 0
        assert os.fstat(opened.fileno()).st_size == 13
        link.unlink()
        assert stat.S_ISDIR(os.stat(root).st_mode)

print(json.dumps({'metadata_identity': 'PASS', 'cases': [
    'unicode', 'mode_change', 'hardlink', 'symlink_follow', 'lstat',
    'rename', 'replacement', 'unlinked_open_file', 'directory']}), flush=True)
