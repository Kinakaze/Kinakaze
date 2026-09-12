"""Directory type/name correctness and real GTK icon-directory enumeration cost."""
import json
import os
import statistics
import tempfile
import time
from pathlib import Path

with tempfile.TemporaryDirectory(prefix='kinakaze-dir-types-') as directory:
    root = Path(directory)
    (root / 'normal').write_bytes(b'content')
    (root / 'empty').touch()
    (root / 'subdir').mkdir()
    (root / 'two words.').touch()
    (root / '中文:文件').write_text('name')
    (root / 'file-link').symlink_to('normal')
    (root / 'dir-link').symlink_to('subdir', target_is_directory=True)
    (root / 'dangling').symlink_to('missing')
    entries = {entry.name: entry for entry in os.scandir(directory)}
    assert set(entries) == {'normal', 'empty', 'subdir', 'two words.', '中文:文件', 'file-link', 'dir-link', 'dangling'}
    for name in ('normal', 'empty', 'two words.', '中文:文件'):
        assert entries[name].is_file(follow_symlinks=False) and not entries[name].is_symlink(), name
    assert entries['subdir'].is_dir(follow_symlinks=False)
    for name in ('file-link', 'dir-link', 'dangling'):
        assert entries[name].is_symlink() and not entries[name].is_dir(follow_symlinks=False), name

results = []
for directory in ('/usr/share/icons/Adwaita/16x16/status', '/usr/share/icons/Adwaita/scalable/actions'):
    samples = []
    expected = None
    for _ in range(5):
        start = time.perf_counter()
        names = os.listdir(directory)
        samples.append((time.perf_counter() - start) * 1000)
        if expected is not None:
            assert set(names) == expected
        expected = set(names)
    results.append({'path': directory, 'count': len(expected), 'median_ms': round(statistics.median(samples), 3)})
print(json.dumps(results), flush=True)
print('DIRECTORY_TYPES_NAMES_AND_ICON_SCAN_OK', flush=True)
