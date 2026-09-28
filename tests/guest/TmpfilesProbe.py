"""Exercise real systemd-tmpfiles ownership and glob expansion on owned files."""
import os
from pathlib import Path
import subprocess
import tempfile

with tempfile.TemporaryDirectory(prefix='tmpfiles-probe-') as temporary:
    root = Path(temporary)
    directory = root / 'data'
    config = root / 'fixture.conf'
    config.write_text(f'd {directory} 0750 1234 1235 -\n'
                      f'f {directory}/first 0600 1234 1235 - hello\n'
                      f'f {directory}/second 0600 1234 1235 - world\n')
    def run():
        result = subprocess.run(['systemd-tmpfiles', '--create', str(config)],
                                capture_output=True, text=True, timeout=20)
        assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
        assert not result.stderr.strip(), result.stderr
    run()
    for path in (directory, directory / 'first', directory / 'second'):
        stat = path.stat()
        assert (stat.st_uid, stat.st_gid) == (1234, 1235), (path, stat)
    assert (directory / 'first').read_text() == 'hello'
    # z entries use systemd's safe_glob callbacks even without a wildcard.
    config.write_text(f'z {directory} 0755 0 0 -\n'
                      f'z {directory}/{{first,second}} 0644 0 0 -\n')
    run()
    for path in (directory, directory / 'first', directory / 'second'):
        stat = path.stat()
        assert (stat.st_uid, stat.st_gid) == (0, 0), (path, stat)
        assert stat.st_mode & 0o777 == (0o755 if path == directory else 0o644)
    run()  # Repeated application is idempotent.
print('TMPFILES_OWNERSHIP_GLOB_OK', flush=True)
