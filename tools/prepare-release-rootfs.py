"""Build the first-run seed from locked packages, independent of an old guest root."""
import argparse
import hashlib
import json
from pathlib import Path
import runpy
import tempfile

WORKSPACE = Path(__file__).resolve().parents[1]
preparer = runpy.run_path(str(Path(__file__).with_name('prepare-root.py')))
from native_image import modules


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--cache', type=Path, default=WORKSPACE / 'artifacts/guest-deps')
    args = parser.parse_args()
    dist = args.dist.resolve()
    cache = preparer['PackageCache'](WORKSPACE / 'tools/guest-deps/dependencies.lock.json', args.cache, args.offline)
    manifest = json.loads((WORKSPACE / 'config/rootfs.manifest.json').read_text(encoding='utf-8'))
    with tempfile.TemporaryDirectory(prefix='kinakaze-release-source-') as temporary:
        plan = preparer['RootPlan'](Path(temporary), dist / 'rootfs', cache, set(modules(dist)), None)
        for package in ('busybox', 'netbase'):
            plan.copy_package(package)
        plan.close_dependencies()
        # Entry points use the actual locked BusyBox payload; no old build paths.
        busybox = plan.entries['bin/busybox']
        for name in ('sh', 'cat', 'echo', 'ls', 'mkdir', 'pwd', 'rm', 'sleep', 'true', 'false', 'env', 'printf'):
            plan.entries[f'bin/{name}'] = busybox
        existing = {entry['path'] for entry in manifest['files']}
        for target, (source, _) in sorted(plan.entries.items()):
            if target in existing:
                raise ValueError(f'configuration and package both own {target}')
            data = source.read_bytes()
            digest = hashlib.sha256(data).hexdigest()
            seed = f'rootfs-seed/{digest}'
            preparer['atomic_write'](dist / seed, data)
            manifest['files'].append(dict(path=target, source=seed, sha256=digest))
    # A newly installed root must also contain every native provider and its
    # shared runtime dependencies, not depend on a separate preinstalled root.
    native_files = [*sorted((dist / 'rootfs/lib').iterdir()),
                    dist / 'rootfs/usr/share/doc/kinakaze-libc/copyright']
    for source in native_files:
        if source.is_symlink() or source.is_junction() or not source.is_file():
            raise ValueError(f'invalid native distribution file: {source}')
        data = source.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        seed = f'rootfs-seed/{digest}'
        preparer['atomic_write'](dist / seed, data)
        manifest['files'].append(dict(path=source.relative_to(dist / 'rootfs').as_posix(),
                                     source=seed, sha256=digest))
    preparer['atomic_write'](dist / 'rootfs.manifest.json', (json.dumps(manifest, indent=2) + '\n').encode())
    # Native providers already make the bundled root nonempty. Configure that
    # distribution at packaging time; runtime initialization is only for a new
    # or empty external root, as required by the public startup contract.
    for name in manifest['directories']:
        (dist / 'rootfs' / name).mkdir(parents=True, exist_ok=True)
    for entry in manifest['files']:
        target = dist / 'rootfs' / entry['path']
        if not target.exists():
            data = (dist / entry['source']).read_bytes() if 'source' in entry else entry['content'].encode()
            preparer['atomic_write'](target, data)
    print(f'Prepared first-run manifest: {len(manifest["files"])} files, {len(plan.edges)} verified dependency edges')


if __name__ == '__main__':
    main()
