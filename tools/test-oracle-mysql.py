"""Fetch pinned Oracle MySQL binaries and test them in an isolated guest prefix."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import urllib.request
import uuid

from init_pool import distribution_hashes


PACKAGES = {
    'mysql-community-server-core': (32094036, '01ddd6029eeadca7c03785fd26e2192da1094a1b9d368b7e90d8f0614dac3f47'),
    'mysql-community-client-core': (1754648, '19c6e1d38f95fa77ebb675c01c0b3c50554026adc5ed0c114cee2a72520c2fea'),
    'mysql-community-client-plugins': (1338020, '95b66f6ba68e2b064e175fa7368ae6d6d52919b92b7d69e6b48d949af642eb07'),
    'libmecab2': (222204, '8c2808eca41dec4ce98a2660c85a84b09dcf4777990cbfb4ad739b624835e3ff'),
}
VERSION = '8.4.11-1debian12'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output', 'cache'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--offline', action='store_true')
    parser.add_argument('--shutdown-timeout', type=int, default=30)
    parser.add_argument('--repeat', type=int, default=1)
    args = parser.parse_args()
    if args.shutdown_timeout <= 0 or args.repeat <= 0:
        parser.error('shutdown timeout and repeat must be positive')
    root, dist, output, cache = (path.resolve() for path in (args.root, args.dist, args.output, args.cache))
    output.mkdir(parents=True, exist_ok=True)
    cache.mkdir(parents=True, exist_ok=True)
    guest = '/tmp/oracle-mysql-' + uuid.uuid4().hex
    stage = root / guest.lstrip('/')
    stage.mkdir(parents=True)
    report = dict(version=VERSION, root=str(root), dist=str(dist), guest=guest,
                  sha256=distribution_hashes(dist), packages=[], results=[], passed=False,
                  repeat=args.repeat, shutdown_timeout=args.shutdown_timeout)
    destination = output / 'results.json'
    destination.write_text(json.dumps(report, indent=2), encoding='utf-8')
    for package, (size, digest) in PACKAGES.items():
        version = '0.996-14+b14' if package == 'libmecab2' else VERSION
        filename = f'{package}_{version}_amd64.deb'
        source = cache / filename
        base = ('https://deb.debian.org/debian/pool/main/m/mecab/' if package == 'libmecab2' else
                'https://repo.mysql.com/apt/debian/pool/mysql-8.4-lts/m/mysql-community/')
        url = base + filename
        if not source.exists():
            if args.offline:
                raise FileNotFoundError(source)
            with urllib.request.urlopen(url, timeout=60) as response:
                data = response.read()
            if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
                raise ValueError('Oracle package hash mismatch: ' + filename)
            source.write_bytes(data)
        if source.stat().st_size != size or hashlib.sha256(source.read_bytes()).hexdigest() != digest:
            raise ValueError('Cached package hash mismatch: ' + filename)
        shutil.copyfile(source, stage / filename)
        report['packages'].append(dict(name=filename, url=url, sha256=digest))
    shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/guest/OracleMysqlRuntimeProbe.py',
                    stage / 'probe.py')
    execute = runpy.run_path(str(Path(__file__).with_name('test-debian-commands.py')))['execute']
    environment = {key: value for key, value in os.environ.items() if not key.startswith('KINAKAZE_')}
    commands = [
        ('extract', 'mkdir prefix; for package in *.deb; do dpkg-deb -x "$package" prefix; done', 180),
    ]
    commands.extend(
        ('runtime' if iteration == 0 else f'runtime-{iteration}',
         'export LD_LIBRARY_PATH="$PWD/prefix/usr/lib/x86_64-linux-gnu"; '
         'python3 probe.py --prefix "$PWD/prefix" --log-path "$PWD/server.log"' +
         f' --shutdown-timeout {args.shutdown_timeout}', 240 + 3 * args.shutdown_timeout)
        for iteration in range(args.repeat))
    for name, script, timeout in commands:
        row = execute([str(dist / 'worker.exe'), 'oneshot', '--root', str(root), '--dist', str(dist),
                       '--cwd', guest, '--', '/bin/bash', '-c',
                       'set -eu; export PATH=/usr/sbin:/usr/bin:/sbin:/bin; ' + script],
                      output / name, environment, timeout=timeout)
        row.update(name=name, passed=row['exit_code'] == 0 and not row['timed_out'])
        if name.startswith('runtime'):
            row['passed'] &= 'ORACLE_MYSQL_TRANSACTIONS_CRASH_RECOVERY_OK' in row['stdout']
        report['results'].append(row)
        report['passed'] = name == commands[-1][0] and all(item['passed'] for item in report['results'])
        destination.write_text(json.dumps(report, indent=2), encoding='utf-8')
        print(json.dumps(dict(name=name, passed=row['passed'], seconds=row['seconds'])), flush=True)
        if not row['passed'] and name == 'extract':
            return 1
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
