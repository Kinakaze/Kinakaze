"""Measure complete owned guest launches, validating exit status and output.

Alternate distribution order each round to reduce drift. Timings include worker,
init, guest execution and exit; loader profiling is optional and measured separately.
This does not claim an OS-cold filesystem cache or GUI readiness measurement.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time

from session_process import SessionProcess


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', action='append', required=True, metavar='LABEL=PATH')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=5)
    parser.add_argument('--warmup', type=int, default=1)
    parser.add_argument('--timeout', type=float, default=90)
    parser.add_argument('--expect', action='append', default=[])
    parser.add_argument('--profile', action='store_true')
    parser.add_argument('--cpu-metrics', action='store_true', help='record owned process-tree CPU and IO counters')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command or args.repeat < 1 or args.warmup < 0 or args.timeout <= 0:
        parser.error('supply a command, positive repeat/timeout and nonnegative warmup')
    distributions = {}
    for entry in args.dist:
        label, sep, directory = entry.partition('=')
        if not sep or not label or any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in label):
            parser.error('distributions must be LABEL=PATH with a simple unique label')
        if label in distributions:
            parser.error('duplicate distribution label')
        distributions[label] = Path(directory).resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    report = dict(command=command, root=str(args.root.resolve()), scope=__doc__.strip(),
                  profile=args.profile, distributions={}, results=[], summary={})
    for label, directory in distributions.items():
        files = [directory / 'worker.exe', directory / 'init.exe',
                 *sorted((directory / 'rootfs/lib').glob('*'))]
        report['distributions'][label] = dict(path=str(directory), sha256={
            str(path.relative_to(directory)): sha256(path)
            for path in files if path.is_file()})
    failed = False
    for round_number in range(-args.warmup, args.repeat):
        labels = list(distributions)
        if round_number % 2:
            labels.reverse()
        for label in labels:
            directory = distributions[label]
            prefix = args.output / f'{label}-{round_number}'
            environment = os.environ.copy()
            environment.pop('KINAKAZE_LOADER_PROFILE', None)
            environment.pop('KINAKAZE_STARTUP_PROFILE', None)
            if args.profile:
                profile = prefix.resolve().with_suffix('.profile')
                profile.mkdir(exist_ok=True)
                environment['KINAKAZE_LOADER_PROFILE'] = str(profile)
                environment['KINAKAZE_STARTUP_PROFILE'] = str(profile)
            start = time.perf_counter_ns()
            child = SessionProcess([str(directory / 'worker.exe'), 'run', '--root', str(args.root.resolve()),
                                    '--dist', str(directory), '--', *command],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment)
            status = 'passed'
            cpu_metrics = None
            try:
                stdout, stderr = child.process.communicate(timeout=args.timeout)
                elapsed_ms = (time.perf_counter_ns() - start) / 1e6
                if args.cpu_metrics:
                    cpu_metrics = child.cpu_metrics()
            except subprocess.TimeoutExpired:
                elapsed_ms = (time.perf_counter_ns() - start) / 1e6
                status = 'timeout'
                child.close()
                stdout, stderr = child.process.communicate(timeout=10)
            finally:
                child.close()
            prefix.with_suffix('.stdout.log').write_bytes(stdout)
            prefix.with_suffix('.stderr.log').write_bytes(stderr)
            output = (stdout + stderr).decode('utf-8', errors='replace')
            missing = [marker for marker in args.expect if marker not in output]
            if status == 'passed' and (child.process.returncode != 0 or missing):
                status = 'failed'
            failed |= status != 'passed'
            row = dict(distribution=label, round=round_number, warmup=round_number < 0,
                       elapsed_ms=elapsed_ms, exit_code=child.process.returncode,
                       status=status, missing_markers=missing)
            if args.cpu_metrics:
                row['cpu_metrics'] = cpu_metrics
            report['results'].append(row)
            print(json.dumps(row), flush=True)
            (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    for label in distributions:
        rows = [row for row in report['results'] if row['distribution'] == label and not row['warmup']]
        values = [row['elapsed_ms'] for row in rows if row['status'] == 'passed']
        report['summary'][label] = dict(passed=len(values), total=len(rows),
                                       median_ms=statistics.median(values) if values else None,
                                       min_ms=min(values) if values else None,
                                       max_ms=max(values) if values else None)
    (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report['summary']), flush=True)
    return int(failed)


if __name__ == '__main__':
    raise SystemExit(main())
