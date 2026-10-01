"""Real redis-cli reads, transactions and persistence through a private socket."""

from pathlib import Path
import subprocess
import tempfile
import time


with tempfile.TemporaryDirectory(prefix="redis-cli-software-") as directory:
    root = Path(directory)
    address = root / "redis.sock"
    logfile = root / "redis.log"
    with logfile.open("wb") as log:
        server = subprocess.Popen(
            [
                "/usr/bin/redis-server",
                "--port",
                "0",
                "--unixsocket",
                str(address),
                "--dir",
                directory,
                "--save",
                "",
                "--appendonly",
                "no",
            ],
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
        )

    def cli(*arguments, input_text=None):
        result = subprocess.run(
            ["/usr/bin/redis-cli", "-s", str(address), "--raw", *arguments],
            input=input_text,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
        )
        assert result.returncode == 0, (arguments, result.stdout, result.stderr)
        return result.stdout.strip()

    try:
        deadline = time.monotonic() + 20
        while not address.exists():
            assert server.poll() is None, "Redis exited before creating its socket"
            assert time.monotonic() < deadline, "Redis socket readiness timeout"
            time.sleep(0.05)
        assert cli("PING") == "PONG"
        assert cli("SET", "software-value", "41") == "OK"
        assert cli("INCR", "software-value") == "42"
        assert cli("GET", "software-value") == "42"
        assert (
            cli(input_text="MULTI\nINCR software-value\nGET software-value\nEXEC\n")
            == "OK\nQUEUED\nQUEUED\n43\n43"
        )
        assert cli("SAVE") == "OK"
        assert (root / "dump.rdb").stat().st_size > 0
        checked = subprocess.run(
            ["/usr/bin/redis-check-rdb", str(root / "dump.rdb")],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
        )
        assert checked.returncode == 0, (checked.stdout, checked.stderr)
        assert "RDB looks OK" in checked.stdout
        print("REDIS_CLI_TRANSACTIONS_SNAPSHOT_OK")
    finally:
        if server.poll() is None:
            server.terminate()
            server.wait(timeout=10)
        print(logfile.read_text(errors="replace")[-8000:])
