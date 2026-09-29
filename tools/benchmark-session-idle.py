"""Measure a disposable persistent session and its real Python broker.

Prepare the supplied root with the distribution's current manifest and native
modules first. Refuses to sample or stop an already running session.
"""
import argparse
import json
from pathlib import Path
import subprocess
import time

import psutil
from init_pool import distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--seconds', type=float, default=15)
    args = parser.parse_args()
    if not 1 <= args.seconds <= 120:
        parser.error('seconds must be within [1, 120]')
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    manifest = json.loads(args.manifest.read_text(encoding='utf-8'))
    manifest['startup']['tray'] = False
    manifest_path = output / 'manifest.json'
    manifest_path.write_text(json.dumps(manifest), encoding='utf-8')
    options = ['--root', str(root), '--dist', str(dist), '--rootfs-manifest', str(manifest_path)]

    def run(arguments, check=True):
        result = subprocess.run([str(dist / 'worker.exe'), *arguments],
            stdin=subprocess.DEVNULL, capture_output=True, timeout=90,
            creationflags=subprocess.CREATE_NO_WINDOW)
        if check and result.returncode:
            raise RuntimeError(result.stderr.decode(errors='replace'))
        return result

    status = run(['session', 'status', *options], check=False)
    if status.returncode == 0:
        state = json.loads(status.stdout)
        if state.get('pid'):
            raise RuntimeError('root already has a running session; use an idle test root')
    report = dict(root=str(root), dist=str(dist), sha256=distribution_hashes(dist),
                  status='failed', samples=[], scope='100 percent equals one CPU core')
    owned = False
    try:
        begin = time.monotonic()
        run(['session', 'start', *options])
        owned = True
        state = json.loads(run(['session', 'status', *options]).stdout)
        report['startup_ms'] = (time.monotonic() - begin) * 1000
        supervisor = psutil.Process(state['pid'])
        time.sleep(2)
        remaining = args.seconds
        while remaining > 0:
            processes = [supervisor, *supervisor.children(recursive=True)]
            before = {}
            for process in processes:
                try:
                    command = ' '.join(process.cmdline()).replace('\\', '/')
                    role = ('session-python' if '/usr/lib/kinakaze/session.py' in command else
                            'systemd' if '/lib/systemd/systemd --system' in command else
                            'udev' if 'systemd-udevd' in command else 'other')
                    cpu = process.cpu_times()
                    before[process.pid] = (process, cpu.user + cpu.system, role)
                except psutil.Error:
                    pass
            begin = time.monotonic()
            duration = min(5, remaining)
            time.sleep(duration)
            elapsed = time.monotonic() - begin
            rows = []
            for process, initial, role in before.values():
                try:
                    cpu = process.cpu_times()
                    rows.append(dict(pid=process.pid, role=role,
                        one_core_percent=(cpu.user + cpu.system - initial) / elapsed * 100,
                        working_set_bytes=process.memory_info().rss))
                except psutil.Error:
                    pass
            assert any(row['role'] == 'session-python' for row in rows), 'missing session broker'
            assert not any(row['role'] == 'udev' for row in rows), 'udev started by default'
            report['samples'].append(dict(wall_seconds=elapsed, processes=rows,
                one_core_percent=sum(row['one_core_percent'] for row in rows)))
            remaining -= duration
        report['status'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        if owned:
            begin = time.monotonic()
            stopped = run(['session', 'stop', *options], check=False)
            report['shutdown_ms'] = (time.monotonic() - begin) * 1000
            if stopped.returncode:
                report.update(status='failed', shutdown_error=stopped.stderr.decode(errors='replace'))
        (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps({key: value for key, value in report.items() if key != 'sha256'}, indent=2))
    return int(report['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
