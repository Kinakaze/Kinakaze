"""Prepare upstream/OCI test inputs inside a previously cloned disposable root.

Uses the application's archived links and exact Oracle MySQL packages. All
guest mutation runs through an owned worker process tree, with saved output.
"""
import argparse
import json
import os
from pathlib import Path
import runpy
import shlex
import shutil
import tomllib
import uuid


PACKAGES = "memcached caddy prometheus prometheus-node-exporter php8.2-cli php8.2-fpm php8.2-sqlite3 php8.2-mysql php8.2-curl php8.2-mbstring php8.2-xml php8.2-zip php8.2-gd php8.2-intl adminer phpmyadmin wordpress influxdb rabbitmq-server golang-go nodejs tomcat10 git clickhouse-server clickhouse-client"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("root", "dist", "cache", "output"):
        parser.add_argument("--" + key, required=True, type=Path)
    parser.add_argument("--debian", action="store_true", help="install the declared Debian packages")
    parser.add_argument("--mysql-cache", type=Path)
    parser.add_argument("--astrbot", action="store_true", help="install pinned AstrBot source dependencies using Python 3.12")
    args = parser.parse_args()
    root, dist, cache, output = (p.resolve() for p in (args.root, args.dist, args.cache, args.output))
    workspace = Path(__file__).resolve().parents[1]
    if not root.is_relative_to(workspace / "artifacts") or not (root / "var/lib/dpkg/status").is_file():
        parser.error("use a cloned disposable Debian root under artifacts")
    commands = ["set -euo pipefail", "export DEBIAN_FRONTEND=noninteractive"]
    if args.debian:
        commands.extend(["apt-get update", "apt-get install -y --no-install-recommends " + PACKAGES])
    metadata = cache / "artifacts.json"
    lock = json.loads(metadata.read_text(encoding="utf-8")) if metadata.exists() else {}
    # Do not glob every .deb in /tmp: install only the declared saved inputs.
    for name, row in lock.items():
        for path in row.get("executables", []):
            commands.append("chmod 755 " + shlex.quote("/" + path))
        if row["kind"] == "deb":
            commands.append("dpkg-deb -x " + shlex.quote("/tmp/" + name + ".deb") + " /")
        for path, item in row.get("links", {}).items():
            source = item["target"]
            commands.append("mkdir -p " + shlex.quote("/" + str(Path(path).parent).replace("\\", "/")))
            if item["hard"]:
                source = "/opt/1panel-apps/" + name + "/" + "/".join(Path(source).parts[1:])
            commands.append("ln " + ("-f" if item["hard"] else "-sf") + " -- " +
                            shlex.quote(source) + " " + shlex.quote("/" + path))
    # Executable modes are not retained by a Windows tar extraction. Directory
    # traversal stays readable; chmod executable payloads used by this suite.
    prefixes = " ".join(shlex.quote("/opt/1panel-apps/" + name) for name in lock)
    if prefixes:
        commands.append("chmod -R a+rX " + prefixes)
    for name, row in lock.items():
        if row["kind"].startswith("binary:") and not row["kind"].endswith((".jar", ".war")):
            commands.append("chmod 755 " + shlex.quote("/opt/1panel-apps/" + name + "/" + row["kind"].split(":", 1)[1]))
    if prefixes:
        commands.append("find " + prefixes + " -type f \\( -name filebrowser -o -name mongod -o -name mongos -o -name node -o -name code-server -o -name '*.node' -o -name alist -o -name openlist -o -name cloudreve -o -name ntfy -o -name AdGuardHome -o -name frpc -o -name frps -o -name valkey-server -o -name keydb-server -o -name minio -o -name registry -o -name portainer \\) -exec chmod 755 {} +")
    if args.astrbot:
        application = root / "opt/1panel-apps/astrbot"
        project = tomllib.loads((application / "pyproject.toml").read_text(encoding="utf-8"))
        (application / "kinakaze-requirements.txt").write_text(
            "\n".join(project["project"]["dependencies"]) + "\n", encoding="utf-8")
        commands.append("/opt/1panel-apps/python312/bin/python3.12 -m pip install --disable-pip-version-check --report /opt/1panel-apps/astrbot/kinakaze-install-report.json -r /opt/1panel-apps/astrbot/kinakaze-requirements.txt")
    if args.mysql_cache:
        spec = runpy.run_path(str(workspace / "tools/test-oracle-mysql.py"))
        import hashlib
        commands.append("mkdir -p /opt/1panel-apps/mysql")
        for name, (size, digest) in spec["PACKAGES"].items():
            version = "0.996-14+b14" if name == "libmecab2" else spec["VERSION"]
            filename = f"{name}_{version}_amd64.deb"
            package = args.mysql_cache / filename
            if package.stat().st_size != size or hashlib.sha256(package.read_bytes()).hexdigest() != digest:
                raise ValueError("Oracle MySQL package hash mismatch: " + filename)
            shutil.copyfile(package, root / "tmp" / filename)
            commands.append("dpkg-deb -x " + shlex.quote("/tmp/" + filename) + " /opt/1panel-apps/mysql")
    execute = runpy.run_path(str(workspace / "tools/test-debian-commands.py"))["execute"]
    guest_script = "/tmp/panel-provision-" + uuid.uuid4().hex + ".sh"
    (root / guest_script.lstrip("/")).write_text("\n".join(commands) + "\n", encoding="utf-8", newline="\n")
    result = execute([str(dist / "worker.exe"), "oneshot", "--root", str(root), "--dist", str(dist),
                      "--", "/bin/bash", guest_script], output, os.environ.copy(), timeout=3600 if args.debian or args.astrbot else 180)
    (output / "results.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(dict(exit_code=result["exit_code"], timed_out=result["timed_out"], seconds=result["seconds"])))
    return int(result["exit_code"] != 0 or result["timed_out"])


if __name__ == "__main__":
    raise SystemExit(main())
