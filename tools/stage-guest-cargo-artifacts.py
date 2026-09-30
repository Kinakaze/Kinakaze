"""Stage one complete Cargo build into a new guest distribution snapshot."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('build', 'base', 'output', 'report'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    rows = [json.loads(line) for line in args.build.read_text(encoding='utf-8-sig').splitlines()
            if line.startswith('{')]
    if not any(row.get('reason') == 'build-finished' and row.get('success') for row in rows):
        parser.error('build log does not contain a successful build-finished record')
    if args.output.exists():
        parser.error('output already exists; choose a new snapshot directory')
    copies = {}
    for row in rows:
        if row.get('reason') != 'compiler-artifact':
            continue
        name = row['target']['name']
        if name in ('init', 'worker') and row.get('executable'):
            copies[name + '.exe'] = Path(row['executable']).resolve(strict=True)
        definition = Path(row['manifest_path']).parent / 'exports.def'
        if not definition.is_file():
            continue
        match = re.search(r'^LIBRARY\s+"?([^"\s]+)', definition.read_text(encoding='utf-8'), re.M)
        if not match:
            continue
        for filename in row['filenames']:
            if filename.endswith('.dll'):
                destination = 'rootfs/lib/' + match[1]
                source = Path(filename).resolve(strict=True)
                if destination in copies and copies[destination] != source:
                    parser.error(f'ambiguous dependency {destination}')
                copies[destination] = source
    required = {'init.exe', 'worker.exe', 'rootfs/lib/libc.so.6',
                'rootfs/lib/kinakaze_kernel.so', 'rootfs/lib/kinakaze_vfs.so'}
    if not required <= copies.keys():
        parser.error(f'missing build artifacts: {sorted(required - copies.keys())}')
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    for source in (sysroot / 'bin').glob('std-*.dll'):
        copies[source.name] = source
    shutil.copytree(args.base, args.output)
    records = {}
    for name, source in copies.items():
        destination = args.output / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        records[name] = dict(source=str(source), sha256=hashlib.sha256(destination.read_bytes()).hexdigest())
    manifest_path = args.output / 'rootfs.manifest.json'
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    for row in manifest['files']:
        name = 'rootfs/' + row['path']
        if name in records:
            row['sha256'] = records[name]['sha256']
    manifest_path.write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    report = dict(build=str(args.build.resolve()), build_sha256=hashlib.sha256(args.build.read_bytes()).hexdigest(),
                  base=str(args.base.resolve()), candidate=str(args.output.resolve()), files=records)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(dict(candidate=report['candidate'], staged=len(records)), indent=2))


if __name__ == '__main__':
    main()
