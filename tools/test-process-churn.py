"""Verify bounded init transactions during real fork/exec, while the owner is alive."""
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
    parser.add_argument('--count', type=int, default=1100)
    parser.add_argument('--modes', nargs='+', choices=('fork', 'exec'), default=['fork', 'exec'])
    parser.add_argument('--timeout', type=float, default=300)
    args = parser.parse_args()
    if args.count < 1 or args.timeout <= 0:
        parser.error('count and timeout must be positive')
    root, dist = args.root.resolve(), args.dist.resolve()
    guest = '/var/tmp/process-churn-' + uuid.uuid4().hex
    stage = root / guest.lstrip('/')
    stage.mkdir(parents=True)
    shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/guest/ProcessChurnProbe.py', stage / 'probe.py')
    report = dict(root=str(root), dist=str(dist), count=args.count,
                  sha256=distribution_hashes(dist), results=[])
    for mode in args.modes:
        output = args.output / mode
        row = dict(mode=mode, status='failed')
        with InitPool(root, dist, output, size=1, timeout=args.timeout) as pool:
            before = pool.controller.call('Stats')['Stats']
            application = pool.launch(['/usr/bin/python3', guest + '/probe.py',
                                       mode, str(args.count), guest + '/' + mode])
            exited = threading.Event()
            outcome = {}
            def wait():
                try:
                    outcome['status'] = application.wait()
                except Exception as error:
                    outcome['error'] = str(error)
                finally:
                    exited.set()
            waiter = threading.Thread(target=wait, daemon=True)
            waiter.start()
            deadline = time.monotonic() + args.timeout - 5
            while True:
                with pool.lock:
                    ready = ('CHURN_READY ' + mode).encode() in pool.buffers[0]
                if ready or exited.is_set() or pool.child.process.poll() is not None or time.monotonic() > deadline:
                    break
                time.sleep(.02)
            during = pool.controller.call('Stats')['Stats'] if ready else None
            (stage / mode).touch()
            if not exited.wait(10):
                pool.close()
            waiter.join(5)
            status = outcome.get('status')
            row.update(exit_code=status, ready=ready, before=before, during=during,
                       application_cpu_metrics=application.cpu_metrics, error=outcome.get('error'))
            # A last completion can await the next worker request; no record may
            # accumulate with operation count. One standby worker is also alive.
            row['status'] = 'passed' if ready and status == 0 and during['transactions'] <= before['transactions'] + 1 else 'failed'
        report['results'].append(row)
        (args.output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
        print(json.dumps(row), flush=True)
    return int(any(row['status'] != 'passed' for row in report['results']))


if __name__ == '__main__':
    raise SystemExit(main())
