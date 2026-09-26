"""Build an offline first-run image, including APT, from reviewed package locks."""
import argparse
import hashlib
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import tomllib

WORKSPACE = Path(__file__).resolve().parents[1]
preparer = runpy.run_path(str(Path(__file__).with_name('prepare-root.py')))
from native_image import modules
from rootfs_bootstrap import configure_packages


def prepare(dist, cache, preset_path):
    preset = json.loads(preset_path.read_text(encoding='utf-8'))
    if preset.get('schema') != 1:
        raise ValueError('unsupported rootfs package preset')
    manifest = json.loads((WORKSPACE / 'config/rootfs.manifest.json').read_text(encoding='utf-8'))
    payloads = {entry['path']: entry['content'].encode() for entry in manifest['files']}
    version = tomllib.loads((WORKSPACE / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
    with tempfile.TemporaryDirectory(prefix='kinakaze-release-source-') as temporary:
        plan = preparer['RootPlan'](Path(temporary), dist / 'rootfs', cache, set(modules(dist)), None)
        for package in preset['packages']:
            plan.copy_package(package)
        plan.close_dependencies()
        busybox = plan.entries['bin/busybox']
        for name in preset['busybox_applets']:
            if '/' in name or preparer['relative_path'](name) != name:
                raise ValueError(f'invalid BusyBox applet: {name}')
            if not any(f'{directory}/{name}' in plan.entries for directory in ('bin', 'usr/bin', 'sbin', 'usr/sbin')):
                plan.entries[f'bin/{name}'] = busybox
        for target, (source, _) in sorted(plan.entries.items()):
            if target in payloads:
                raise ValueError(f'configuration and package both own {target}')
            payloads[target] = source.read_bytes()
        packages = sorted(plan.packages)
        edges = len(plan.edges)
    native_files = [*sorted(path for path in (dist / 'rootfs/lib').iterdir() if path.is_file()),
                    dist / 'rootfs/usr/share/doc/kinakaze-libc/copyright']
    for source in native_files:
        if source.is_symlink() or source.is_junction() or not source.is_file():
            raise ValueError(f'invalid native distribution file: {source}')
        target = source.relative_to(dist / 'rootfs').as_posix()
        if target in payloads:
            raise ValueError(f'package would replace native provider: {target}')
        payloads[target] = source.read_bytes()
    configure_packages(payloads, cache.packages, packages, preset['native_packages'], version)
    # Never overwrite package-manager state when rebuilding a used image.
    expected = set(payloads)
    root = dist / 'rootfs'
    for path in root.rglob('*'):
        if path.is_symlink() or path.is_junction():
            raise ValueError(f'redirected distribution input: {path}')
        if path.is_file():
            target = path.relative_to(root).as_posix()
            if target not in expected or path.read_bytes() != payloads[target]:
                raise ValueError(f'distribution root has modified or unplanned files: {path}; build into a fresh dist')
    manifest['files'] = []
    directories = set(manifest['directories'])
    for target in payloads:
        directories.update(parent.as_posix() for parent in Path(target).parents if parent.as_posix() != '.')
    manifest['directories'] = sorted(directories)
    manifest['permissions'] = {name: 0o755 for name in directories}
    manifest['permissions'].update({'/': 0o755, 'root': 0o700, 'tmp': 0o1777, 'var/tmp': 0o1777, 'dev/shm': 0o1777})
    for target, data in sorted(payloads.items()):
        digest = hashlib.sha256(data).hexdigest()
        seed = f'rootfs-seed/{digest}'
        preparer['atomic_write'](dist / seed, data)
        manifest['files'].append(dict(path=target, source=seed, sha256=digest))
        manifest['permissions'][target] = 0o755 if data.startswith((b'\x7fELF', b'#!', b'MZ')) else 0o644
    preparer['atomic_write'](dist / 'rootfs.manifest.json', (json.dumps(manifest, indent=2) + '\n').encode())
    preparer['atomic_write'](dist / 'kinakaze.cmd', (WORKSPACE / 'config/kinakaze.cmd').read_bytes())
    # Use the same offline installer as end users so NTFS inode permissions are
    # identical. RootPlan has finished reading every native file above.
    with tempfile.TemporaryDirectory(prefix='kinakaze-image-', dir=dist) as temporary:
        installed = Path(temporary) / 'rootfs'
        subprocess.run([str(dist / 'worker.exe'), 'setup', '--root', str(installed), '--dist', str(dist)], check=True)
        (installed / '.kinakaze-rootfs.sha256').unlink()
        # Only the preflight-verified, build-owned distribution root is replaced.
        backup = Path(temporary) / 'native-root'
        root.rename(backup)
        try:
            installed.rename(root)
        except BaseException:
            backup.rename(root)
            raise
    print(f'Prepared first-run manifest: {len(payloads)} files, {len(packages)} packages, {edges} verified dependency edges')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--cache', type=Path, default=WORKSPACE / 'artifacts/guest-deps')
    parser.add_argument('--preset', type=Path, default=WORKSPACE / 'config/rootfs.packages.json')
    args = parser.parse_args()
    cache = preparer['PackageCache'](WORKSPACE / 'tools/guest-deps/dependencies.lock.json', args.cache, args.offline)
    prepare(args.dist.resolve(), cache, args.preset)


if __name__ == '__main__':
    main()
