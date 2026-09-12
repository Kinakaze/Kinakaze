"""Ancestor shortcuts must observe directory replacement and symlinks immediately."""
import os
import tempfile
import time
from pathlib import Path

with tempfile.TemporaryDirectory(prefix="kinakaze-ancestor-") as temporary:
    root = Path(temporary)
    (root / "first").mkdir()
    (root / "first/item").write_bytes(b"first")
    (root / "second").mkdir()
    (root / "second/item").write_bytes(b"second-value")
    os.symlink("first", root / "current")
    assert (root / "current/item").read_bytes() == b"first"
    os.unlink(root / "current")
    os.symlink("second", root / "current")
    assert (root / "current/item").read_bytes() == b"second-value"
    os.rename(root / "first", root / "old-first")
    os.symlink("second", root / "first")
    assert (root / "first/item").read_bytes() == b"second-value"
    assert os.path.islink(root / "first")
    assert os.readlink(root / "first") == "second"

path = "/usr/share/icons/Adwaita/scalable/actions/document-open-symbolic.svg"
size = os.stat(path).st_size
started = time.perf_counter()
for _ in range(300):
    assert os.stat(path).st_size == size
elapsed_ms = (time.perf_counter() - started) * 1000
print("PATH_ANCESTOR_REPLACEMENT_SYMLINK_OK", "stat_300_ms", round(elapsed_ms, 2), flush=True)
