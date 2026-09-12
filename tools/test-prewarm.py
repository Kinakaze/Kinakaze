"""Bounded real-init checks: no guest side effects before release, EOF, exit and cancellation."""
import argparse
import concurrent.futures
import ctypes
from ctypes import wintypes
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid

import psutil
from session_process import SessionProcess

spec = importlib.util.spec_from_file_location('prewarm', Path(__file__).with_name('prewarm-startup.py'))
prewarm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prewarm)


def check(args, case):
    marker = args.root / 'tmp' / ('prewarm-test-' + uuid.uuid4().hex)
    marker.parent.mkdir(exist_ok=True)
    endpoint = r'\\.\pipe\kinakaze-prewarm-test-' + uuid.uuid4().hex
    token = uuid.uuid4().hex + uuid.uuid4().hex
    env = os.environ.copy()
    env['KINAKAZE_V2_TOKEN'] = token
    for key in ['KINAKAZE_STARTUP_PROFILE', 'KINAKAZE_LOADER_PROFILE']:
        env.pop(key, None)
    # The empty package exercises death before the worker can authenticate.
    with tempfile.TemporaryDirectory(prefix='kinakaze-prewarm-test-') as empty:
        dist = empty if case == 'bootstrap-failure' else str(args.dist)
        owner = None
        if case == 'controller-death':
            owner = SessionProcess([sys.executable, '-c', 'import time; time.sleep(30)'],
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        controller_pid = owner.process.pid if owner else os.getpid()
        command = ['/bin/sh', '-c', f'printf activated > /tmp/{marker.name}; printf PREWARM_STDOUT; printf PREWARM_STDERR >&2; exit 7']
        pool_size = getattr(args, 'pool_size', None)
        launch = ['--prewarm-pool', str(pool_size)] if pool_size else ['--', *command]
        readiness = {'AwaitPoolReady': {'minimum': pool_size}} if pool_size else {'AwaitPrewarmReady': {'pid': 1}}
        child = SessionProcess([str(args.dist / 'init.exe'), '--pipe', endpoint,
            '--controller-pid', str(controller_pid), '--prewarm-root', str(args.root),
            '--prewarm-dist', dist, *launch], stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
        executor = concurrent.futures.ThreadPoolExecutor(max_workers=1)
        def collect():
            try:
                return child.process.communicate(timeout=20)
            except subprocess.TimeoutExpired:
                child.close()
                return child.process.communicate(timeout=10)
        output = executor.submit(collect)
        controller = None
        handle = None
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        try:
            if case == 'controller-death':
                deadline = time.monotonic() + 10
                while True:
                    children = psutil.Process(child.process.pid).children()
                    if children:
                        break
                    assert time.monotonic() < deadline, 'init did not create its worker'
                    time.sleep(0.01)
                handle = kernel.OpenProcess(0x00100000, False, children[0].pid)
                assert handle
                time.sleep(0.3)
                idle = None
                owner.process.kill()
                owner.process.wait(timeout=5)
            else:
                controller = prewarm.Controller(endpoint, token, child, time.monotonic() + 10)
            if case == 'bootstrap-failure':
                started = time.monotonic()
                try:
                    controller.call(readiness)
                except RuntimeError as error:
                    assert 'Aborted' in str(error) or 'NotFound' in str(error), str(error)
                else:
                    raise AssertionError('failed preparation was reported ready')
                assert time.monotonic() - started < 5, 'early worker death left a blocked waiter'
            elif case != 'controller-death':
                controller.call(readiness)
                children = psutil.Process(child.process.pid).children()
                assert len(children) == (pool_size or 1), 'unexpected standby/console host count'
                # Pin the native object before checking cleanup; PID reuse is harmless.
                handle = kernel.OpenProcess(0x00100000, False, children[0].pid)
                assert handle, 'could not pin prepared worker'
                assert not marker.exists(), 'guest code ran during prewarming'
                before = child.cpu_metrics()
                controller.pipe.close()
                controller = None
                time.sleep(0.1)
                assert not marker.exists(), 'controller EOF activated guest code'
                idle = child.cpu_metrics()['total_cpu_ms'] - before['total_cpu_ms']
                controller = prewarm.Controller(endpoint, token, child, time.monotonic() + 10)
                if case == 'activate-and-exit':
                    if pool_size:
                        pid = controller.call('ReservePoolWorker')['PoolWorker']['pid']
                        controller.call({'ActivatePoolWorker': {'pid': pid, 'launch': dict(
                            arguments=command, cwd='/', environment=None)}})
                    else:
                        pid = 1
                        controller.call({'ActivatePrewarm': {'pid': pid}})
                    assert controller.call({'AwaitExit': {'pid': pid}}) == {'Exit': {'status': 7}}
                    assert marker.read_text(encoding='utf-8') == 'activated'
            if controller:
                controller.call('Shutdown')
            stdout, stderr = output.result(timeout=10)
            assert child.process.returncode == 0, stderr.decode(errors='replace')
            if handle:
                # Check BEFORE closing the outer test Job: init must own cleanup.
                assert kernel.WaitForSingleObject(handle, 5000) == 0, 'worker outlived init'
            if case == 'activate-and-exit':
                assert b'PREWARM_STDOUT' in stdout and b'PREWARM_STDERR' in stderr
            else:
                assert not marker.exists(), 'canceled/failed preparation executed the guest'
                assert b'PREWARM_STDOUT' not in stdout
            return dict(case=case, status='passed', idle_cpu_ms=idle if handle else None)
        finally:
            if controller:
                controller.pipe.close()
            if handle:
                kernel.CloseHandle(handle)
            child.close()
            if owner:
                owner.close()
            executor.shutdown(wait=True)
            if not output.cancelled() and output.exception() is None:
                stdout, stderr = output.result()
                (args.output / f'{case}.stdout.log').write_bytes(stdout)
                (args.output / f'{case}.stderr.log').write_bytes(stderr)
            marker.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--pool-size', type=int, choices=range(1, 9), help='exercise generic pool lifecycle instead of fixed prewarm')
    args = parser.parse_args()
    args.root, args.dist = args.root.resolve(), args.dist.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    rows = []
    for case in ['activate-and-exit', 'cancel-ready-worker', 'bootstrap-failure', 'controller-death']:
        try:
            row = check(args, case)
        except Exception as error:
            row = dict(case=case, status='failed', error=str(error))
        rows.append(row)
        print(json.dumps(row), flush=True)
        (args.output / 'results.json').write_text(json.dumps(rows, indent=2), encoding='utf-8')
    return int(any(row['status'] != 'passed' for row in rows))


if __name__ == '__main__':
    raise SystemExit(main())
