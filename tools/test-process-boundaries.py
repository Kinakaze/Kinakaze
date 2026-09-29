"""Run process lifecycle probes and archive stress under one owned init session."""
import argparse
import json
from pathlib import Path
import shutil
import threading
import time
import uuid

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--rounds', type=int, default=32)
    parser.add_argument('--diagnostics', action='store_true')
    args = parser.parse_args()
    if not 1 <= args.rounds <= 1000:
        parser.error('rounds must be 1..1000')
    root, dist, output = (p.resolve() for p in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    guest = '/root/process-boundaries-' + uuid.uuid4().hex
    fixture = root / guest.lstrip('/')
    fixture.mkdir()
    probes = [('ProcessExitBoundaryProbe.py', 'PROCESS_EXIT_BOUNDARY_OK'),
              ('ChildSignalProbe.py', 'CHILD_SIGNAL_EXIT_STOP_CONTINUE_REAP_EXEC_OK')]
    for name, _ in probes:
        shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/guest' / name, fixture / name)
    report = dict(guest=guest, rounds=args.rounds, diagnostics=args.diagnostics,
                  sha256=distribution_hashes(dist), results=[], passed=False)
    destination = output / 'results.json'
    stopped = threading.Event()
    sampler = None
    try:
        with InitPool(root, dist, output, size=1, timeout=max(180, args.rounds * 15)) as pool:
            def sample():
                with (output / 'manager-stats.jsonl').open('w') as log:
                    while not stopped.is_set():
                        try:
                            with pool.control_lock:
                                stats = pool.controller.call('Stats')['Stats']
                            log.write(json.dumps(dict(time=time.time(), stats=stats)) + '\n')
                            log.flush()
                        except Exception:
                            return
                        stopped.wait(.5)

            if args.diagnostics:
                sampler = threading.Thread(target=sample, daemon=True)
                sampler.start()
            for name, marker in probes:
                command = ['/usr/bin/python3', guest + '/' + name]
                if name == 'ProcessExitBoundaryProbe.py':
                    command.append(str(args.rounds))
                row = pool.run(command, expect=[marker])
                report['results'].append(dict(name=name, **row))
                destination.write_text(json.dumps(report, indent=2) + '\n')
                print(name, row['status'], flush=True)
                if row['status'] != 'passed':
                    stopped.set()
                    return 1
            script = ('set -e; cd ' + guest + '; mkdir tree extracted; '
                      'for i in {1..30}; do printf archive-data > tree/f$i; done; '
                      'for ((i=0;i<' + str(args.rounds) + ';i++)); do '
                      'tar -czf data.tar.gz tree; tar -xzf data.tar.gz -C extracted; '
                      'diff -r tree extracted/tree; done; echo ARCHIVE_BOUNDARY_OK')
            row = pool.run(['/bin/bash', '-c', script], expect=['ARCHIVE_BOUNDARY_OK'])
            report['results'].append(dict(name='archive', **row))
            print('archive', row['status'], flush=True)
            report['passed'] = all(r['status'] == 'passed' for r in report['results'])
            stopped.set()
    finally:
        stopped.set()
        if sampler is not None:
            sampler.join(timeout=5)
        destination.write_text(json.dumps(report, indent=2) + '\n')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
