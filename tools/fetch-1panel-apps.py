"""Fetch pinned upstream application artifacts into a disposable test root.

Record URL, size and SHA-256 for every input; --offline verifies the saved lock.
This is application provisioning, not a Docker image installer.
"""

import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
import urllib.request
import zipfile


ARTIFACTS = {
    "astrbot": ("4.28.2", "https://github.com/AstrBotDevs/AstrBot/archive/3c7adafa1397e182d60b1016bf88759265113c8a.tar.gz", "tar-strip"),
    "astrbot-dashboard": ("4.28.2", "https://github.com/AstrBotDevs/AstrBot/releases/download/v4.28.2/AstrBot-v4.28.2-dashboard.zip", "zip"),
    "python312": ("3.12.11+20250918", "https://github.com/astral-sh/python-build-standalone/releases/download/20250918/cpython-3.12.11%2B20250918-x86_64-unknown-linux-gnu-install_only.tar.gz", "tar-strip"),
    "filebrowser": ("2.32.0", "https://github.com/filebrowser/filebrowser/releases/download/v2.32.0/linux-amd64-filebrowser.tar.gz", "tar"),
    "gitea": ("1.24.3", "https://dl.gitea.com/gitea/1.24.3/gitea-1.24.3-linux-amd64", "binary:gitea"),
    "mongodb": ("7.0.14", "https://fastdl.mongodb.org/linux/mongodb-linux-x86_64-debian12-7.0.14.tgz", "tar-strip"),
    "minio": ("2025-04-22", "https://dl.min.io/server/minio/release/linux-amd64/archive/minio.RELEASE.2025-04-22T22-12-26Z", "binary:minio"),
    "grafana": ("12.0.2", "https://dl.grafana.com/oss/release/grafana_12.0.2_amd64.deb", "deb"),
    "jenkins": ("2.504.3", "https://get.jenkins.io/war-stable/2.504.3/jenkins.war", "binary:jenkins.war"),
    "halo": ("2.20.19", "https://github.com/halo-dev/halo/releases/download/v2.20.19/halo-2.20.19.jar", "binary:halo.jar"),
    "meilisearch": ("1.15.2", "https://github.com/meilisearch/meilisearch/releases/download/v1.15.2/meilisearch-linux-amd64", "binary:meilisearch"),
    "frp": ("0.62.1", "https://github.com/fatedier/frp/releases/download/v0.62.1/frp_0.62.1_linux_amd64.tar.gz", "tar-strip"),
    "adguardhome": ("0.107.62", "https://github.com/AdguardTeam/AdGuardHome/releases/download/v0.107.62/AdGuardHome_linux_amd64.tar.gz", "tar-strip"),
    "ntfy": ("2.13.0", "https://github.com/binwiederhier/ntfy/releases/download/v2.13.0/ntfy_2.13.0_linux_amd64.tar.gz", "tar-strip"),
    "openresty": ("1.27.1.2-1~bookworm1", "https://openresty.org/package/debian/pool/openresty/o/openresty/openresty_1.27.1.2-1~bookworm1_amd64.deb", "deb"),
    "openresty-openssl3": ("3.4.1-1~bookworm1", "https://openresty.org/package/debian/pool/openresty/o/openresty-openssl3/openresty-openssl3_3.4.1-1~bookworm1_amd64.deb", "deb"),
    "openresty-pcre2": ("10.44-2~bookworm1", "https://openresty.org/package/debian/pool/openresty/o/openresty-pcre2/openresty-pcre2_10.44-2~bookworm1_amd64.deb", "deb"),
    "openresty-zlib": ("1.3.1-1~bookworm1", "https://openresty.org/package/debian/pool/openresty/o/openresty-zlib/openresty-zlib_1.3.1-1~bookworm1_amd64.deb", "deb"),
    "code-server": ("4.101.2", "https://github.com/coder/code-server/releases/download/v4.101.2/code-server-4.101.2-linux-amd64.tar.gz", "tar-strip"),
    "elasticsearch": ("7.17.28", "https://artifacts.elastic.co/downloads/elasticsearch/elasticsearch-7.17.28-linux-x86_64.tar.gz", "tar-strip"),
    "alist": ("3.41.0", "https://github.com/alist-org/alist/releases/download/v3.41.0/alist-linux-amd64.tar.gz", "tar"),
    "openlist": ("4.0.3", "https://github.com/OpenListTeam/OpenList/releases/download/v4.0.3/openlist-linux-amd64.tar.gz", "tar"),
    "cloudreve": ("3.8.3", "https://github.com/cloudreve/Cloudreve/releases/download/3.8.3/cloudreve_3.8.3_linux_amd64.tar.gz", "tar"),
    "nextcloud": ("31.0.5", "https://download.nextcloud.com/server/releases/nextcloud-31.0.5.tar.bz2", "tar-strip"),
    "emqx": ("5.8.8", "https://github.com/emqx/emqx/releases/download/v5.8.8/emqx-5.8.8-debian12-amd64.tar.gz", "tar"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("root", "cache"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--only", nargs="+", choices=ARTIFACTS)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    workspace = Path(__file__).resolve().parents[1]
    root, cache = args.root.resolve(), args.cache.resolve()
    if not root.is_relative_to(workspace / "artifacts"):
        parser.error("use a disposable root under artifacts")
    cache.mkdir(parents=True, exist_ok=True)
    lock_path = cache / "artifacts.json"
    lock = json.loads(lock_path.read_text(encoding="utf-8")) if lock_path.exists() else {}

    def fetch(name):
        version, url, kind = ARTIFACTS[name]
        path = cache / (name + "-" + url.rsplit("/", 1)[-1])
        if not path.is_file():
            if args.offline:
                raise FileNotFoundError(path)
            temporary = path.with_suffix(path.suffix + ".part")
            with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "Kinakaze-Compatibility/1.0"}), timeout=60) as response:
                with temporary.open("wb") as output:
                    shutil.copyfileobj(response, output, 1024 * 1024)
            temporary.replace(path)
        with path.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        row = dict(version=version, url=url, kind=kind, filename=path.name, size=path.stat().st_size, sha256=digest)
        if name in lock and any(lock[name].get(key) != value for key, value in row.items()):
            raise ValueError("cached artifact differs from recorded input: " + name)
        if args.offline and name not in lock:
            raise ValueError("offline artifact has no recorded hash: " + name)
        destination = root / "opt/1panel-apps" / name
        destination.mkdir(parents=True, exist_ok=True)
        if kind.startswith("binary:"):
            shutil.copyfile(path, destination / kind.split(":", 1)[1])
        elif kind == "deb":
            shutil.copyfile(path, root / "tmp" / (name + ".deb"))
        elif kind == "zip":
            with zipfile.ZipFile(path) as archive:
                for member in archive.infolist():
                    target = (destination / member.filename).resolve()
                    if not target.is_relative_to(destination.resolve()):
                        raise ValueError("archive path leaves application prefix: " + member.filename)
                    if member.is_dir():
                        target.mkdir(parents=True, exist_ok=True)
                    else:
                        target.parent.mkdir(parents=True, exist_ok=True)
                        with archive.open(member) as source, target.open("wb") as output:
                            shutil.copyfileobj(source, output)
        else:
            with tarfile.open(path) as archive:
                for member in archive.getmembers():
                    parts = Path(member.name).parts
                    if kind == "tar-strip":
                        parts = parts[1:]
                    if not parts:
                        continue
                    target = destination.joinpath(*parts).resolve()
                    if not target.is_relative_to(destination.resolve()):
                        raise ValueError("archive path leaves application prefix: " + member.name)
                    if member.isdir():
                        target.mkdir(parents=True, exist_ok=True)
                    elif member.isfile():
                        target.parent.mkdir(parents=True, exist_ok=True)
                        with archive.extractfile(member) as source, target.open("wb") as output:
                            shutil.copyfileobj(source, output)
                        if member.mode & 0o111:
                            row.setdefault("executables", []).append(str(target.relative_to(root)).replace("\\", "/"))
                    elif member.issym() or member.islnk():
                        # Recreate links with guest ln, retaining their targets
                        # in evidence instead of interpreting Linux paths on Windows.
                        row.setdefault("links", {})[str(target.relative_to(root)).replace("\\", "/")] = dict(target=member.linkname, hard=member.islnk())
        return name, row

    failed = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        futures = {pool.submit(fetch, name): name for name in args.only or ARTIFACTS}
        for future in concurrent.futures.as_completed(futures):
            name = futures[future]
            try:
                _, row = future.result()
                lock[name] = row
                lock_path.write_text(json.dumps(lock, indent=2), encoding="utf-8")
                print(json.dumps(dict(name=name, status="fetched", sha256=row["sha256"], bytes=row["size"])), flush=True)
            except Exception as error:
                failed.append(dict(name=name, error=str(error)))
                print(json.dumps(dict(name=name, status="fetch_failed", error=str(error))), flush=True)
    (cache / "failures.json").write_text(json.dumps(failed, indent=2), encoding="utf-8")
    return int(bool(failed))


if __name__ == "__main__":
    raise SystemExit(main())
