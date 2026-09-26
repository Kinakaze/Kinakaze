"""Prepare a V2 guest root with a verified, complete Linux ELF dependency closure."""
import argparse
from collections import deque
import hashlib
import json
from pathlib import Path
import shutil
import sys
import zipfile

WORKSPACE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).parent))
from guest_installation import java_home as discover_java_home, minecraft_version
sys.path.insert(0, str(Path(__file__).with_name('guest-deps')))
from guest_deps import PackageCache, atomic_write, elf_info, linux_x86_64_member, relative_path


def same_content(first, second):
    if first.stat().st_size != second.stat().st_size:
        return False
    with first.open('rb') as left, second.open('rb') as right:
        return hashlib.file_digest(left, 'sha256').digest() == hashlib.file_digest(right, 'sha256').digest()


class RootPlan:
    def __init__(self, source, root, cache, provided, java_home):
        self.source, self.root, self.cache = source, root, cache
        self.provided, self.java_home = provided, java_home
        self.entries, self.pending, self.seen, self.edges, self.native_members = {}, deque(), set(), [], []
        self.sonames = {}
        self.packages, self.copying_packages = set(), set()
        self.search = [source / path for path in ('usr/lib', 'lib/x86_64-linux-gnu', 'usr/lib/x86_64-linux-gnu', 'lib', 'usr/lib64')]

    def add(self, origin, target, provenance=None, inspect_jars=False):
        origin = origin.resolve()
        target = relative_path(str(target).replace('\\', '/'))
        if not origin.is_file():
            raise FileNotFoundError(origin)
        allowed = origin.is_relative_to(self.source) or origin.is_relative_to(self.cache.directory)
        if not allowed:
            raise ValueError(f'input resolves outside source/dependency cache: {origin}')
        previous = self.entries.get(target)
        if previous:
            if previous[0] != origin and not same_content(previous[0], origin):
                raise ValueError(f'two different files target {target}')
            if provenance:
                self.entries[target] = (origin, provenance)
            return
        self.entries[target] = (origin, provenance)
        with origin.open('rb') as stream:
            magic = stream.read(4)
        if magic == b'\x7fELF':
            content = origin.read_bytes()
            # Compiler startup objects carry relocations, not DT_NEEDED. They
            # are authenticated package inputs and are never loaded as programs.
            if (len(content) >= 64 and content[:7] == b'\x7fELF\x02\x01\x01'
                    and content[16:20] == b'\x01\x00\x3e\x00'):
                return
            try:
                info = elf_info(content)
            except ValueError as error:
                raise ValueError(f'{origin}: {error}') from error
            if info.soname:
                previous = self.sonames.get(info.soname)
                if previous and previous[0] != origin and not same_content(previous[0], origin):
                    raise ValueError(f'two different inputs provide {info.soname}: {previous[0]} and {origin}')
                self.sonames[info.soname] = (origin, target, provenance)
            self.pending.append((origin, str(origin), info))
        elif inspect_jars and origin.suffix.lower() == '.jar':
            with zipfile.ZipFile(origin) as archive:
                for member in archive.infolist():
                    if member.is_dir() or '.so' not in Path(member.filename).name:
                        continue
                    if member.file_size > 128 * 1024 * 1024:
                        raise ValueError(f'native JAR member exceeds size limit: {origin}!{member.filename}')
                    content = archive.read(member)
                    if not linux_x86_64_member(member.filename, content):
                        continue
                    info = elf_info(content)
                    label = f'{origin}!{member.filename}'
                    self.native_members.append({'archive': target, 'member': member.filename,
                                                'sha256': hashlib.sha256(content).hexdigest(), 'needed': list(info.needed)})
                    self.pending.append((origin, label, info))

    def copy_source(self, relative, target=None, inspect_jars=False):
        relative = Path(relative)
        self.add(self.source / relative, target or relative, inspect_jars=inspect_jars)

    def copy_package(self, name):
        if name in self.packages:
            return
        if name in self.copying_packages:
            raise ValueError(f'locked package dependency cycle: {name}')
        self.copying_packages.add(name)
        files, provenance = self.cache.materialize(name)
        for dependency in self.cache.packages[name].get('requires', []):
            self.copy_package(dependency)
        for target, origin in files.items():
            self.add(origin, target, provenance)
        self.copying_packages.remove(name)
        self.packages.add(name)

    def copy_tree(self, relative, inspect_jars=False):
        directory = self.source / relative
        if not directory.is_dir():
            raise FileNotFoundError(f'required artifact directory: {directory}')
        count = 0
        for path in sorted(directory.rglob('*')):
            if path.is_file():
                self.copy_source(path.relative_to(self.source), inspect_jars=inspect_jars)
                count += 1
        if not count:
            raise ValueError(f'required artifact directory is empty: {directory}')

    def _resolve_local(self, dependency, origin, runpaths):
        if not dependency or '\\' in dependency or '\0' in dependency:
            raise ValueError(f'invalid ELF dependency name: {dependency!r}')
        if '/' in dependency:
            candidates = [self.source / dependency.lstrip('/')] if dependency.startswith('/') else [origin.parent / dependency]
        else:
            candidates = [origin.parent / dependency]
            for runpath in runpaths:
                path = runpath.replace('${ORIGIN}', str(origin.parent)).replace('$ORIGIN', str(origin.parent))
                if '$' in path:
                    continue
                if path.startswith('/'):
                    directory = self.source / path.lstrip('/')
                else:
                    directory = Path(path) if Path(path).is_absolute() else origin.parent / path
                candidates.append(directory / dependency)
            candidates += [directory / dependency for directory in self.search]
            if self.java_home:
                candidates += list((self.source / self.java_home).rglob(dependency))
        for path in candidates:
            if not path.is_file():
                continue
            resolved = path.resolve()
            if not resolved.is_relative_to(self.source):
                continue
            content = resolved.read_bytes()
            if content[:4] != b'\x7fELF':
                raise ValueError(f'{path}: undeclared PE provider cannot enter the guest root')
            elf_info(content)
            return resolved
        return None

    def resolve(self, dependency, origin, label, runpaths=()):
        name = Path(dependency).name
        if name in self.provided:
            self.edges.append({'from': label, 'needed': dependency, 'provider': name})
            return
        if '/' not in dependency and name in self.sonames:
            _, target, provenance = self.sonames[name]
            edge = {'from': label, 'needed': dependency, 'path': target}
            if provenance:
                edge['package'] = provenance['package']
            self.edges.append(edge)
            return
        found = self._resolve_local(dependency, origin, runpaths)
        if found is not None:
            target = found.relative_to(self.source).as_posix()
            self.add(found, target)
            self.edges.append({'from': label, 'needed': dependency, 'path': target})
            return
        result = self.cache.resolve(dependency)
        if result is None:
            raise FileNotFoundError(f'{label}: dependency {dependency}; absent from source and reviewed dependencies.lock.json')
        _, target, provenance, _ = result
        # A runtime library also needs its reviewed plugins, schemas and data.
        # Keep package-owned resources and explicit package edges in the closure.
        self.copy_package(provenance['package'])
        self.edges.append({'from': label, 'needed': dependency, 'path': target, 'package': provenance['package']})

    def close_dependencies(self):
        while self.pending:
            origin, label, info = self.pending.popleft()
            if label in self.seen:
                continue
            self.seen.add(label)
            for dependency in info.needed:
                self.resolve(dependency, origin, label, info.runpaths)
            if info.interpreter:
                self.resolve(info.interpreter, origin, label)

    def install(self, profiles, report_path=None):
        # Preflight has resolved every edge before the first guest-root mutation.
        records = [] if report_path else None
        for directory in ('bin', 'usr/bin', 'usr/lib', 'lib/x86_64-linux-gnu', 'tmp', 'dev/shm', 'etc', 'root'):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        for relative, (origin, provenance) in sorted(self.entries.items()):
            destination = self.root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            # Existing ELF mappings cannot be overwritten on Windows. Reusing
            # byte-identical reviewed payloads also avoids rewriting the whole
            # desktop dependency closure when adding a single application.
            if not destination.is_file() or not same_content(origin, destination):
                shutil.copyfile(origin, destination)
            if records is not None:
                with destination.open('rb') as stream:
                    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
                record = {'source': str(origin.relative_to(self.source)) if origin.is_relative_to(self.source) else str(origin),
                          'path': relative, 'sha256': digest}
                if provenance:
                    record['external_package'] = provenance
                records.append(record)
        if report_path:
            report = {'schema': 2, 'source': str(self.source), 'profiles': profiles,
                      'dependency_lock_sha256': hashlib.sha256(self.cache.lock_path.read_bytes()).hexdigest(),
                      'files': records, 'dependencies': self.edges, 'native_jar_members': self.native_members,
                      'provided_sonames': sorted(self.provided)}
            atomic_write(report_path, json.dumps(report, indent=2).encode('utf-8'))
        return len(self.entries)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--package', action='append', default=[], metavar='NAME',
                        help='seed reviewed package files/trees from dependencies.lock.json')
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--report', type=Path, help='write dependency provenance outside the guest root')
    parser.add_argument('--dist', type=Path, default=WORKSPACE / 'dist')
    parser.add_argument('--java', action='store_true')
    parser.add_argument('--program', action='append', default=[], metavar='RELATIVE_PATH',
                        help='include an additional Linux executable and its ELF dependency closure (repeatable)')
    parser.add_argument('--tree', action='append', default=[], metavar='RELATIVE_DIRECTORY',
                        help='include program resources such as Python stdlib or compiler files (repeatable)')
    parser.add_argument('--busybox-applet', action='append', default=[], metavar='NAME',
                        help='install an explicit BusyBox command alias in /bin (repeatable)')
    parser.add_argument('--minecraft', action='store_true')
    parser.add_argument('--java-home', help='JRE directory relative to source; auto-detected when unique')
    parser.add_argument('--minecraft-version', help='installed version; auto-detected when unique')
    parser.add_argument('--deps-lock', type=Path, default=WORKSPACE / 'tools/guest-deps/dependencies.lock.json')
    parser.add_argument('--deps-cache', type=Path, default=WORKSPACE / 'artifacts/guest-deps')
    parser.add_argument('--offline', action='store_true', help='require the verified package cache; never download')
    parser.add_argument('--check-only', action='store_true', help='resolve the closure and verify/cache packages without copying a root')
    args = parser.parse_args()
    source, root, cache_directory = args.source.resolve(), args.root.resolve(), args.deps_cache.resolve()
    if not source.is_dir():
        raise FileNotFoundError(source)
    if root == source or source in root.parents or root in source.parents:
        raise ValueError('destination and source must be separate, non-overlapping directories')
    if cache_directory == source or source in cache_directory.parents:
        raise ValueError('dependency cache must not modify the source runtime')
    java_home = relative_path(args.java_home) if args.java_home else (discover_java_home(source) if args.java or args.minecraft else None)
    version = relative_path(args.minecraft_version) if args.minecraft_version else (minecraft_version(source) if args.minecraft else None)
    if version and '/' in version:
        raise ValueError('Minecraft version must be one directory name')
    from native_image import modules
    provided = set(modules(args.dist))
    cache = PackageCache(args.deps_lock, cache_directory, args.offline)
    plan = RootPlan(source, root, cache, provided, java_home)
    for program in ('busybox', 'curl'):
        plan.copy_source(Path('usr/bin') / program)
    plan.copy_source('usr/bin/busybox', 'bin/busybox')
    plan.copy_source('usr/bin/busybox', 'bin/sh')
    for name in ('passwd', 'group', 'hosts', 'services', 'protocols', 'networks', 'resolv.conf', 'nsswitch.conf', 'environment', 'ssl/certs/ca-certificates.crt'):
        path = Path('etc') / name
        # Adding packages to an existing desktop must retain its accounts,
        # selected shell, resolver and other locally configured defaults.
        if (source / path).is_file() and not (root / path).exists():
            plan.copy_source(path)
    profiles = ['busybox', 'curl']
    # Debian/OpenSSL's compiled default CA file is a link to this bundle.
    # Materialize the verified input at that guest path as well, so Python and
    # other OpenSSL callers discover trust roots without injected environment.
    ca_bundle = 'etc/ssl/certs/ca-certificates.crt'
    openssl_ca = 'usr/lib/ssl/cert.pem'
    if (source / openssl_ca).is_file():
        plan.copy_source(openssl_ca)
    elif (source / ca_bundle).is_file():
        plan.copy_source(ca_bundle, openssl_ca)
    for package in args.package:
        plan.copy_package(package)
        profiles.append(f'package:{package}')
    for applet in args.busybox_applet:
        if not applet or not all(character.isascii() and (character.isalnum() or character in '_-[') for character in applet):
            raise ValueError(f'invalid BusyBox applet name: {applet!r}')
        plan.copy_source('usr/bin/busybox', f'bin/{applet}')
        profiles.append(f'busybox:{applet}')
    for program in args.program:
        program = relative_path(program)
        plan.copy_source(program)
        profiles.append(f'program:{program}')
    for directory in args.tree:
        directory = relative_path(directory)
        plan.copy_tree(directory)
        profiles.append(f'tree:{directory}')
    if args.java or args.minecraft:
        profiles.append('java')
        plan.copy_tree(java_home)
        # OpenJDK discovers fontconfig with dlopen, outside DT_NEEDED.
        plan.resolve('libfontconfig.so.1', source / java_home / 'bin/java', 'java:font-discovery')
    if args.minecraft:
        profiles.append('minecraft')
        for directory in (f'versions/{version}', 'libraries', 'assets'):
            plan.copy_tree(Path('minecraft') / directory, inspect_jars=True)
        # OSHI/JNA explicitly dlopen libudev while querying Linux hardware.
        plan.resolve('libudev.so.1', source / 'minecraft', 'minecraft:hardware-discovery')
    plan.close_dependencies()
    if args.check_only:
        print(f'Checked {len(plan.entries)} files, {len(plan.edges)} dependency edges and {len(plan.native_members)} Linux x86-64 native JAR members; root untouched')
    else:
        count = plan.install(profiles, args.report)
        print(f'Prepared {count} files in {root}; all {len(plan.edges)} ELF dependency edges resolved')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, zipfile.BadZipFile) as error:
        raise SystemExit(f'prepare-root: {error}') from error
