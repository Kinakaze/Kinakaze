"""Compare descriptor metadata reuse with its control in the same distribution."""
import argparse
from contextlib import contextmanager
import importlib.util
import json
import os
from pathlib import Path
import statistics
import threading
import time

import psutil
from init_pool import InitPool, distribution_hashes


REPEATED = r'''
import json, os, tempfile, time
root = tempfile.mkdtemp(prefix='fd-metadata-repeat-')
path = root + '/payload'
results = {}
fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o640)
try:
    assert os.write(fd, b'payload') == 7
    inode = os.fstat(fd).st_ino
    began = time.monotonic_ns()
    for i in range(20000):
        value = os.fstat(fd)
        assert value.st_size == 7 and value.st_ino == inode
    results['fstat_20000_ms'] = (time.monotonic_ns() - began) / 1e6
    alias = os.dup(fd)
    os.close(fd)
    fd = alias
    began = time.monotonic_ns()
    for i in range(4000):
        os.fchmod(fd, 0o600 if i % 2 else 0o640)
        os.fchown(fd, 0, 0)
        assert os.fstat(fd).st_mode & 0o777 == (0o600 if i % 2 else 0o640)
    results['metadata_4000_ms'] = (time.monotonic_ns() - began) / 1e6
    os.rename(path, root + '/renamed')
    os.unlink(root + '/renamed')
    assert os.fstat(fd).st_size == 7
    os.lseek(fd, 0, os.SEEK_SET)
    assert os.read(fd, 7) == b'payload'
finally:
    os.close(fd)
    os.rmdir(root)
with open('/var/tmp/fd-metadata-result.json', 'w') as file:
    json.dump(results, file)
print('FD_METADATA_OK')
'''


@contextmanager
def metadata_mode(enabled):
    previous = os.environ.get('KINAKAZE_FD_METADATA')
    os.environ['KINAKAZE_FD_METADATA'] = '1' if enabled else '0'
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop('KINAKAZE_FD_METADATA', None)
        else:
            os.environ['KINAKAZE_FD_METADATA'] = previous


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--files', type=int, default=512)
    args = parser.parse_args()
    if args.repeat <= 0 or args.files <= 0:
        parser.error('repeat and files must be positive')
    root, dist, output = args.root.resolve(), args.dist.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    spec = importlib.util.spec_from_file_location('metadata_fixture',
        Path(__file__).with_name('benchmark-file-metadata.py'))
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    report = dict(passed=False, hashes=distribution_hashes(dist),
                  files=args.files, rows=[], host_samples=[])
    stopped = threading.Event()

    def sample_host():
        while not stopped.wait(.5):
            compilers = [process.info for process in psutil.process_iter(['pid', 'name'])
                         if (process.info['name'] or '').lower() in
                         ('cargo.exe', 'rustc.exe', 'link.exe')]
            report['host_samples'].append(dict(time_ns=time.time_ns(),
                cpu_percent=psutil.cpu_percent(), compilers=compilers))

    def save():
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    monitor = threading.Thread(target=sample_host, daemon=True)
    monitor.start()
    try:
        for iteration in range(args.repeat):
            for enabled in ([False, True] if iteration % 2 == 0 else [True, False]):
                label = 'reused' if enabled else 'ordinary'
                with metadata_mode(enabled), InitPool(root, dist,
                        output / f'{iteration}-{label}', size=2, timeout=240) as pool:
                    for scenario, source, marker in (
                        ('repeated', REPEATED, 'FD_METADATA_OK'),
                        ('install', f'COUNT={args.files}\n' + fixture.SOURCE, 'FILE_METADATA_OK')):
                        row = pool.run(['/usr/bin/python3', '-c', source], expect=[marker])
                        row.update(iteration=iteration, mode=label, scenario=scenario)
                        if scenario == 'repeated' and row['status'] == 'passed':
                            row['guest'] = json.loads((root / 'var/tmp/fd-metadata-result.json').read_text())
                        report['rows'].append(row)
                        save()
                        print(json.dumps(dict(iteration=iteration, mode=label, scenario=scenario,
                            status=row['status'], ms=row['request_to_exit_ms'])), flush=True)
                        assert row['status'] == 'passed', row
        report['summary'] = {}
        for scenario in ('repeated', 'install'):
            report['summary'][scenario] = {}
            for mode in ('ordinary', 'reused'):
                rows = [row for row in report['rows'] if row['scenario'] == scenario and row['mode'] == mode]
                result = dict(wall_ms=statistics.median(row['request_to_exit_ms'] for row in rows),
                    cpu_ms=statistics.median(row['cpu_metrics']['total_cpu_ms'] for row in rows))
                if scenario == 'repeated':
                    result['guest'] = {key: statistics.median(row['guest'][key] for row in rows)
                        for key in rows[0]['guest']}
                report['summary'][scenario][mode] = result
        assert distribution_hashes(dist) == report['hashes'], 'distribution changed during comparison'
        report['passed'] = True
    except Exception as error:
        report['error'] = repr(error)
    finally:
        stopped.set()
        monitor.join()
        save()
    print(json.dumps({key: report[key] for key in ('passed', 'summary', 'error') if key in report}), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
