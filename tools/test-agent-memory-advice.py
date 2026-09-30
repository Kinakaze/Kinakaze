"""Check sparse discard correctness and bounded resources in one live guest."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import time
import uuid

import psutil
from agent_resource_watch import AgentResourceWatch
from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=False)
    guest = '/tmp/memory-advice-' + uuid.uuid4().hex
    control = root / guest.lstrip('/')
    control.mkdir()
    probe = Path(__file__).resolve().parents[1] / 'tests/guest/AgentMemoryAdviceProbe.py'
    source = probe.read_text(encoding='utf-8')
    report = dict(sha256=distribution_hashes(dist), results=[], passed=False)
    watcher = None
    try:
        with InitPool(root, dist, output / 'session', size=1, timeout=120,
                      memory_limit_bytes=4 * 1024**3) as pool:
            watcher = AgentResourceWatch(pool, max_private_commit=4 * 1024**3)
            application = pool.launch(['/usr/bin/python3', '-c', source, guest])
            kernel = c.WinDLL('kernel32', use_last_error=True)
            kernel.GetProcessId.argtypes = [w.HANDLE]
            kernel.GetProcessId.restype = w.DWORD
            native = psutil.Process(kernel.GetProcessId(application.sample.handle))
            for phase in range(4):
                deadline = time.monotonic() + 40
                marker = f'MEMORY_PHASE_{phase}_READY'.encode()
                while True:
                    with pool.lock:
                        if marker in pool.buffers[0]:
                            break
                    if pool.timed_out.is_set() or time.monotonic() > deadline:
                        raise RuntimeError(f'missing phase {phase}; see guest stderr')
                    time.sleep(.01)
                memory = native.memory_info()
                row = dict(phase=phase, handles=native.num_handles(), threads=native.num_threads(),
                           private_commit_bytes=memory.private, working_set_bytes=memory.rss)
                report['results'].append(row)
                (control / f'go-{phase}').touch()
            report['exit_code'] = application.wait()
            assert report['exit_code'] == 0
            report['memory'] = pool.child.memory_metrics()
            # Warm one complete guest/host barrier as well as the memory path.
            # Preserve the first checkpoint so one-time setup stays visible.
            warm = report['results'][1]
            for row in report['results'][2:]:
                assert row['handles'] <= warm['handles'] + 4, row
                assert row['threads'] <= warm['threads'] + 2, row
                assert row['private_commit_bytes'] <= warm['private_commit_bytes'] + 16 * 1024**2, row
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    finally:
        if watcher:
            report['resources'] = watcher.finish()
            report['passed'] &= report['resources']['cleanup_passed'] and not report['resources']['limit_exceeded']
        (output / 'report.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(dict(passed=report['passed'], results=report['results'],
                          error=report.get('error'))), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
