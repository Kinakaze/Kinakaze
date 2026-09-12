"""Measure application executable/dependency startup without opening windows."""
import json
import os
import statistics
import subprocess
import sys
import time

environment = dict(os.environ, DISPLAY="", WAYLAND_DISPLAY="", GDK_BACKEND="x11")
report = {"scope": "warm executable/dependency startup to --version exit; excludes first frame and desktop activation", "apps": {}}
for name in ("gnome-calculator", "gedit", "eog", "evince", "gnome-control-center", "nautilus"):
    samples = []
    for iteration in range(6):
        started = time.perf_counter_ns()
        try:
            result = subprocess.run(["/usr/bin/" + name, "--version"], env=environment,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        except subprocess.TimeoutExpired:
            report["apps"][name] = {"error": "--version timed out after 10 seconds without DISPLAY"}
            break
        elapsed = (time.perf_counter_ns() - started) / 1_000_000
        if result.returncode:
            report["apps"][name] = {"error": result.stderr.decode(errors="replace"), "returncode": result.returncode}
            break
        if iteration:
            samples.append(elapsed)
    else:
        report["apps"][name] = {"median_ms": statistics.median(samples), "samples_ms": samples}
    print(json.dumps({name: report["apps"][name]}), flush=True)
print(json.dumps(report, sort_keys=True), flush=True)
failed = sum("error" in item for item in report["apps"].values())
print(f"StartupAppsProbe: {len(report['apps']) - failed} measured; {failed} failed", flush=True)
sys.exit(bool(failed))
