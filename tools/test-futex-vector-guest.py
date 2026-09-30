"""Run futex ABI, signal, alias and fork probes in a real Debian worker."""
import argparse
import json
import os
from pathlib import Path

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--probe', choices=('pi', 'scalar', 'signal', 'timeout', 'vector', 'requeue'), default='vector')
    args = parser.parse_args()
    name, marker = {'pi': ('FutexPiProbe.py', 'FUTEX_PI_PASS'),
                    'scalar': ('FutexScalarProbe.py', 'FUTEX_SCALAR_PASS'),
                    'signal': ('FutexSignalRestartProbe.py', 'FUTEX_SIGNAL_RESTART_PASS'),
                    'timeout': ('FutexTimeoutProbe.py', 'FUTEX_TIMEOUT_PASS'),
                    'vector': ('FutexVectorProbe.py', 'FUTEX_VECTOR_PASS'),
                    'requeue': ('FutexRequeueProbe.py', 'FUTEX_REQUEUE_PASS')}[args.probe]
    source = (Path(__file__).resolve().parents[1] / 'tests/guest' / name).read_text(encoding='utf-8')
    records = []
    previous = os.environ.get('KINAKAZE_FUTEX_OPT')
    try:
        for enabled in [False, True]:
            # This native switch is cached before a prewarmed worker receives
            # its guest environ. Set the host environment before spawning init
            # and use an independent pool for each configuration.
            os.environ['KINAKAZE_FUTEX_OPT'] = str(int(enabled))
            case = args.output / ('candidate' if enabled else 'control')
            with InitPool(args.root, args.dist, case, size=1, timeout=90) as pool:
                row = pool.run(['/usr/bin/python3', '-c', source], expect=[marker],
                               environment=['PATH=/usr/bin:/bin', 'LC_ALL=C',
                                            'KINAKAZE_FUTEX_OPT=' + str(int(enabled))])
            row['optimized'] = enabled
            records.append(row)
            print(json.dumps(dict(optimized=enabled, status=row['status'], milliseconds=row['request_to_exit_ms'])), flush=True)
    finally:
        if previous is None:
            os.environ.pop('KINAKAZE_FUTEX_OPT', None)
        else:
            os.environ['KINAKAZE_FUTEX_OPT'] = previous
    report = dict(probe=args.probe, passed=all(row['status'] == 'passed' for row in records),
                  distribution_sha256=distribution_hashes(args.dist), rows=records)
    (args.output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
