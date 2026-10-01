"""Additional upstream applications; shares owned service lifetimes with the matrix."""
import base64
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import pwd
import signal
import shutil
import socket
import struct
import time
import urllib.parse

from PanelAppsRuntimeProbe import ROOT, JAVA, PAYLOAD, api, command, port, request, service, wait


def bson(document):
    body = b""
    for name, value in document.items():
        key = name.encode() + b"\0"
        if isinstance(value, bool):
            body += b"\x08" + key + bytes([value])
        elif isinstance(value, int):
            body += b"\x10" + key + struct.pack("<i", value)
        elif isinstance(value, str):
            text = value.encode() + b"\0"
            body += b"\x02" + key + struct.pack("<i", len(text)) + text
        elif isinstance(value, dict):
            body += b"\x03" + key + bson(value)
        elif isinstance(value, list):
            body += b"\x04" + key + bson({str(i): item for i, item in enumerate(value)})
        else:
            raise TypeError(type(value))
    return struct.pack("<i", len(body) + 5) + body + b"\0"


def unbson(data):
    length = struct.unpack_from("<i", data)[0]
    assert 5 <= length <= len(data)
    position, result = 4, {}
    while position < length - 1:
        kind = data[position]
        position += 1
        end = data.index(b"\0", position)
        name = data[position:end].decode()
        position = end + 1
        if kind == 1:
            value = struct.unpack_from("<d", data, position)[0]; position += 8
        elif kind == 2:
            size = struct.unpack_from("<i", data, position)[0]; position += 4
            value = data[position:position + size - 1].decode(); position += size
        elif kind in (3, 4):
            size = struct.unpack_from("<i", data, position)[0]
            value = unbson(data[position:position + size]); position += size
            if kind == 4:
                value = list(value.values())
        elif kind == 5:
            size = struct.unpack_from("<i", data, position)[0]; position += 5
            value = data[position:position + size]; position += size
        elif kind == 7:
            value = data[position:position + 12]; position += 12
        elif kind == 8:
            value = bool(data[position]); position += 1
        elif kind in (9, 17, 18):
            value = struct.unpack_from("<q", data, position)[0]; position += 8
        elif kind == 10:
            value = None
        elif kind == 16:
            value = struct.unpack_from("<i", data, position)[0]; position += 4
        else:
            raise AssertionError(("unsupported BSON reply", kind, name))
        result[name] = value
    return result


def mongodb():
    address = port()
    database = ROOT / "mongodb-data"
    database.mkdir()
    def call(document):
        message = struct.pack("<i", 0) + b"\0" + bson(document)
        with socket.create_connection(("127.0.0.1", address), timeout=10) as connection:
            connection.sendall(struct.pack("<iiii", len(message) + 16, 42, 0, 2013) + message)
            stream = connection.makefile("rb")
            header = stream.read(16)
            assert len(header) == 16
            size, _, response_to, opcode = struct.unpack("<iiii", header)
            assert 16 <= size <= 16 * 1024 * 1024 and response_to == 42 and opcode == 2013
            response = stream.read(size - 16)
            assert len(response) == size - 16 and response[4] == 0
            result = unbson(response[5:])
            stream.close()
            assert result["ok"] == 1, result
            return result
    for iteration in range(2):
        with service(["/opt/1panel-apps/mongodb/bin/mongod", "--dbpath", str(database), "--bind_ip", "127.0.0.1", "--port", str(address),
                      "--wiredTigerCacheSizeGB", "0.25", "--setParameter", "diagnosticDataCollectionEnabled=false"], "mongodb") as process:
            wait(process, lambda: call({"ping": 1, "$db": "admin"})["ok"] == 1, timeout=90)
            if iteration == 0:
                assert call({"insert": "values", "documents": [{"_id": 1, "value": 42}], "writeConcern": {"w": 1, "j": True}, "$db": "probe"})["n"] == 1
                assert call({"update": "values", "updates": [{"q": {"_id": 1}, "u": {"$inc": {"value": 8}}}], "$db": "probe"})["n"] == 1
            result = call({"find": "values", "filter": {"_id": 1}, "$db": "probe"})
            assert result["cursor"]["firstBatch"] == [{"_id": 1, "value": 50}], result
    print("MONGODB_WIREDTIGER_INSERT_UPDATE_FIND_RESTART_OK")


def meilisearch():
    address = port()
    headers = {"Authorization": "Bearer panel-master-key-at-least-16-characters"}
    env = dict(os.environ, MEILI_MASTER_KEY=headers["Authorization"][7:], MEILI_ENV="production", MEILI_NO_ANALYTICS="true")
    def task(uid):
        deadline = time.monotonic() + 30
        while True:
            result = api(address, f"/tasks/{uid}", headers=headers)
            if result["status"] == "succeeded":
                return
            assert result["status"] != "failed" and time.monotonic() < deadline, result
            time.sleep(0.1)
    for iteration in range(2):
        with service(["/opt/1panel-apps/meilisearch/meilisearch", "--http-addr", f"127.0.0.1:{address}", "--db-path", str(ROOT / "meili-data")], "meilisearch", env=env) as process:
            wait(process, lambda: request(address, "/health")[0] == 200)
            if iteration == 0:
                result = api(address, "/indexes", "POST", dict(uid="books", primaryKey="id"), headers)
                task(result["taskUid"])
                result = api(address, "/indexes/books/documents", "POST", [{"id": 42, "title": "Kinakaze compatibility"}], headers)
                task(result["taskUid"])
            result = api(address, "/indexes/books/search", "POST", {"q": "compatibility"}, headers)
            assert result["hits"][0]["id"] == 42, result
    print("MEILISEARCH_INDEX_TASK_SEARCH_DATABASE_RESTART_OK")


def jenkins():
    address = port()
    home = ROOT / "jenkins"
    job = home / "jobs/probe"
    job.mkdir(parents=True)
    (job / "config.xml").write_text('''<project><actions/><description>Compatibility</description>
<keepDependencies>false</keepDependencies><properties/><scm class="hudson.scm.NullSCM"/>
<canRoam>true</canRoam><disabled>false</disabled><blockBuildWhenDownstreamBuilding>false</blockBuildWhenDownstreamBuilding>
<blockBuildWhenUpstreamBuilding>false</blockBuildWhenUpstreamBuilding><triggers/><concurrentBuild>false</concurrentBuild>
<builders><hudson.tasks.Shell><command>printf JENKINS_BUILD_OK; printf persisted &gt; artifact.txt</command></hudson.tasks.Shell></builders>
<publishers/><buildWrappers/></project>''')
    # This test instance uses a loopback-only, locally owned anonymous build
    # configuration. The HTTP crumb is still required for state changes.
    env = dict(os.environ, JENKINS_HOME=str(home))
    with service([JAVA, "-Xmx256m", "-Djenkins.install.runSetupWizard=false", "-jar", "/opt/1panel-apps/jenkins/jenkins.war",
                  "--httpListenAddress=127.0.0.1", f"--httpPort={address}"], "jenkins", env=env, shutdown_codes=(143,)) as process:
        wait(process, lambda: request(address, "/api/json")[0] == 200, timeout=120)
        # Obtain the crumb and session together.
        status, cookies, data = request(address, "/crumbIssuer/api/json")
        crumb = json.loads(data)
        headers = {crumb["crumbRequestField"]: crumb["crumb"], "Cookie": cookies.get("Set-Cookie", "").split(";", 1)[0]}
        assert request(address, "/job/probe/build", "POST", b"", headers)[0] in (201, 302)
        wait(process, lambda: request(address, "/job/probe/lastBuild/api/json")[0] == 200, timeout=60)
        deadline = time.monotonic() + 60
        while True:
            result = api(address, "/job/probe/lastBuild/api/json")
            if not result["building"]:
                assert result["result"] == "SUCCESS", result
                break
            assert time.monotonic() < deadline
            time.sleep(0.2)
        assert b"JENKINS_BUILD_OK" in request(address, "/job/probe/lastBuild/consoleText")[2]
        assert (job / "workspace/artifact.txt").read_text() == "persisted"
    print("JENKINS_REAL_SHELL_JOB_LOG_ARTIFACT_OK")


def gitea():
    address = port()
    account = pwd.getpwnam("nobody")
    ROOT.chmod(0o755)
    data = ROOT / "gitea-data"
    data.mkdir()
    os.chown(data, account.pw_uid, account.pw_gid)
    def demote():
        os.setgroups([]); os.setgid(account.pw_gid); os.setuid(account.pw_uid)
    config = data / "gitea.ini"
    config.write_text(f'''APP_NAME = Kinakaze
RUN_USER = nobody
RUN_MODE = prod
WORK_PATH = {data}
[server]
HTTP_ADDR = 127.0.0.1
HTTP_PORT = {address}
ROOT_URL = http://127.0.0.1:{address}/
DISABLE_SSH = true
OFFLINE_MODE = true
[database]
DB_TYPE = sqlite3
PATH = {data}/gitea.db
[repository]
ROOT = {data}/repositories
[security]
INSTALL_LOCK = true
SECRET_KEY = compatibility-test-key
[service]
DISABLE_REGISTRATION = true
[log]
MODE = console
''')
    config.chmod(0o644)
    os.chown(config, account.pw_uid, account.pw_gid)
    binary = "/opt/1panel-apps/gitea/gitea"
    env = dict(os.environ, HOME=str(data), GITEA_WORK_DIR=str(data))
    auth = {"Authorization": "Basic " + base64.b64encode(b"probe:panel-test-12345").decode()}
    for iteration in range(2):
        with service([binary, "web", "--config", str(config)], "gitea", env=env, preexec_fn=demote) as process:
            wait(process, lambda: request(address, "/api/v1/version")[0] == 200, timeout=90)
            if iteration == 0:
                command([binary, "admin", "user", "create", "--config", str(config), "--username", "probe", "--password", "panel-test-12345",
                         "--email", "probe@example.invalid", "--admin", "--must-change-password=false"], env=env, preexec_fn=demote)
                api(address, "/api/v1/user/repos", "POST", dict(name="probe", auto_init=True, default_branch="main"), auth)
                api(address, "/api/v1/repos/probe/probe/contents/payload.bin", "POST",
                    dict(content=base64.b64encode(PAYLOAD).decode(), message="compatibility commit", branch="main"), auth)
            result = api(address, "/api/v1/repos/probe/probe/contents/payload.bin", headers=auth)
            assert base64.b64decode(result["content"]) == PAYLOAD, result
    print("GITEA_SQLITE_AUTH_GIT_REPOSITORY_COMMIT_RESTART_OK")


def list_service(name):
    address = port()
    data = ROOT / (name + "-data")
    data.mkdir()
    (data / "config.json").write_text(json.dumps({"address": "127.0.0.1", "port": address, "database": {"type": "sqlite3", "db_file": str(data / "data.db")},
        "scheme": {"address": "127.0.0.1", "http_port": address, "https_port": -1, "force_https": False}}))
    binary = "/opt/1panel-apps/" + name + "/" + name
    with service([binary, "server", "--data", str(data)], name) as process:
        wait(process, lambda: request(address, "/api/public/settings")[0] == 200, timeout=60)
        result = api(address, "/api/public/settings")
        assert result["code"] == 200 and result["data"], result
        assert request(address)[0] == 200
        # Storage mounts require provider credentials and are a separate scope.
    print(name.upper() + "_SQLITE_HTTP_SETTINGS_API_OK")


def ntfy():
    address = port()
    config = ROOT / "ntfy.yml"
    config.write_text(f"listen-http: '127.0.0.1:{address}'\ncache-file: '{ROOT}/ntfy.db'\nbase-url: 'http://127.0.0.1:{address}'\n")
    for iteration in range(2):
        # Upstream 2.13.0 only handles SIGHUP; SIGTERM terminates the process.
        with service(["/opt/1panel-apps/ntfy/ntfy", "serve", "--config", str(config)], "ntfy", shutdown_codes=(-15,)) as process:
            wait(process, lambda: request(address, "/v1/health")[0] == 200)
            if iteration == 0:
                status, _, body = request(address, "/probe", "POST", b"message-42", {"Title": "Compatibility"})
                assert status == 200 and json.loads(body)["message"] == "message-42"
            status, _, body = request(address, "/probe/json?poll=1&since=all")
            assert status == 200 and any(json.loads(line).get("message") == "message-42" for line in body.splitlines()), body
    print("NTFY_PUBLISH_POLL_SQLITE_RESTART_OK")


def code_server():
    address = port()
    env = dict(os.environ, PASSWORD="panel-test-12345")
    argv = ["/opt/1panel-apps/code-server/bin/code-server", "--bind-addr", f"127.0.0.1:{address}",
            "--user-data-dir", str(ROOT / "code-data"), "--extensions-dir", str(ROOT / "extensions"), "--disable-telemetry", str(ROOT)]
    with service(argv, "code-server", env=env) as process:
        wait(process, lambda: request(address, "/login")[0] == 200, timeout=90)
        status, headers, _ = request(address, "/login", "POST", b"password=panel-test-12345", {"Content-Type": "application/x-www-form-urlencoded"})
        assert status in (302, 303) and "Set-Cookie" in headers, (status, headers)
        cookie = headers["Set-Cookie"].split(";", 1)[0]
        status, _, body = request(address, "/", headers={"Cookie": cookie})
        if status == 302:
            status, _, body = request(address, "/?folder=" + urllib.parse.quote(str(ROOT)), headers={"Cookie": cookie})
        assert status == 200 and b"vscode" in body.lower(), (status, body[-1000:])
    print("CODE_SERVER_REAL_NODE_AUTH_EDITOR_PAGE_OK")


def registry():
    address = port()
    config = ROOT / "registry.yml"
    config.write_text(f"version: 0.1\nstorage:\n  filesystem:\n    rootdirectory: {ROOT}/registry-data\nhttp:\n  addr: 127.0.0.1:{address}\n")
    digest = "sha256:" + hashlib.sha256(PAYLOAD).hexdigest()
    for iteration in range(2):
        # Registry 2.8.3 has no SIGTERM handler; kernel signal death is its
        # normal lifecycle. Blob durability is checked after reopening.
        with service(["/opt/1panel-apps/docker-registry/registry", "serve", str(config)], "registry", shutdown_codes=(-15,)) as process:
            wait(process, lambda: request(address, "/v2/")[0] == 200)
            if iteration == 0:
                status, headers, _ = request(address, "/v2/probe/blobs/uploads/", "POST", b"")
                assert status == 202, status
                location = urllib.parse.urlsplit(headers["Location"])
                path = location.path + "?" + location.query + "&digest=" + urllib.parse.quote(digest)
                assert request(address, path, "PUT", PAYLOAD, {"Content-Type": "application/octet-stream"})[0] == 201
            assert request(address, "/v2/probe/blobs/" + digest)[2] == PAYLOAD
    print("REGISTRY_REAL_OCI_BLOB_UPLOAD_HASH_DOWNLOAD_RESTART_OK")


def nextcloud():
    web = Path('/opt/1panel-apps/nextcloud')
    configuration = ROOT / 'nextcloud-config'
    configuration.mkdir()
    env = dict(os.environ, NEXTCLOUD_CONFIG_DIR=str(configuration))
    address = port()
    occ = ['/usr/bin/php', '-d', 'memory_limit=512M', str(web / 'occ')]
    command(occ + ['maintenance:install', '--database=sqlite', '--admin-user=probe',
                  '--admin-pass=panel-test-12345', '--data-dir=' + str(ROOT / 'nc-data')], env=env, timeout=180)
    command(occ + ['config:system:set', 'trusted_domains', '1', '--value=127.0.0.1:' + str(address)], env=env)
    auth = {'Authorization': 'Basic ' + base64.b64encode(b'probe:panel-test-12345').decode()}
    path = '/remote.php/dav/files/probe/payload.bin'
    for iteration in range(2):
        with service(['/usr/bin/php', '-d', 'memory_limit=512M', '-S', f'127.0.0.1:{address}', '-t', str(web)],
                     'nextcloud', env=env, shutdown_codes=(-15,)) as process:
            wait(process, lambda: api(address, '/status.php')['installed'] is True)
            if iteration == 0:
                assert request(address, path, 'PUT', PAYLOAD, auth, timeout=60)[0] in (201, 204)
            assert request(address, path, headers=auth, timeout=60)[2] == PAYLOAD
    print('NEXTCLOUD_OCC_SQLITE_WEBDAV_AUTH_UPLOAD_RESTART_OK')


def clickhouse():
    import shutil
    address = port()
    data = ROOT / 'clickhouse-data'
    data.mkdir()
    config = ROOT / 'clickhouse.xml'
    config.write_text(f'''<yandex><logger><level>warning</level><console>true</console></logger>
<http_port>{address}</http_port><listen_host>127.0.0.1</listen_host><path>{data}/</path>
<tmp_path>{ROOT}/clickhouse-tmp/</tmp_path><users_config>{ROOT}/users.xml</users_config>
<default_profile>default</default_profile><default_database>default</default_database>
<mark_cache_size>1048576</mark_cache_size><uncompressed_cache_size>1048576</uncompressed_cache_size>
</yandex>''')
    shutil.copyfile('/etc/clickhouse-server/users.xml', ROOT / 'users.xml')
    def sql(query):
        status, _, body = request(address, '/', 'POST', query.encode(),
                                  {'Content-Type': 'text/plain'}, timeout=30)
        assert status == 200, (status, body[-4096:])
        return body.strip()
    for iteration in range(2):
        with service(['/usr/sbin/clickhouse-server', '--config-file=' + str(config)], 'clickhouse') as process:
            wait(process, lambda: request(address, '/ping')[0] == 200, timeout=90)
            if iteration == 0:
                sql('CREATE TABLE probe (id UInt64, value String) ENGINE = MergeTree() ORDER BY id')
                sql("INSERT INTO probe VALUES (1, 'persisted'),(2, 'value')")
            assert sql('SELECT sum(id),count() FROM probe') == b'3\t2'
            assert b'persisted' in sql('SELECT value FROM probe WHERE id=1')
    print('CLICKHOUSE_SQL_MERGETREE_INSERT_AGGREGATE_RESTART_OK')


def halo():
    address = port()
    data = ROOT / 'halo-data'
    argv = [JAVA, '-Xmx384m', '-jar', '/opt/1panel-apps/halo/halo.jar',
            '--server.address=127.0.0.1', f'--server.port={address}',
            '--halo.work-dir=' + str(data), '--halo.external-url=http://127.0.0.1:' + str(address),
            '--spring.sql.init.mode=always']
    # First-run setup and embedded H2 migrations are real application behavior.
    with service(argv, 'halo', shutdown_codes=(143,)) as process:
        wait(process, lambda: request(address, '/actuator/health')[0] == 200, timeout=120)
        assert api(address, '/actuator/health')['status'] == 'UP'
        status, _, body = request(address, '/console')
        assert status in (200, 302) and (body or status == 302)
    assert any(data.rglob('*.mv.db')), 'H2 database was not created'
    print('HALO_REAL_JAVA_H2_MIGRATION_HEALTH_SETUP_OK')


def elasticsearch():
    address = port()
    account = pwd.getpwnam('nobody')
    ROOT.chmod(0o755)
    data, configuration = ROOT / 'es-data', ROOT / 'es-conf'
    data.mkdir(); configuration.mkdir()
    os.chown(data, account.pw_uid, account.pw_gid)
    os.chown(configuration, account.pw_uid, account.pw_gid)
    configuration.chmod(0o755)
    (configuration / 'elasticsearch.yml').write_text(f'''cluster.name: panel-probe
node.name: probe
network.host: 127.0.0.1
http.port: {address}
transport.port: {port()}
discovery.type: single-node
path.data: {data}
path.logs: {data}/logs
xpack.security.enabled: false
xpack.ml.enabled: false
''')
    (configuration / 'jvm.options').write_text('-Xms256m\n-Xmx256m\n')
    shutil.copyfile('/opt/1panel-apps/elasticsearch/config/log4j2.properties', configuration / 'log4j2.properties')
    def demote():
        os.setgroups([]); os.setgid(account.pw_gid); os.setuid(account.pw_uid)
    env = dict(os.environ, ES_PATH_CONF=str(configuration), ES_JAVA_HOME='/usr/lib/jvm/java-17-openjdk-amd64')
    def healthy():
        status, _, body = request(address, '/_cluster/health?wait_for_status=yellow&timeout=1s')
        if status == 408:
            return False
        assert status == 200, (status, body[-4096:])
        return json.loads(body)['status'] in ('yellow', 'green')
    for iteration in range(2):
        with service(['/opt/1panel-apps/elasticsearch/bin/elasticsearch'], 'elasticsearch', env=env,
                     preexec_fn=demote, shutdown_codes=(143,)) as process:
            wait(process, healthy, timeout=120)
            if iteration == 0:
                assert api(address, '/probe/_doc/1?refresh=true', 'PUT', {'value': 'persisted'}, timeout=60)['result'] == 'created'
            assert api(address, '/probe/_doc/1')['_source'] == {'value': 'persisted'}
            assert api(address, '/probe/_search', 'POST', {'query': {'match': {'value': 'persisted'}}})['hits']['total']['value'] == 1
    print('ELASTICSEARCH_REAL_INDEX_REFRESH_SEARCH_RESTART_OK')


def astrbot():
    address = port()
    prefix = Path('/opt/1panel-apps/astrbot')
    state = ROOT / 'astrbot-state'
    state.mkdir()
    env = dict(os.environ, ASTRBOT_ROOT=str(state), PYTHONPATH=str(prefix))
    python = '/opt/1panel-apps/python312/bin/python3.12'
    password = 'KinakazeProbe42'
    prepare = '''import copy,json,os
from pathlib import Path
from astrbot.core.config.default import DEFAULT_CONFIG
from astrbot.core.utils.auth_password import hash_dashboard_password
configuration = copy.deepcopy(DEFAULT_CONFIG)
configuration['dashboard'].update(host='127.0.0.1', port=int(os.environ['PROBE_PORT']),
    username='probe', password=hash_dashboard_password('KinakazeProbe42'),
    pbkdf2_password=hash_dashboard_password('KinakazeProbe42'), password_storage_upgraded=True,
    auth_rate_limit={'enable': False, 'average_interval': 1.0, 'max_burst': 3})
directory = Path(os.environ['ASTRBOT_ROOT']) / 'data'
directory.mkdir(parents=True, exist_ok=True)
(directory / 'cmd_config.json').write_text(json.dumps(configuration))
'''
    command([python, '-c', prepare], env=dict(env, PROBE_PORT=str(address)), timeout=120)
    profile_id = None
    for iteration in range(2):
        with service([python, str(prefix / 'main.py'), '--webui-dir',
                      '/opt/1panel-apps/astrbot-dashboard/dist'], 'astrbot', env=env,
                     shutdown_codes=(-signal.SIGTERM,)) as process:
            wait(process, lambda: request(address, '/api/auth/login', 'POST', b'{}',
                 {'Content-Type': 'application/json'})[0] in (200, 400, 401, 422), timeout=150)
            assert request(address, '/api/config/abconfs')[0] == 401
            status, _, dashboard = request(address, '/')
            assert status == 200 and b'<html' in dashboard.lower(), (status, dashboard[:200])
            login = api(address, '/api/auth/login', 'POST', {'username': 'probe', 'password': password})
            assert login['status'] == 'ok', login
            credentials = {'Authorization': 'Bearer ' + login['data']['token']}
            if iteration == 0:
                created = api(address, '/api/config/abconf/new', 'POST', {'name': 'Kinakaze persistence'}, credentials)
                assert created['status'] == 'ok', created
                profile_id = created['data']['conf_id']
            profile = api(address, '/api/config/abconf?id=' + urllib.parse.quote(profile_id), headers=credentials)
            assert profile['status'] == 'ok' and isinstance(profile['data']['config'], dict), profile
    print('ASTRBOT_AUTH_CONFIG_CREATE_READ_RESTART_OK')


PROBES = {"astrbot": astrbot, "mongodb": mongodb, "meilisearch": meilisearch, "jenkins": jenkins, "gitea": gitea,
          "alist": lambda: list_service("alist"), "openlist": lambda: list_service("openlist"),
          "ntfy": ntfy, "code-server": code_server, "docker-registry": registry,
          "nextcloud": nextcloud, "clickhouse": clickhouse, "halo": halo,
          "elasticsearch": elasticsearch}
