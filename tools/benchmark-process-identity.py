"""Alternate fixed builds on live process lookups and fork/exec/wait.

Guest timings exclude Python startup; every iteration checks identities or exact
child exit status. The result includes distribution hashes and whole-command CPU.
"""
import argparse
import json
import os
from pathlib import Path
import statistics

from init_pool import InitPool, distribution_hashes


SOURCE = r'''
import json, os, time
iterations, forks = %d, %d
pid = os.getpid()
group, session = os.getpgid(pid), os.getsid(pid)
began = time.monotonic_ns()
for i in range(iterations):
    assert os.getpgid(pid) == group
    assert os.getsid(pid) == session
lookups = (time.monotonic_ns() - began) / 1e6
reader, writer = os.pipe()
helper = os.fork()
if helper == 0:
    os.close(writer)
    assert os.read(reader, 1) == b''
    os._exit(0)
os.close(reader)
try:
    began = time.monotonic_ns()
    for i in range(iterations):
        assert os.getpgid(helper) == group
        assert os.getsid(helper) == session
    descendant = (time.monotonic_ns() - began) / 1e6
finally:
    os.close(writer)
    waited, status = os.waitpid(helper, 0)
    assert waited == helper and status == 0
began = time.monotonic_ns()
for i in range(forks):
    child = os.fork()
    if child == 0:
        assert os.getpid() != pid and os.getppid() == pid
        assert os.getpgid(0) == group and os.getsid(0) == session
        os.execl('/bin/true', 'true')
    waited, status = os.waitpid(child, 0)
    assert waited == child and os.WIFEXITED(status) and os.WEXITSTATUS(status) == 0
result = dict(lookups_ms=lookups, descendant_lookups_ms=descendant,
              fork_exec_wait_ms=(time.monotonic_ns() - began) / 1e6)
with open('/var/tmp/process-identity-result.json', 'w') as file:
    json.dump(result, file)
print('PROCESS_IDENTITY_OK', flush=True)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'control-dist', 'candidate-dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--iterations', type=int, default=20000)
    parser.add_argument('--forks', type=int, default=64)
    args = parser.parse_args()
    if min(args.rounds, args.iterations, args.forks) < 1:
        parser.error('rounds, iterations and forks must be positive')
    args.output.mkdir(parents=True, exist_ok=False)
    for name in ('KINAKAZE_STARTUP_PROFILE', 'KINAKAZE_LOADER_PROFILE', 'KINAKAZE_FORK_TRACE'):
        os.environ.pop(name, None)
    report = dict(passed=False, iterations=args.iterations, forks=args.forks, runs=[], hashes={})
    output = args.output / 'report.json'
    try:
        for label in ('control', 'candidate'):
            dist = getattr(args, label + '_dist').resolve()
            report['hashes'][label] = distribution_hashes(dist)
        for iteration in range(-1, args.rounds):
            order = ('control', 'candidate') if iteration % 2 == 0 else ('candidate', 'control')
            for label in order:
                root = args.root.resolve()
                dist = getattr(args, label + '_dist').resolve()
                with InitPool(root, dist, args.output / f'{iteration}-{label}', size=2, timeout=180) as pool:
                    row = pool.run(['/usr/bin/python3.11', '-c', SOURCE % (args.iterations, args.forks)],
                                   expect=['PROCESS_IDENTITY_OK'],
                                   environment=['PATH=/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL=C'])
                row.update(round=iteration, label=label, warmup=iteration < 0)
                report['runs'].append(row)
                if row['status'] != 'passed':
                    raise AssertionError(f'{iteration} {label}: {row["status"]}')
                row['guest'] = json.loads((root / 'var/tmp/process-identity-result.json').read_text())
                print(json.dumps(dict(round=iteration, label=label, **row['guest'])), flush=True)
                output.write_text(json.dumps(report, indent=2), encoding='utf-8')
        report['medians'] = {
            label: {key: statistics.median(row['guest'][key] for row in report['runs']
                                          if row['label'] == label and not row['warmup'])
                    for key in ('lookups_ms', 'descendant_lookups_ms', 'fork_exec_wait_ms')}
            for label in ('control', 'candidate')}
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    finally:
        output.write_text(json.dumps(report, indent=2), encoding='utf-8')
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
