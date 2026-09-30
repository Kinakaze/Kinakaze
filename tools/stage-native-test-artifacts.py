"""Freeze native tests and SONAME imports from one successful Cargo JSON build."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    rows = [json.loads(line) for line in args.build.read_text(encoding='utf-8-sig').splitlines()
            if line.startswith('{')]
    if not any(row.get('reason') == 'build-finished' and row.get('success') for row in rows):
        parser.error('build log does not contain a successful build-finished record')
    artifacts = [row for row in rows if row.get('reason') == 'compiler-artifact']
    copies, executables = {}, {}
    for row in artifacts:
        definition = Path(row['manifest_path']).parent / 'exports.def'
        alias = None
        if definition.is_file():
            match = re.search(r'^LIBRARY\s+"?([^"\s]+)', definition.read_text(encoding='utf-8'), re.M)
            alias = match[1] if match else None
        for filename in row['filenames']:
            source = Path(filename).resolve(strict=True)
            if source.suffix.lower() == '.dll':
                name = alias or source.name
                if name in copies and copies[name] != source:
                    parser.error(f'ambiguous dependency {name}: {copies[name]} and {source}')
                copies[name] = source
        if row.get('executable') and row['profile']['test']:
            source = Path(row['executable']).resolve(strict=True)
            copies[source.name] = source
            executables[row['target']['name']] = source.name
    if not executables:
        parser.error('build log contains no native test executables')
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    for source in (sysroot / 'bin').glob('std-*.dll'):
        copies[source.name] = source
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    records = {}
    for name, source in copies.items():
        shutil.copy2(source, output / name)
        records[name] = dict(source=str(source), sha256=hashlib.sha256((output / name).read_bytes()).hexdigest())
    manifest = dict(build=str(args.build.resolve()),
                    build_sha256=hashlib.sha256(args.build.read_bytes()).hexdigest(),
                    executables=executables, files=records)
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    print(json.dumps(dict(output=str(output), executables=executables), indent=2))


if __name__ == '__main__':
    main()
