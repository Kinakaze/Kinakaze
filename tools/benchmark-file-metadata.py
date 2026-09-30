"""Paired installation-style metadata operations with inode-lifetime checks."""
import argparse
import json
from pathlib import Path
import time
from init_pool import InitPool, distribution_hashes

SOURCE = r'''
import errno, os, tempfile, time
root = tempfile.mkdtemp(prefix='file-metadata-')
started = time.monotonic()
try:
    for i in range(COUNT):
        path = root + '/payload'
        final = root + '/installed'
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o640)
        try:
            assert os.write(fd, b'payload' * 100) == 700
            os.fchown(fd, 0, 0)
            os.fchmod(fd, 0o644)
            os.utime(fd, ns=(1000000000000000000, 1000000000000000000))
            os.fsync(fd)
            os.rename(path, final)
            # Open descriptions retain their identity after replacement/unlink.
            os.unlink(final)
            assert os.fstat(fd).st_size == 700
            os.lseek(fd, 0, os.SEEK_SET)
            assert os.read(fd, 700) == b'payload' * 100
            os.fsync(fd)
        finally:
            os.close(fd)
    print('FILE_METADATA_OK count=%d seconds=%.6f' % (COUNT, time.monotonic()-started))
finally:
    os.rmdir(root)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--count', type=int, default=1000)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = dict(passed=False, count=args.count,
                  hashes=distribution_hashes(args.dist.resolve()), phases=[])
    try:
        with InitPool(args.root, args.dist, output / 'session', size=2,
                      timeout=180) as pool:
            began = time.time_ns()
            row = pool.run(['/usr/bin/python3', '-c', 'COUNT=' + str(args.count) + '\n' + SOURCE],
                           expect=['FILE_METADATA_OK'])
            row.update(phase='metadata', start_unix_ns=began, end_unix_ns=time.time_ns())
            report['phases'].append(row)
            report['passed'] = row['status'] == 'passed'
    finally:
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report['phases']))
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
