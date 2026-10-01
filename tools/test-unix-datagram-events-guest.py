"""Run named-datagram event waits and the neighboring Unix ABI regressions."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import uuid

from init_pool import InitPool, distribution_hashes

PROBES = {
    'UnixDatagramEventsProbe': ['UNIX_DATAGRAM_EVENTS_OK'],
    'UnixBlockingProbe': ['UnixBlockingProbe: PASS'],
    'UnixReadinessProbe': ['UnixReadinessProbe: PASS'],
    'UnixPeekProbe': ['UnixPeekProbe: PASS'],
    'UnixCredentialsProbe': ['UNIX_CREDENTIALS_LIFETIME_OK'],
    'UnixSocketOptionsProbe': ['UNIX_SOCKET_OPTIONS_OK'],
    'DatagramRightsProbe': ['bound-datagram-queue-after-close'],
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--probe', action='append', choices=PROBES)
    parser.add_argument('--timeout', type=float, default=120)
    args = parser.parse_args()
    fixtures = Path(__file__).resolve().parents[1] / 'tests/guest'
    selected = args.probe or list(PROBES)
    guest = '/tmp/kinakaze-dgram-events-' + uuid.uuid4().hex
    staging = args.root.resolve() / guest.lstrip('/')
    staging.mkdir(parents=True)
    args.output.mkdir(parents=True, exist_ok=True)
    report = dict(dist=str(args.dist.resolve()), distribution_sha256=distribution_hashes(args.dist),
                  fixture_sha256={}, rows=[], passed=False)
    for name in selected:
        source = fixtures / (name + '.py')
        shutil.copyfile(source, staging / source.name)
        report['fixture_sha256'][source.name] = hashlib.sha256(source.read_bytes()).hexdigest()
    for name in selected:
        with InitPool(args.root, args.dist, args.output / name, size=1, timeout=args.timeout) as pool:
            row = pool.run(['/usr/bin/python3.11', guest + '/' + name + '.py'], cwd=guest,
                           expect=PROBES[name], environment=['PATH=/usr/bin:/bin', 'LC_ALL=C'])
        row['probe'] = name
        report['rows'].append(row)
        report['passed'] = len(report['rows']) == len(selected) and all(r['status'] == 'passed' for r in report['rows'])
        (args.output / 'report.json').write_bytes(json.dumps(report, indent=2).encode('utf-8'))
        print(json.dumps(dict(probe=name, status=row['status'])), flush=True)
        if row['status'] != 'passed':
            return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
