"""Authenticated rsync TCP upload/download with compression and file metadata."""
import hashlib
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

with tempfile.TemporaryDirectory(prefix='kinakaze-rsync-daemon-') as directory:
    prefix = Path(directory)
    source, storage, downloaded = (prefix / name for name in ('source', 'storage', 'downloaded'))
    source.mkdir()
    storage.mkdir()
    payload = bytes(range(256)) * 8192
    (source / 'payload').write_bytes(payload)
    (source / 'name with spaces').write_text('round trip\n')
    os.link(source / 'payload', source / 'hardlink')
    os.symlink('payload', source / 'symlink')
    secret = prefix / 'secrets'
    secret.write_text('acceptance:temporary-password\n')
    secret.chmod(0o600)
    password = prefix / 'password'
    password.write_text('temporary-password\n')
    password.chmod(0o600)
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    config = prefix / 'rsyncd.conf'
    config.write_text(f'''address = 127.0.0.1
port = {port}
pid file = {prefix}/rsync.pid
lock file = {prefix}/rsync.lock
log file = {prefix}/rsync.log
use chroot = no
[files]
path = {storage}
read only = no
uid = 0
gid = 0
auth users = acceptance
secrets file = {secret}
''')
    url = f'rsync://acceptance@127.0.0.1:{port}/files/'
    def run(*arguments, expected=0):
        result = subprocess.run(['/usr/bin/rsync', '--timeout=15', '--password-file=' + str(password), *arguments],
                                capture_output=True, timeout=25)
        assert result.returncode == expected, (arguments, result.returncode, result.stdout[-4096:], result.stderr[-4096:])
        return result
    with (prefix / 'console.log').open('wb') as console:
        server = subprocess.Popen(['/usr/bin/rsync', '--daemon', '--no-detach', '--config=' + str(config)],
                                  stdout=console, stderr=console)
        try:
            deadline = time.monotonic() + 10
            while True:
                assert server.poll() is None, (prefix / 'console.log').read_text(errors='replace')
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=0.2): break
                except OSError:
                    assert time.monotonic() < deadline
                    time.sleep(0.05)
            run('-azH', '--compress-choice=zstd', '--preallocate', str(source) + '/', url)
            assert hashlib.sha256((storage / 'payload').read_bytes()).digest() == hashlib.sha256(payload).digest()
            assert os.stat(storage / 'payload').st_ino == os.stat(storage / 'hardlink').st_ino
            # A non-chroot rsync daemon protects stored links and unmunges
            # them when sending them back to a client.
            assert os.readlink(storage / 'symlink') == '/rsyncd-munged/payload'
            (source / 'name with spaces').unlink()
            (source / 'payload').write_bytes(b'changed' + payload[7:])
            run('-azcH', '--delete', str(source) + '/', url)
            assert not (storage / 'name with spaces').exists()
            run('-azH', url, str(downloaded) + '/')
            assert (downloaded / 'payload').read_bytes() == (source / 'payload').read_bytes()
            assert os.readlink(downloaded / 'symlink') == 'payload'
            password.write_text('incorrect-password\n')
            denied = run('--list-only', url, expected=5)
            assert b'auth failed' in denied.stderr
            print('RSYNC_TCP_AUTH_COMPRESSION_OK', flush=True)
        finally:
            if server.poll() is None:
                server.terminate()
            server.wait(timeout=10)
            for name in ('console.log', 'rsync.log'):
                path = prefix / name
                if path.exists(): print(name + ':\n' + path.read_text(errors='replace')[-8192:], flush=True)
