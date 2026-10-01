"""Real services used by the 1Panel application compatibility matrix.

All state belongs to the current test directory. Ports bind to loopback, and
every owned child is waited for. Logs survive failed probes in the matrix stage.
"""

import base64
from contextlib import contextmanager
import hashlib
import hmac
import http.client
import json
import os
from pathlib import Path
import pwd
import signal
import socket
import struct
import subprocess
import sys
import time
import urllib.parse
import uuid


ROOT = Path.cwd()
JAVA = "/usr/lib/jvm/java-17-openjdk-amd64/bin/java"
PAYLOAD = bytes(range(256)) * 64


def port():
    with socket.socket() as connection:
        connection.bind(("127.0.0.1", 0))
        return connection.getsockname()[1]


def command(argv, **kwargs):
    result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=kwargs.pop("timeout", 90), **kwargs)
    assert result.returncode == 0, (argv, result.returncode, result.stdout[-4096:], result.stderr[-4096:])
    return result.stdout.decode().strip()


def request(address, path="/", method="GET", body=None, headers=None, timeout=10):
    connection = http.client.HTTPConnection("127.0.0.1", address, timeout=timeout)
    try:
        connection.request(method, path, body=body, headers=headers or {})
        response = connection.getresponse()
        return response.status, dict(response.getheaders()), response.read()
    finally:
        connection.close()


def api(address, path, method="GET", body=None, headers=None, timeout=10):
    headers = dict(headers or {})
    if body is not None:
        body = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    status, _, data = request(address, path, method, body, headers, timeout=timeout)
    assert 200 <= status < 300, (status, path, data[-4096:])
    return json.loads(data) if data else None


def wait(process, ready, timeout=40):
    deadline = time.monotonic() + timeout
    while True:
        assert process.poll() is None, ("service exited", process.returncode)
        try:
            if ready():
                return
        except (OSError, http.client.HTTPException):
            pass
        assert time.monotonic() < deadline, "service readiness timeout"
        time.sleep(0.1)


@contextmanager
def service(argv, name, env=None, preexec_fn=None, shutdown_codes=(0,), cwd=None):
    logfile = ROOT / (name + ".log")
    with logfile.open("ab", buffering=0) as log:
        process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                   env=env, preexec_fn=preexec_fn, cwd=cwd)
        try:
            yield process
            process.send_signal(signal.SIGTERM)
            assert process.wait(timeout=25) in shutdown_codes, (name, "unclean shutdown", process.returncode)
            if process.returncode != 0 and name == "filebrowser":
                # File Browser v2.32.0 cmd/root.go races log.Fatal(http.Serve)
                # against cleanupHandler's os.Exit(0) after listener.Close().
                # Require the actual shutdown trace as well as later reopen.
                text = logfile.read_text(errors="replace")
                assert name == "filebrowser" and "Caught signal terminated: shutting down." in text and "use of closed network connection" in text
                print("FILEBROWSER_UPSTREAM_LISTENER_CLOSE_EXIT_1", flush=True)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
            print(name + " log:\n" + logfile.read_text(errors="replace")[-12000:], flush=True)


def php():
    address = port()
    (ROOT / "index.php").write_text('''<?php
$db=new PDO('sqlite:' . __DIR__ . '/php.sqlite');
$db->exec('CREATE TABLE IF NOT EXISTS values_table(value INTEGER)');
if($_SERVER['REQUEST_METHOD']==='POST') {
 $db->beginTransaction(); $db->exec('INSERT INTO values_table VALUES(42)'); $db->commit();
 $db->beginTransaction(); $db->exec('INSERT INTO values_table VALUES(99)'); $db->rollBack();
}
header('Content-Type: application/json');
echo json_encode(['sum'=>(int)$db->query('SELECT COALESCE(SUM(value),0) FROM values_table')->fetchColumn(),
 'hash'=>hash('sha256',file_get_contents('php://input')), 'pid'=>getmypid()]);
''')
    for iteration in range(2):
        with service(["/usr/bin/php", "-S", f"127.0.0.1:{address}", "-t", str(ROOT)], "php", shutdown_codes=(-15,)) as process:
            wait(process, lambda: request(address)[0] == 200)
            if iteration == 0:
                status, _, data = request(address, "/", "POST", PAYLOAD)
                result = json.loads(data)
                assert status == 200 and result["sum"] == 42
                assert result["hash"] == hashlib.sha256(PAYLOAD).hexdigest()
            else:
                assert api(address, "/")["sum"] == 42
    print("PHP_HTTP_SQLITE_TRANSACTION_RESTART_OK")


def fcgi_pair(name, value):
    name, value = name.encode(), value.encode()
    def size(length):
        return bytes([length]) if length < 128 else struct.pack("!I", length | 0x80000000)
    return size(len(name)) + size(len(value)) + name + value


def fcgi_record(kind, data):
    return struct.pack("!BBHHBB", 1, kind, 1, len(data), 0, 0) + data


def php_fpm():
    address = port()
    ROOT.chmod(0o755)
    script = ROOT / "fpm.php"
    script.write_text("<?php echo 'FPM_OK_' . hash('sha256', file_get_contents('php://input')); ?>")
    script.chmod(0o644)
    configuration = ROOT / "fpm.conf"
    configuration.write_text(f"""[global]
daemonize = no
error_log = {ROOT}/fpm-error.log
pid = {ROOT}/fpm.pid
[probe]
user = nobody
group = nogroup
listen = 127.0.0.1:{address}
pm = static
pm.max_children = 2
catch_workers_output = yes
""")
    with service(["/usr/sbin/php-fpm8.2", "--nodaemonize", "--fpm-config", str(configuration)], "php-fpm") as process:
        def connectable():
            with socket.create_connection(("127.0.0.1", address), timeout=1):
                return True
        wait(process, connectable)
        for _ in range(8):
            with socket.create_connection(("127.0.0.1", address), timeout=10) as connection:
                params = dict(SCRIPT_FILENAME=str(script), REQUEST_METHOD="POST", CONTENT_TYPE="application/octet-stream",
                              CONTENT_LENGTH=str(len(PAYLOAD)), SERVER_PROTOCOL="HTTP/1.1", GATEWAY_INTERFACE="CGI/1.1",
                              SERVER_NAME="localhost", SERVER_PORT=str(address), QUERY_STRING="")
                frame = fcgi_record(1, struct.pack("!HB5x", 1, 0))
                frame += fcgi_record(4, b"".join(fcgi_pair(k, v) for k, v in params.items())) + fcgi_record(4, b"")
                frame += fcgi_record(5, PAYLOAD) + fcgi_record(5, b"")
                connection.sendall(frame)
                stream = connection.makefile("rb")
                output = b""
                while True:
                    header = stream.read(8)
                    assert len(header) == 8, "truncated FastCGI record"
                    version, kind, request_id, length, padding, _ = struct.unpack("!BBHHBB", header)
                    assert version == 1 and request_id == 1
                    body = stream.read(length)
                    assert len(body) == length
                    stream.read(padding)
                    if kind == 6:
                        output += body
                    elif kind == 3:
                        assert struct.unpack("!IB3x", body) == (0, 0)
                        break
                stream.close()
                assert b"FPM_OK_" + hashlib.sha256(PAYLOAD).hexdigest().encode() in output, output
    print("PHP_FPM_WORKERS_FASTCGI_BINARY_POST_OK")


def memcached():
    address = port()
    with service(["/usr/bin/memcached", "-u", "root", "-l", "127.0.0.1", "-p", str(address), "-U", "0", "-t", "4"], "memcached") as process:
        def connectable():
            with socket.create_connection(("127.0.0.1", address), timeout=1):
                return True
        wait(process, connectable)
        with socket.create_connection(("127.0.0.1", address), timeout=10) as connection:
            stream = connection.makefile("rb")
            connection.sendall(b"set binary 0 0 %d\r\n" % len(PAYLOAD) + PAYLOAD + b"\r\n")
            assert stream.readline() == b"STORED\r\n"
            connection.sendall(b"get binary\r\n")
            assert stream.readline() == b"VALUE binary 0 %d\r\n" % len(PAYLOAD)
            assert stream.read(len(PAYLOAD) + 2) == PAYLOAD + b"\r\n"
            assert stream.readline() == b"END\r\n"
            connection.sendall(b"set counter 0 0 2\r\n40\r\nincr counter 2\r\ndelete binary\r\nget binary\r\n")
            assert [stream.readline() for _ in range(4)] == [b"STORED\r\n", b"42\r\n", b"DELETED\r\n", b"END\r\n"]
            connection.sendall(b"set ttl 0 1 1\r\nx\r\n")
            assert stream.readline() == b"STORED\r\n"
            time.sleep(2)
            connection.sendall(b"get ttl\r\n")
            assert stream.readline() == b"END\r\n"
            stream.close()
    print("MEMCACHED_BINARY_COUNTER_DELETE_EXPIRY_OK")


def caddy():
    address = port()
    (ROOT / "payload.bin").write_bytes(PAYLOAD)
    configuration = ROOT / "Caddyfile"
    configuration.write_text(f"{{\nadmin off\nauto_https off\n}}\nhttp://127.0.0.1:{address} {{\nroot * {ROOT}\nfile_server\n}}\n")
    with service(["/usr/bin/caddy", "run", "--config", str(configuration), "--adapter", "caddyfile"], "caddy") as process:
        wait(process, lambda: request(address, "/payload.bin")[0] == 200)
        assert request(address, "/payload.bin")[2] == PAYLOAD
        status, _, body = request(address, "/payload.bin", headers={"Range": "bytes=7-42"})
        assert status == 206 and body == PAYLOAD[7:43]
        assert request(address, "/absent")[0] == 404
    print("CADDY_HTTP_BINARY_RANGE_NOT_FOUND_OK")


def openresty():
    address = port()
    configuration = ROOT / "nginx.conf"
    configuration.write_text(f"""worker_processes 2;
pid {ROOT}/openresty.pid;
error_log {ROOT}/openresty-error.log info;
events {{ worker_connections 64; }}
http {{
access_log off;
client_body_temp_path {ROOT}/body;
lua_shared_dict probe 1m;
server {{ listen 127.0.0.1:{address};
location / {{ content_by_lua_block {{
 local dict=ngx.shared.probe
 local count,err=dict:incr('count',1,0)
 if err then error(err) end
 ngx.say('OPENRESTY_LUA_', count)
}} }}
}} }}
""")
    with service(["/usr/local/openresty/nginx/sbin/nginx", "-p", str(ROOT) + "/", "-c", str(configuration), "-g", "daemon off;"], "openresty") as process:
        wait(process, lambda: request(address)[0] == 200)
        values = [int(request(address)[2].decode().strip().split("_")[-1]) for _ in range(12)]
        assert values == list(range(values[0], values[0] + 12)), values
        process.send_signal(signal.SIGHUP)
        time.sleep(0.5)
        assert int(request(address)[2].decode().strip().split("_")[-1]) == values[-1] + 1
    print("OPENRESTY_LUA_SHARED_DICTIONARY_RELOAD_OK")


def node_exporter():
    address = port()
    with service(["/usr/bin/prometheus-node-exporter", f"--web.listen-address=127.0.0.1:{address}"], "node-exporter") as process:
        wait(process, lambda: request(address, "/metrics")[0] == 200)
        status, _, body = request(address, "/metrics")
        assert status == 200
        assert b"node_cpu_seconds_total" in body and b"node_memory_MemTotal_bytes" in body
        failures = [line for line in body.decode().splitlines()
                    if line.startswith("node_scrape_collector_success{") and line.endswith(" 0")]
        assert not failures, failures
    print("NODE_EXPORTER_DEFAULT_COLLECTORS_OK")


def prometheus():
    address = port()
    config = ROOT / "prometheus.yml"
    config.write_text(f"global:\n  scrape_interval: 1s\nscrape_configs:\n  - job_name: self\n    static_configs:\n      - targets: ['127.0.0.1:{address}']\n")
    argv = ["/usr/bin/prometheus", "--config.file=" + str(config), "--storage.tsdb.path=" + str(ROOT / "tsdb"),
            "--web.listen-address=127.0.0.1:" + str(address), "--web.enable-lifecycle"]
    for iteration in range(2):
        with service(argv, "prometheus") as process:
            wait(process, lambda: request(address, "/-/ready")[0] == 200)
            deadline = time.monotonic() + 20
            while True:
                result = api(address, "/api/v1/query?query=up")
                if result["data"]["result"]:
                    assert result["data"]["result"][0]["value"][1] == "1", result
                    break
                assert time.monotonic() < deadline, result
                time.sleep(0.2)
            wait(process, lambda: any(target["health"] == "up" for target in
                 api(address, "/api/v1/targets")["data"]["activeTargets"]), timeout=20)
            if iteration == 0:
                assert request(address, "/-/reload", "POST")[0] == 200
    print("PROMETHEUS_SCRAPE_QUERY_WAL_RESTART_RELOAD_OK")


def influxdb():
    address = port()
    config = ROOT / "influxdb.conf"
    config.write_text(f'''[meta]
dir = "{ROOT}/meta"
[data]
dir = "{ROOT}/data"
wal-dir = "{ROOT}/wal"
[http]
bind-address = "127.0.0.1:{address}"
[reporting]
disabled = true
''')
    def query(sql):
        return api(address, "/query?q=" + urllib.parse.quote(sql))
    for iteration in range(2):
        with service(["/usr/bin/influxd", "-config", str(config)], "influxdb") as process:
            wait(process, lambda: request(address, "/ping")[0] == 204)
            if iteration == 0:
                query("CREATE DATABASE probe")
                assert request(address, "/write?db=probe", "POST", b"temperature,host=test value=42i\n")[0] == 204
            result = api(address, "/query?db=probe&q=" + urllib.parse.quote("SELECT value FROM temperature"))
            assert result["results"][0]["series"][0]["values"][0][1] == 42, result
    print("INFLUXDB_LINE_PROTOCOL_QUERY_RESTART_OK")


def php_application(name):
    paths = {"adminer": ("/usr/share/adminer", "/adminer.php", b"Adminer"),
             "phpmyadmin": ("/usr/share/phpmyadmin", "/index.php", b"phpMyAdmin")}
    document_root, path, expected = paths[name]
    address = port()
    with service(["/usr/bin/php", "-S", f"127.0.0.1:{address}", "-t", document_root], name, shutdown_codes=(-15,)) as process:
        wait(process, lambda: request(address, path)[0] == 200)
        status, _, body = request(address, path)
        assert status == 200 and expected in body and b"<form" in body, body[-4096:]
        assert b"Fatal error" not in body and b"Uncaught" not in body
    print(name.upper() + "_REAL_PHP_LOGIN_FORM_OK")


@contextmanager
def mariadb():
    data = ROOT / "mysql-data"
    address = ROOT / "mysql.sock"
    command(["/usr/bin/mariadb-install-db", "--no-defaults", "--datadir=" + str(data),
             "--auth-root-authentication-method=normal", "--skip-test-db"])
    argv = ["/usr/sbin/mariadbd", "--no-defaults", "--user=root", "--datadir=" + str(data),
            "--socket=" + str(address), "--pid-file=" + str(ROOT / "mysql.pid"), "--skip-networking",
            "--innodb-buffer-pool-size=64M"]
    def sql(value):
        return command(["/usr/bin/mariadb", "--no-defaults", "--socket=" + str(address), "-uroot", "-NBe", value], timeout=10)
    with service(argv, "wordpress-mariadb") as process:
        deadline = time.monotonic() + 45
        while True:
            assert process.poll() is None
            try:
                if sql("SELECT 42") == "42":
                    break
            except AssertionError:
                pass
            assert time.monotonic() < deadline
            time.sleep(0.1)
        yield address, sql


@contextmanager
def wordpress_server(address, web):
    backend = port()
    pool = ROOT / 'wordpress-fpm.conf'
    pool.write_text(f'''[global]
daemonize = no
error_log = {ROOT}/wordpress-fpm-error.log
pid = {ROOT}/wordpress-fpm.pid
[wordpress]
user = root
group = root
listen = 127.0.0.1:{backend}
pm = static
pm.max_children = 2
catch_workers_output = yes
''')
    configuration = ROOT / 'wordpress.Caddyfile'
    configuration.write_text(f'''http://127.0.0.1:{address} {{
    root * {web}
    php_fastcgi 127.0.0.1:{backend}
    file_server
}}
''')
    with service(['/usr/sbin/php-fpm8.2', '--nodaemonize', '--allow-to-run-as-root',
                  '--fpm-config', str(pool)], 'wordpress-fpm') as backend_process:
        def ready():
            with socket.create_connection(('127.0.0.1', backend), timeout=1):
                return True
        wait(backend_process, ready)
        with service(['/usr/bin/caddy', 'run', '--config', str(configuration), '--adapter', 'caddyfile'],
                     'wordpress', env=dict(os.environ, HOME=str(ROOT))) as process:
            yield process


def wordpress():
    address = port()
    with mariadb() as (mysql_socket, sql):
        sql("CREATE DATABASE wordpress; CREATE USER 'wordpress'@'localhost' IDENTIFIED BY 'panel-test'; "
            "GRANT ALL ON wordpress.* TO 'wordpress'@'localhost'")
        # Keep the packaged source immutable; the application's wp-config is
        # owned by this disposable test workspace.
        import shutil
        web = ROOT / "wordpress"
        shutil.copytree("/usr/share/wordpress", web, symlinks=True)
        (web / "wp-config.php").write_text(f'''<?php
define('DB_NAME','wordpress'); define('DB_USER','wordpress'); define('DB_PASSWORD','panel-test');
define('DB_HOST','localhost:{mysql_socket}'); define('DB_CHARSET','utf8'); define('DB_COLLATE','');
$table_prefix='wp_'; define('WP_DEBUG',false); define('WP_HTTP_BLOCK_EXTERNAL',true);
if(!defined('ABSPATH')) define('ABSPATH',__DIR__.'/');
require_once ABSPATH.'wp-settings.php';
''')
        with wordpress_server(address, web) as process:
            wait(process, lambda: request(address, "/wp-admin/install.php")[0] == 200)
            form = urllib.parse.urlencode(dict(weblog_title="Kinakaze Test", user_name="probe", admin_password="panel-test-12345",
                admin_password2="panel-test-12345", admin_email="probe@example.invalid", blog_public="0", Submit="Install WordPress")).encode()
            status, _, body = request(address, "/wp-admin/install.php?step=2", "POST", form,
                                      {"Content-Type": "application/x-www-form-urlencoded"}, timeout=120)
            assert status == 200 and b"Success!" in body, (status, body[-4096:])
            assert sql("SELECT COUNT(*) FROM wordpress.wp_users") == "1"
            status, _, body = request(address)
            assert status == 200 and b"Kinakaze Test" in body, (status, body[-4096:])
        with wordpress_server(address, web) as process:
            wait(process, lambda: request(address)[0] == 200)
            assert b"Kinakaze Test" in request(address)[2]
    print("WORDPRESS_INSTALL_REAL_DATABASE_PAGE_RESTART_OK")


def tomcat():
    import shutil
    address = port()
    base = ROOT / "tomcat"
    (base / "conf").mkdir(parents=True)
    for path in Path('/etc/tomcat10').glob('*'):
        if path.is_file():
            shutil.copyfile(path, base / 'conf' / path.name)
    for name in ("logs", "temp", "work", "webapps/ROOT"):
        (base / name).mkdir(parents=True)
    (base / "conf/server.xml").write_text(f'''<Server port="-1"><Service name="Catalina">
<Connector address="127.0.0.1" port="{address}" protocol="HTTP/1.1" />
<Engine name="Catalina" defaultHost="localhost"><Host name="localhost" appBase="webapps" /></Engine>
</Service></Server>''')
    (base / "webapps/ROOT/index.jsp").write_text('<%@ page contentType="text/plain" %>TOMCAT_JSP_<%= 6*7 %>')
    env = dict(os.environ, JAVA_HOME="/usr/lib/jvm/java-17-openjdk-amd64", CATALINA_HOME="/usr/share/tomcat10", CATALINA_BASE=str(base))
    # HotSpot runs shutdown hooks and then exits with 128 + SIGTERM.
    with service(["/bin/bash", "/usr/share/tomcat10/bin/catalina.sh", "run"], "tomcat", env=env, shutdown_codes=(143,)) as process:
        wait(process, lambda: request(address, "/index.jsp")[0] == 200, timeout=90)
        assert request(address, "/index.jsp")[2].strip() == b"TOMCAT_JSP_42"
        assert request(address, "/missing")[0] == 404
    print("TOMCAT_HTTP_JSP_COMPILATION_OK")


def filebrowser():
    binary = "/opt/1panel-apps/filebrowser/filebrowser"
    database = str(ROOT / "filebrowser.db")
    files = ROOT / "files"
    files.mkdir()
    command([binary, "config", "init", "-d", database])
    command([binary, "users", "add", "probe", "panel-test-12345", "--perm.admin", "-d", database])
    address = port()
    for iteration in range(2):
        with service([binary, "-d", database, "-r", str(files), "-a", "127.0.0.1", "-p", str(address)], "filebrowser", shutdown_codes=(0, 1)) as process:
            wait(process, lambda: request(address)[0] == 200)
            status, _, token = request(address, "/api/login", "POST", json.dumps(dict(username="probe", password="panel-test-12345")),
                                       {"Content-Type": "application/json"})
            assert status == 200, (status, token)
            headers = {"X-Auth": token.decode()}
            assert request(address, "/api/resources/")[0] in (401, 403)
            if iteration == 0:
                assert request(address, "/api/resources/payload.bin", "POST", PAYLOAD, headers)[0] in (200, 201)
            assert request(address, "/api/raw/payload.bin", headers=headers)[2] == PAYLOAD
            assert "payload.bin" in str(api(address, "/api/resources/", headers=headers))
    assert (files / "payload.bin").read_bytes() == PAYLOAD
    print("FILEBROWSER_AUTH_UPLOAD_DOWNLOAD_RESTART_OK")


def grafana():
    address = port()
    config = ROOT / "grafana.ini"
    config.write_text(f"""[paths]
data = {ROOT}/grafana-data
logs = {ROOT}/grafana-logs
plugins = {ROOT}/grafana-plugins
provisioning = {ROOT}/grafana-provisioning
[server]
http_addr = 127.0.0.1
http_port = {address}
[security]
admin_user = probe
admin_password = panel-test-12345
[analytics]
reporting_enabled = false
check_for_updates = false
check_for_plugin_updates = false
""")
    credentials = {"Authorization": "Basic " + base64.b64encode(b"probe:panel-test-12345").decode()}
    for iteration in range(2):
        with service(["/usr/share/grafana/bin/grafana", "server", "--homepath", "/usr/share/grafana", "--config", str(config)], "grafana") as process:
            wait(process, lambda: request(address, "/api/health")[0] == 200, timeout=90)
            assert api(address, "/api/health")["database"] == "ok"
            if iteration == 0:
                result = api(address, "/api/dashboards/db", "POST", {"dashboard": {"uid": "kinakaze", "title": "Kinakaze", "panels": []}}, credentials)
                assert result["status"] == "success"
            assert api(address, "/api/dashboards/uid/kinakaze", headers=credentials)["dashboard"]["title"] == "Kinakaze"
    print("GRAFANA_AUTH_DASHBOARD_DATABASE_RESTART_OK")


def python_runtime():
    import concurrent.futures
    import sqlite3
    import ssl
    with sqlite3.connect(ROOT / "python.sqlite") as db:
        db.execute("CREATE TABLE t(v)")
        db.execute("INSERT INTO t VALUES(42)")
        assert db.execute("SELECT v FROM t").fetchone() == (42,)
    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        assert sum(pool.map(lambda n: n * n, range(100))) == 328350
    assert command(["/bin/echo", "child"]) == "child"
    assert ssl.OPENSSL_VERSION
    print("PYTHON_SQLITE_THREADS_CHILD_OK")


def rabbitmq():
    address, management = port(), port()
    config = ROOT / "rabbitmq.conf"
    config.write_text(f"listeners.tcp.1 = 127.0.0.1:{address}\nmanagement.tcp.ip = 127.0.0.1\nmanagement.tcp.port = {management}\nloopback_users.guest = true\n")
    plugins = ROOT / "enabled_plugins"
    plugins.write_text("[rabbitmq_management].\n")
    env = dict(os.environ, HOME=str(ROOT), RABBITMQ_ALLOW_INPUT="", RABBITMQ_CONFIG_FILE=str(config),
               RABBITMQ_ENABLED_PLUGINS_FILE=str(plugins), RABBITMQ_MNESIA_BASE=str(ROOT / "mnesia"),
               RABBITMQ_LOG_BASE=str(ROOT / "rabbit-logs"), RABBITMQ_PID_FILE=str(ROOT / "rabbit.pid"),
               RABBITMQ_NODENAME="panel" + uuid.uuid4().hex[:8] + "@localhost", RABBITMQ_NODE_PORT=str(address),
               RABBITMQ_DIST_PORT=str(port()),
               RABBITMQ_SERVER_ADDITIONAL_ERL_ARGS="+S 2:2 +A 4")
    credentials = {"Authorization": "Basic " + base64.b64encode(b"guest:guest").decode()}
    for iteration in range(2):
        with service(["/usr/lib/rabbitmq/bin/rabbitmq-server"], "rabbitmq", env=env) as process:
            wait(process, lambda: request(management, "/api/overview", headers=credentials)[0] == 200, timeout=100)
            if iteration == 0:
                api(management, "/api/queues/%2F/probe", "PUT", dict(durable=True, auto_delete=False, arguments={}), credentials)
                result = api(management, "/api/exchanges/%2F/amq.default/publish", "POST",
                             dict(properties={"delivery_mode": 2}, routing_key="probe", payload="message-42", payload_encoding="string"), credentials)
                assert result["routed"] is True
            else:
                result = api(management, "/api/queues/%2F/probe/get", "POST",
                             dict(count=1, ackmode="ack_requeue_false", encoding="auto", truncate=100), credentials)
                assert len(result) == 1 and result[0]["payload"] == "message-42", result
    print("RABBITMQ_DURABLE_QUEUE_PUBLISH_RESTART_CONSUME_OK")


def main():
    from PanelAppsExtraProbe import PROBES
    from PanelAppsInstalledProbe import PROBES as INSTALLED_PROBES
    probes = {"php8": php, "php-fpm": php_fpm, "memcached": memcached, "caddy": caddy,
              "openresty": openresty, "node-exporter": node_exporter, "prometheus": prometheus,
              "influxdb": influxdb, "wordpress": wordpress, "tomcat": tomcat,
              "filebrowser": filebrowser, "grafana": grafana, "python": python_runtime, "rabbitmq": rabbitmq,
              "adminer": lambda: php_application("adminer"), "phpmyadmin": lambda: php_application("phpmyadmin")}
    probes.update(PROBES)
    probes.update(INSTALLED_PROBES)
    name = sys.argv[1]
    if name not in probes:
        print("PANEL_SCENARIO_MISSING " + name, flush=True)
        return 126
    probes[name]()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
