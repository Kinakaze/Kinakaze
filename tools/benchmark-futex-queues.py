"""Alternate optimization switches in fixed release native test binaries.

Native queue/template/page-copy timings are component measurements, not guest
syscall throughput or end-to-end installation performance.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess


CASES = {
    'pthread': ('KINAKAZE_PTHREAD_PARK_OPT', 'parking::tests::benchmark_event_parking', 'PTHREAD_PARK_BENCH'),
    'futex': ('KINAKAZE_FUTEX_OPT', 'futex::queue_tests::benchmark_shared_queue', 'FUTEX_BENCH'),
    'waitv-background': ('KINAKAZE_FUTEX_OPT', 'sysadmin::futex_vector::batch_tests::benchmark_vector_registration', 'FUTEX_VECTOR_BENCH'),
    'waitv': ('KINAKAZE_FUTEX_OPT', 'sysadmin::futex_vector::tests::benchmark_waitv_registration', 'WAITV_BENCH'),
    'requeue': ('KINAKAZE_FUTEX_OPT', 'sysadmin::futex_requeue::tests::benchmark_requeue2_paths', 'REQUEUE_BENCH'),
    'tmpfs': ('KINAKAZE_TMPFS_READ_OPT', 'tmpfs::read_pages::tests::benchmark_shared_read', 'TMPFS_READ_BENCH'),
    'syscall': ('KINAKAZE_SYSCALL_TEMPLATE', 'execution::instruction_trampoline::tests::benchmark_syscall_templates', 'SYSCALL_TEMPLATE_BENCH'),
    'route': ('KINAKAZE_IO_ROUTE_OPT', 'io_route_tests::benchmark_io_routes', 'IO_ROUTE_BENCH'),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--case', choices=CASES, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--rounds', type=int, default=5)
    args = parser.parse_args()
    if not 1 <= args.rounds <= 30:
        parser.error('rounds must be between 1 and 30')
    binary = args.binary.resolve(strict=True)
    sysroot = subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip()
    environment = dict(os.environ, PATH=str(binary.parent) + os.pathsep + str(Path(sysroot) / 'bin') + os.pathsep + os.environ['PATH'])
    switch, test, marker = CASES[args.case]
    dependencies = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in binary.parent.glob('*.so*') if path.is_file()}
    report = dict(passed=False, case=args.case, binary=str(binary), sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), dependencies=dependencies, rows=[])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    try:
        for iteration in range(args.rounds + 1):
            order = (False, True) if iteration % 2 == 0 else (True, False)
            for enabled in order:
                environment[switch] = str(int(enabled))
                child = subprocess.run([str(binary), '--exact', test, '--ignored', '--nocapture', '--test-threads=1'], env=environment, text=True, encoding='utf-8', errors='replace', capture_output=True, timeout=120, creationflags=0x08000000 if os.name == 'nt' else 0)
                if child.returncode:
                    raise RuntimeError(f'{test}: exit={child.returncode}\n{child.stdout}\n{child.stderr}')
                lines = [line.partition(marker + ' ')[2] for line in child.stdout.splitlines() if marker + ' ' in line]
                if len(lines) != 1:
                    raise RuntimeError(f'missing result: {child.stdout}')
                row = json.loads(lines[0])
                if row.pop('optimized') != enabled:
                    raise RuntimeError(f'{switch} was not applied')
                row.update(round=iteration, enabled=enabled, warmup=iteration == 0)
                report['rows'].append(row)
                args.output.write_text(json.dumps(report, indent=2), encoding='utf-8')
        metrics = [key for key in report['rows'][0] if key.endswith('_ns')]
        report['medians_ns'] = {
            label: {key: statistics.median(row[key] for row in report['rows'] if not row['warmup'] and row['enabled'] == enabled) for key in metrics}
            for label, enabled in [('control', False), ('candidate', True)]
        }
        cycles = [key for key in report['rows'][0] if key.endswith('_cycles')]
        if cycles:
            report['medians_cycles'] = {
                label: {key: statistics.median(row[key] for row in report['rows'] if not row['warmup'] and row['enabled'] == enabled) for key in cycles}
                for label, enabled in [('control', False), ('candidate', True)]
            }
        report['passed'] = True
        print(json.dumps(report['medians_ns'], indent=2))
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        args.output.write_text(json.dumps(report, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
