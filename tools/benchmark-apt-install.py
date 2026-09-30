"""Measure APT download, unpack/configure, reinstall and purge in isolated state.

A loopback HTTP repository removes mirror variability. Payload hashes, dependency
resolution, maintainer scripts and removal are checked. No system package state
is changed; timings are warm-host measurements, not Internet download speeds.
"""
import argparse
import gzip
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import io
import json
from pathlib import Path
import random
import statistics
import tarfile
import threading
import uuid

from init_pool import InitPool, distribution_hashes


def archive(files):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode='w', format=tarfile.GNU_FORMAT) as tar:
        directories = set()
        for name, data, mode in files:
            for parent in reversed(Path(name).parents):
                directory = parent.as_posix()
                if directory != '.' and directory not in directories:
                    entry = tarfile.TarInfo('./' + directory + '/')
                    entry.type, entry.mode, entry.mtime = tarfile.DIRTYPE, 0o755, 1700000000
                    tar.addfile(entry)
                    directories.add(directory)
            entry = tarfile.TarInfo(name)
            entry.size, entry.mode, entry.mtime = len(data), mode, 1700000000
            tar.addfile(entry, io.BytesIO(data))
    return gzip.compress(buffer.getvalue(), mtime=0)


def deb(control, files, script):
    members = [('debian-binary', b'2.0\n'), ('control.tar.gz', archive([
        ('./control', control.encode(), 0o644), ('./postinst', script.encode(), 0o755)])),
        ('data.tar.gz', archive(files))]
    data = bytearray(b'!<arch>\n')
    for name, payload in members:
        data.extend(f'{name + "/":<16}{0:<12}{0:<6}{0:<6}{"100644":<8}{len(payload):<10}`\n'.encode())
        data.extend(payload)
        if len(payload) % 2:
            data.extend(b'\n')
    return bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', action='append', required=True, metavar='LABEL=PATH')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--files', type=int, default=600)
    parser.add_argument('--large-mb', type=int, default=8)
    parser.add_argument('--timeout', type=float, default=600)
    args = parser.parse_args()
    if min(args.repeat, args.files, args.large_mb, args.timeout) <= 0:
        parser.error('counts and timeout must be positive')
    distributions = {}
    for value in args.dist:
        label, sep, directory = value.partition('=')
        if not sep or not label or not all(c.isalnum() or c in '-_' for c in label) or label in distributions:
            parser.error('use unique LABEL=PATH distributions')
        distributions[label] = Path(directory).resolve()
    root, output = args.root.resolve(), args.output.resolve()
    if output.exists():
        parser.error('--output must be new to retain earlier evidence')
    output.mkdir(parents=True)
    repository = output / 'repository'
    repository.mkdir()
    payloads = [(f'./usr/share/kinakaze-apt-bench/d{i // 50}/file-{i}',
                 (f'payload-{i}\n' * 100).encode(), 0o644) for i in range(args.files)]
    payloads.append(('./usr/share/kinakaze-apt-bench/large',
                     random.Random(0).randbytes(args.large_mb * 1024**2), 0o644))
    packages = ('kinakaze-apt-bench-data', 'kinakaze-apt-bench-app')
    index = ''
    for name in packages:
        dependency = f'Depends: {packages[0]} (= 1)\n' if name == packages[1] else ''
        control = (f'Package: {name}\nVersion: 1\nArchitecture: amd64\n'
                   f'Maintainer: Probe <probe@example.invalid>\n{dependency}'
                   'Description: Isolated APT performance fixture\n')
        script = f'#!/bin/sh\nset -eu\nprintf configured > "$DPKG_ROOT/{name}.configured"\n'
        data = deb(control, payloads if name == packages[0] else [
            ('./usr/share/kinakaze-apt-bench/app', b'app\n', 0o644)], script)
        (repository / (name + '.deb')).write_bytes(data)
        index += control + f'Filename: ./{name}.deb\nSize: {len(data)}\nSHA256: {hashlib.sha256(data).hexdigest()}\n\n'
    (repository / 'Packages').write_text(index, encoding='utf-8', newline='\n')

    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=str(repository), **kw)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    report = dict(scope=__doc__, files=args.files, large_mb=args.large_mb,
                  root=str(root), distributions={k: dict(path=str(v), sha256=distribution_hashes(v))
                  for k, v in distributions.items()}, results=[], summary={}, passed=False)

    def save():
        (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    try:
        save()
        for iteration in range(args.repeat):
            labels = list(distributions)
            if iteration % 2:
                labels.reverse()
            for label in labels:
                guest = '/var/tmp/apt-benchmark-' + uuid.uuid4().hex
                stage = root / guest.lstrip('/')
                for directory in ('state/lists/partial', 'cache/archives/partial', 'log', 'install', 'admin/updates', 'admin/info'):
                    (stage / directory).mkdir(parents=True)
                (stage / 'admin/status').write_bytes(b'')
                (stage / 'sources.list').write_text(
                    f'deb [trusted=yes] http://127.0.0.1:{server.server_port} ./\n', encoding='utf-8')
                settings = {
                    'Dir::Etc::sourcelist': guest + '/sources.list', 'Dir::Etc::sourceparts': '-',
                    'Dir::State': guest + '/state', 'Dir::State::status': guest + '/admin/status',
                    'Dir::Cache': guest + '/cache', 'Dir::Log': guest + '/log',
                    'APT::Get::List-Cleanup': 'false', 'APT::Install-Recommends': 'false',
                    'Acquire::http::Proxy': 'DIRECT', 'Acquire::Retries': '0',
                    'APT::Update::Error-Mode': 'any', 'Dpkg::Use-Pty': 'false',
                }
                apt = ['/usr/bin/apt-get']
                for key, value in settings.items():
                    apt.extend(['-o', key + '=' + value])
                for value in ('--admindir=' + guest + '/admin', '--instdir=' + guest + '/install', '--force-script-chrootless'):
                    apt.extend(['-o', 'DPkg::Options::=' + value])
                row = dict(distribution=label, iteration=iteration, guest=guest, phases=[], passed=False)
                report['results'].append(row)
                save()
                with InitPool(root, distributions[label], output / f'{label}-{iteration}', size=1, timeout=args.timeout,
                              memory_limit_bytes=4 * 1024**3) as pool:
                    def run(name, command):
                        result = pool.run(command, environment=['PATH=/usr/sbin:/usr/bin:/sbin:/bin',
                            'LC_ALL=C', 'DEBIAN_FRONTEND=noninteractive'])
                        result['phase'] = name
                        row['phases'].append(result)
                        save()
                        print(json.dumps(dict(distribution=label, iteration=iteration, phase=name,
                            status=result['status'], milliseconds=result['request_to_exit_ms'])), flush=True)
                        if result['status'] != 'passed':
                            raise AssertionError(f'{label}: {name}: {result["status"]}')

                    run('update', apt + ['update'])
                    run('download', apt + ['-y', '--download-only', 'install', packages[1]])
                    for phase in ('install', 'reinstall'):
                        for package in packages:
                            (stage / 'install' / (package + '.configured')).unlink(missing_ok=True)
                        run(phase, apt + ['-y', '--no-download', *(['--reinstall'] if phase == 'reinstall' else []),
                                         'install', *packages])
                        for name, data, _ in payloads:
                            installed = stage / 'install' / name.removeprefix('./')
                            assert hashlib.sha256(installed.read_bytes()).digest() == hashlib.sha256(data).digest(), name
                        assert (stage / 'install/usr/share/kinakaze-apt-bench/app').read_bytes() == b'app\n'
                        for package in packages:
                            assert (stage / 'install' / (package + '.configured')).read_bytes() == b'configured'
                        status = (stage / 'admin/status').read_text(encoding='utf-8')
                        assert status.count('Status: install ok installed') == 2, status
                    run('purge', apt + ['-y', 'purge', *packages])
                    assert not (stage / 'install/usr/share/kinakaze-apt-bench').exists()
                    run('audit', ['/usr/bin/dpkg', '--admindir=' + guest + '/admin', '--audit'])
                    row['cpu_metrics'] = pool.child.cpu_metrics()
                row['passed'] = True
                save()
        for label in distributions:
            report['summary'][label] = {phase: statistics.median(
                p['request_to_exit_ms'] for r in report['results'] if r['distribution'] == label
                for p in r['phases'] if p['phase'] == phase) for phase in ('update', 'download', 'install', 'reinstall', 'purge')}
        report['passed'] = all(r['passed'] for r in report['results'])
    except Exception as error:
        report['error'] = repr(error)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        save()
    print(json.dumps(dict(passed=report['passed'], summary=report['summary'], error=report.get('error'))), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
