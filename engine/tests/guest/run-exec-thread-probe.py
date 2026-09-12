"""Build and run actual Linux pthread/exec/IFUNC retirement regression on Windows.

The independent distribution/root never overwrite the main dist or guest root.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys


def checked(arguments, workspace):
    command = [str(item) for item in arguments]
    command[0] = shutil.which(command[0]) or command[0]
    result = subprocess.run(command, cwd=workspace, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, errors="replace", creationflags=subprocess.CREATE_NO_WINDOW)
    if result.returncode:
        print(result.stdout, file=sys.stderr)
    result.check_returncode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--dist", type=Path)
    parser.add_argument("--timeout", type=float, default=25)
    arguments = parser.parse_args()
    workspace = Path(__file__).resolve().parents[3]
    sources = Path(__file__).resolve().parent
    root = workspace / "artifacts/process-thread-root"
    distribution = (arguments.dist or workspace / "artifacts/process-thread-dist").resolve()
    output = workspace / "artifacts/process-thread-probe"
    for directory in (root, distribution, output):
        directory.mkdir(parents=True, exist_ok=True)
    if not arguments.skip_build:
        checked(["pwsh", "-NoProfile", "-File", "tools/build.ps1", "-SkipFormat", "-SkipTests",
                 "-DistDirectory", distribution], workspace)
    binaries = distribution
    for name in ("exec_thread_probe", "exec_ifunc_probe"):
        obj = output / f"{name}.o"
        checked(["clang", "--target=x86_64-linux-gnu", "-ffreestanding", "-fno-stack-protector",
                 "-fno-builtin", "-fPIC", "-O1", "-c", sources / f"{name}.c", "-o", obj], workspace)
        checked(["ld.lld", "-pie", "-z", "now", "-z", "relro", "--dynamic-linker",
                 "/lib64/ld-linux-x86-64.so.2", "-e", "_start",
                 obj, "-L", workspace / "target/debug/elf-imports", "-l:libc.so.6", "-l:libpthread.so.0",
                 "-o", root / name], workspace)
    relocations = subprocess.check_output(
        [shutil.which("llvm-readobj"), "--relocations", "--program-headers", str(root / "exec_ifunc_probe")],
        text=True, creationflags=subprocess.CREATE_NO_WINDOW)
    (output / "ifunc-relocations.txt").write_text(relocations, encoding="utf-8")
    if "R_X86_64_IRELATIVE" not in relocations:
        raise RuntimeError("target ELF lacks the mandatory GNU IFUNC relocation")
    image = (root / "exec_ifunc_probe").read_bytes()
    if image[:6] != b"\x7fELF\x02\x01":
        raise RuntimeError("target is not a little-endian ELF64 image")
    table = struct.unpack_from("<Q", image, 32)[0]
    stride, count = struct.unpack_from("<HH", image, 54)
    ranges = []
    for index in range(count):
        kind, _, _, address, _, _, size, _ = struct.unpack_from("<IIQQQQQQ", image, table + stride * index)
        if kind == 0x6474E552:
            ranges.append((address, address + size))
    slots = [int(value, 16) for value in re.findall(r"0x([0-9A-Fa-f]+)\s+R_X86_64_IRELATIVE", relocations)]
    if not slots or not all(any(start <= slot and slot + 8 <= end for start, end in ranges) for slot in slots):
        raise RuntimeError("IFUNC destination slots must lie inside PT_GNU_RELRO")
    (root / "exec-thread-state.bin").write_bytes(bytes(4096))
    command = [str(binaries / "worker.exe"), "run", "--root", str(root),
               "--dist", str(distribution), "--", "/exec_thread_probe"]
    stdout_path = output / "stdout.log"
    stderr_path = output / "stderr.log"
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        child = subprocess.Popen(command, cwd=workspace, env=os.environ.copy(), stdout=stdout,
                                 stderr=stderr, creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            code = child.wait(timeout=arguments.timeout)
        except subprocess.TimeoutExpired:
            subprocess.run(["taskkill", "/PID", str(child.pid), "/T", "/F"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           creationflags=subprocess.CREATE_NO_WINDOW, check=False)
            child.wait(timeout=5)
            raise RuntimeError(f"thread exec probe exceeded {arguments.timeout}s; see {stderr_path}")
    stdout = stdout_path.read_text(encoding="utf-8", errors="replace")
    stderr = stderr_path.read_text(encoding="utf-8", errors="replace")
    result = {"exit_code": code, "failed_exec_recovered": "THREAD_FAILED_EXEC_RECOVERED" in stdout,
              "ifunc_retirement_passed": "THREAD_EXEC_IFUNC_OK" in stdout,
              "gnu_irelative": True, "ifunc_slot_inside_relro": True,
              "stdout": stdout, "stderr": stderr}
    (output / "result.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(json.dumps(result, indent=2))
    if code != 0 or not result["failed_exec_recovered"] or not result["ifunc_retirement_passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
