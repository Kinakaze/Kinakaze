"""Audit versioned ELF imports against native images in a distribution.

Reads ELF files, directories, ZIPs and JARs. Native Linux dependencies outside
the native images are reported separately for the guest-root preparer.
"""
import argparse
import json
from pathlib import Path

from generate import ROOT, import_inventory
import sys
sys.path.insert(0, str(ROOT / "tools"))
from native_image import modules


def audit(images, observed, evidence):
    missing, external = [], set()
    for (soname, name), versions in sorted(observed.items()):
        if soname not in images:
            external.add(soname)
            continue
        exports = images[soname]
        if name not in exports and not any(export.startswith(name + '@') for export in exports):
            missing.append(dict(soname=soname, name=name, versions=sorted(versions), reason="missing_symbol"))
        elif absent := {version for version in versions if f'{name}@{version}' not in exports}:
            missing.append(dict(soname=soname, name=name, versions=sorted(absent), reason="missing_version"))
    return dict(native_elf_members=len(evidence), version_requirements=len(observed),
                missing=missing, external_libraries=sorted(external), evidence=evidence)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--observe", type=Path, action="append", required=True)
    parser.add_argument("--dist", type=Path, default=ROOT / "dist")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    observed, evidence = import_inventory(args.observe)
    report = audit(modules(args.dist), observed, evidence)
    text = json.dumps(report, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
    print(json.dumps({key: value for key, value in report.items() if key != "evidence"}, indent=2))
    return int(bool(report["missing"]))


if __name__ == "__main__":
    raise SystemExit(main())
