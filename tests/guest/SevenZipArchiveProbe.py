"""The installed archive manager must extract real nested and Unicode entries."""
from pathlib import Path
import subprocess
import tempfile
import zipfile

with tempfile.TemporaryDirectory(prefix='kinakaze-file-roller-') as folder:
    base = Path(folder)
    archive = base / 'desktop archive test.zip'
    output = base / 'extracted'
    output.mkdir()
    expected = {'readme.txt': b'Archive Manager desktop test\n',
                'nested/中文 文件.txt': '真实的解压内容\n'.encode(),
                'nested/binary.bin': bytes(range(256)) * 8}
    with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as writer:
        for name, data in expected.items():
            writer.writestr(name, data)
    backend = subprocess.run(['/usr/bin/7z', 'l', str(archive)], capture_output=True, text=True, errors='replace', timeout=15)
    print('ARCHIVE_BACKEND', backend.returncode, backend.stdout[:1000], backend.stderr[:1000], flush=True)
    assert backend.returncode == 0, 'installed 7z backend could not list the fixture'
    result = subprocess.run(['/usr/bin/7z', 'x', '-o'+str(output), str(archive)],capture_output=True,text=True,errors='replace',timeout=15)
    assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
    actual = {str(path.relative_to(output)): path.read_bytes() for path in output.rglob('*') if path.is_file()}
    assert actual == expected, (list(actual), result.stderr)
print('SEVEN_ZIP_UNICODE_NESTED_BINARY_EXTRACTION_OK', flush=True)
