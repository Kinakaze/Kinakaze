"""Real Linux io_uring ABI, memory advice and pthread mask regressions."""
import argparse
import json
from pathlib import Path
from init_pool import InitPool, distribution_hashes

CASES = {
    'uring': ('LinuxUringProbe', 'LINUX_URING_MMAP_ASYNC_RW_VECTORS_OVERFLOW_WAKE_OK'),
    'memory-advice': ('MadvisePolicyProbe', 'MADVISE_DUMP_FORK_ZERO_RESTORE_OK'),
    'pthread-mask': ('PthreadSignalMaskProbe', 'PTHREAD_MASK_INHERIT_SIGWAIT_OK'),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--only', nargs='+', choices=CASES)
    parser.add_argument('--repeat', type=int, default=1)
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error('repeat must be positive')
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    report = dict(root=str(root), dist=str(dist), sha256=distribution_hashes(dist), results=[])
    for iteration in range(args.repeat):
        for name in args.only or CASES:
            probe, marker = CASES[name]
            source = (Path(__file__).resolve().parents[1] / 'tests/guest' / (probe + '.py')).read_text()
            try:
                with InitPool(root, dist, output / f'{name}-{iteration}', size=1, timeout=90) as pool:
                    row = pool.run(['/usr/bin/python3', '-c', source], expect=[marker])
            except Exception as error:
                row = dict(status='failed', error=str(error))
            row.update(name=name, iteration=iteration)
            report['results'].append(row)
            print(json.dumps(dict(name=name, iteration=iteration, status=row['status'])), flush=True)
            report['passed'] = all(row['status'] == 'passed' for row in report['results'])
            (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
