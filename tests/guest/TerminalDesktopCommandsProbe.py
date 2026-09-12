"""Run real coreutils/procps commands and check Linux filename round trips."""
import json
import os
import subprocess
import tempfile
from pathlib import Path

names = ["plain.txt", "two words.txt", "中文文件.txt", "package:amd64", "trailing."]
with tempfile.TemporaryDirectory(prefix="kinakaze-terminal-") as directory:
    for name in names:
        Path(directory, name).write_text(name)
    listing = subprocess.run(
        ["/bin/ls", "-1", "--quoting-style=literal", "--color=never", directory],
        capture_output=True, text=True, timeout=15,
    )
    assert listing.returncode == 0, (listing.returncode, listing.stderr)
    assert sorted(listing.stdout.splitlines()) == sorted(names), listing.stdout
    assert sorted(os.listdir(directory)) == sorted(names)
    print("LS_FILENAME_ROUNDTRIP_OK", flush=True)
    print(listing.stdout, end="", flush=True)

top = subprocess.run(["/usr/bin/top", "-b", "-n", "1"], capture_output=True, text=True, timeout=20)
assert top.returncode == 0, (top.returncode, top.stderr)
assert "PID" in top.stdout and "Tasks:" in top.stdout, top.stdout
print("\n".join(top.stdout.splitlines()[:12]), flush=True)
print("TOP_REAL_PROCESS_LIST_OK", flush=True)
Path("/tmp/terminal-desktop-commands-result.json").write_text(json.dumps({
    "ls": listing.returncode, "names": names, "top": top.returncode,
    "status": "passed",
}))
