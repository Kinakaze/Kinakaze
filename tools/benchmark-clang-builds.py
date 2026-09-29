"""Compare validated Clang builds sequentially in a disposable developer root."""
import argparse
import json
from pathlib import Path
import shutil
import uuid

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', action='append', required=True, metavar='LABEL=PATH')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--link-directory', default='/root/kinakaze-link')
    parser.add_argument('--units', type=int, default=32)
    parser.add_argument('--jobs', type=int, default=4)
    args = parser.parse_args()
    if not 2 <= args.units <= 256 or not 1 <= args.jobs <= 32:
        parser.error('units must be 2..256 and jobs 1..32')
    distributions = {}
    for item in args.dist:
        label, separator, path = item.partition('=')
        if (not separator or not label or label in distributions
                or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in label)):
            parser.error('use unique simple LABEL=PATH distributions')
        distributions[label] = Path(path).resolve()
    root = args.root.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    guest = '/root/clang-comparison-' + uuid.uuid4().hex
    fixture = root / guest.lstrip('/')
    fixture.mkdir()
    shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/guest/ClangBuildProbe.py', fixture / 'probe.py')
    report = dict(root=str(root), guest=guest, units=args.units, jobs=args.jobs,
                  scope='Sequential dev/release or version comparison; warm OS caches, not cold-cache timing.',
                  results=[], passed=False)
    destination = args.output / 'results.json'
    try:
        for label, dist in distributions.items():
            row = dict(label=label, distribution=str(dist), sha256=distribution_hashes(dist))
            report['results'].append(row)
            directory = guest + '/' + label
            with InitPool(root, dist, args.output / label, size=1, timeout=2400) as pool:
                row['process'] = pool.run(['/usr/bin/python3', guest + '/probe.py',
                    '--units', str(args.units), '--jobs', str(args.jobs),
                    '--directory', directory, '--link-directory', args.link_directory],
                    expect=['CLANG_BUILD_OK'])
            result = root / directory.lstrip('/') / 'results.json'
            if result.exists():
                row['guest'] = json.loads(result.read_text())
            row['passed'] = row['process']['status'] == 'passed' and row.get('guest', {}).get('passed', False)
            destination.write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps(dict(label=label, passed=row['passed'], builds=row.get('guest', {}).get('builds', []))), flush=True)
        report['passed'] = all(row['passed'] for row in report['results'])
    finally:
        destination.write_text(json.dumps(report, indent=2) + '\n')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
