"""Build a first-run Debian manifest; release images fetch locked packages online."""
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
from rootfs_bootstrap import configure_packages, configure_standard, preserve_debconf_extractor
from debian_standard import load as load_standard, stored_path


def prepare(dist, cache, preset_path, online=False, elf_imports=None):
    preset = json.loads(preset_path.read_text(encoding='utf-8'))
    if preset.get('schema') != 1:
        raise ValueError('unsupported rootfs package preset')
    manifest = json.loads((WORKSPACE / 'config/rootfs.manifest.json').read_text(encoding='utf-8'))
    # Keep the editable guest service source authoritative in release seeds.
    for entry in manifest['files']:
        if entry['path'] == 'usr/lib/kinakaze/session.py':
            entry['content'] = (WORKSPACE / 'config/session.py').read_text(encoding='utf-8')
    payloads = {entry['path']: entry['content'].encode() for entry in manifest['files']}
    version = tomllib.loads((WORKSPACE / 'Cargo.toml').read_text(encoding='utf-8'))['workspace']['package']['version']
    links, modes, package_directories = {}, {}, set()
    locked = cache.packages
    sources = {}
    if 'debian_standard_lock' in preset:
        manifest['case_sensitive'] = True
        lock_path = preset_path.parent / preset['debian_standard_lock']
        base, links, modes, package_directories, records, adaptations = load_standard(
            lock_path, cache.directory, set(modules(dist)), cache.offline, sources=sources)
        # Config overlays are explicit Kinakaze policy; package data is retained
        # in the locked original archives and all other payloads are installed.
        preserve_debconf_extractor(base, payloads)
        base.update(payloads)
        payloads = base
        packages = sorted(record['package'] for record in records)
        locked = {record['package']: record for record in records}
        edges = None
        for item in adaptations:
            if item['reason'] == 'native ABI provider':
                links[item['path']] = '/lib/' + item.get('provider', Path(item['path']).name)
        payloads['usr/share/kinakaze/debian-standard.json'] = (json.dumps(
            dict(schema=1, selection=json.loads(lock_path.read_text())['selection'],
                 adaptations=adaptations), indent=2) + '\n').encode()
        configure_standard(payloads, links, preset)
        modes['etc/gshadow'] = 0o600
    else:
        if online:
            raise ValueError('online installation requires a Debian standard package lock')
        with tempfile.TemporaryDirectory(prefix='kinakaze-release-source-') as temporary:
            plan = preparer['RootPlan'](Path(temporary), dist / 'rootfs', cache, set(modules(dist)), None)
            for package in preset['packages']:
                plan.copy_package(package)
            plan.close_dependencies()
            for target, (source, _) in sorted(plan.entries.items()):
                if target in payloads:
                    raise ValueError(f'configuration and package both own {target}')
                payloads[target] = source.read_bytes()
            packages = sorted(plan.packages)
            edges = len(plan.edges)
    # Explicit manifest modes also apply to generated/overlaid configuration.
    modes.update(manifest.get('permissions', {}))
    links.update(manifest.get('links', {}))
    modes['etc/shadow'] = 0o600
    for name in preset['busybox_applets']:
        if '/' in name or preparer['relative_path'](name) != name:
            raise ValueError(f'invalid BusyBox applet: {name}')
        if not any(f'{directory}/{name}' in payloads or f'{directory}/{name}' in links
                   for directory in ('bin', 'usr/bin', 'sbin', 'usr/sbin')):
            links[f'bin/{name}'] = '/bin/busybox'
    linker_targets = set()
    native_files = [*sorted(path for path in (dist / 'rootfs/lib').iterdir() if path.is_file()),
                    dist / 'rootfs/usr/share/doc/kinakaze-libc/copyright',
                    dist / 'rootfs/usr/share/doc/kinakaze-libm/copyright']
    for source in native_files:
        if source.is_symlink() or source.is_junction() or not source.is_file():
            raise ValueError(f'invalid native distribution file: {source}')
        target = source.relative_to(dist / 'rootfs').as_posix()
        if target in payloads:
            raise ValueError(f'package would replace native provider: {target}')
        payloads[target] = source.read_bytes()
        if online and source.parent == dist / 'rootfs/lib':
            preparer['atomic_write'](dist / 'native' / source.name, payloads[target])
        # Guest linkers need ELF interfaces in offline images too. Native PE
        # providers remain in /lib for runtime loading; they are not ELF input
        # files that GCC, Clang or Rust's linker can consume.
        if source.parent == dist / 'rootfs/lib':
            if elf_imports is not None and source.name in {
                    'libc.so.6', 'libm.so.6', 'libpthread.so.0', 'libdl.so.2',
                    'librt.so.1', 'libresolv.so.2', 'ld-linux-x86-64.so.2'}:
                interface = (elf_imports / source.name).read_bytes()
                if not interface.startswith(b'\x7fELF'):
                    raise ValueError(f'invalid ELF linker interface: {source.name}')
                directory = 'usr/lib' if source.name == 'ld-linux-x86-64.so.2' else 'lib'
                interface_target = f'{directory}/x86_64-linux-gnu/{source.name}'
                payloads[interface_target] = interface
                linker_targets.add(interface_target)
                if source.name == 'ld-linux-x86-64.so.2':
                    # A GNU linker script may otherwise consume the native PE
                    # command facade, producing invalid leaf shared libraries.
                    # Keep the executable /lib64 alias intact and change only
                    # the link-time input when the development script exists.
                    script = 'usr/lib/x86_64-linux-gnu/libc.so'
                    if script in payloads:
                        payloads[script] = payloads[script].replace(
                            b'/lib64/ld-linux-x86-64.so.2',
                            ('/' + interface_target).encode())
    configure_packages(payloads, locked, packages, preset['native_packages'], version, links)
    for name in payloads:
        links.pop(name, None)
    payloads = {stored_path(name): data for name, data in payloads.items()}
    sources = {stored_path(name): entry for name, entry in sources.items()}
    by_hash = {entry['sha256']: entry for entry in sources.values()}
    links = {stored_path(name): target for name, target in links.items()}
    modes = {stored_path(name): mode for name, mode in modes.items()}
    # Never overwrite package-manager state when rebuilding a used image.
    expected = set(payloads) | set(links)
    root = dist / 'rootfs'
    for path in root.rglob('*'):
        if path.is_symlink() or path.is_junction():
            raise ValueError(f'redirected distribution input: {path}')
        if path.is_file():
            target = path.relative_to(root).as_posix()
            if target not in expected or target in payloads and path.read_bytes() != payloads[target]:
                raise ValueError(f'distribution root has modified or unplanned files: {path}; build into a fresh dist')
    manifest['files'] = []
    directories = set(manifest['directories']) | {stored_path(name) for name in package_directories}
    for target in set(payloads) | set(links):
        directories.update(parent.as_posix() for parent in Path(target).parents if parent.as_posix() != '.')
    directories.difference_update(links)
    manifest['directories'] = sorted(directories)
    manifest['permissions'] = {name: 0o755 for name in directories}
    manifest['permissions'].update({name: mode for name, mode in modes.items() if name in directories})
    manifest['permissions'].update({'/': 0o755, 'root': 0o700, 'tmp': 0o1777, 'var/tmp': 0o1777, 'dev/shm': 0o1777})
    for target, data in sorted(payloads.items()):
        digest = hashlib.sha256(data).hexdigest()
        if online:
            native_source = dist / 'native' / Path(target).name
            if target.startswith('lib/') and native_source.is_file() and native_source.read_bytes() == data:
                entry = dict(source='native/' + Path(target).name, sha256=digest)
            elif target in linker_targets:
                seed = f'rootfs-seed/{digest}'
                preparer['atomic_write'](dist / seed, data)
                entry = dict(source=seed, sha256=digest)
            elif digest in by_hash:
                entry = by_hash[digest]
            else:
                entry = dict(content=data.decode('utf-8'))
            manifest['files'].append(dict(path=target, **entry))
        else:
            seed = f'rootfs-seed/{digest}'
            preparer['atomic_write'](dist / seed, data)
            manifest['files'].append(dict(path=target, source=seed, sha256=digest))
        manifest['permissions'][target] = modes.get(target, 0o755 if data.startswith((b'\x7fELF', b'#!', b'MZ')) else 0o644)
    manifest['links'] = links
    if online:
        manifest['archives'] = {record['package']: {key: record[key] for key in ('url', 'sha256', 'size')}
                                for record in records}
    preparer['atomic_write'](dist / 'rootfs.manifest.json', (json.dumps(manifest, indent=2) + '\n').encode())
    if online:
        # The verified build-owned native root is already copied to native/.
        # Keep the default destination absent so first launch runs the installer.
        if root.resolve().parent != dist.resolve():
            raise ValueError('native staging root escaped distribution')
        with tempfile.TemporaryDirectory(prefix='kinakaze-native-', dir=dist) as temporary:
            root.rename(Path(temporary) / 'rootfs')
        print(f'Prepared online Debian manifest: {len(payloads)} files, {len(packages)} packages; rootfs is created on first launch')
        return
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
    print(f'Prepared first-run manifest: {len(payloads)} files, {len(links)} links, {len(packages)} packages'
          + (f', {edges} verified ELF dependency edges' if edges is not None else ', complete Debian standard APT plan'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--online', action='store_true', help='generate a small release that downloads Debian packages on first launch')
    parser.add_argument('--elf-imports', type=Path, help='generated ELF interfaces for native C libraries')
    parser.add_argument('--cache', type=Path, default=WORKSPACE / 'artifacts/guest-deps')
    parser.add_argument('--preset', type=Path, default=WORKSPACE / 'config/rootfs.packages.json')
    args = parser.parse_args()
    cache = preparer['PackageCache'](WORKSPACE / 'tools/guest-deps/dependencies.lock.json', args.cache, args.offline)
    prepare(args.dist.resolve(), cache, args.preset, args.online, args.elf_imports)


if __name__ == '__main__':
    main()
