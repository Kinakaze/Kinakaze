"""Real TLS -> nginx -> Unix-domain HTTP upstream, including buffered file I/O."""
import gzip
import hashlib
import http.client
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import tempfile
import time

with tempfile.TemporaryDirectory(prefix='kinakaze-nginx-proxy-') as directory:
    prefix = Path(directory)
    prefix.chmod(0o755)
    payload = bytes(range(256)) * 8192
    (prefix / 'payload.bin').write_bytes(payload)
    (prefix / 'payload.bin').chmod(0o644)
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    certificate, key = prefix / 'server.crt', prefix / 'server.key'
    with (prefix / 'certificate.log').open('wb') as output:
        try:
            result = subprocess.run([
                '/usr/bin/openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes',
                '-days', '1', '-subj', '/CN=localhost', '-addext', 'subjectAltName=IP:127.0.0.1,DNS:localhost',
                '-keyout', str(key), '-out', str(certificate),
            ], stdout=output, stderr=output, timeout=20)
            assert result.returncode == 0, result.returncode
        finally:
            print('certificate.log:\n' + (prefix / 'certificate.log').read_text(errors='replace')[-8192:], flush=True)
    configuration = prefix / 'nginx.conf'
    configuration.write_text(f'''user sshd;
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
    upstream local_backend {{ server unix:{prefix}/upstream.sock; keepalive 4; }}
    server {{
        listen unix:{prefix}/upstream.sock;
        root {prefix};
        default_type application/octet-stream;
        location = /headers {{ return 200 "$request_method|$http_x_probe|$args"; }}
    }}
    server {{
        listen 127.0.0.1:{port} ssl;
        ssl_certificate {certificate};
        ssl_certificate_key {key};
        gzip on;
        gzip_types application/octet-stream;
        gzip_min_length 256;
        location /proxy/ {{
            proxy_pass http://local_backend/;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_set_header X-Probe native-unix;
            proxy_set_header Accept-Encoding "";
            proxy_buffering on;
            proxy_buffers 4 4k;
        }}
    }}
}}
''')
    context = ssl.create_default_context(cafile=str(certificate))
    connection = None
    with (prefix / 'console.log').open('wb') as output:
        process = subprocess.Popen(['/usr/sbin/nginx', '-p', str(prefix) + '/', '-c', str(configuration),
                                    '-g', 'daemon off;'], stdout=output, stderr=output)
        try:
            deadline = time.monotonic() + 15
            while True:
                assert process.poll() is None, 'nginx exited during TLS startup'
                connection = http.client.HTTPSConnection('127.0.0.1', port, context=context, timeout=12)
                try:
                    connection.request('GET', '/proxy/headers?startup=1')
                    response = connection.getresponse()
                    assert response.status == 200, (response.status, response.read())
                    assert response.read() == b'GET|native-unix|startup=1'
                    break
                except (ConnectionRefusedError, ConnectionResetError):
                    connection.close()
                assert time.monotonic() < deadline, 'TLS proxy did not become ready'
                time.sleep(0.05)
            assert connection.sock.version() in ('TLSv1.2', 'TLSv1.3')
            tls_socket = connection.sock
            for index in range(8):
                connection.request('POST', '/proxy/headers?v=' + str(index), body=b'request-body' * 8192)
                response = connection.getresponse()
                assert response.status == 200 and response.read() == f'POST|native-unix|v={index}'.encode()
                assert connection.sock is tls_socket, 'HTTP client connection was not reused'
            connection.request('GET', '/proxy/payload.bin')
            response = connection.getresponse()
            assert response.status == 200 and hashlib.sha256(response.read()).digest() == hashlib.sha256(payload).digest()
            connection.request('GET', '/proxy/payload.bin', headers={'Range': 'bytes=117-173'})
            response = connection.getresponse()
            assert response.status == 206 and response.read() == payload[117:174]
            connection.request('GET', '/proxy/payload.bin', headers={'Accept-Encoding': 'gzip'})
            response = connection.getresponse()
            assert response.status == 200 and response.getheader('Content-Encoding') == 'gzip', (response.status, response.getheaders())
            encoded = response.read()
            assert len(encoded) < len(payload) and gzip.decompress(encoded) == payload
            connection.close()
            process.send_signal(signal.SIGQUIT)
            assert process.wait(timeout=15) == 0
            assert not (prefix / 'nginx.pid').exists()
            assert not (prefix / 'upstream.sock').exists()
            errors = (prefix / 'error.log').read_text()
            assert not any(level in errors for level in ('[alert]', '[crit]', '[emerg]')), errors
            print('NGINX_TLS_UNIX_PROXY_GZIP_KEEPALIVE_OK', flush=True)
        finally:
            if connection is not None:
                connection.close()
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            for name in ('console.log', 'error.log'):
                path = prefix / name
                if path.exists():
                    print(name + ':\n' + path.read_text(errors='replace')[-8192:], flush=True)
