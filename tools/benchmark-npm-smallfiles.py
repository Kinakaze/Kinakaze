"""Compare complete offline npm installs of deterministic small-file tarballs.

Each run has a new project and cache. Lifecycle scripts remain enabled. Package
contents are verified after timing; this measures installation, not downloading.
"""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import statistics
import tarfile
import uuid

from init_pool import InitPool, distribution_hashes


def write_json(path, value):
    path.write_bytes(json.dumps(value, indent=2).encode('utf-8'))


def fixtures(directory, packages, files, size):
    tarballs = directory / 'tarballs'
    tarballs.mkdir()
    dependencies, expected, archives = {}, {}, {}
    for package in range(packages):
        name = f'smallfile-{package:04}'
        content = {
            'package.json': json.dumps(dict(name=name, version='1.0.0', main='index.js')).encode(),
            'index.js': f'module.exports = {package};\n'.encode(),
        }
        for index in range(files):
            seed = f'// {name}/{index:04}\n'.encode()
            content[f'lib/{index:04}.js'] = (seed * (size // len(seed) + 1))[:size]
        archive = tarballs / (name + '.tgz')
        with archive.open('wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as tar:
                for filename, payload in content.items():
                    entry = tarfile.TarInfo('package/' + filename)
                    entry.size, entry.mode, entry.mtime = len(payload), 0o644, 0
                    tar.addfile(entry, io.BytesIO(payload))
        archives[archive.name] = hashlib.sha256(archive.read_bytes()).hexdigest()
        dependencies[name] = 'file:../tarballs/' + archive.name
        expected.update({f'node_modules/{name}/{filename}': hashlib.sha256(payload).hexdigest()
                         for filename, payload in content.items()})
    manifest = dict(name='smallfiles-install-benchmark', version='1.0.0', private=True,
                    dependencies=dependencies,
                    scripts=dict(postinstall='node -e "require(\'fs\').writeFileSync(\'lifecycle-ok\', \'enabled\')"'))
    return manifest, expected, archives


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'control', 'candidate', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--rounds', type=int, default=5)
    parser.add_argument('--packages', type=int, default=64)
    parser.add_argument('--files', type=int, default=32)
    parser.add_argument('--size', type=int, default=4096)
    parser.add_argument('--timeout', type=float, default=120)
    args = parser.parse_args()
    if not 1 <= args.rounds <= 30 or min(args.packages, args.files, args.size) <= 0:
        parser.error('require 1..30 rounds and positive fixture dimensions')
    args.output.mkdir(parents=True, exist_ok=False)
    guest = '/var/tmp/kinakaze-npm-smallfiles-' + uuid.uuid4().hex
    directory = args.root.resolve() / guest.lstrip('/')
    directory.mkdir(parents=True)
    manifest, expected, archives = fixtures(directory, args.packages, args.files, args.size)
    report = dict(passed=False, root=str(args.root.resolve()), fixture=guest,
                  packages=args.packages, payload_files=args.files, payload_bytes=args.size,
                  verified_files_per_install=len(expected), lifecycle_scripts=True,
                  fresh_cache_per_install=True, archive_sha256=archives,
                  distributions={name: dict(path=str(dist.resolve()), sha256=distribution_hashes(dist))
                                 for name, dist in [('control', args.control), ('candidate', args.candidate)]}, rows=[])
    write_json(args.output / 'report.json', report)
    try:
        for iteration in range(args.rounds + 1):
            order = ['control', 'candidate'] if iteration % 2 == 0 else ['candidate', 'control']
            for label in order:
                project = directory / f'{label}-{iteration:02}'
                project.mkdir()
                write_json(project / 'package.json', manifest)
                cwd = guest + '/' + project.name
                dist = getattr(args, label)
                with InitPool(args.root, dist, args.output / project.name, size=1, timeout=args.timeout) as pool:
                    row = pool.run(['/usr/bin/node', '/usr/share/nodejs/npm/bin/npm-cli.js', 'install',
                                    '--offline', '--no-audit', '--no-fund', '--cache', cwd + '/cache'],
                                   cwd=cwd, environment=['PATH=/usr/bin:/bin', 'LC_ALL=C'])
                row.update(label=label, round=iteration, warmup=iteration == 0)
                if row['status'] != 'passed':
                    report['rows'].append(row)
                    raise RuntimeError(f'npm install failed: {project.name}; inspect pool logs')
                for name, digest in expected.items():
                    if hashlib.sha256((project / name).read_bytes()).hexdigest() != digest:
                        raise RuntimeError(f'package content mismatch: {project.name}/{name}')
                if (project / 'lifecycle-ok').read_bytes() != b'enabled':
                    raise RuntimeError('postinstall did not run')
                lock = json.loads((project / 'package-lock.json').read_text(encoding='utf-8'))
                if len([name for name in lock['packages'] if name.startswith('node_modules/')]) != args.packages:
                    raise RuntimeError('package lock is incomplete')
                row['verified_files'] = len(expected)
                report['rows'].append(row)
                write_json(args.output / 'report.json', report)
                print(json.dumps(dict(label=label, round=iteration, elapsed_ms=row['request_to_exit_ms'],
                                      verified_files=len(expected))), flush=True)
        report['medians_ms'] = {label: statistics.median(row['request_to_exit_ms'] for row in report['rows']
                                                       if row['label'] == label and not row['warmup'])
                                for label in ['control', 'candidate']}
        report['passed'] = True
        print(json.dumps(report['medians_ms']), flush=True)
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        write_json(args.output / 'report.json', report)


if __name__ == '__main__':
    main()
