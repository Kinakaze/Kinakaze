"""Test application workloads from the 1Panel store on Kinakaze.

The panel itself is never started. The application denominator includes
missing applications and missing scenarios. Debian/upstream binary workloads
are distinct from the store's Docker image and version; neither is equivalent
to container compatibility or complete application feature coverage.
"""

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shlex
import shutil
import subprocess
import uuid

from init_pool import distribution_hashes


APPLICATIONS = {
    "openresty": ("/usr/local/openresty/nginx/sbin/nginx",),
    "mysql": ("/opt/1panel-apps/mysql/usr/sbin/mysqld",),
    "mariadb": ("/usr/sbin/mariadbd",),
    "postgresql": ("/usr/lib/postgresql/15/bin/postgres",),
    "redis": ("/usr/bin/redis-server",),
    "valkey": ("/opt/1panel-apps/valkey/bin/valkey-server",),
    "keydb": ("/opt/1panel-apps/keydb/bin/keydb-server",),
    "memcached": ("/usr/bin/memcached",),
    "mongodb": ("/opt/1panel-apps/mongodb/bin/mongod",),
    "clickhouse": ("/usr/sbin/clickhouse-server",),
    "influxdb": ("/usr/bin/influxd",),
    "rabbitmq": ("/usr/sbin/rabbitmq-server",),
    "elasticsearch": ("/opt/1panel-apps/elasticsearch/bin/elasticsearch",),
    "meilisearch": ("/opt/1panel-apps/meilisearch/meilisearch",),
    "caddy": ("/usr/bin/caddy",),
    "php8": ("/usr/bin/php",),
    "php-fpm": ("/usr/sbin/php-fpm8.2",),
    "python": ("/usr/bin/python3",),
    "node": ("/usr/bin/node",),
    "go": ("/usr/bin/go",),
    "java": ("/usr/lib/jvm/java-17-openjdk-amd64/bin/java",),
    "tomcat": ("/usr/share/tomcat10/bin/catalina.sh",),
    "wordpress": ("/usr/share/wordpress/index.php",),
    "phpmyadmin": ("/usr/share/phpmyadmin/index.php",),
    "adminer": ("/usr/share/adminer/adminer.php",),
    "nextcloud": ("/opt/1panel-apps/nextcloud/occ",),
    "halo": ("/opt/1panel-apps/halo/halo.jar",),
    "gitea": ("/opt/1panel-apps/gitea/gitea",),
    "code-server": ("/opt/1panel-apps/code-server/bin/code-server",),
    "jenkins": ("/opt/1panel-apps/jenkins/jenkins.war",),
    "minio": ("/opt/1panel-apps/minio/minio",),
    "filebrowser": ("/opt/1panel-apps/filebrowser/filebrowser",),
    "alist": ("/opt/1panel-apps/alist/alist",),
    "openlist": ("/opt/1panel-apps/openlist/openlist",),
    "cloudreve": ("/opt/1panel-apps/cloudreve/cloudreve",),
    "prometheus": ("/usr/bin/prometheus",),
    "node-exporter": ("/usr/bin/prometheus-node-exporter",),
    "grafana": ("/usr/share/grafana/bin/grafana",),
    "uptime-kuma": ("/opt/1panel-apps/uptime-kuma/server/server.js",),
    "n8n": ("/opt/1panel-apps/n8n/node_modules/n8n/bin/n8n",),
    "ollama": ("/opt/1panel-apps/ollama/bin/ollama",),
    "docker-registry": ("/opt/1panel-apps/docker-registry/registry",),
    "portainer-ce": ("/opt/1panel-apps/portainer-ce/portainer",),
    "redis-commander": ("/opt/1panel-apps/redis-commander/node_modules/redis-commander/bin/redis-commander.js",),
    "mongo-express": ("/opt/1panel-apps/mongo-express/app.js",),
    "emqx": ("/opt/1panel-apps/emqx/bin/emqx",),
    "ntfy": ("/opt/1panel-apps/ntfy/ntfy",),
    "frpc": ("/opt/1panel-apps/frp/frpc",),
    "frps": ("/opt/1panel-apps/frp/frps",),
    "adguardhome": ("/opt/1panel-apps/adguardhome/AdGuardHome",),
    "astrbot": ("/opt/1panel-apps/astrbot/main.py", "/opt/1panel-apps/python312/bin/python3.12",
                "/opt/1panel-apps/astrbot-dashboard/dist/index.html"),
}

EXISTING = {
    "mariadb": "MariadbRuntimeProbe.py",
    "postgresql": "PostgresqlRuntimeProbe.py",
    "redis": "RedisRuntimeProbe.py",
    "valkey": "RedisRuntimeProbe.py",
    "keydb": "RedisRuntimeProbe.py",
    "node": "NodeRuntimeProbe.js",
    "java": "JavaRuntimeProbe.java",
    "go": "GoRuntimeProbe.go",
}


def store_inventory(store):
    commit = subprocess.check_output(
        ["git", "-C", str(store), "rev-parse", "HEAD"], text=True).strip()
    names = subprocess.check_output(
        ["git", "-C", str(store), "ls-tree", "-d", "--name-only", commit + ":apps"],
        text=True).splitlines()
    if not names:
        raise ValueError("official store contains no applications")
    applications = {name: APPLICATIONS.get(name) for name in sorted(names)}
    evidence = dict(url="https://github.com/1Panel-dev/appstore", commit=commit,
                    metadata_sha256={})
    for name in applications:
        metadata = subprocess.check_output(
            ["git", "-C", str(store), "show", f"{commit}:apps/{name}/data.yml"])
        evidence["metadata_sha256"][name] = hashlib.sha256(metadata).hexdigest()
    return applications, evidence


def update_coverage(report, repeat):
    applications = report["applications"]
    report["counts"] = dict(Counter(row["status"] for row in report["results"]))
    passed = {name for name in applications if all(
        any(row["name"] == name and row["round"] == iteration and row["status"] == "passed"
            for row in report["results"]) for iteration in range(repeat))}
    report["coverage"] = dict(total=len(applications), passed=len(passed),
                              percent=100 * len(passed) / len(applications),
                              unselected=len(applications) - len(report["selected"]))
    report["complete"] = len(passed) == len(applications)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("root", "dist", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--store", type=Path, help="official appstore checkout for source evidence")
    parser.add_argument("--only", help="regular expression matching application names")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    if args.repeat < 1 or args.timeout < 1:
        parser.error("repeat and timeout must be positive")
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    workspace = Path(__file__).resolve().parents[1]
    if not root.is_relative_to(workspace / "artifacts"):
        parser.error("use a disposable root under artifacts")
    if not (dist / "worker.exe").is_file():
        parser.error("distribution worker is missing")
    source = workspace / "tests/guest"
    output.mkdir(parents=True, exist_ok=True)
    execute = runpy.run_path(str(Path(__file__).with_name("test-debian-commands.py")))["execute"]
    applications = dict(APPLICATIONS)
    store_evidence = None
    if args.store:
        try:
            applications, store_evidence = store_inventory(args.store.resolve())
        except ValueError as error:
            parser.error(str(error))
    selected = [name for name in applications if not args.only or re.search(args.only, name)]
    if not selected:
        parser.error("no applications selected")
    report = dict(schema=2, scope=__doc__, root=str(root), dist=str(dist),
                  sha256=distribution_hashes(dist), applications=list(applications),
                  selected=selected, results=[], complete=False)
    if store_evidence:
        report["store"] = store_evidence
    package_status = root / "var/lib/dpkg/status"
    if package_status.is_file():
        shutil.copyfile(package_status, output / "dpkg-status")
        report["dpkg_status_sha256"] = hashlib.sha256(package_status.read_bytes()).hexdigest()
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(("KINAKAZE_", "OPENAI_", "ANTHROPIC_"))}
    environment.update(TERM="xterm-256color", LC_ALL="C.UTF-8", TZ="UTC0")

    def save():
        update_coverage(report, args.repeat)
        (output / "results.json").write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")

    save()
    for iteration in range(args.repeat):
        for name in selected:
            row = dict(name=name, round=iteration, scope="registered functional scenario")
            if applications[name] is None:
                row.update(status="missing_scenario", reason="no registered application workload")
                report["results"].append(row)
                save()
                print(json.dumps(row), flush=True)
                continue
            guest = "/tmp/panel-apps-" + uuid.uuid4().hex
            stage = root / guest.lstrip("/")
            stage.mkdir(parents=True)
            for filename in ("PanelAppsRuntimeProbe.py", "PanelAppsExtraProbe.py", "PanelAppsInstalledProbe.py", *EXISTING.values(), "OracleMysqlRuntimeProbe.py"):
                shutil.copyfile(source / filename, stage / filename)
            row["fixture_sha256"] = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                                     for path in sorted(stage.iterdir()) if path.is_file()}
            marker = "PANEL_APP_OK_" + name
            if name in EXISTING:
                fixture = EXISTING[name]
                script = ("node " + fixture if name == "node" else
                          "go run " + fixture if name == "go" else
                          "javac JavaRuntimeProbe.java; java -cp . JavaRuntimeProbe spawn" if name == "java" else
                          "python3 " + fixture)
                if name in ("valkey", "keydb"):
                    script = "export PANEL_REDIS_SERVER=" + APPLICATIONS[name][0] + "; " + script
            elif name == "mysql":
                script = ('export LD_LIBRARY_PATH=/opt/1panel-apps/mysql/usr/lib/x86_64-linux-gnu; '
                          'python3 OracleMysqlRuntimeProbe.py --prefix /opt/1panel-apps/mysql --log-path "$PWD/mysql.log"')
            else:
                script = "python3 PanelAppsRuntimeProbe.py " + shlex.quote(name)
            # Check paths inside the guest: manifest and VFS links are not
            # necessarily Windows links visible to pathlib on the host.
            required = "\n".join("test -e " + shlex.quote(path) +
                " || { printf 'PANEL_APPLICATION_MISSING %s\\n' " + shlex.quote(path) + "; exit 125; }"
                for path in APPLICATIONS[name])
            shell = ("set -euo pipefail\nexport PATH=/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin "
                     "HOME=/root\n" + required + "\n" + script + "\nprintf '%s\\n' " + shlex.quote(marker))
            try:
                row.update(execute([str(dist / "worker.exe"), "oneshot", "--root", str(root),
                    "--dist", str(dist), "--cwd", guest, "--", "/bin/bash", "-c", shell],
                    output / f"{name}-{iteration}", environment, timeout=args.timeout))
                row["status"] = ("timeout" if row["timed_out"] else
                    "missing_application" if row["exit_code"] == 125 and "PANEL_APPLICATION_MISSING " in row["stdout"] else
                    "missing_scenario" if row["exit_code"] == 126 and "PANEL_SCENARIO_MISSING " in row["stdout"] else
                    "passed" if row["exit_code"] == 0 and marker in row["stdout"] else "failed")
                row["fixture"] = guest
            except Exception as error:
                row.update(status="harness_error", error=str(error))
            report["results"].append(row)
            save()
            print(json.dumps({key: row[key] for key in ("name", "round", "status", "seconds") if key in row}), flush=True)
    return int(any(row["status"] != "passed" for row in report["results"]))


if __name__ == "__main__":
    raise SystemExit(main())
