"""Inventory and exercise the packaged Debian command surface, recording every result.

Help/version probes prove startup only. Functional cases have separate assertions.
Each Windows process tree is held in a kill-on-close Job, including on timeout.
"""

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor, as_completed
import ctypes
from ctypes import wintypes as w
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import posixpath
import re
import subprocess
import socketserver
import struct
import tempfile
import threading
import time

COMMAND_DIRS = ("bin/", "sbin/", "usr/bin/", "usr/sbin/")
NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)


def inventory(dist):
    manifest = json.loads((dist / "rootfs.manifest.json").read_text(encoding="utf-8"))
    files = {entry["path"]: entry for entry in manifest["files"]}
    links = manifest.get("links", {})
    permissions = manifest.get("permissions", {})

    def resolve(path):
        seen = set()
        while path in links:
            if path in seen:
                raise ValueError("manifest link cycle: " + path)
            seen.add(path)
            target = links[path]
            path = posixpath.normpath(
                target.lstrip("/")
                if target.startswith("/")
                else posixpath.join(posixpath.dirname(path), target)
            )
        return path

    result = []
    for path in sorted(files.keys() | links.keys()):
        if not any(
            path.startswith(d) and "/" not in path[len(d) :] for d in COMMAND_DIRS
        ):
            continue
        target = resolve(path)
        entry = files.get(target)
        data = (
            (dist / entry["source"]).read_bytes()
            if entry and "source" in entry
            else entry.get("content", "").encode()
            if entry
            else b""
        )
        mode = permissions.get(target, 0o644)
        result.append(
            dict(
                path="/" + path,
                target="/" + target,
                alias=path in links,
                mode=oct(mode),
                executable=bool(mode & 0o111),
                format="ELF"
                if data.startswith(b"\x7fELF")
                else "script"
                if data.startswith(b"#!")
                else "PE"
                if data.startswith(b"MZ")
                else "other",
                sha256=hashlib.sha256(data).hexdigest() if entry else None,
            )
        )
    return result


def coverage(entries, results):
    executable = {entry["path"]: entry for entry in entries if entry["executable"]}
    outcomes = {row["path"]: row["status"] for row in results}
    passed = {path for path in executable if outcomes.get(path) == "passed"}
    targets = {entry["target"] for entry in executable.values()}
    passed_targets = {executable[path]["target"] for path in passed}
    return dict(
        scope="Fixed manifest executable paths; functional and startup phases are separate. "
        "Missing, unselected and untested commands remain in the denominator. "
        "A target passes when at least one of its command paths passes.",
        executable_paths=len(executable),
        passed_paths=len(passed),
        passed_percent=100 * len(passed) / len(executable) if executable else None,
        unselected_paths=sum(path not in outcomes for path in executable),
        alias_paths=sum(entry["alias"] for entry in executable.values()),
        distinct_targets=len(targets),
        passed_targets=len(passed_targets),
        target_passed_percent=100 * len(passed_targets) / len(targets) if targets else None,
    )


class ProcessTree:
    """Assign a suspended supervisor before any guest descendant can start."""

    def __init__(self, process):
        class Basic(ctypes.Structure):
            _fields_ = [
                ("process_time", ctypes.c_int64),
                ("job_time", ctypes.c_int64),
                ("flags", w.DWORD),
                ("min_ws", ctypes.c_size_t),
                ("max_ws", ctypes.c_size_t),
                ("process_limit", w.DWORD),
                ("affinity", ctypes.c_size_t),
                ("priority", w.DWORD),
                ("scheduling", w.DWORD),
            ]

        class Limits(ctypes.Structure):
            _fields_ = [
                ("basic", Basic),
                ("io", ctypes.c_uint64 * 6),
                ("memory", ctypes.c_size_t * 4),
            ]

        api = ctypes.WinDLL("kernel32", use_last_error=True)
        api.CreateJobObjectW.argtypes = [ctypes.c_void_p, w.LPCWSTR]
        api.CreateJobObjectW.restype = w.HANDLE
        api.SetInformationJobObject.argtypes = [
            w.HANDLE,
            ctypes.c_int,
            ctypes.c_void_p,
            w.DWORD,
        ]
        api.AssignProcessToJobObject.argtypes = [w.HANDLE, w.HANDLE]
        api.CloseHandle.argtypes = [w.HANDLE]
        self.api, self.handle = api, api.CreateJobObjectW(None, None)
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            limits = Limits()
            limits.basic.flags = 0x2000  # KILL_ON_JOB_CLOSE
            if not api.SetInformationJobObject(
                self.handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)
            ):
                raise ctypes.WinError(ctypes.get_last_error())
            if not api.AssignProcessToJobObject(
                self.handle, w.HANDLE(int(process._handle))
            ):
                raise ctypes.WinError(ctypes.get_last_error())
            resume = ctypes.WinDLL("ntdll").NtResumeProcess
            resume.argtypes = [w.HANDLE]
            resume.restype = ctypes.c_long
            if resume(w.HANDLE(int(process._handle))) < 0:
                raise OSError("NtResumeProcess failed")
        except BaseException:
            self.close()
            process.kill()
            raise

    def close(self):
        if self.handle:
            self.api.CloseHandle(self.handle)
            self.handle = None


def execute(command, directory, env, input_data=b"", timeout=12):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "stdin").write_bytes(input_data)
    started = time.monotonic()
    timed_out = False
    with (
        (directory / "stdin").open("rb") as stdin,
        (directory / "stdout").open("wb") as stdout,
        (directory / "stderr").open("wb") as stderr,
    ):
        process = subprocess.Popen(
            command,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            env=env,
            creationflags=NO_WINDOW | 4,
        )
        try:
            tree = ProcessTree(process)
        except BaseException:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)
            raise
        try:
            try:
                code = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
                tree.close()
                code = process.wait(timeout=10)
        finally:
            tree.close()
    output = {}
    for name in ("stdout", "stderr"):
        with (directory / name).open("rb") as stream:
            data = stream.read(128 * 1024)
        output[name] = data.decode("utf-8", errors="replace")
        output[name + "_bytes"] = (directory / name).stat().st_size
    return dict(
        exit_code=code,
        timed_out=timed_out,
        seconds=round(time.monotonic() - started, 3),
        **output,
    )


def failure_kind(result):
    text = result["stdout"] + "\n" + result["stderr"]
    if result["timed_out"]:
        return "timeout"
    if re.search(
        r"undefined symbol|missing.*symbol|unresolved|resolve.*symbol|symbol.*not found|no native export",
        text,
        re.I,
    ):
        return "missing_symbol"
    if re.search(
        r"unimplemented|not implemented|Function not implemented|ENOSYS", text, re.I
    ):
        return "unimplemented"
    if re.search(
        r"Can.t locate .* in @INC|requires the .*module|please install .*perl|install the perl-doc|package .* is not installed|failed to find uid|shadow password file is not present|[Cc]an.t open cache file|trigger records not yet in existence",
        text,
    ):
        return "missing_package_or_configuration"
    if re.search(
        r"Failed to connect to bus|not been booted with systemd|No such device|Operation not supported|cannot open /dev/|/dev/mem:|/sys/|/proc/|[Nn]etlink|NETLINK|Module .* not found|kernel doesn.t have",
        text,
        re.I,
    ):
        return "environment_or_kernel"
    if result["exit_code"] in (128 + 9, 128 + 11, 128 + 15):
        return "signal_exit"
    if (
        result["exit_code"] is not None
        and 0xC0000000 <= (result["exit_code"] & 0xFFFFFFFF) < 0xD0000000
    ):
        return "native_crash"
    return "exit_or_assertion"


def smoke_args(path):
    name = posixpath.basename(path)
    # These tools do not implement GNU long options; use their documented query.
    if name in ("ssh", "slogin", "scp", "sftp"):
        return ["-V"]
    if name in ("openssl",):
        return ["version"]
    if name == "lsof":
        return ["-h"]
    if name in ("awk", "mawk"):
        return ["-W", "version"]
    if name == "[":
        return ["]"]
    if name in ("bzip2recover", "bzexe", "e2label"):
        return []
    if name in (
        "e2fsck",
        "fsck.ext2",
        "fsck.ext3",
        "fsck.ext4",
        "dbus-daemon",
        "nslookup",
    ):
        return ["-V"] if name != "dbus-daemon" else ["--version"]
    if name == "fstab-decode":
        return ["/usr/bin/printf", "fstab-start-ok"]
    if name in ("rtmon", "tc", "genl"):
        return ["help"] if name == "rtmon" else ["-help"]
    if name in ("zipnote", "zipsplit", "zipinfo", "c_rehash", "h2xs", "dnstap-read"):
        return ["-h"]
    if name == "validlocale":
        return ["C.UTF-8"]
    if name == "nologin":
        return []
    if name in ("dash", "sh", "ash", "rbash", "bash"):
        return ["-c", "printf SHELL_START_OK"]
    if name in ("true", "false", "test", "["):
        return []
    if name in ("zic", "zdump", "ldconfig"):
        return ["--version"]
    if name in (
        "policy-rc.d",
        "ownership",
        "dhclient-script",
        "installkernel",
        "pam_namespace_helper",
        "mkhomedir_helper",
        "unix_chkpwd",
        "unix_update",
        "pwhistory_helper",
        "clear_console",
        "bashbug",
        "perlbug",
        "perlthanks",
        "sensible-browser",
        "sensible-editor",
        "sensible-pager",
        "exicyclog",
        "exiwhat",
        "e2scrub_all",
        "shadowconfig",
        "killall5",
        "instmodsh",
        "debconf-apt-progress",
        "ssh-argv0",
        "zstdgrep",
        "funzip",
        "ssh-add",
        "update-pciids",
    ):
        return None  # These helpers may interpret unknown options as real work.
    return ["--help"]


def save(path, data):
    path.write_text(
        json.dumps(data, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )


class NetworkFixtures:
    """Deterministic loopback services; no public network is used by the cases."""

    def __init__(self):
        class HTTP(BaseHTTPRequestHandler):
            def do_GET(self):
                body = b"kinakaze-http-ok\n"
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        class DNS(socketserver.BaseRequestHandler):
            def handle(self):
                data, sock = self.request
                if len(data) < 17:
                    return
                end = 12
                while end < len(data) and data[end]:
                    end += data[end] + 1
                end += 5
                if end > len(data):
                    return
                qtype, qclass = struct.unpack("!HH", data[end - 4 : end])
                answer = (
                    b"\xc0\x0c"
                    + struct.pack("!HHIH", 1, 1, 30, 4)
                    + bytes((127, 0, 0, 42))
                    if (qtype, qclass) == (1, 1)
                    else b""
                )
                header = data[:2] + struct.pack("!HHHHH", 0x8180, 1, bool(answer), 0, 0)
                sock.sendto(header + data[12:end] + answer, self.client_address)

        self.http = ThreadingHTTPServer(("127.0.0.1", 0), HTTP)
        self.dns = socketserver.ThreadingUDPServer(("127.0.0.1", 0), DNS)
        for server in (self.http, self.dns):
            threading.Thread(target=server.serve_forever, daemon=True).start()
        port = self.http.server_address[1]
        self.exports = (
            f"export AUDIT_HTTP_URL=http://127.0.0.1:{port}/ AUDIT_HTTP_PORT={port} "
            f"AUDIT_DNS_PORT={self.dns.server_address[1]}\n"
        )

    def close(self):
        for server in (self.http, self.dns):
            server.shutdown()
            server.server_close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--inventory-dist", type=Path,
                        help="prepared Debian payload to inventory and install; native runtime comes from --dist")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--phase", choices=("smoke", "functional"), required=True)
    parser.add_argument("--jobs", type=int, default=6)
    parser.add_argument("--timeout", type=int, default=12)
    parser.add_argument("--only", help="regular expression selecting command paths")
    parser.add_argument(
        "--merge",
        action="store_true",
        help="retain earlier results for paths outside --only",
    )
    args = parser.parse_args()
    dist, output = args.dist.resolve(), args.output.resolve()
    inventory_dist = (args.inventory_dist or args.dist).resolve()
    output.mkdir(parents=True, exist_ok=True)
    entries = inventory(inventory_dist)
    images = {
        str(path.relative_to(dist)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in (dist / "worker.exe", dist / "init.exe", *sorted((dist / "rootfs/lib").glob("*")))
        if path.is_file()
    }
    images["rootfs.manifest.json"] = hashlib.sha256(
        (inventory_dist / "rootfs.manifest.json").read_bytes()).hexdigest()
    save(output / "inventory.json", entries)
    location = output / f"root-location-{args.phase}.json"
    if location.exists():
        root = Path(json.loads(location.read_text(encoding="utf-8"))["root"])
    else:
        # The ordinary Windows temp volume supports the case-sensitive guest
        # directories even when the checkout volume's directory policy does not.
        root = Path(tempfile.mkdtemp(prefix="kinakaze command audit ")) / "rootfs"
        save(location, dict(root=str(root)))
    env = {k: v for k, v in os.environ.items() if not k.startswith("KINAKAZE_")}
    env["PATH"] = ""
    env.update(TERM="xterm", LC_ALL="C.UTF-8", TZ="UTC0")
    worker = str(dist / "worker.exe")
    marker = root / ".kinakaze-rootfs.sha256"
    if (
        marker.exists()
        and marker.read_text(encoding="utf-8").strip() != images["rootfs.manifest.json"]
    ):
        raise RuntimeError(
            "Existing audit root uses another manifest; choose a new --output"
        )
    if not marker.exists():
        setup = execute(
            [worker, "setup", "--root", str(root), "--dist", str(inventory_dist)],
            output / f"setup-{args.phase}",
            env,
            timeout=180,
        )
        save(output / f"setup-{args.phase}.json", setup)
        if setup["exit_code"] != 0:
            raise RuntimeError(f"rootfs setup failed; see setup-{args.phase}.json")
    if args.phase == "functional":
        from debian_command_cases import cases, prepare

        catalog = cases()
    else:
        catalog = {}
    network = NetworkFixtures() if args.phase == "functional" else None
    run_id = time.strftime("%Y%m%dT%H%M%S")

    def run(entry):
        path = entry["path"]
        name = posixpath.basename(path)
        key = path.strip("/").replace("/", "__")
        record = dict(path=path, target=entry["target"], phase=args.phase)
        if not entry["executable"]:
            return dict(record, status="not_executable")
        case = catalog.get(name)
        if args.phase == "functional" and case is None:
            return dict(record, status="not_tested", reason="No functional case yet")
        arguments = smoke_args(path) if args.phase == "smoke" else []
        if arguments is None:
            return dict(
                record,
                status="not_tested",
                reason="Helper requires an explicit isolated case",
            )
        directory = output / args.phase / run_id / key
        record["log_directory"] = str(directory.relative_to(output))
        guest_cwd = f"/tmp/command-audit/{args.phase}/{run_id}/{key}"
        case_root = root / guest_cwd.lstrip("/")
        case_root.mkdir(parents=True, exist_ok=True)
        base = [
            worker,
            "oneshot",
            "--root",
            str(root),
            "--dist",
            str(dist),
            "--cwd",
            guest_cwd,
            "--",
        ]
        if case is None:
            command = base + [path, *arguments]
            expected = (
                (1,)
                if name in ("false", "nologin")
                else (0, 1)
                if name in ("test", "[")
                else (0,)
            )
            input_data = b""
        else:
            prepare(case_root, case)
            script = (
                "set -eu\nexport PATH=/usr/sbin:/usr/bin:/sbin:/bin HOME=/root\n"
                + network.exports
                + "chmod -R u+rwX .\n"
                + case["script"]
                + '\nprintf "\\nAUDIT_FUNCTION_OK\\n"\n'
            )
            command = base + ["/bin/bash", "-c", script, "--", path]
            expected = (0,)
            input_data = case.get("input", "").encode()
            record["purpose"] = case["purpose"]
        record["argv"] = (
            [path, *arguments] if case is None else ["bash", "-c", script, "--", path]
        )
        record["timeout_seconds"] = (
            max(case["timeout"], args.timeout) if case else args.timeout
        )
        result = execute(
            command,
            directory,
            env,
            input_data,
            record["timeout_seconds"],
        )
        if case:
            diagnostics = {}
            for filename in ("out", "err", "expected", "transcript"):
                diagnostic = case_root / filename
                if diagnostic.is_file():
                    with diagnostic.open("rb") as stream:
                        diagnostics[filename] = stream.read(16384).decode(
                            "utf-8", errors="replace"
                        )
            record["fixture_diagnostics"] = diagnostics
        record.update(result)
        text = result["stdout"] + "\n" + result["stderr"]
        if result["timed_out"]:
            record.update(status="timeout", failure="timeout")
        elif result["exit_code"] in expected and (
            case is None or "AUDIT_FUNCTION_OK" in result["stdout"]
        ):
            record["status"] = "passed"
        elif (
            case is None
            and result["exit_code"] in (1, 2, 3, 10, 16, 21, 25, 64, 255, 4294967295)
            and re.search(
                r"usage[: ]|syntax:|unrecognized option|invalid option|unknown option|requires an argument|dnstap-read \[|dbus-update-activation-environment \[|h2xs \[OPTIONS",
                text,
                re.I,
            )
        ):
            record["status"] = "usage_only"
        else:
            diagnostic_result = dict(result)
            diagnostic_result["stderr"] += "\n" + record.get(
                "fixture_diagnostics", {}
            ).get("err", "")
            record.update(status="failed", failure=failure_kind(diagnostic_result))
        return record

    selected = [e for e in entries if not args.only or re.search(args.only, e["path"])]
    if not selected:
        parser.error("no executable commands selected; check --inventory-dist and --only")
    results = []
    with (
        ThreadPoolExecutor(max_workers=args.jobs) as pool,
        (output / f"{args.phase}-{run_id}.jsonl").open("w", encoding="utf-8") as log,
    ):
        futures = {pool.submit(run, entry): entry for entry in selected}
        for future in as_completed(futures):
            entry = futures[future]
            try:
                result = future.result()
            except Exception as error:
                result = dict(
                    path=entry["path"],
                    phase=args.phase,
                    status="harness_error",
                    error=str(error),
                )
            results.append(result)
            log.write(json.dumps(result, ensure_ascii=False) + "\n")
            log.flush()
            if len(results) % 25 == 0 or result["status"] in (
                "timeout",
                "harness_error",
            ):
                print(
                    f"{args.phase}: {len(results)}/{len(selected)} {dict(Counter(r['status'] for r in results))}",
                    flush=True,
                )
    if network:
        network.close()
    report_path = output / f"{args.phase}.json"
    if args.merge and report_path.exists():
        old_report = json.loads(report_path.read_text(encoding="utf-8"))
        if old_report["images"] != images:
            raise RuntimeError(
                "Cannot merge results from different images; choose a new --output"
            )
        previous = old_report["results"]
        selected_paths = {r["path"] for r in results}
        results += [r for r in previous if r["path"] not in selected_paths]
    report = dict(
        phase=args.phase,
        distribution=str(dist),
        inventory_distribution=str(inventory_dist),
        updated=run_id,
        harness_sha256={
            p.name: hashlib.sha256(p.read_bytes()).hexdigest()
            for p in (
                Path(__file__),
                Path(__file__).with_name("debian_command_cases.py"),
                Path(__file__).resolve().parents[1] / "tests/guest/gpgv-fixture.json",
            )
        },
        images=images,
        counts=dict(Counter(r["status"] for r in results)),
        coverage=coverage(entries, results),
        results=sorted(results, key=lambda r: r["path"]),
    )
    save(output / f"{args.phase}.json", report)
    print(json.dumps(report["counts"], indent=2))
    return int(any(r["status"] in ("failed", "timeout", "harness_error") for r in results))


if __name__ == "__main__":
    raise SystemExit(main())
