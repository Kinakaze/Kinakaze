"""Alternate the ordinary PE discovery and shared catalog in the same build."""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import statistics
import threading
import time

import psutil
from init_pool import InitPool, distribution_hashes


@contextmanager
def catalog_mode(enabled):
    previous = os.environ.get('KINAKAZE_NATIVE_CATALOG')
    os.environ['KINAKAZE_NATIVE_CATALOG'] = '1' if enabled else '0'
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop('KINAKAZE_NATIVE_CATALOG', None)
        else:
            os.environ['KINAKAZE_NATIVE_CATALOG'] = previous


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=4)
    parser.add_argument('--children', type=int, default=32)
    parser.add_argument('--profile', action='store_true')
    args = parser.parse_args()
    if args.repeat <= 0 or args.children <= 0:
        parser.error('repeat and children must be positive')
    root, dist, output = args.root.resolve(), args.dist.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = dict(passed=False, diagnostic=args.profile, hashes=distribution_hashes(dist),
                  children=args.children, rows=[], host_samples=[])
    stopped = threading.Event()

    def sample_host():
        while not stopped.wait(.25):
            compilers = []
            for process in psutil.process_iter(['pid', 'name']):
                if (process.info['name'] or '').lower() in ('cargo.exe', 'rustc.exe', 'link.exe'):
                    compilers.append(process.info)
            report['host_samples'].append(dict(time_ns=time.time_ns(), cpu_percent=psutil.cpu_percent(), compilers=compilers))

    def save():
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    monitor = threading.Thread(target=sample_host, daemon=True)
    monitor.start()
    try:
        for iteration in range(args.repeat):
            for enabled in ([False, True] if iteration % 2 == 0 else [True, False]):
                label = 'shared' if enabled else 'ordinary'
                with catalog_mode(enabled), InitPool(root, dist, output / f'{iteration}-{label}',
                        size=None, profile=args.profile, timeout=180) as pool:
                    scenarios = [
                        ('echo', ['/bin/busybox', 'echo', 'CATALOG_ECHO_OK'], 'CATALOG_ECHO_OK'),
                        ('spawn', ['/bin/bash', '-c',
                            f'for ((i=0;i<{args.children};i++)); do /bin/busybox true || exit; done; echo CATALOG_SPAWN_OK'],
                            'CATALOG_SPAWN_OK'),
                    ]
                    for scenario, command, marker in scenarios:
                        row = pool.run(command, expect=[marker])
                        row.update(iteration=iteration, mode=label, scenario=scenario,
                                   pool_preparation_ms=pool.preparation_ms)
                        report['rows'].append(row)
                        save()
                        print(json.dumps(dict(iteration=iteration, mode=label, scenario=scenario,
                                              status=row['status'], ms=row['request_to_exit_ms'])), flush=True)
                        assert row['status'] == 'passed', row
        report['summary'] = {}
        for scenario in ('echo', 'spawn'):
            modes = {}
            for mode in ('ordinary', 'shared'):
                rows = [row for row in report['rows'] if row['scenario'] == scenario and row['mode'] == mode]
                modes[mode] = dict(
                    wall_ms=statistics.median(row['request_to_exit_ms'] for row in rows),
                    cpu_ms=statistics.median(row['cpu_metrics']['total_cpu_ms'] for row in rows),
                    page_faults=statistics.median(row['cpu_metrics']['page_faults'] for row in rows),
                    processes=statistics.median(row['cpu_metrics']['processes'] for row in rows))
            report['summary'][scenario] = modes
        report['passed'] = True
        assert distribution_hashes(dist) == report['hashes'], 'distribution changed during comparison'
    except Exception as error:
        report['error'] = repr(error)
    finally:
        stopped.set()
        monitor.join()
        save()
    print(json.dumps({key: report[key] for key in ('passed', 'summary', 'error') if key in report}), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
