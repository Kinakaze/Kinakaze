"""Native BPF, FIFO and Netlink lifetimes across real Debian workers."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
import time
import uuid
from pathlib import Path

import psutil
from init_pool import InitPool


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    source = (Path(__file__).resolve().parents[1] / 'tests/guest/BpfFifoNetlinkLifecycleProbe.py').read_text(encoding='utf-8')
    target = '/tmp/native-descriptions-' + uuid.uuid4().hex
    checks = []
    with InitPool(args.root, args.dist, args.output, size=1, timeout=180) as pool:
        def command(mode):
            return ['/usr/bin/python3', '-c', source, mode, target, source]

        def run(mode):
            row = pool.run(command(mode), expect=['PASS_' + mode])
            row['name'] = mode
            checks.append(row)
            assert row['status'] == 'passed', row

        def launch_ready(mode, marker):
            with pool.lock:
                offset = len(pool.buffers[0])
            application = pool.launch(command(mode))
            deadline = time.monotonic() + 15
            while True:
                with pool.lock:
                    if marker in pool.buffers[0][offset:]:
                        return application
                assert time.monotonic() < deadline, mode + ' did not become ready'
                time.sleep(.01)

        owner = launch_ready('crash-owner', b'HANDLE_CRASH_READY')
        kernel = c.WinDLL('kernel32', use_last_error=True)
        kernel.GetProcessId.argtypes = [w.HANDLE]
        kernel.GetProcessId.restype = w.DWORD
        psutil.Process(kernel.GetProcessId(owner.sample.handle)).kill()
        owner.wait()
        for mode in ['crash-check', 'failed-load', 'dup-exec', 'rights']:
            run(mode)
        owner = launch_ready('fifo-owner', b'FIFO_OWNER_READY')
        with InitPool(args.root, args.dist, args.output / 'isolated', size=1, timeout=30) as isolated:
            row = isolated.run(command('fifo-isolated'), expect=['PASS_fifo-isolated'])
            row['name'] = 'fifo-isolated'
            checks.append(row)
            assert row['status'] == 'passed', row
        run('fifo-original')
        assert owner.wait() == 0
        run('fifo-clean')
        owner = launch_ready('netlink-owner', b'NETLINK_OWNER_READY')
        run('netlink-occupied')
        assert owner.wait() == 0
        for mode in ['netlink-released', 'netlink-fork', 'netlink-rights']:
            run(mode)
    result = dict(passed=True, checks=checks)
    (args.output / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(dict(passed=True, checks=[row['name'] for row in checks]), indent=2))


if __name__ == '__main__':
    main()
