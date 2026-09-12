"""Headless warm-cache startup/I/O measurements; touches only a private temp dir."""
import json
import os
import statistics
import subprocess
import tempfile
import time


def measure(operation, repeats=7):
    operation()
    samples = []
    for _ in range(repeats):
        started = time.perf_counter_ns()
        operation()
        samples.append((time.perf_counter_ns() - started) / 1_000_000)
    return {"median_ms": statistics.median(samples), "samples_ms": samples}


def main():
    with tempfile.TemporaryDirectory(prefix="kinakaze-startup-io-") as directory:
        path = os.path.join(directory, "data")
        size = 8 * 1024 * 1024
        block = bytes(range(256)) * 4096
        with open(path, "wb", buffering=0) as output:
            for _ in range(size // len(block)):
                output.write(block)
        report = {"scope": "warm page cache; no GUI input; no concurrent build", "bytes": size}
        for chunk in (4096, 65536, 1048576):
            def read_file():
                total = 0
                with open(path, "rb", buffering=0) as source:
                    while True:
                        data = source.read(chunk)
                        if not data:
                            break
                        total += len(data)
                        assert data[0] == 0 and data[-1] == 255
                assert total == size
            report[f"read_{chunk}"] = measure(read_file)

        def metadata():
            for _ in range(200):
                assert os.stat(path).st_size == size
        report["stat_200"] = measure(metadata)

        def launch():
            subprocess.run(["/bin/busybox", "true"], check=True)
        report["spawn_busybox"] = measure(launch, 12)
        print(json.dumps(report, sort_keys=True), flush=True)
        print("StartupIoProbe: PASS", flush=True)


if __name__ == "__main__":
    main()
