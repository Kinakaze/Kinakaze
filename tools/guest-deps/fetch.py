"""Fetch and verify the exact packages in dependencies.lock.json."""
import argparse
from pathlib import Path
from guest_deps import PackageCache

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--lock", type=Path, default=Path(__file__).with_name("dependencies.lock.json"))
parser.add_argument("--cache", type=Path, default=Path(__file__).resolve().parents[2] / "artifacts/guest-deps")
parser.add_argument("--offline", action="store_true")
args = parser.parse_args()
cache = PackageCache(args.lock, args.cache, args.offline)
cache.fetch_all()
print(f"Verified {len(cache.packages)} locked Debian package(s) in {cache.directory}")
