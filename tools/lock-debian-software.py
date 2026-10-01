"""Freeze 500 executable Debian packages in official popcon vote order.

Inputs are retained official popcon, Contents and APT index snapshots. A package
qualifies only if Contents lists an entry in /bin, /sbin, /usr/bin or /usr/sbin
and the chosen Debian suite supplies an amd64/all version. Dependencies do not
occupy additional slots unless they themselves ship command entries.
"""

import argparse
from datetime import datetime, timezone
import gzip
import hashlib
import json
from pathlib import Path
import re


def package_index(directory):
    result = {}
    snapshots = []
    for path in sorted(directory.glob("*_Packages")):
        data = path.read_bytes()
        snapshots.append(dict(file=path.name, sha256=hashlib.sha256(data).hexdigest()))
        for record in data.decode("utf-8").replace("\r\n", "\n").split("\n\n"):
            fields = dict(re.findall(r"^([^ :\n]+): (.*)$", record, re.M))
            if fields.get("Architecture") not in ("all", "amd64"):
                continue
            if "Package" in fields:
                result[fields["Package"]] = fields
    return result, snapshots


def command_owners(data):
    result = {}
    for line in gzip.decompress(data).decode("utf-8").splitlines():
        fields = line.split()
        if len(fields) != 2:
            continue
        path, owners = fields
        # These conventional command-directory aliases are directories, not tools.
        if path in ("usr/bin/X11", "usr/bin/mh"):
            continue
        if path.rpartition("/")[0] not in ("bin", "sbin", "usr/bin", "usr/sbin"):
            continue
        for owner in owners.split(","):
            result.setdefault(owner.rsplit("/", 1)[-1], set()).add("/" + path)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--popcon", type=Path, required=True)
    parser.add_argument(
        "--contents",
        type=Path,
        action="append",
        required=True,
        help="repeat for Contents-amd64.gz and Contents-all.gz",
    )
    parser.add_argument("--apt-lists", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--count", type=int, default=500)
    parser.add_argument("--suite", default="bookworm")
    args = parser.parse_args()
    if args.count < 1:
        parser.error("--count must be positive")
    index, snapshots = package_index(args.apt_lists)
    popcon = args.popcon.read_bytes()
    owners, contents_sources = {}, []
    for path in args.contents:
        contents = path.read_bytes()
        for name, commands in command_owners(contents).items():
            owners.setdefault(name, set()).update(commands)
        contents_sources.append(
            dict(
                url=f"https://deb.debian.org/debian/dists/{args.suite}/main/{path.name}",
                sha256=hashlib.sha256(contents).hexdigest(),
            )
        )
    packages, seen = [], set()
    for line in popcon.decode("utf-8").splitlines():
        columns = line.split()
        if len(columns) < 7 or not columns[0].isdigit():
            continue
        name = columns[1]
        if name in seen or name not in owners or name not in index:
            continue
        fields = index[name]
        packages.append(
            dict(
                order=len(packages) + 1,
                popularity_rank=int(columns[0]),
                votes=int(columns[3]),
                package=name,
                version=fields["Version"],
                architecture=fields["Architecture"],
                filename=fields["Filename"],
                size=int(fields["Size"]),
                sha256=fields["SHA256"],
                commands=sorted(owners[name]),
            )
        )
        seen.add(name)
        if len(packages) == args.count:
            break
    if len(packages) != args.count:
        parser.error(
            f"only {len(packages)} qualifying packages; provide a longer popcon snapshot"
        )
    lock = dict(
        schema=1,
        suite=args.suite,
        architecture="amd64",
        created=datetime.now(timezone.utc).isoformat(),
        selection="Official popcon vote order, filtered to packages with command entries "
        "in Contents and amd64/all versions in the selected APT indexes.",
        sources=dict(
            popcon=dict(
                url="https://popcon.debian.org/by_vote",
                snapshot_sha256=hashlib.sha256(popcon).hexdigest(),
                snapshot_bytes=len(popcon),
                snapshot_scope="Provided snapshot, including a sufficient prefix of the ranking when supplied.",
                header=[
                    line
                    for line in popcon.decode().splitlines()
                    if line.startswith("#")
                ][:8],
            ),
            contents=contents_sources,
            apt_indexes=snapshots,
        ),
        packages=packages,
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(lock, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(
        f"Locked {len(packages)} packages; last popcon rank {packages[-1]['popularity_rank']}"
    )


if __name__ == "__main__":
    main()
