"""Install real cached Debian Node.js packages in a disposable guest root.

The caller supplies a fresh root created by worker setup. All normal maintainer
scripts, triggers and durability operations run. Only package acquisition is
redirected to a local, hashed repository; download time is reported separately.
"""
import argparse
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import io
import json
from pathlib import Path
import threading
import time
import tarfile

from init_pool import InitPool, distribution_hashes


def installed_packages(root, requested):
    status = (root / 'var/lib/dpkg/status').read_text(encoding='utf-8')
    records = {}
    for record in status.split('\n\n'):
        if not record.startswith('Package: '):
            continue
        name = record.splitlines()[0].removeprefix('Package: ')
        if 'Status: install ok installed\n' in record:
            records[name] = next(line[9:] for line in record.splitlines() if line.startswith('Version: '))
    for package in requested:
        assert package in records, f'{package} is not configured'
    return records


def dpkg_timings(root):
    from datetime import datetime
    rows = []
    for line in (root / 'var/log/dpkg.log').read_text(encoding='utf-8').splitlines():
        date, clock, *event = line.split()
        rows.append((datetime.fromisoformat(date + 'T' + clock), event))
    starts = [(stamp, event[2]) for stamp, event in rows if event[:2] == ['startup', 'archives']]
    configure = next((stamp for stamp, event in rows if event[:2] == ['startup', 'packages']), None)
    return dict(installed_packages=sum(event[:1] == ['install'] for _, event in rows),
                unpack_seconds=(configure - starts[0][0]).total_seconds() if starts and configure else None,
                configure_seconds=(rows[-1][0] - configure).total_seconds() if configure else None,
                resolution_seconds=1)


def collect_samples(pool, stopped, output):
    import ctypes as c
    import psutil
    import runpy
    sample = runpy.run_path(str(Path(__file__).with_name('sample-native-stacks.py')))['sample']
    folder = output / 'samples'
    folder.mkdir()
    previous = {}
    number = 0
    while not stopped.wait(.5) and number < 80:
        # Native exec can outlive its Windows parent; use the owning Job rather
        # than parent chains, which lose precisely the busy dpkg grandchildren.
        storage = c.create_string_buffer(65536)
        with pool.child._job_lock:
            if not pool.child.job:
                break
            if not pool.child.kernel.QueryInformationJobObject(pool.child.job, 3,
                    storage, len(storage), None):
                continue
        count = c.c_uint32.from_buffer(storage, 4).value
        pids = (c.c_size_t * min(count, 8191)).from_buffer(storage, 8)
        current = {}
        for pid in pids:
            try:
                process = psutil.Process(pid)
                current[pid] = (process.create_time(), sum(process.cpu_times()[:2]))
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                pass
        active = [(values[1] - previous[pid][1], pid) for pid, values in current.items()
                  if pid in previous and values[0] == previous[pid][0]]
        previous = current
        if not active:
            continue
        delta, pid = max(active)
        if delta <= 0 or not pool.child.owns_process(pid):
            continue
        try:
            snapshot = sample(pid)
            snapshot['sample_cpu_seconds'] = delta
            (folder / f'{number:03d}-{pid}.json').write_text(json.dumps(snapshot))
            number += 1
        except (psutil.NoSuchProcess, psutil.AccessDenied, OSError):
            pass


def control(path):
    with path.open('rb') as stream:
        if stream.read(8) != b'!<arch>\n':
            raise ValueError(f'not a deb: {path}')
        while header := stream.read(60):
            size = int(header[48:58])
            name = header[:16].decode().strip().rstrip('/')
            if name.startswith('control.tar'):
                with tarfile.open(fileobj=io.BytesIO(stream.read(size))) as archive:
                    member = next(m for m in archive if m.name.removeprefix('./') == 'control')
                    return archive.extractfile(member).read().decode().rstrip() + '\n'
            stream.seek(size + size % 2, 1)
    raise ValueError(f'no control: {path}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'archives', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--package', action='append', default=None)
    parser.add_argument('--timeout', type=float, default=900)
    parser.add_argument('--sample', action='store_true', help='diagnostic native snapshots; excludes performance comparison')
    parser.add_argument('--pool-size', type=int, choices=range(1, 9), default=None,
                        help='override init default (otherwise exercise the default two workers)')
    args = parser.parse_args()
    root, dist, archives, output = (getattr(args, key).resolve() for key in ('root', 'dist', 'archives', 'output'))
    # Refuse installed/shared roots: this tool is intentionally an installation,
    # not a reinstall benchmark against an already populated package database.
    if (root / 'usr/bin/node').exists() or (root / 'usr/bin/npm').exists():
        parser.error('supply a fresh disposable root without Node.js/npm')
    if not root.is_relative_to(Path(__file__).resolve().parents[1] / 'artifacts/node-install-20260930'):
        parser.error('root must be a disposable fixture under artifacts/node-install-20260930')
    output.mkdir(parents=True, exist_ok=False)
    repository = {}
    index = []
    for path in sorted(archives.glob('*.deb')):
        data = control(path)
        with path.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        repository[path.name] = path
        index.append(data + f'Filename: pool/{path.name}\nSize: {path.stat().st_size}\nSHA256: {digest}\n\n')
    packages = ''.join(index).encode()
    (output / 'Packages').write_bytes(packages)

    class Handler(SimpleHTTPRequestHandler):
        def do_GET(self):
            from urllib.parse import unquote, urlsplit
            import posixpath
            name = posixpath.normpath(unquote(urlsplit(self.path).path))
            if name == '/Packages':
                self.send_response(200)
                self.send_header('Content-Length', str(len(packages)))
                self.end_headers()
                self.wfile.write(packages)
            elif name.startswith('/pool/') and name[6:] in repository:
                path = repository[name[6:]]
                self.send_response(200)
                self.send_header('Content-Length', str(path.stat().st_size))
                self.end_headers()
                with path.open('rb') as stream:
                    import shutil
                    shutil.copyfileobj(stream, self.wfile)
            else:
                self.send_error(404)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    serving = threading.Thread(target=server.serve_forever, daemon=True)
    serving.start()
    fixture = root / 'var/tmp/node-install-benchmark'
    fixture.mkdir(parents=True, exist_ok=True)
    (fixture / 'sources.list').write_text(f'deb [trusted=yes] http://127.0.0.1:{server.server_port} ./\n')
    (fixture / 'sourceparts').mkdir(exist_ok=True)
    report = dict(passed=False, root=str(root), dist=str(dist), hashes=distribution_hashes(dist),
                  packages=args.package or ['nodejs', 'npm'], diagnostic=args.sample,
                  pool_size=args.pool_size or 2, phases=[])

    def save():
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    try:
        with InitPool(root, dist, output / 'session', size=args.pool_size, timeout=args.timeout,
                      memory_limit_bytes=6 * 1024**3) as pool:
            apt = ['/usr/bin/apt-get', '-o', 'Dir::Etc::sourcelist=/var/tmp/node-install-benchmark/sources.list',
                   '-o', 'Dir::Etc::sourceparts=/var/tmp/node-install-benchmark/sourceparts',
                   '-o', 'Acquire::Retries=0', '-o', 'Acquire::http::Proxy=DIRECT',
                   '-o', 'APT::Update::Error-Mode=any']

            def run(phase, command):
                print('starting ' + phase, flush=True)
                start_unix_ns = time.time_ns()
                stopped = threading.Event()
                sampler = None
                if phase == 'install' and args.sample:
                    sampler = threading.Thread(target=collect_samples, args=(pool, stopped, output), daemon=True)
                    sampler.start()
                try:
                    row = pool.run(command, environment=['PATH=/usr/sbin:/usr/bin:/sbin:/bin',
                                   'LC_ALL=C', 'DEBIAN_FRONTEND=noninteractive'])
                finally:
                    stopped.set()
                    if sampler:
                        sampler.join(timeout=10)
                        if sampler.is_alive():
                            raise RuntimeError('sampler did not retire')
                row['phase'] = phase
                row['start_unix_ns'] = start_unix_ns
                row['end_unix_ns'] = time.time_ns()
                report['phases'].append(row)
                save()
                print(json.dumps(dict(phase=phase, status=row['status'], ms=row['request_to_exit_ms'])), flush=True)
                if row['status'] != 'passed':
                    raise AssertionError(f'{phase}: {row["status"]}')

            run('update', apt + ['update'])
            run('download', apt + ['-y', '--download-only', 'install', *report['packages']])
            run('plan', apt + ['-s', '--no-download', 'install', *report['packages']])
            run('install', apt + ['-y', '--no-download', 'install', *report['packages']])
            run('node-smoke', ['/usr/bin/node', '-e',
                "const a=require('assert'),c=require('crypto'); a.equal(c.createHash('sha256').update('node').digest('hex'),'545ea538461003efdc8c81c244531b003f6f26cfccf6c0073b3239fdedf49446'); let sum=0;for(let i=0;i<100000;i++)sum+=i;a.equal(sum,4999950000);console.log(process.version)"])
            if 'npm' in report['packages']:
                run('npm-smoke', ['/usr/bin/npm', '--version'])
            run('audit', ['/bin/sh', '-c', '/usr/bin/dpkg --audit > /var/tmp/node-install-benchmark/audit.txt'])
            run('verify', ['/bin/sh', '-c', '/usr/bin/dpkg --verify > /var/tmp/node-install-benchmark/verify.txt'])
            report['memory_metrics'] = pool.child.memory_metrics()
        audit = (fixture / 'audit.txt').read_text(encoding='utf-8')
        assert not audit.strip(), 'dpkg audit: ' + audit[:2000]
        verification = (fixture / 'verify.txt').read_text(encoding='utf-8')
        assert not verification.strip(), 'dpkg verify: ' + verification[:2000]
        report['installed'] = installed_packages(root, report['packages'])
        report['dpkg_timings'] = dpkg_timings(root)
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {str(error)[:2000]}'
    finally:
        server.shutdown()
        server.server_close()
        serving.join(timeout=5)
        save()
    print(json.dumps(dict(passed=report['passed'], error=report.get('error'))), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
