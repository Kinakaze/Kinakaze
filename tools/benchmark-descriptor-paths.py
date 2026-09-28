"""Compare validated FD workloads in persistent init sessions, alternating order.

Guest timings exclude Python and worker startup. Host launch and CPU timings are
reported separately. This is a warm benchmark, not an OS-cold-cache claim.
"""
import argparse
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import statistics

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', action='append', required=True, metavar='LABEL=PATH')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=5)
    parser.add_argument('--warmup', type=int, default=1)
    parser.add_argument('--iterations', type=int, default=5000)
    parser.add_argument('--modes', nargs='+', default=[
        'select', 'poll', 'epoll-ready', 'epoll-empty', 'pipe-churn', 'eventfd-churn'])
    args = parser.parse_args()
    if args.repeat < 1 or args.warmup < 0 or not 0 < args.iterations <= 1_000_000:
        parser.error('repeat and iterations must be positive; warmup must be nonnegative')
    distributions = {}
    for item in args.dist:
        label, separator, path = item.partition('=')
        if not separator or not label or label in distributions or not all(
                c.isascii() and (c.isalnum() or c in '-_') for c in label):
            parser.error('supply unique, simple LABEL=PATH distributions')
        distributions[label] = Path(path).resolve()
    source_path = Path(__file__).resolve().parents[1] / 'tests/guest/DescriptorPerformanceProbe.py'
    source = source_path.read_text(encoding='utf-8')
    report = dict(scope=__doc__, root=str(args.root.resolve()),
        probe_sha256=hashlib.sha256(source.encode()).hexdigest(),
        distributions={label: dict(path=str(path), sha256=distribution_hashes(path))
            for label, path in distributions.items()}, results=[], summary={})
    args.output.mkdir(parents=True, exist_ok=True)

    def save():
        (args.output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    with ExitStack() as stack:
        pools = {label: stack.enter_context(InitPool(args.root, path,
            args.output / label, size=1, timeout=1800)) for label, path in distributions.items()}
        for mode in args.modes:
            for round_number in range(-args.warmup, args.repeat):
                labels = list(pools)
                if round_number % 2:
                    labels.reverse()
                for label in labels:
                    pool = pools[label]
                    with pool.lock:
                        offset = len(pool.buffers[0])
                    row = pool.run(['/usr/bin/python3', '-c', source, mode, str(args.iterations)],
                        expect=['PERF_END'])
                    row.pop('command', None)
                    row.update(distribution=label, mode=mode, round=round_number,
                        warmup=round_number < 0)
                    with pool.lock:
                        lines = bytes(pool.buffers[0][offset:]).decode('utf-8', errors='replace').splitlines()
                    timings = [json.loads(line.removeprefix('PERF_OK '))
                        for line in lines if line.startswith('PERF_OK ')]
                    if len(timings) == 1:
                        row['guest'] = timings[0]
                    else:
                        row['status'] = 'failed'
                    report['results'].append(row)
                    save()
                    print(json.dumps({key: row.get(key) for key in
                        ('distribution', 'mode', 'round', 'status', 'guest')}), flush=True)
                    if row['status'] != 'passed':
                        return 1
    for mode in args.modes:
        report['summary'][mode] = {}
        for label in distributions:
            rows = [row for row in report['results'] if row['distribution'] == label
                and row['mode'] == mode and not row['warmup']]
            report['summary'][mode][label] = dict(samples=len(rows),
                guest_median_ms=statistics.median(row['guest']['elapsed_ms'] for row in rows),
                launch_median_ms=statistics.median(row['request_to_exit_ms'] for row in rows),
                application_cpu_median_ms=statistics.median(
                    row['application_cpu_metrics']['total_cpu_ms'] for row in rows))
    save()
    print(json.dumps(report['summary'], indent=2))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
