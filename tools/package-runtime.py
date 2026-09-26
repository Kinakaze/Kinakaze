"""Archive a tested, configured native distribution with notices and hashes."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'artifacts/releases')
    parser.add_argument('--source-cache', type=Path, default=ROOT / 'artifacts/guest-sources')
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    report = json.loads(args.report.read_text(encoding='utf-8'))
    if report.get('passed') is not True:
        raise ValueError('runtime acceptance report is not passing')
    dist = args.dist.resolve()
    manifest = json.loads((dist / 'rootfs.manifest.json').read_text(encoding='utf-8'))
    if (dist / 'rootfs/.kinakaze-rootfs.sha256').exists():
        raise ValueError('publish an uninitialized distribution, not a used guest root')
    files = {}
    for name in ('init.exe', 'worker.exe', 'rootfs.manifest.json'):
        files[name] = (dist / name).read_bytes()
    for directory in ('rootfs',):
        for path in sorted((dist / directory).rglob('*')):
            if path.is_symlink() or path.is_junction():
                raise ValueError(f'redirected distribution input: {path}')
            if path.is_file():
                files[path.relative_to(dist).as_posix()] = path.read_bytes()
    for entry in manifest['files']:
        if 'source' in entry:
            if entry['source'] != f'rootfs-seed/{entry["sha256"]}' or len(entry['sha256']) != 64:
                raise ValueError(f'invalid release seed: {entry["source"]}')
            source = dist / entry['source']
            if source.is_symlink() or source.is_junction() or source.parent.is_junction():
                raise ValueError(f'redirected seed: {source}')
            files[entry['source']] = source.read_bytes()
        if 'source' in entry and hashlib.sha256(files[entry['source']]).hexdigest() != entry['sha256']:
            raise ValueError(f'seed hash mismatch: {entry["path"]}')
        expected = entry.get('sha256') or hashlib.sha256(entry['content'].encode()).hexdigest()
        if hashlib.sha256(files[f'rootfs/{entry["path"]}']).hexdigest() != expected:
            raise ValueError(f'bundled root differs from first-install manifest: {entry["path"]}')
    planned = {f'rootfs/{entry["path"]}' for entry in manifest['files']}
    unexpected = {name for name in files if name.startswith('rootfs/')} - planned
    if unexpected:
        raise ValueError(f'root contains files absent from the install manifest: {sorted(unexpected)}')
    # Nonempty roots skip initialization. Keep even empty mount/temp directories
    # in the ZIP so the bundled root has the same layout as a first install.
    for directory in manifest['directories']:
        if not (dist / 'rootfs' / directory).is_dir():
            raise ValueError(f'missing root directory: {directory}')
        files[f'rootfs/{directory}/'] = b''
    if not report.get('images'):
        raise ValueError('acceptance report has no image hashes')
    for name, expected in report['images'].items():
        if hashlib.sha256(files[name]).hexdigest() != expected:
            raise ValueError(f'acceptance report belongs to a different image: {name}')
    for name in ('LICENSE-MIT', 'LICENSE-APACHE', 'THIRD_PARTY.md'):
        files[name] = (ROOT / name).read_bytes()
    files['README.md'] = (ROOT / 'docs/runtime-release-v0.1.0.md').read_bytes()
    files['validation.json'] = args.report.read_bytes()
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
        '--filter-platform', 'x86_64-pc-windows-msvc'], cwd=ROOT))
    dependencies = []
    for package in metadata['packages']:
        if package['id'] in metadata['workspace_members']:
            continue
        directory = Path(package['manifest_path']).parent
        label = f'{package["name"]}-{package["version"]}'
        dependencies.append(dict(name=package['name'], version=package['version'], license=package['license']))
        for path in sorted(directory.iterdir()):
            if path.is_file() and path.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE', 'COPYRIGHT')):
                files[f'licenses/{label}/{path.name}'] = path.read_bytes()
    files['licenses/dependencies.json'] = (json.dumps(dependencies, indent=2) + '\n').encode()
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    files['licenses/rust/COPYRIGHT-library.html'] = (sysroot / 'share/doc/rust/COPYRIGHT-library.html').read_bytes()
    files['licenses/kinakaze-libm-powl.c'] = (ROOT / 'libs/libm/src/ld80/powl.c').read_bytes()
    source_lock = ROOT / 'config/release-sources.lock.json'
    files['sources/manifest.json'] = source_lock.read_bytes()
    for package in json.loads(source_lock.read_text(encoding='utf-8'))['packages']:
        for source in package['files']:
            filename = source['filename']
            if Path(filename).name != filename or '/' in filename or '\\' in filename:
                raise ValueError(f'invalid source filename: {filename}')
            cached = args.source_cache / filename
            if cached.exists():
                data = cached.read_bytes()
            elif args.offline:
                raise FileNotFoundError(f'corresponding source is not cached: {filename}')
            else:
                with urllib.request.urlopen(source['url'], timeout=60) as response:
                    data = response.read()
            if len(data) != source['size'] or hashlib.sha256(data).hexdigest() != source['sha256']:
                raise ValueError(f'corresponding source hash mismatch: {filename}')
            if not cached.exists():
                args.source_cache.mkdir(parents=True, exist_ok=True)
                cached.write_bytes(data)
            files[f'sources/{package["package"]}/{filename}'] = data
    files['build.json'] = (json.dumps(dict(version=version, revision=revision,
        rustc=subprocess.check_output(['rustc', '--version'], text=True).strip()), indent=2) + '\n').encode()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    archive = args.output_dir / f'Kinakaze-{version}-windows-x86_64.zip'
    prefix = f'Kinakaze-{version}/'
    timestamp = int(subprocess.check_output(['git', 'show', '-s', '--format=%ct', 'HEAD'], cwd=ROOT))
    date = datetime.fromtimestamp(timestamp, timezone.utc).timetuple()[:6]
    with zipfile.ZipFile(archive, 'x', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as output:
        for name, data in sorted(files.items()):
            info = zipfile.ZipInfo(prefix + name, date_time=date)
            info.compress_type = zipfile.ZIP_DEFLATED
            output.writestr(info, data)
    with archive.open('rb') as stream:
        checksum = hashlib.file_digest(stream, 'sha256').hexdigest()
    archive.with_suffix('.zip.sha256').write_text(f'{checksum}  {archive.name}\n', encoding='utf-8')
    print(archive)


if __name__ == '__main__':
    main()
