"""Exercise the real nginx master/workers using only an isolated test prefix."""
import concurrent.futures
import hashlib
import http.client
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time

with tempfile.TemporaryDirectory(prefix='kinakaze-nginx-') as directory:
    prefix = Path(directory)
    prefix.chmod(0o755)
    payload = bytes(range(256)) * 8192
    (prefix / 'payload.bin').write_bytes(payload)
    (prefix / 'payload.bin').chmod(0o644)
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    configuration = prefix / 'nginx.conf'
    template = '''user sshd;
worker_processes 2;
pid {prefix}/nginx.pid;
error_log {prefix}/error.log info;
events {{ worker_connections 64; use epoll; }}
http {{
    access_log off;
    client_body_temp_path {prefix}/body;
    proxy_temp_path {prefix}/proxy;
    fastcgi_temp_path {prefix}/fastcgi;
    uwsgi_temp_path {prefix}/uwsgi;
    scgi_temp_path {prefix}/scgi;
    sendfile on;
    server {{
        listen 127.0.0.1:{port};
        root {prefix};
        location = /probe {{ return 200 "{marker}"; }}
    }}
}}
'''
    def configure(marker):
        configuration.write_text(template.format(prefix=prefix, port=port, marker=marker))

    def request(path='/probe', method='GET', headers=None):
        connection = http.client.HTTPConnection('127.0.0.1', port, timeout=8)
        try:
            connection.request(method, path, headers=headers or {})
            response = connection.getresponse()
            return response.status, dict(response.getheaders()), response.read()
        finally:
            connection.close()

    configure('NGINX_FIRST')
    with (prefix / 'console.log').open('wb') as output:
        process = subprocess.Popen(['/usr/sbin/nginx', '-p', str(prefix) + '/',
                                    '-c', str(configuration), '-g', 'daemon off;'],
                                   stdout=output, stderr=output)
        try:
            deadline = time.monotonic() + 12
            while True:
                assert process.poll() is None, 'nginx exited during startup'
                try:
                    status, _, body = request()
                    if status == 200 and body == b'NGINX_FIRST':
                        break
                except OSError:
                    pass
                assert time.monotonic() < deadline, 'nginx did not become ready'
                time.sleep(0.05)
            status, headers, body = request('/payload.bin')
            assert status == 200 and hashlib.sha256(body).digest() == hashlib.sha256(payload).digest()
            status, headers, body = request('/payload.bin', 'HEAD')
            assert status == 200 and not body and int(headers['Content-Length']) == len(payload)
            status, headers, body = request('/payload.bin', headers={'Range': 'bytes=123-150'})
            assert status == 206 and body == payload[123:151]
            assert request('/missing')[0] == 404
            with concurrent.futures.ThreadPoolExecutor(max_workers=8) as clients:
                assert all(result[0] == 200 and result[2] == b'NGINX_FIRST'
                           for result in clients.map(lambda _: request(), range(24)))
            configure('NGINX_RELOADED')
            process.send_signal(signal.SIGHUP)
            deadline = time.monotonic() + 12
            while request()[2] != b'NGINX_RELOADED':
                assert process.poll() is None and time.monotonic() < deadline
                time.sleep(0.05)
            process.send_signal(signal.SIGQUIT)
            assert process.wait(timeout=12) == 0
            assert not (prefix / 'nginx.pid').exists()
            errors = (prefix / 'error.log').read_text()
            assert not any(level in errors for level in ('[alert]', '[crit]', '[emerg]')), errors
            print('NGINX_HTTP_FILES_RELOAD_OK', flush=True)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            for name in ('console.log', 'error.log'):
                path = prefix / name
                if path.exists():
                    print(name + ':\n' + path.read_text(errors='replace')[-8192:], flush=True)
