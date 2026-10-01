"""Compare fixed distributions on missing metadata, account queries, and fork.

Uses separate disposable guest roots and alternates execution order. Timings
inside the guest exclude Python startup; end-to-end time and Job CPU are also
retained. It does not install packages or change guest configuration.
"""
import argparse
import json
import os
from pathlib import Path
import statistics

from init_pool import InitPool, distribution_hashes


SOURCE = r'''
import errno, grp, json, os, pwd, tempfile, time
results = {}
directory = tempfile.mkdtemp(prefix='deep-paths-')
try:
    missing = directory + '/absent'
    began = time.monotonic_ns()
    for i in range(10000):
        for operation in (os.stat, os.lstat):
            try:
                operation(missing)
            except OSError as error:
                assert error.errno == errno.ENOENT
            else:
                raise AssertionError('missing name succeeded')
    results['missing_20000_ms'] = (time.monotonic_ns() - began) / 1e6
    with open(missing, 'w') as file:
        file.write('fresh')
    assert os.stat(missing).st_size == 5 and os.lstat(missing).st_size == 5
    fd = os.open(missing, os.O_RDWR)
    try:
        inode = os.fstat(fd).st_ino
        began = time.monotonic_ns()
        for i in range(10000):
            for record in (os.stat(missing), os.fstat(fd)):
                assert record.st_size == 5 and record.st_ino == inode
        results['stat_20000_ms'] = (time.monotonic_ns() - began) / 1e6
        os.fchmod(fd, 0o644)
        os.fchown(fd, 0, 0)
        began = time.monotonic_ns()
        for i in range(2000):
            os.fchmod(fd, 0o644)
            os.fchown(fd, 0, 0)
        results['metadata_2000_ms'] = (time.monotonic_ns() - began) / 1e6
        os.lseek(fd, 3, os.SEEK_SET)
        began = time.monotonic_ns()
        for i in range(10000):
            assert os.pwrite(fd, b'fresh', 0) == 5
            assert os.pread(fd, 5, 0) == b'fresh'
        results['readwrite_10000_ms'] = (time.monotonic_ns() - began) / 1e6
        assert os.lseek(fd, 0, os.SEEK_CUR) == 3
        os.fsync(fd)
    finally:
        os.close(fd)
    fd = os.open(missing, os.O_RDONLY)
    try:
        began = time.monotonic_ns()
        for i in range(10000):
            assert os.pread(fd, 5, 0) == b'fresh'
        results['readonly_10000_ms'] = (time.monotonic_ns() - began) / 1e6
        assert os.pread(fd, 10, 5) == b''
        writer = os.open(missing, os.O_RDWR)
        try:
            assert os.pwrite(writer, b'changed', 0) == 7
            assert os.pread(fd, 10, 0) == b'changed'
            os.ftruncate(writer, 2)
            assert os.pread(fd, 10, 0) == b'ch'
            assert os.pread(fd, 10, 2) == b''
        finally:
            os.close(writer)
        assert os.lseek(fd, 0, os.SEEK_CUR) == 0
    finally:
        os.close(fd)
    os.unlink(missing)
    began = time.monotonic_ns()
    for i in range(10000):
        assert pwd.getpwnam('root').pw_uid == 0
        assert grp.getgrnam('root').gr_gid == 0
    results['accounts_20000_ms'] = (time.monotonic_ns() - began) / 1e6
    # Ensure enumeration resets independently of ordinary name/ID lookups.
    assert pwd.getpwall() == pwd.getpwall()
    assert grp.getgrall() == grp.getgrall()
    began = time.monotonic_ns()
    for i in range(128):
        pid = os.fork()
        if pid == 0:
            os._exit(0)
        waited, status = os.waitpid(pid, 0)
        assert waited == pid and status == 0
    results['fork_128_ms'] = (time.monotonic_ns() - began) / 1e6
finally:
    os.rmdir(directory)
with open('/var/tmp/deep-paths-result.json', 'w') as file:
    json.dump(results, file)
print('DEEP_PATHS_OK', flush=True)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('control-dist', 'candidate-dist', 'control-root', 'candidate-root', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--rounds', type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.rounds <= 10:
        parser.error('rounds must be between 1 and 10')
    args.output.mkdir(parents=True, exist_ok=False)
    for name in ('KINAKAZE_IO_TRACE_DIR', 'KINAKAZE_STARTUP_PROFILE'):
        os.environ.pop(name, None)
    report = dict(passed=False, rounds=args.rounds, runs=[], hashes={})
    report_file = args.output / 'report.json'
    try:
        for label in ('control', 'candidate'):
            report['hashes'][label] = distribution_hashes(getattr(args, label + '_dist').resolve())
        for iteration in range(args.rounds):
            order = ('control', 'candidate') if iteration % 2 == 0 else ('candidate', 'control')
            for label in order:
                root = getattr(args, label + '_root').resolve()
                dist = getattr(args, label + '_dist').resolve()
                with InitPool(root, dist, args.output / f'{iteration}-{label}', size=2, timeout=180) as pool:
                    row = pool.run(['/usr/bin/python3.11', '-c', SOURCE], expect=['DEEP_PATHS_OK'],
                                   environment=['PATH=/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL=C'])
                row.update(round=iteration, label=label)
                report['runs'].append(row)
                if row['status'] != 'passed':
                    raise AssertionError(f'{iteration} {label}: {row["status"]}')
                row['guest'] = json.loads((root / 'var/tmp/deep-paths-result.json').read_text(encoding='utf-8'))
                print(json.dumps(dict(round=iteration, label=label, **row['guest'])), flush=True)
                report_file.write_text(json.dumps(report, indent=2), encoding='utf-8')
        report['medians'] = {
            label: {key: statistics.median(row['guest'][key] for row in report['runs'] if row['label'] == label)
                    for key in ('missing_20000_ms', 'stat_20000_ms', 'metadata_2000_ms',
                                'readwrite_10000_ms', 'readonly_10000_ms',
                                'accounts_20000_ms', 'fork_128_ms')}
            for label in ('control', 'candidate')
        }
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        report_file.write_text(json.dumps(report, indent=2), encoding='utf-8')
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
