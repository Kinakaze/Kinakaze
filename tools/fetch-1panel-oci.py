"""Extract pinned application binaries from OCI layers without running Docker.

Only the application's declared binaries are extracted. Image/layer digests
are verified and saved; testing these binaries does not test the image runtime.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
import urllib.parse
import urllib.request


class RegistryRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        redirected = super().redirect_request(request, response, code, message, headers, new_url)
        if redirected and urllib.parse.urlparse(request.full_url).netloc != urllib.parse.urlparse(new_url).netloc:
            redirected.remove_header("Authorization")
        return redirected


IMAGES = {
    "minio": ("minio/minio", "RELEASE.2025-04-22T22-12-26Z", {"usr/bin/minio": "minio"}),
    "valkey": ("valkey/valkey", "7.2.8", {"usr/local/bin/valkey-server": "bin/valkey-server"}),
    "keydb": ("eqalpha/keydb", "x86_64_v6.3.4", {"usr/local/bin/keydb-server": "bin/keydb-server", "usr/bin/keydb-server": "bin/keydb-server",
        "usr/lib/x86_64-linux-gnu/libssl.so.1.1": "lib/libssl.so.1.1", "usr/lib/x86_64-linux-gnu/libcrypto.so.1.1": "lib/libcrypto.so.1.1"}),
    "docker-registry": ("library/registry", "2.8.3", {"bin/registry": "registry"}),
    "portainer-ce": ("portainer/portainer-ce", "2.27.6", {"portainer": "portainer"}),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("root", "cache"):
        parser.add_argument("--" + key, required=True, type=Path)
    parser.add_argument("--only", nargs="+", choices=IMAGES)
    args = parser.parse_args()
    root, cache = args.root.resolve(), args.cache.resolve()
    if not root.is_relative_to(Path(__file__).resolve().parents[1] / "artifacts"):
        parser.error("use a disposable root under artifacts")
    cache.mkdir(parents=True, exist_ok=True)
    opener = urllib.request.build_opener(RegistryRedirect())
    lock_path = cache / "images.json"
    previous = {row['name']: row for row in json.loads(lock_path.read_text(encoding='utf-8'))} if lock_path.exists() else {}
    results = dict(previous)
    media = ", ".join(("application/vnd.oci.image.index.v1+json", "application/vnd.docker.distribution.manifest.list.v2+json",
                      "application/vnd.oci.image.manifest.v1+json", "application/vnd.docker.distribution.manifest.v2+json"))
    for name in args.only or IMAGES:
        repository, tag, files = IMAGES[name]
        registry_host = "registry-1.docker.io"
        row = dict(name=name, repository=repository, tag=tag, files={}, layers=[], status="failed")
        try:
            auth = ("https://auth.docker.io/token?service=registry.docker.io&scope=" +
                    urllib.parse.quote("repository:" + repository + ":pull"))
            token = json.load(urllib.request.urlopen(auth, timeout=30))["token"]
            headers = {"Authorization": "Bearer " + token, "Accept": media}
            def manifest(reference):
                url = "https://" + registry_host + "/v2/" + repository + "/manifests/" + reference
                with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=45) as response:
                    data = response.read()
                digest = "sha256:" + hashlib.sha256(data).hexdigest()
                if reference.startswith("sha256:") and digest != reference:
                    raise ValueError("manifest digest mismatch")
                return json.loads(data), digest
            index, row["tag_digest"] = manifest(tag)
            if "manifests" in index:
                reference = next(item["digest"] for item in index["manifests"]
                                 if item.get("platform", {}).get("os") == "linux" and item["platform"].get("architecture") == "amd64")
                index, row["manifest_digest"] = manifest(reference)
            else:
                row["manifest_digest"] = row["tag_digest"]
            saved = previous.get(name, {})
            if saved.get('repository') == repository and saved.get('tag') == tag and saved.get('manifest_digest') not in (None, row['manifest_digest']):
                raise ValueError('tag manifest differs from recorded input')
            destination = root / "opt/1panel-apps" / name
            destination.mkdir(parents=True, exist_ok=True)
            for layer in index["layers"]:
                digest = layer["digest"]
                path = cache / (digest.split(":", 1)[1] + ".tar.gz")
                if not path.is_file():
                    url = "https://" + registry_host + "/v2/" + repository + "/blobs/" + digest
                    with opener.open(urllib.request.Request(url, headers=headers), timeout=60) as response, path.with_suffix(".part").open("wb") as output:
                        shutil.copyfileobj(response, output, 1024 * 1024)
                    path.with_suffix(".part").replace(path)
                with path.open("rb") as stream:
                    actual = "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest()
                if actual != digest or path.stat().st_size != layer["size"]:
                    raise ValueError("layer digest/size mismatch")
                row["layers"].append(digest)
                with tarfile.open(path) as archive:
                    for member in archive:
                        candidate = member.name.removeprefix("./").lstrip("/")
                        if candidate in files and member.isfile():
                            target = destination / files[candidate]
                            target.parent.mkdir(parents=True, exist_ok=True)
                            with archive.extractfile(member) as source, target.open("wb") as output:
                                shutil.copyfileobj(source, output)
                            row["files"][files[candidate]] = hashlib.sha256(target.read_bytes()).hexdigest()
            if not row["files"]:
                raise ValueError("declared application binary absent from image")
            row["status"] = "fetched"
        except Exception as error:
            row["error"] = str(error)
        results[name] = row
        lock_path.write_text(json.dumps(list(results.values()), indent=2), encoding="utf-8")
        print(json.dumps({key: row[key] for key in ("name", "status", "error") if key in row}), flush=True)
    return int(any(results[name]["status"] != "fetched" for name in args.only or IMAGES))


if __name__ == "__main__":
    raise SystemExit(main())
