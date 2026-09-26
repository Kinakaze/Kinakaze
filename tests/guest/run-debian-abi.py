"""Run the Debian ABI regression in a fresh, fully installed guest root."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import runpy
import sys
import tempfile

from elf_probe import build, WORKSPACE
from debian_rpc_fixture import RpcPeer


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--link-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    dist, output = args.dist.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    sys.path.insert(0, str(WORKSPACE / "tools"))
    execute = runpy.run_path(str(WORKSPACE / "tools/test-debian-commands.py"))[
        "execute"
    ]
    environment = {k: v for k, v in os.environ.items() if not k.startswith("KINAKAZE_")}
    environment.update(PATH="", HOME="/root", LC_ALL="C.UTF-8", TERM="xterm", TZ="UTC0")
    worker = str(dist / "worker.exe")
    manifest_hash = hashlib.sha256(
        (dist / "rootfs.manifest.json").read_bytes()
    ).hexdigest()
    location = output / "root-location.json"
    if location.exists():
        root = Path(json.loads(location.read_text())["root"])
    else:
        root = None
    marker = root / ".kinakaze-rootfs.sha256" if root is not None else None
    if (
        root is None
        or not marker.exists()
        or marker.read_text().strip() != manifest_hash
    ):
        root = Path(tempfile.mkdtemp(prefix="kinakaze debian abi ")) / "rootfs"
        location.write_text(json.dumps({"root": str(root)}) + "\n", encoding="utf-8")
    report = {}
    if not root.exists():
        report["setup"] = execute(
            [worker, "setup", "--root", str(root), "--dist", str(dist)],
            output,
            environment,
            timeout=180,
        )
        if report["setup"]["exit_code"] != 0:
            raise RuntimeError(report["setup"])
    source = Path(__file__).with_name("debian_abi_probe.c")
    build(
        source, root, dist, libraries=("libm.so.6", "libpthread.so.0"), link_dir=args.link_dir.resolve()
    )
    command = [
        worker,
        "run",
        "--root",
        str(root),
        "--dist",
        str(dist),
        "--",
        "/bin/bash",
        "-c",
        "export PATH=/usr/sbin:/usr/bin:/sbin:/bin; chmod 755 /probe; exec /probe",
    ]
    with RpcPeer() as peer:
        (root / "tmp/abi-rpc-port").write_text(str(peer.port), encoding="ascii")
        report["probe"] = execute(command, output, environment, timeout=60)
        report["rpc"] = {"requests": peer.requests, "errors": peer.errors}
    for label, path in (
        ("source", source),
        ("elf", root / "probe"),
        ("worker", dist / "worker.exe"),
        ("manifest", dist / "rootfs.manifest.json"),
    ):
        report[label + "_sha256"] = hashlib.sha256(path.read_bytes()).hexdigest()
    passed = (
        report["probe"]["exit_code"] == 0
        and not report["probe"]["timed_out"]
        and "DEBIAN_ABI_OK" in report["probe"]["stdout"]
        and report["rpc"] == {"requests": [-17], "errors": []}
    )
    report["passed"] = passed
    (output / "report.json").write_text(
        json.dumps(report, indent=2) + "\n", encoding="utf-8"
    )
    print(report["probe"]["stdout"], end="")
    print(report["probe"]["stderr"], end="", file=sys.stderr)
    if not passed:
        raise SystemExit(
            "Debian ABI regression failed: " + str(report["probe"]["exit_code"])
        )


if __name__ == "__main__":
    main()
