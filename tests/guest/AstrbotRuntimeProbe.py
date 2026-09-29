"""Run an installed AstrBot checkout: real API, authentication and restart.

No external chat account or model credentials are used. This tests the backend
and its SQLite state, not delivery through a messaging platform or model.
"""
from concurrent.futures import ThreadPoolExecutor
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

app, root = map(Path, sys.argv[1:3])
root.mkdir(parents=True, exist_ok=True)
(root / 'data').mkdir(exist_ok=True)
with socket.socket() as reservation:
    reservation.bind(('127.0.0.1', 0))
    port = reservation.getsockname()[1]
password = 'Kinakaze-Local-Probe-29!'
configuration = root / 'data/cmd_config.json'
configuration.write_text(json.dumps({'dashboard': {'enable': True,
    'host': '127.0.0.1', 'port': port, 'username': 'probe'}, 'platform': [], 'provider': []}))
environment = dict(os.environ, ASTRBOT_ROOT=str(root), TZ='UTC',
                   ASTRBOT_DASHBOARD_INITIAL_PASSWORD=password)
checks = []
process = None


def request(path, data=None, token=None):
    client = http.client.HTTPConnection('127.0.0.1', port, timeout=8)
    headers = {'Content-Type': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    try:
        client.request('GET' if data is None else 'POST', path,
                       body=None if data is None else json.dumps(data), headers=headers)
        response = client.getresponse()
        body = response.read()
        return response.status, json.loads(body)
    finally:
        client.close()


try:
    for generation in range(2):
        begin = time.monotonic()
        with (root / f'server-{generation}.log').open('wb') as log:
            process = subprocess.Popen([sys.executable, '-u', str(app / 'main.py')],
                cwd=app, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        while True:
            assert process.poll() is None, ('AstrBot exited before readiness', process.returncode)
            try:
                status, versions = request('/api/stat/versions')
                if status == 200 and versions.get('status') == 'ok':
                    break
            except (OSError, ValueError):
                pass
            assert time.monotonic() - begin < 180, 'AstrBot startup timed out'
            time.sleep(.1)
        startup_ms = (time.monotonic() - begin) * 1000
        status, login = request('/api/auth/login', {'username': 'probe', 'password': password})
        assert status == 200 and login['status'] == 'ok', (status, login)
        token = login['data']['token']
        with ThreadPoolExecutor(4) as clients:
            rows = list(clients.map(lambda _: request('/api/stat/versions'), range(16)))
        assert all(status == 200 and row['status'] == 'ok' for status, row in rows), rows
        # Keep serving after initial readiness and after concurrent clients.
        time.sleep(3)
        assert process.poll() is None
        assert request('/api/stat/start-time', token=token)[0] == 200
        assert json.loads(configuration.read_text(encoding='utf-8-sig'))['dashboard']['username'] == 'probe'
        checks.append(dict(generation=generation, startup_ms=startup_ms,
                           concurrent_requests=16, authenticated=True, versions=versions))
        process.send_signal(signal.SIGINT)
        exit_code = process.wait(timeout=30)
        assert exit_code in (0, -signal.SIGINT, 130), exit_code
        process = None
    assert any((root / 'data').rglob('*.db')), 'AstrBot did not create its database'
    print(json.dumps(checks), flush=True)
    print('ASTRBOT_API_AUTH_RESTART_OK', flush=True)
finally:
    if process is not None and process.poll() is None:
        process.kill()
        process.wait(timeout=10)
    for log in sorted(root.glob('server-*.log')):
        print(log.name, log.read_text(errors='replace')[-16000:], flush=True)
