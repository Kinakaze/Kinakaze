"""Sequential, resumable functional checks for the fixed Debian software queue.

Every package retains its place in the denominator. Passing means the registered
functional scenarios passed, not that every command or feature is supported.
Missing packages and missing scenarios are gaps. No --help fallback is counted.
Only disposable roots are accepted, since package tools may alter guest state.
"""

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import runpy
import re
import shutil
import time
import uuid

from debian_command_cases import cases, prepare
from init_pool import distribution_hashes


def save(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("root", "dist", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument(
        "--lock", type=Path, default=Path("config/debian-software-top500.lock.json")
    )
    parser.add_argument("--only", help="exact package names separated by commas")
    parser.add_argument(
        "--resume",
        action="store_true",
        help="reuse passing rows only with identical images and cases",
    )
    parser.add_argument(
        "--stop-on-gap",
        action="store_true",
        help="return after the first non-passing package",
    )
    parser.add_argument("--timeout", type=float, default=12)
    parser.add_argument(
        "--startup",
        action="store_true",
        help="record a separate safe startup probe; never counts as functional pass",
    )
    parser.add_argument(
        "--case-limit",
        type=int,
        default=0,
        help="0 tests every registered command; otherwise a first-pass limit",
    )
    args = parser.parse_args()
    if args.timeout <= 0 or args.case_limit < 0:
        parser.error("invalid timeout or case limit")
    root, dist, output = (
        path.resolve() for path in (args.root, args.dist, args.output)
    )
    workspace = Path(__file__).resolve().parents[1]
    if not root.is_relative_to(workspace / "artifacts") or not root.is_dir():
        parser.error(
            "--root must be a disposable, prepared root under workspace artifacts"
        )
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.CreateFileW.argtypes = (
            wintypes.LPCWSTR,
            wintypes.DWORD,
            wintypes.DWORD,
            ctypes.c_void_p,
            wintypes.DWORD,
            wintypes.DWORD,
            wintypes.HANDLE,
        )
        kernel.CreateFileW.restype = wintypes.HANDLE
        kernel.GetFileInformationByHandleEx.argtypes = (
            wintypes.HANDLE,
            ctypes.c_int,
            ctypes.c_void_p,
            wintypes.DWORD,
        )
        kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
        for directory in (root, root / "usr/bin", root / "usr/share", root / "usr/lib"):
            handle = kernel.CreateFileW(
                str(directory), 0x80, 7, None, 3, 0x02000000, None
            )
            if handle == wintypes.HANDLE(-1).value:
                raise ctypes.WinError(ctypes.get_last_error())
            try:
                flags = wintypes.DWORD()
                if not kernel.GetFileInformationByHandleEx(
                    handle, 23, ctypes.byref(flags), 4
                ):
                    raise ctypes.WinError(ctypes.get_last_error())
                if not flags.value & 1:
                    parser.error(
                        f"case-insensitive guest directory: {directory}; use clone-debian-root.py"
                    )
            finally:
                kernel.CloseHandle(handle)
    lock = json.loads(args.lock.read_text(encoding="utf-8"))
    packages = lock["packages"]
    names = {package["package"] for package in packages}
    only = set(args.only.split(",")) if args.only else names
    if not only.issubset(names):
        parser.error("unknown package in --only: " + ",".join(sorted(only - names)))
    output.mkdir(parents=True, exist_ok=True)
    harness = runpy.run_path(str(Path(__file__).with_name("test-debian-commands.py")))
    execute, network_type = harness["execute"], harness["NetworkFixtures"]
    catalog = cases()
    extra_path = Path(__file__).with_name("debian_software_cases.py")
    if extra_path.exists():
        extra = runpy.run_path(str(extra_path))["cases"]()
        catalog.update(extra)
    fingerprint = dict(
        lock=hashlib.sha256(args.lock.read_bytes()).hexdigest(),
        images=distribution_hashes(dist),
        cases=hashlib.sha256(json.dumps(catalog, sort_keys=True).encode()).hexdigest(),
        harness=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        command_harness=hashlib.sha256(
            Path(__file__).with_name("test-debian-commands.py").read_bytes()
        ).hexdigest(),
        fixtures=hashlib.sha256(
            Path(__file__).with_name("debian_command_cases.py").read_bytes()
        ).hexdigest(),
        case_limit=args.case_limit,
        startup=args.startup,
        package_state=hashlib.sha256(
            (root / "var/lib/dpkg/status").read_bytes()
        ).hexdigest(),
    )
    fixture_source = workspace / "tests/guest"
    fingerprint["application_fixtures"] = {
        filename: hashlib.sha256((fixture_source / filename).read_bytes()).hexdigest()
        for case in catalog.values()
        for filename in case.get("fixtures", ())
    }
    report_path = output / "results.json"
    if report_path.exists():
        if not args.resume:
            parser.error("output exists; use a new output or --resume")
        report = json.loads(report_path.read_text(encoding="utf-8"))
        if report["fingerprint"] != fingerprint or report["root"] != str(root):
            parser.error("cannot resume with different root, images, lock or scenarios")
    else:
        report = dict(
            scope=__doc__,
            root=str(root),
            dist=str(dist),
            fingerprint=fingerprint,
            total=len(packages),
            results=[],
            counts={},
            complete=False,
        )
    previous = {row["package"]: row for row in report["results"]}
    run_id = time.strftime("%Y%m%dT%H%M%S") + "-" + uuid.uuid4().hex[:8]
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("KINAKAZE_")
    }
    env.update(PATH="", HOME="/root", TERM="xterm", LC_ALL="C.UTF-8", TZ="UTC0")
    versions, package_states = {}, {}
    for record in (
        (root / "var/lib/dpkg/status").read_text(encoding="utf-8").split("\n\n")
    ):
        fields = dict(re.findall(r"^([^ :\n]+): (.*)$", record, re.M))
        name = fields.get("Package")
        if name:
            versions[name] = fields.get("Version")
            package_states[name] = fields.get("Status")
        for provided, version in re.findall(
            r"([^ ,]+) \(= ([^)]+)\)", fields.get("Provides", "")
        ):
            versions.setdefault(provided, version)
            package_states.setdefault(provided, "provided by " + str(name))
    # Query through the guest VFS: host is_file() cannot see virtual symlinks.
    inventory_guest = "/tmp/debian-software-inventory-" + uuid.uuid4().hex
    inventory_stage = root / inventory_guest.lstrip("/")
    inventory_stage.mkdir(parents=True)
    save(inventory_stage / "queue.json", packages)
    (inventory_stage / "inventory.py").write_text(
        "import hashlib,json,os\n"
        'rows=json.load(open("queue.json"))\n'
        'result={p["package"]:[c for c in p["commands"] if os.path.isfile(c) and os.access(c,os.X_OK)] for p in rows}\n'
        "images={}; cache={}\n"
        "for commands in result.values():\n"
        " for command in commands:\n"
        "  target=os.path.realpath(command)\n"
        "  if target not in cache:\n"
        '   with open(command,"rb") as stream: cache[target]=hashlib.file_digest(stream,"sha256").hexdigest()\n'
        '  images[command]={"target":target,"sha256":cache[target]}\n'
        'json.dump({"commands":result,"images":images},open("availability.json","w"),sort_keys=True)\n',
        encoding="utf-8",
    )
    availability_run = execute(
        [
            str(dist / "worker.exe"),
            "oneshot",
            "--root",
            str(root),
            "--dist",
            str(dist),
            "--cwd",
            inventory_guest,
            "--",
            "/usr/bin/python3",
            "inventory.py",
        ],
        output / ("inventory-" + run_id),
        env,
        timeout=60,
    )
    save(output / ("inventory-" + run_id + ".json"), availability_run)
    if availability_run["exit_code"] != 0 or availability_run["timed_out"]:
        raise RuntimeError("guest command inventory failed; see inventory log")
    inventory_bytes = (inventory_stage / "availability.json").read_bytes()
    inventory_hash = hashlib.sha256(inventory_bytes).hexdigest()
    if args.resume and report.get("root_inventory_sha256") != inventory_hash:
        parser.error(
            "cannot resume after guest executable contents changed; use a new output"
        )
    report["root_inventory_sha256"] = inventory_hash
    inventory_data = json.loads(inventory_bytes)
    availability = inventory_data["commands"]
    save(output / ("command-images-" + run_id + ".json"), inventory_data["images"])
    network = network_type()

    def checkpoint():
        report["results"] = [
            previous[p["package"]] for p in packages if p["package"] in previous
        ]
        counts = Counter(row["status"] for row in report["results"])
        counts["not_run"] = len(packages) - len(report["results"])
        report["counts"] = dict(counts)
        report["startup_counts"] = dict(
            Counter(
                row["startup"]["status"]
                for row in report["results"]
                if "startup" in row
            )
        )
        report["functional_passed_percent"] = 100 * counts["passed"] / len(packages)
        report["complete"] = counts["passed"] == len(packages)
        report["scan_complete"] = counts["not_run"] == 0 and not counts["in_progress"]
        report["updated"] = run_id
        save(report_path, report)

    checkpoint()
    try:
        for package in packages:
            name = package["package"]
            if name not in only or (
                args.resume and previous.get(name, {}).get("status") == "passed"
            ):
                continue
            row = dict(
                order=package["order"],
                package=name,
                version=package["version"],
                popularity_rank=package["popularity_rank"],
                tests=[],
                commands=package["commands"],
            )
            row["available_commands"] = availability[name]
            row["tested_version"] = versions.get(name)
            row["package_state"] = package_states.get(name, "not registered")
            if args.startup and availability[name]:
                startup_path = next(
                    (
                        path
                        for path in availability[name]
                        if harness["smoke_args"](path) is not None
                    ),
                    None,
                )
                if startup_path:
                    arguments = harness["smoke_args"](startup_path)
                    directory = (
                        output / "startup" / run_id / f"{package['order']:03d}-{name}"
                    )
                    startup = dict(
                        path=startup_path,
                        arguments=arguments,
                        log_directory=str(directory.relative_to(output)),
                        scope="startup only",
                    )
                    try:
                        startup.update(
                            execute(
                                [
                                    str(dist / "worker.exe"),
                                    "oneshot",
                                    "--root",
                                    str(root),
                                    "--dist",
                                    str(dist),
                                    "--",
                                    startup_path,
                                    *arguments,
                                ],
                                directory,
                                env,
                                timeout=args.timeout,
                            )
                        )
                        startup["status"] = (
                            "timeout"
                            if startup["timed_out"]
                            else "passed"
                            if startup["exit_code"] == 0
                            else "nonzero_exit"
                        )
                        if startup["status"] != "passed":
                            startup["failure"] = harness["failure_kind"](startup)
                    except Exception as error:
                        startup.update(status="harness_error", error=str(error))
                    row["startup"] = startup
            registered = [
                path for path in package["commands"] if Path(path).name in catalog
            ]
            row["commands_without_scenarios"] = [
                path for path in package["commands"] if path not in registered
            ]
            if args.case_limit:
                row["commands_deferred"] = registered[args.case_limit :]
                registered = registered[: args.case_limit]
            if not availability[name]:
                row.update(
                    status="missing_package",
                    reason="No executable package entry in the guest VFS",
                )
            elif not registered:
                row.update(
                    status="not_tested", reason="No registered functional scenario"
                )
            else:
                for number, path in enumerate(registered):
                    case = catalog[Path(path).name]
                    guest = f"/tmp/debian-software/{run_id}/{package['order']:03d}-{name}/{number}"
                    stage = root / guest.lstrip("/")
                    stage.mkdir(parents=True)
                    prepare(stage, case)
                    for fixture in case.get("fixtures", ()):
                        shutil.copyfile(fixture_source / fixture, stage / fixture)
                    script = (
                        "set -eu\nexport PATH=/usr/sbin:/usr/bin:/sbin:/bin HOME=/root\n"
                        + network.exports
                        + 'if ! test -x "$1"; then printf "SOFTWARE_COMMAND_MISSING\\n"; exit 125; fi\n'
                        + "chmod -R u+rwX .\n"
                        + case["script"]
                        + '\nprintf "\\nSOFTWARE_FUNCTION_OK\\n"\n'
                    )
                    directory = (
                        output
                        / "runs"
                        / run_id
                        / f"{package['order']:03d}-{name}"
                        / str(number)
                    )
                    test = dict(
                        path=path,
                        purpose=case["purpose"],
                        script=script,
                        cwd=guest,
                        timeout_seconds=max(args.timeout, case["timeout"]),
                        log_directory=str(directory.relative_to(output)),
                    )
                    test["image"] = inventory_data["images"].get(path)
                    try:
                        result = execute(
                            [
                                str(dist / "worker.exe"),
                                "oneshot",
                                "--root",
                                str(root),
                                "--dist",
                                str(dist),
                                "--cwd",
                                guest,
                                "--",
                                "/bin/bash",
                                "-c",
                                script,
                                "--",
                                path,
                            ],
                            directory,
                            env,
                            case.get("input", "").encode(),
                            test["timeout_seconds"],
                        )
                        test.update(result)
                        test["fixture_diagnostics"] = {
                            filename: (stage / filename)
                            .read_bytes()[:16384]
                            .decode("utf-8", errors="replace")
                            for filename in ("out", "err", "expected", "transcript")
                            if (stage / filename).is_file()
                        }
                        if result["timed_out"]:
                            test.update(status="timeout", failure="timeout")
                        elif (
                            result["exit_code"] == 125
                            and "SOFTWARE_COMMAND_MISSING" in result["stdout"]
                        ):
                            test["status"] = "missing_command"
                        elif (
                            result["exit_code"] == 0
                            and "SOFTWARE_FUNCTION_OK" in result["stdout"]
                        ):
                            test["status"] = "passed"
                        else:
                            diagnostic = dict(result)
                            diagnostic["stderr"] += "\n" + test[
                                "fixture_diagnostics"
                            ].get("err", "")
                            test.update(
                                status="failed",
                                failure=harness["failure_kind"](diagnostic),
                            )
                    except Exception as error:
                        test.update(status="harness_error", error=str(error))
                    row["tests"].append(test)
                    # Persist each command too, so interruption loses no completed work.
                    previous[name] = dict(row, status="in_progress")
                    checkpoint()
                statuses = Counter(test["status"] for test in row["tests"])
                row["test_counts"] = dict(statuses)
                row["status"] = (
                    "passed"
                    if set(statuses) == {"passed"}
                    else "missing_package"
                    if set(statuses) == {"missing_command"}
                    else "timeout"
                    if "timeout" in statuses
                    else "failed"
                )
            previous[name] = row
            checkpoint()
            print(
                json.dumps(
                    {key: row[key] for key in ("order", "package", "status")}
                    | dict(tests=len(row["tests"]), counts=report["counts"]),
                    ensure_ascii=False,
                ),
                flush=True,
            )
            if args.stop_on_gap and row["status"] != "passed":
                break
    finally:
        network.close()
        checkpoint()
    return int(not report["complete"])


if __name__ == "__main__":
    raise SystemExit(main())
