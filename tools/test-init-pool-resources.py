"""Retain closed pools to verify cleanup without relying on Python GC."""
import argparse
import json
from pathlib import Path
import psutil
import time

from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=8)
    args = parser.parse_args()
    if args.repeat < 2:
        parser.error('--repeat must be at least two')
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    if output.exists():
        parser.error('--output must be new to retain previous evidence')
    output.mkdir(parents=True)
    owner = psutil.Process()

    def sample():
        return dict(handles=owner.num_handles(), threads=owner.num_threads(),
                    private_commit_bytes=owner.memory_info().private)

    report = dict(root=str(root), dist=str(dist), sha256=distribution_hashes(dist),
                  before=sample(), results=[], passed=False)
    retained = []
    source = "import os; os.write(1, b'x' * 1048576); os.write(2, b'y' * 1048576)"

    def check_closed(pool, iteration):
        row = dict(iteration=iteration, after=sample(),
                   buffered_bytes=sum(map(len, pool.buffers)),
                   open_streams=sum(not stream.closed for stream in
                                    (pool.child.process.stdout, pool.child.process.stderr)),
                   live_drainers=sum(thread.is_alive() for thread in pool.drainers),
                   samples=len(pool.samples), job_open=bool(pool.child.job),
                   watchdog_present=pool.watchdog is not None)
        report['results'].append(row)
        assert not any(row[name] for name in
                       ('buffered_bytes', 'open_streams', 'live_drainers', 'samples',
                        'job_open', 'watchdog_present')), row
        warm = report['results'][0]['after']
        assert row['after']['handles'] <= warm['handles'] + 2, row
        assert row['after']['threads'] <= warm['threads'] + 1, row
        assert row['after']['private_commit_bytes'] <= warm['private_commit_bytes'] + 8 * 1024 * 1024, row
        print(json.dumps(row), flush=True)

    try:
        for iteration in range(args.repeat):
            with InitPool(root, dist, output / str(iteration), size=None, timeout=30) as pool:
                retained.append(pool)
                result = pool.run(['/usr/bin/python3', '-c', source])
                assert result['status'] == 'passed', result
            # Compare after the first warm-up, retaining every closed object.
            check_closed(pool, iteration)
        # Keep a child alive until the watchdog kills its owned process tree.
        try:
            with InitPool(root, dist, output / 'timeout', size=None, timeout=2,
                          memory_limit_bytes=1024**3) as pool:
                retained.append(pool)
                application = pool.launch(['/usr/bin/python3', '-c',
                    "import subprocess,time; subprocess.Popen(['/bin/sleep','60']); "
                    "print('RESOURCE_TIMEOUT_READY',flush=True); time.sleep(60)"])
                deadline = time.monotonic() + 1
                while True:
                    with pool.lock:
                        ready = b'RESOURCE_TIMEOUT_READY' in pool.buffers[0]
                    if ready:
                        break
                    assert time.monotonic() < deadline, 'timeout fixture did not start'
                    time.sleep(.01)
                application.wait()
                raise AssertionError('watchdog did not interrupt application wait')
        except RuntimeError:
            assert pool.timed_out.is_set()
        check_closed(pool, 'timeout')
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    finally:
        (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
