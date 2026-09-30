"""Measure native resources across repeated operations in one live guest."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import time
import uuid

import psutil
from init_pool import InitPool, distribution_hashes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    root, dist, output = (path.resolve() for path in (args.root, args.dist, args.output))
    if output.exists():
        parser.error('--output must be new to preserve previous evidence')
    output.mkdir(parents=True)
    guest = '/tmp/agent-resources-' + uuid.uuid4().hex
    control = root / guest.lstrip('/')
    control.mkdir(parents=True)
    source = (Path(__file__).resolve().parents[1] / 'tests/guest/AgentResourceLifetimeProbe.py').read_text(encoding='utf-8')
    report = dict(root=str(root), dist=str(dist), guest=guest, sha256=distribution_hashes(dist),
                  results=[], passed=False)
    try:
        with InitPool(root, dist, output / 'session', size=1, timeout=90,
                      memory_limit_bytes=1024**3) as pool:
            application = pool.launch(['/usr/bin/python3', '-c', source, guest])
            kernel = c.WinDLL('kernel32', use_last_error=True)
            kernel.GetProcessId.argtypes = [w.HANDLE]
            kernel.GetProcessId.restype = w.DWORD

            def ready(marker):
                deadline = time.monotonic() + 30
                while True:
                    with pool.lock:
                        seen = marker.encode() in pool.buffers[0]
                    if seen:
                        return
                    assert not pool.timed_out.is_set() and time.monotonic() < deadline, marker
                    time.sleep(.01)

            def sample(phase):
                native = psutil.Process(kernel.GetProcessId(application.sample.handle))
                return dict(phase=phase, handles=native.num_handles(), threads=native.num_threads(),
                            private_commit_bytes=native.memory_info().private)

            ready('RESOURCE_WARM_READY')
            before = sample('warm')
            report['results'].append(before)
            for phase in (1, 2):
                (control / ('go-' + str(phase))).touch()
                ready('RESOURCE_PHASE_{}_READY'.format(phase))
                row = sample(phase)
                report['results'].append(row)
                assert row['handles'] <= before['handles'] + 8, row
                assert row['threads'] <= before['threads'] + 2, row
                assert row['private_commit_bytes'] <= before['private_commit_bytes'] + 16 * 1024**2, row
            (control / 'stop').touch()
            assert application.wait() == 0
            report['memory_metrics'] = pool.child.memory_metrics()
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    finally:
        (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(dict(passed=report['passed'], resources=report['results'], error=report.get('error'))), flush=True)
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
