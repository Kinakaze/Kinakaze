import argparse
import json
import os
from pathlib import Path
import statistics
import sys

workspace = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(workspace / 'tools'))
from init_pool import InitPool, distribution_hashes

SOURCE = '''import os, time
def descendants(depth):
    if depth == 0:
        return
    pid = os.fork()
    assert pid >= 0
    if pid == 0:
        descendants(depth - 1)
        os._exit(0)
    waited, status = os.waitpid(pid, 0)
    assert waited == pid and status == 0
began = time.monotonic_ns()
for iteration in range(8):
    descendants(16)
print('DESCENDANTS_128_OK', (time.monotonic_ns() - began) / 1e6, flush=True)
'''
parser = argparse.ArgumentParser()
for name in ('root', 'dist', 'output'):
    parser.add_argument('--' + name, type=Path, required=True)
parser.add_argument('--rounds', type=int, default=3)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
rows = []
for iteration in range(args.rounds):
    labels = ('off', 'on') if iteration % 2 == 0 else ('on', 'off')
    for label in labels:
        previous = os.environ.get('KINAKAZE_FORK_WRITE_WATCH_CHILDREN')
        os.environ['KINAKAZE_FORK_WRITE_WATCH_CHILDREN'] = '0' if label == 'off' else '1'
        try:
            with InitPool(args.root, args.dist, args.output / f'{iteration}-{label}', size=2, timeout=180) as pool:
                result = pool.run(['/usr/bin/python3.11', '-c', SOURCE],
                    environment=['PATH=/usr/bin:/bin', 'LC_ALL=C'], expect=['DESCENDANTS_128_OK'])
        finally:
            if previous is None:
                os.environ.pop('KINAKAZE_FORK_WRITE_WATCH_CHILDREN', None)
            else:
                os.environ['KINAKAZE_FORK_WRITE_WATCH_CHILDREN'] = previous
        assert result['status'] == 'passed', result
        result.update(round=iteration, label=label)
        rows.append(result)
        print(json.dumps(dict(round=iteration, label=label, ms=result['request_to_exit_ms'],
            cpu=result['cpu_metrics'])), flush=True)
        (args.output / 'report.json').write_text(json.dumps(dict(passed=False, rows=rows), indent=2))
report = dict(passed=True, rows=rows, hashes=distribution_hashes(args.dist),
    medians={label: {key: statistics.median(
        row['cpu_metrics'][key] for row in rows if row['label'] == label)
        for key in ('total_cpu_ms', 'page_faults', 'processes')}
        | {'request_to_exit_ms': statistics.median(row['request_to_exit_ms']
            for row in rows if row['label'] == label)}
        for label in ('off', 'on')})
(args.output / 'report.json').write_text(json.dumps(report, indent=2))
print(json.dumps(report['medians']), flush=True)
