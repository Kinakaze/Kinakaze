"""Validate persistent registries, SysV IPC, BPF isolation and fresh time contexts."""
import argparse
import json
import time
import uuid
from pathlib import Path

from init_pool import InitPool


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    source = (Path(__file__).resolve().parents[1] / 'tests/guest/KernelStateProbe.py').read_text(encoding='utf-8')
    suffix = uuid.uuid4().hex
    group = '/sys/fs/cgroup/kernel-state-' + suffix
    signal = '/tmp/kernel-state-' + suffix
    key = -int(suffix[:7], 16) - 1
    checks = []

    def command(mode, path=group):
        return ['/usr/bin/python3', '-c', source, mode, str(key), path]

    def run(pool, mode):
        row = pool.run(command(mode), expect=['PASS_' + mode])
        row['name'] = mode
        checks.append(row)
        assert row['status'] == 'passed', row

    with InitPool(args.root, args.dist, args.output, size=1, timeout=90) as pool:
        for mode in ('pty-set', 'pty-read', 'ipc-create', 'ipc-read', 'ipc-isolated', 'ipc-read', 'bpf-create', 'bpf-read'):
            run(pool, mode)
        with InitPool(args.root, args.dist, args.output / 'isolated', size=1, timeout=90) as isolated:
            run(isolated, 'ipc-absent')
            run(isolated, 'bpf-absent')
        for mode in ('bpf-remove', 'bpf-absent', 'ipc-remove', 'ipc-absent'):
            run(pool, mode)
        with pool.lock:
            offset = len(pool.buffers[0])
        parent = pool.launch(command('time-parent', signal))
        deadline = time.monotonic() + 20
        while True:
            with pool.lock:
                ready = b'TIME_READY' in pool.buffers[0][offset:]
            if ready:
                break
            assert time.monotonic() < deadline, 'time parent did not become ready'
            time.sleep(.01)
        child = pool.controller.call('ReservePoolWorker')['PoolWorker']['pid']
        pool.controller.call({'ActivatePoolWorkerUnderParent': dict(pid=child, parent_pid=parent.pid,
            launch=dict(arguments=command('time-child', signal), cwd='/', environment=None))})
        status = pool.controller.call({'AwaitExit': {'pid': child}})['Exit']['status']
        assert status == 0, 'fresh command lost parent time namespace'
        assert parent.wait() == 0
        row = pool.run(['/bin/rm', signal])
        assert row['status'] == 'passed', row
        checks.append(dict(name='fresh time namespace inheritance', status='passed'))
    report = dict(passed=True, checks=checks)
    (args.output / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
