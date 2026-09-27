"""Exercise release EXEs against a local mirror/proxy with an empty user PATH."""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import re
import subprocess
import tarfile
from threading import Event, Thread
import time


def validate_startup_progress(dist, temporary, env, results):
    """Hold a download open until the foreground client reports byte progress."""
    payload = b'progress fixture\n' * 32768
    reported, release = Event(), Event()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Length', str(len(payload)))
            self.end_headers()
            self.wfile.write(payload[:65536])
            self.wfile.flush()
            time.sleep(1.1)
            self.wfile.write(payload[65536:131072])
            self.wfile.flush()
            release.wait(15)
            self.wfile.write(payload[131072:])

        def log_message(self, *_args):
            pass

    directory = temporary / 'startup progress'
    directory.mkdir()
    root = directory / 'root'
    with ThreadingHTTPServer(('127.0.0.1', 0), Handler) as server:
        thread = Thread(target=lambda: server.serve_forever(poll_interval=.01), daemon=True)
        thread.start()
        manifest = directory / 'manifest.json'
        manifest.write_text(json.dumps({
            'schema': 1, 'directories': ['etc'],
            'download': {'proxy': 'direct'},
            'startup': {'command': ['/unused'], 'tray': False,
                        'terminals': [{'name': 'probe', 'command': ['/unused']}],
                        'default_terminal': 'probe'},
            # A deliberate hash failure finishes before any guest is booted.
            'archives': {'probe': {'url': f'http://127.0.0.1:{server.server_port}/probe.deb',
                                    'size': len(payload), 'sha256': '0' * 64}},
            'files': [{'path': 'etc/probe', 'archive': 'probe', 'member': 'etc/probe',
                       'sha256': '0' * 64}],
        }), encoding='utf-8')
        try:
            for executable in ('worker.exe', 'init.exe'):
                reported.clear()
                release.clear()
                lines = []
                process = subprocess.Popen([
                    str(dist / executable), 'session', 'start', '--root', str(root),
                    '--dist', str(dist), '--rootfs-manifest', str(manifest), '--no-tray',
                ], cwd=directory, env=env, stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                   creationflags=subprocess.CREATE_NO_WINDOW)

                def collect():
                    for line in process.stderr:
                        text = line.decode('utf-8', errors='replace')
                        lines.append(text)
                        match = re.search(r'Downloading probe: (\d+) / (\d+) bytes', text)
                        if match and 0 < int(match[1]) < int(match[2]):
                            reported.set()

                reader = Thread(target=collect, daemon=True)
                reader.start()
                try:
                    visible_before_completion = reported.wait(12) and process.poll() is None
                finally:
                    release.set()
                    process.wait(timeout=45)
                    reader.join(timeout=5)
                    process.stderr.close()
                output = ''.join(lines)
                assert visible_before_completion, (executable, output)
                assert process.returncode != 0 and 'hash/size mismatch' in output, output
                assert 'Startup log:' in output and 'init failed' in output, output
                assert 'OLD_RUN_MUST_NOT_REPLAY' not in output, output
                assert not any(root.iterdir()), root
                # The next entry must forward its current run, not stale errors.
                logs = list(directory.glob('.kinakaze-session-*/init.log'))
                assert len(logs) == 1, logs
                with logs[0].open('a', encoding='utf-8') as log:
                    log.write('OLD_RUN_MUST_NOT_REPLAY\n')
                results.append(f'{executable}: live download progress before completion, inline errors, no stale log replay')
        finally:
            release.set()
            server.shutdown()
            thread.join(timeout=5)


def validate_download_settings(dist, temporary, env, results):
    content = b'native mirror and proxy download\n'
    payload = io.BytesIO()
    with tarfile.open(fileobj=payload, mode='w:xz') as archive:
        info = tarfile.TarInfo('etc/probe')
        info.size = len(content)
        info.mode = 0o644
        archive.addfile(info, io.BytesIO(content))
    data = payload.getvalue()
    header = f'{"data.tar.xz":<16}{0:<12}{0:<6}{0:<6}{"100644":<8}{len(data):<10}`\n'.encode()
    deb = b'!<arch>\n' + header + data + (b'\n' if len(data) % 2 else b'')
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_CONNECT(self):
            # Observe HTTPS proxy routing without TLS interception or external
            # networking. The proxy deliberately rejects the tunnel; the native
            # installer must report failure and leave no partially installed root.
            requests.append(self.path)
            self.send_error(502, 'Test proxy deliberately rejects the tunnel')

        def do_GET(self):
            requests.append(self.path)
            self.send_response(200)
            self.send_header('Content-Length', str(len(deb)))
            self.end_headers()
            self.wfile.write(deb)

        def log_message(self, *_args):
            pass

    with ThreadingHTTPServer(('127.0.0.1', 0), Handler) as server:
        thread = Thread(target=lambda: server.serve_forever(poll_interval=0.01), daemon=True)
        thread.start()
        proxy = f'http://127.0.0.1:{server.server_port}'
        mirror = proxy + '/mirror'
        unreachable = 'http://127.0.0.1:1'
        cases = [
            ('manifest-direct', mirror, 'direct', {}, '/mirror/pool/probe.deb'),
            ('environment-direct', unreachable, unreachable,
             {'KINAKAZE_DEBIAN_MIRROR': mirror, 'KINAKAZE_DOWNLOAD_PROXY': 'direct'},
             '/mirror/pool/probe.deb'),
            ('manifest-proxy', 'https://kinakaze-mirror.invalid', proxy, {},
             'kinakaze-mirror.invalid:443'),
            ('environment-proxy', mirror, 'direct',
             {'KINAKAZE_DEBIAN_MIRROR': 'https://kinakaze-mirror.invalid', 'KINAKAZE_DOWNLOAD_PROXY': proxy},
             'kinakaze-mirror.invalid:443'),
        ]
        try:
            for name, source, configured_proxy, overrides, expected in cases:
                directory = temporary / name
                directory.mkdir()
                manifest = directory / 'rootfs.manifest.json'
                manifest.write_text(json.dumps({
                    'schema': 1,
                    'directories': ['etc'],
                    'download': {'debian_mirror': source, 'proxy': configured_proxy},
                    'archives': {'probe': {
                        'url': 'https://deb.debian.org/debian/pool/probe.deb',
                        'size': len(deb), 'sha256': hashlib.sha256(deb).hexdigest(),
                    }},
                    'files': [{'path': 'etc/probe', 'archive': 'probe', 'member': 'etc/probe',
                               'sha256': hashlib.sha256(content).hexdigest()}],
                }), encoding='utf-8')
                child_env = dict(env, PATH='', KINAKAZE_ROOTFS_CACHE=str(directory / 'cache'),
                                 HTTP_PROXY=unreachable, HTTPS_PROXY=unreachable, ALL_PROXY=unreachable)
                child_env.update(overrides)
                result = subprocess.run([
                    str(dist / 'worker.exe'), 'setup', '--dist', str(dist),
                    '--root', str(directory / 'root'), '--rootfs-manifest', str(manifest),
                ], cwd=directory, env=child_env, capture_output=True, timeout=45,
                   creationflags=subprocess.CREATE_NO_WINDOW)
                if name.endswith('-direct'):
                    assert result.returncode == 0, (name, result.stderr.decode(errors='replace'))
                    assert (directory / 'root/etc/probe').read_bytes() == content, name
                    assert requests == [expected], (name, requests)
                    evidence = 'verified local payload'
                else:
                    assert result.returncode != 0, name
                    assert requests == [expected] * 3, (name, requests, result.stderr.decode(errors='replace'))
                    assert not (directory / 'root').exists(), name
                    evidence = 'HTTPS CONNECT routes to selected proxy; rejected tunnel leaves no root'
                requests.clear()
                results.append(f'native download settings: {name}, empty PATH, {evidence}')
        finally:
            server.shutdown()
            thread.join(timeout=5)
