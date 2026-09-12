"""Measure successive applications served by one persistent init prewarm pool.

Timings include reservation waits and full guest execution through exit. Optional
ready markers measure observed application readiness without discarding exit time.
"""
import argparse
import json
from pathlib import Path
import statistics
import time

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--size', type=int, default=2)
    parser.add_argument('--repeat', type=int, default=10)
    parser.add_argument('--interval', type=float, default=0, help='seconds between requests, reported separately')
    parser.add_argument('--timeout', type=float, default=120)
    parser.add_argument('--profile', action='store_true')
    parser.add_argument('--expect', action='append', default=[])
    parser.add_argument('--ready-marker')
    parser.add_argument('--commands-json', type=Path, help='list of command/cwd/environment/expect/expected_exit/ready_marker objects')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not 1 <= args.size <= 8 or args.repeat < 1 or not 0 <= args.interval <= 60 or args.timeout <= 0:
        parser.error('invalid size, repeat, interval or timeout')
    if args.commands_json:
        if command: parser.error('choose a command or commands-json')
        cases = json.loads(args.commands_json.read_text(encoding='utf-8'))
    elif command:
        cases = [dict(command=command, expect=args.expect, ready_marker=args.ready_marker)]
    else:
        parser.error('supply an application command')
    args.output.mkdir(parents=True, exist_ok=True)
    report = dict(scope=__doc__, root=str(args.root.resolve()), dist=str(args.dist.resolve()), pool_size=args.size,
        interval_seconds=args.interval, profile=args.profile, sha256=distribution_hashes(args.dist.resolve()), results=[])
    def save():
        (args.output/'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    with InitPool(args.root, args.dist, args.output, args.size, args.timeout, args.profile) as pool:
        report['preparation_ms'] = pool.preparation_ms
        for iteration in range(args.repeat):
            for case in cases:
                if report['results'] and args.interval:
                    time.sleep(args.interval)
                row = dict(round=iteration, **pool.run(**case))
                report['results'].append(row)
                print(json.dumps(row), flush=True)
                save()
        pool.shutdown()
        report['summary'] = {}
        for field in ['reservation_ms', 'request_to_exit_ms', 'activation_to_exit_ms', 'request_to_ready_ms']:
            values = sorted(row[field] for row in report['results'] if row['status']=='passed' and row[field] is not None)
            report['summary'][field] = dict(count=len(values), median=statistics.median(values) if values else None,
                p95=values[max(0,(len(values)*95+99)//100-1)] if values else None, maximum=max(values) if values else None)
        save()
        print(json.dumps(report['summary']), flush=True)
    return int(any(row['status'] != 'passed' for row in report['results']))


if __name__ == '__main__':
    raise SystemExit(main())
