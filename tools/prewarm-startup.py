"""Prepare a single-use worker under init, then time activation through guest exit.

Init owns preparation, the activation barrier, process identity and cleanup.
Guest code (including ELF resolvers/constructors) starts only after activation.
Preparation time is reported separately and is never counted as free startup.
This bounded runner captures stdout/stderr and supplies EOF on guest stdin.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time
import uuid

from session_process import SessionProcess
from init_controller import Controller


def launch(args, index):
    prefix = args.output / f'run-{index}'
    endpoint = r'\\.\pipe\kinakaze-prewarm-' + uuid.uuid4().hex
    token = uuid.uuid4().hex + uuid.uuid4().hex
    env = os.environ.copy()
    env['KINAKAZE_V2_TOKEN'] = token
    env.pop('KINAKAZE_STARTUP_PROFILE', None)
    env.pop('KINAKAZE_LOADER_PROFILE', None)
    if args.profile:
        directory = prefix.resolve().with_suffix('.profile')
        directory.mkdir(exist_ok=True)
        env['KINAKAZE_STARTUP_PROFILE'] = str(directory)
        env['KINAKAZE_LOADER_PROFILE'] = str(directory)
    start = time.perf_counter_ns()
    child = SessionProcess([str(args.dist / 'init.exe'), '--pipe', endpoint,
                            '--controller-pid', str(os.getpid()), '--prewarm-root', str(args.root),
                            '--prewarm-dist', str(args.dist), '--', *args.command],
                           stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    # Drain both output pipes throughout preparation, activation and execution.
    # A watchdog closes the owned Job if any synchronous RPC or guest hangs.
    executor = concurrent.futures.ThreadPoolExecutor(max_workers=1)
    def collect():
        try:
            return child.process.communicate(timeout=args.timeout + args.hold)
        except subprocess.TimeoutExpired:
            child.close()
            return child.process.communicate(timeout=10)
    output = executor.submit(collect)
    controller = None
    try:
        controller = Controller(endpoint, token, child, time.monotonic() + args.timeout)
        controller.call({'AwaitPrewarmReady': {'pid': 1}})
        ready = time.perf_counter_ns()
        prepared_cpu = child.cpu_metrics()
        if args.hold:
            time.sleep(args.hold)
        activation_cpu_start = child.cpu_metrics()
        activated = time.perf_counter_ns()
        controller.call({'ActivatePrewarm': {'pid': 1}})
        reply = controller.call({'AwaitExit': {'pid': 1}})
        exited = time.perf_counter_ns()
        metrics = child.cpu_metrics()
        controller.call('Shutdown')
        stdout, stderr = output.result(timeout=args.timeout)
        ended = time.perf_counter_ns()
        prefix.with_suffix('.stdout.log').write_bytes(stdout)
        prefix.with_suffix('.stderr.log').write_bytes(stderr)
        missing = [marker for marker in args.expect if marker not in (stdout + stderr).decode('utf-8', errors='replace')]
        code = reply['Exit']['status']
        return dict(round=index, preparation_ms=(ready-start)/1e6,
                    hold_ms=(activated-ready)/1e6, activation_to_exit_ms=(exited-activated)/1e6,
                    activation_through_cleanup_ms=(ended-activated)/1e6,
                    total_ms=(ended-start)/1e6, exit_code=code, init_exit_code=child.process.returncode,
                    status='passed' if code == 0 and child.process.returncode == 0 and not missing else 'failed',
                    missing_markers=missing, cpu_metrics=metrics, preparation_cpu_metrics=prepared_cpu,
                    idle_cpu_ms=activation_cpu_start['total_cpu_ms']-prepared_cpu['total_cpu_ms'],
                    activation_cpu_metrics={key: metrics[key]-activation_cpu_start[key] for key in metrics})
    finally:
        if controller:
            controller.pipe.close()
        child.close()
        executor.shutdown(wait=True)
        # Preserve diagnostics even when preparation or an RPC fails.
        if not output.cancelled() and output.exception() is None:
            stdout, stderr = output.result()
            prefix.with_suffix('.stdout.log').write_bytes(stdout)
            prefix.with_suffix('.stderr.log').write_bytes(stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=5)
    parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('--hold', type=float, default=0, help='seconds to leave the prepared worker idle before activation')
    parser.add_argument('--profile', action='store_true')
    parser.add_argument('--expect', action='append', default=[])
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    args.command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not args.command or args.repeat < 1 or args.timeout <= 0 or not 0 <= args.hold <= 60:
        parser.error('supply a command, positive repeat/timeout, and hold in 0..60')
    args.root, args.dist = args.root.resolve(), args.dist.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    files = [args.dist / 'worker.exe', args.dist / 'init.exe', *sorted((args.dist / 'rootfs/lib').glob('*'))]
    hashes = {}
    for path in files:
        if path.is_file():
            with path.open('rb') as stream:
                hashes[str(path.relative_to(args.dist))] = hashlib.file_digest(stream, 'sha256').hexdigest()
    report = dict(command=args.command, root=str(args.root), dist=str(args.dist), scope=__doc__,
                  profile=args.profile, sha256=hashes, results=[])
    for index in range(args.repeat):
        try:
            row = launch(args, index)
        except Exception as error:
            row = dict(round=index, status='failed', error=str(error))
        report['results'].append(row)
        print(json.dumps(row), flush=True)
        (args.output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    report['summary'] = {}
    for field in ['preparation_ms', 'activation_to_exit_ms', 'activation_through_cleanup_ms', 'total_ms']:
        values = sorted(r[field] for r in report['results'] if r['status'] == 'passed')
        report['summary'][field] = dict(count=len(values), median=statistics.median(values) if values else None,
            p95=values[max(0, (len(values)*95+99)//100-1)] if values else None,
            maximum=max(values) if values else None)
    (args.output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report['summary']), flush=True)
    return int(any(row['status'] != 'passed' for row in report['results']))


if __name__ == '__main__':
    raise SystemExit(main())
