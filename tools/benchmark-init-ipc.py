"""Measure authenticated init control round trips with no guest or worker startup.

The result includes Python framing and Windows scheduling, not just init CPU.
Alternate distribution order and report all samples; no timing pass threshold.
"""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import time
import uuid

from init_controller import Controller
from init_pool import distribution_hashes
from session_process import SessionProcess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', action='append', required=True, metavar='LABEL=PATH')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=5)
    parser.add_argument('--requests', type=int, default=5000)
    args = parser.parse_args()
    if args.repeat < 1 or args.requests < 1:
        parser.error('repeat and requests must be positive')
    distributions = {}
    for value in args.dist:
        label, sep, path = value.partition('=')
        if not sep or not label or label in distributions or not all(c.isalnum() or c in '-_' for c in label):
            parser.error('use unique LABEL=PATH distributions')
        distributions[label] = Path(path).resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    report = dict(scope=__doc__, distributions={label: dict(path=str(path), sha256=distribution_hashes(path))
                  for label, path in distributions.items()}, results=[], summary={})
    for iteration in range(args.repeat):
        order = list(distributions.items())
        if iteration % 2:
            order.reverse()
        for label, dist in order:
            endpoint = r'\\.\pipe\kinakaze-ipc-bench-' + uuid.uuid4().hex
            environment = os.environ.copy()
            token = uuid.uuid4().hex + uuid.uuid4().hex
            environment['KINAKAZE_V2_TOKEN'] = token
            with (args.output / f'{label}-{iteration}.log').open('wb') as log:
                child = SessionProcess([str(dist / 'init.exe'), '--pipe', endpoint,
                    '--controller-pid', str(os.getpid())], env=environment,
                    stdin=subprocess.DEVNULL, stdout=log, stderr=log)
                controller = None
                try:
                    controller = Controller(endpoint, token, child, time.monotonic() + 15)
                    for _ in range(100):
                        assert controller.call('Stats')['Stats']['processes'] == 0
                    before = child.cpu_metrics()
                    start = time.perf_counter_ns()
                    for _ in range(args.requests):
                        assert controller.call('Stats')['Stats']['processes'] == 0
                    elapsed = time.perf_counter_ns() - start
                    after = child.cpu_metrics()
                    row = dict(distribution=label, round=iteration, requests=args.requests,
                        microseconds_per_roundtrip=elapsed / args.requests / 1000,
                        requests_per_second=args.requests * 1e9 / elapsed,
                        cpu_metrics={key: after[key]-before[key] for key in after} if before and after else None)
                    report['results'].append(row)
                    print(json.dumps(row), flush=True)
                    controller.call('Shutdown')
                    assert child.process.wait(timeout=10) == 0
                finally:
                    if controller:
                        controller.pipe.close()
                    child.close()
            (args.output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    for label in distributions:
        values = [r['microseconds_per_roundtrip'] for r in report['results'] if r['distribution'] == label]
        report['summary'][label] = dict(median_us=statistics.median(values), min_us=min(values), max_us=max(values))
    (args.output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report['summary']), flush=True)


if __name__ == '__main__':
    main()
