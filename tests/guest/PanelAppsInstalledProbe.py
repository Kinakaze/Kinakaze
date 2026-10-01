"""Functional workloads for additionally installed store applications."""

from datetime import datetime, timezone
import base64
import hashlib
import hmac
import json
import os
import http.server
from pathlib import Path
import threading
import socket
import struct
import urllib.parse

from PanelAppsRuntimeProbe import ROOT, PAYLOAD, api, command, port, request, service, wait


def minio():
    address, console = port(), port()
    access, secret = 'kinakazeprobe', 'KinakazeObjectStore42'
    data = ROOT / 'minio-data'
    data.mkdir()
    env = dict(os.environ, MINIO_ROOT_USER=access, MINIO_ROOT_PASSWORD=secret)

    def signed(method, path, body=b''):
        stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        day = stamp[:8]
        digest = hashlib.sha256(body).hexdigest()
        headers = {'host': f'127.0.0.1:{address}', 'x-amz-content-sha256': digest,
                   'x-amz-date': stamp}
        names = ';'.join(sorted(headers))
        canonical = '\n'.join((method, path, '', ''.join(
            name + ':' + headers[name] + '\n' for name in sorted(headers)), names, digest))
        scope = day + '/us-east-1/s3/aws4_request'
        string = '\n'.join(('AWS4-HMAC-SHA256', stamp, scope,
                            hashlib.sha256(canonical.encode()).hexdigest()))
        key = ('AWS4' + secret).encode()
        for component in (day, 'us-east-1', 's3', 'aws4_request'):
            key = hmac.new(key, component.encode(), hashlib.sha256).digest()
        signature = hmac.new(key, string.encode(), hashlib.sha256).hexdigest()
        headers['Authorization'] = (f'AWS4-HMAC-SHA256 Credential={access}/{scope}, '
                                    f'SignedHeaders={names}, Signature={signature}')
        return request(address, path, method, body, headers, timeout=30)

    for iteration in range(2):
        with service(['/opt/1panel-apps/minio/minio', 'server', '--address',
                      f'127.0.0.1:{address}', '--console-address', f'127.0.0.1:{console}',
                      str(data)], 'minio', env=env) as process:
            wait(process, lambda: request(address, '/minio/health/ready')[0] == 200, timeout=90)
            wait(process, lambda: signed('GET', '/')[0] == 200, timeout=90)
            if iteration == 0:
                status, _, response = signed('PUT', '/compatibility-probe')
                assert status == 200, (status, response)
                status, _, response = signed('PUT', '/compatibility-probe/payload.bin', PAYLOAD)
                assert status == 200, (status, response)
            status, _, response = signed('GET', '/compatibility-probe/payload.bin')
            assert status == 200 and response == PAYLOAD, (status, response[:200])
            if iteration == 1:
                assert signed('DELETE', '/compatibility-probe/payload.bin')[0] == 204
                assert signed('GET', '/compatibility-probe/payload.bin')[0] == 404
    print('MINIO_SIGV4_BUCKET_OBJECT_RESTART_DELETE_OK')


def ollama():
    address = port()
    model = 'smollm2:135m'
    models = Path('/opt/1panel-apps/ollama/models')
    models.mkdir(exist_ok=True)
    env = dict(os.environ, HOME=str(ROOT), OLLAMA_HOST=f'127.0.0.1:{address}',
               OLLAMA_MODELS=str(models), OLLAMA_NUM_PARALLEL='1',
               OLLAMA_MAX_LOADED_MODELS='1', OLLAMA_CONTEXT_LENGTH='512',
               CUDA_VISIBLE_DEVICES='-1', ROCR_VISIBLE_DEVICES='-1')
    for iteration in range(2):
        with service(['/opt/1panel-apps/ollama/bin/ollama', 'serve'], 'ollama', env=env) as process:
            wait(process, lambda: bool(api(address, '/api/version')['version']), timeout=90)
            if iteration == 0:
                result = api(address, '/api/pull', 'POST', {'model': model, 'stream': False}, timeout=600)
                assert result['status'] == 'success', result
            assert any(item['name'] == model for item in api(address, '/api/tags')['models'])
            result = api(address, '/api/generate', 'POST',
                         {'model': model, 'prompt': 'Say hello.', 'stream': False,
                          'options': {'temperature': 0, 'num_predict': 8, 'num_ctx': 512, 'num_gpu': 0}}, timeout=180)
            assert result['done'] and result['response'].strip(), result
    print('OLLAMA_CPU_MODEL_PULL_GENERATE_RESTART_OK')


def redis_commander():
    address, redis_port = port(), port()
    prefix = '/opt/1panel-apps/redis-commander/node_modules/redis-commander'
    env = dict(os.environ, HOME=str(ROOT), NODE_CONFIG_DIR=prefix + '/config')
    with service(['/usr/bin/redis-server', '--bind', '127.0.0.1', '--port', str(redis_port),
                  '--dir', str(ROOT), '--save', '', '--appendonly', 'no'], 'redis') as redis:
        wait(redis, lambda: command(['/usr/bin/redis-cli', '-p', str(redis_port), 'ping']) == 'PONG')
        for iteration in range(2):
            with service(['/opt/1panel-apps/node22/bin/node', prefix + '/bin/redis-commander.js',
                          '--redis-host', '127.0.0.1', '--redis-port', str(redis_port),
                          '--address', '127.0.0.1', '--port', str(address), '--noload',
                          '--http-auth-username', 'probe', '--http-auth-password', 'ProbePassword42'],
                         'redis-commander', env=env, shutdown_codes=(0, -15)) as process:
                wait(process, lambda: request(address)[0] == 200)
                assert request(address, '/connections')[0] in (401, 403)
                login = api(address, '/signin', 'POST', {'username': 'probe', 'password': 'ProbePassword42'})
                assert login['ok'], login
                headers = {'Authorization': 'Bearer ' + login['bearerToken']}
                connections = api(address, '/connections', headers=headers)['connections']
                assert len(connections) == 1, connections
                endpoint = '/apiv1/exec/' + urllib.parse.quote(connections[0]['conId'], safe='')
                if iteration == 0:
                    assert api(address, endpoint, 'POST', {'cmd': 'SET compatibility-probe persisted'}, headers)['data'] == 'OK'
                assert api(address, endpoint, 'POST', {'cmd': 'GET compatibility-probe'}, headers)['data'] == 'persisted'
    print('REDIS_COMMANDER_AUTH_WRITE_READ_RESTART_OK')


def uptime_kuma():
    address = port()
    prefix = '/opt/1panel-apps/uptime-kuma'
    node = '/opt/1panel-apps/node22/bin/node'
    env = dict(os.environ, DATA_DIR=str(ROOT / 'kuma-data'), NODE_ENV='production')
    client = ROOT / 'kuma-client.cjs'
    client.write_text('''const assert = require('node:assert/strict');
const { io } = require('/opt/1panel-apps/uptime-kuma/node_modules/socket.io-client');
const [address, target, iteration] = process.argv.slice(2);
const socket = io('http://127.0.0.1:' + address, {transports: ['websocket'], reconnection: false});
const timer = setTimeout(() => { console.error('monitor test timed out'); process.exit(1); }, 90000);
const call = (event, ...args) => new Promise((resolve, reject) => {
    socket.timeout(20000).emit(event, ...args, (error, result) => {
        if (error) return reject(error);
        if (!result.ok) return reject(new Error(JSON.stringify(result)));
        resolve(result);
    });
});
socket.on('connect_error', error => { console.error(error); process.exit(1); });
socket.on('connect', async () => {
    try {
        if (iteration === '0') await call('setup', 'probe', 'ProbePassword42');
        await call('login', {username: 'probe', password: 'ProbePassword42'});
        const heartbeat = new Promise(resolve => socket.on('heartbeat', beat => {
            if (beat.status === 1) resolve(beat);
        }));
        if (iteration === '0') {
            const created = await call('add', {name: 'compatibility-probe', type: 'http',
                url: 'http://127.0.0.1:' + target, method: 'GET', interval: 20,
                retryInterval: 20, maxretries: 0, timeout: 10, active: true,
                accepted_statuscodes: ['200-299'], notificationIDList: {},
                kafkaProducerBrokers: [], kafkaProducerSaslOptions: {}});
            require('node:fs').writeFileSync('kuma-monitor-id', String(created.monitorID));
        }
        const monitorID = Number(require('node:fs').readFileSync('kuma-monitor-id', 'utf8'));
        const result = await call('getMonitor', monitorID);
        assert.equal(result.monitor.name, 'compatibility-probe');
        assert.equal(result.monitor.url, 'http://127.0.0.1:' + target);
        const beat = await heartbeat;
        assert.equal(Number(beat.monitorID), monitorID);
        socket.disconnect(); clearTimeout(timer);
        console.log('KUMA_MONITOR_HEARTBEAT_OK');
    } catch (error) { console.error(error); process.exit(1); }
});
''')
    target = http.server.ThreadingHTTPServer(('127.0.0.1', 0), http.server.SimpleHTTPRequestHandler)
    thread = threading.Thread(target=target.serve_forever, daemon=True)
    thread.start()
    try:
        for iteration in range(2):
            with service([node, prefix + '/server/server.js', '--host=127.0.0.1',
                          '--port=' + str(address)], 'uptime-kuma', env=env, cwd=prefix) as process:
                wait(process, lambda: request(address, '/dashboard')[0] == 200, timeout=90)
                assert 'KUMA_MONITOR_HEARTBEAT_OK' in command(
                    [node, str(client), str(address), str(target.server_port), str(iteration)], timeout=110)
    finally:
        target.shutdown()
        target.server_close()
        thread.join(timeout=5)
    print('UPTIME_KUMA_SETUP_LOGIN_HTTP_MONITOR_RESTART_OK')


def mongo_express():
    address, mongo_port = port(), port()
    prefix = '/opt/1panel-apps/mongo-express'
    data = ROOT / 'mongo-data'
    data.mkdir()
    env = dict(os.environ, ME_CONFIG_MONGODB_URL=f'mongodb://127.0.0.1:{mongo_port}',
               ME_CONFIG_MONGODB_ENABLE_ADMIN='true', VCAP_APP_HOST='127.0.0.1', PORT=str(address),
               ME_CONFIG_BASICAUTH='true', ME_CONFIG_BASICAUTH_USERNAME='probe',
               ME_CONFIG_BASICAUTH_PASSWORD='ProbePassword42',
               ME_CONFIG_SITE_COOKIESECRET='probe-cookie-secret-42',
               ME_CONFIG_SITE_SESSIONSECRET='probe-session-secret-42')
    headers = {'Authorization': 'Basic ' + base64.b64encode(b'probe:ProbePassword42').decode()}
    with service(['/opt/1panel-apps/mongodb/bin/mongod', '--dbpath', str(data),
                  '--bind_ip', '127.0.0.1', '--port', str(mongo_port),
                  '--wiredTigerCacheSizeGB', '0.25'], 'mongodb') as mongo:
        def ready():
            with socket.create_connection(('127.0.0.1', mongo_port), timeout=1):
                return True
        wait(mongo, ready)
        for iteration in range(2):
            with service(['/opt/1panel-apps/node22/bin/node', prefix + '/app.js'],
                         'mongo-express', env=env, cwd=prefix, shutdown_codes=(0, -15)) as process:
                wait(process, lambda: request(address, headers=headers)[0] == 200, timeout=60)
                assert request(address)[0] == 401
                if iteration == 0:
                    for path, body in [('/', {'database': 'compatibility_probe'}),
                                       ('/db/compatibility_probe', {'collection': 'payloads'}),
                                       ('/db/compatibility_probe/payloads',
                                        {'document': json.dumps({'message': 'persisted-probe-value', 'value': 42})})]:
                        status, _, response = request(address, path, 'POST', urllib.parse.urlencode(body),
                            dict(headers, **{'Content-Type': 'application/x-www-form-urlencoded'}))
                        assert status == 302, (status, response[-1000:])
                status, _, response = request(address, '/db/compatibility_probe/payloads', headers=headers)
                assert status == 200 and b'persisted-probe-value' in response, (status, response[-1000:])
    print('MONGO_EXPRESS_AUTH_DATABASE_COLLECTION_DOCUMENT_RESTART_OK')


def frp():
    server_port, remote_port = port(), port()
    target = http.server.ThreadingHTTPServer(('127.0.0.1', 0), http.server.SimpleHTTPRequestHandler)
    payload = ROOT / 'frp-payload.bin'
    payload.write_bytes(PAYLOAD)
    thread = threading.Thread(target=target.serve_forever, daemon=True)
    thread.start()
    server = ROOT / 'frps.toml'
    server.write_text(f'bindAddr = "127.0.0.1"\nbindPort = {server_port}\n'
                      'proxyBindAddr = "127.0.0.1"\nauth.token = "ProbeTunnel42"\n')
    client = ROOT / 'frpc.toml'
    client.write_text(f'serverAddr = "127.0.0.1"\nserverPort = {server_port}\n'
                      'auth.token = "ProbeTunnel42"\n[[proxies]]\nname = "probe"\ntype = "tcp"\n'
                      f'localIP = "127.0.0.1"\nlocalPort = {target.server_port}\nremotePort = {remote_port}\n')
    try:
        for iteration in range(2):
            with service(['/opt/1panel-apps/frp/frps', '-c', str(server)], 'frps', shutdown_codes=(0, -15)) as server_process:
                def ready():
                    with socket.create_connection(('127.0.0.1', server_port), timeout=1):
                        return True
                wait(server_process, ready)
                with service(['/opt/1panel-apps/frp/frpc', '-c', str(client)], 'frpc', shutdown_codes=(0, -15)) as client_process:
                    wait(client_process, lambda: request(remote_port, '/frp-payload.bin')[0] == 200)
                    status, _, body = request(remote_port, '/frp-payload.bin')
                    assert status == 200 and body == PAYLOAD
    finally:
        target.shutdown()
        target.server_close()
        thread.join(timeout=5)
    print('FRP_AUTHENTICATED_TCP_BINARY_TUNNEL_RESTART_OK')


def adguardhome():
    address, dns_port = port(), port()
    work = ROOT / 'adguard-data'
    work.mkdir()
    config = work / 'AdGuardHome.yaml'
    credentials = {'Authorization': 'Basic ' + base64.b64encode(b'probe:ProbePassword42').decode()}
    domain = 'compatibility-probe.invalid'
    question = b''.join(bytes([len(label)]) + label.encode() for label in domain.split('.')) + b'\0\0\1\0\1'
    packet = struct.pack('!6H', 42, 0x100, 1, 0, 0, 0) + question
    for iteration in range(2):
        with service(['/opt/1panel-apps/adguardhome/AdGuardHome', '-c', str(config),
                      '-w', str(work), '--web-addr', f'127.0.0.1:{address}',
                      '--no-check-update'], 'adguardhome') as process:
            if iteration == 0:
                wait(process, lambda: request(address, '/control/install/get_addresses')[0] == 200)
                status, _, body = request(address, '/control/install/configure', 'POST', json.dumps({
                    'web': {'ip': '127.0.0.1', 'port': address},
                    'dns': {'ip': '127.0.0.1', 'port': dns_port},
                    'username': 'probe', 'password': 'ProbePassword42'}), {'Content-Type': 'application/json'})
                assert status == 200, (status, body)
            wait(process, lambda: request(address, '/control/status', headers=credentials)[0] == 200)
            assert request(address, '/control/status')[0] in (302, 401, 403)
            if iteration == 0:
                status, _, body = request(address, '/control/rewrite/add', 'POST',
                    json.dumps({'domain': domain, 'answer': '192.0.2.42'}),
                    dict(credentials, **{'Content-Type': 'application/json'}))
                assert status == 200, (status, body)
            assert {'domain': domain, 'answer': '192.0.2.42'} in api(address, '/control/rewrite/list', headers=credentials)
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as connection:
                connection.settimeout(5)
                connection.sendto(packet, ('127.0.0.1', dns_port))
                response = connection.recv(4096)
                transaction, flags, questions, answers, _, _ = struct.unpack('!6H', response[:12])
                assert transaction == 42 and flags & 0xf == 0 and questions == 1 and answers == 1
                assert response[-4:] == socket.inet_aton('192.0.2.42'), response
    print('ADGUARDHOME_SETUP_AUTH_DNS_REWRITE_RESTART_OK')


PROBES = {'minio': minio, 'ollama': ollama, 'redis-commander': redis_commander,
          'uptime-kuma': uptime_kuma, 'mongo-express': mongo_express, 'frpc': frp, 'frps': frp,
          'adguardhome': adguardhome}
