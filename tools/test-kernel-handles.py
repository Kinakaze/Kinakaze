"""Cross-process ID, last-close, fork and descriptor transfer regression suite."""
import argparse
import json
import uuid
from pathlib import Path
from init_pool import InitPool


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'dist', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    probes = Path(__file__).resolve().parents[1] / 'tests/guest'
    source = (probes / 'KernelHandleProbe.py').read_text(encoding='utf-8')
    suffix = uuid.uuid4().hex
    fixture, key = '/tmp/kernel-handles-' + suffix, -int(suffix[:7], 16) - 1
    checks = []
    report = args.output / 'result.json'
    args.output.mkdir(parents=True, exist_ok=True)
    report.write_text(json.dumps(dict(passed=False, checks=[])), encoding='utf-8')
    with InitPool(args.root, args.dist, args.output, size=1, timeout=180) as pool:
        def run(name, command, marker):
            row = pool.run(command, expect=[marker])
            checks.append(dict(name=name, status=row['status'], exit_code=row['exit_code']))
            report.write_text(json.dumps(dict(passed=False, checks=checks), indent=2), encoding='utf-8')
            assert row['status'] == 'passed', row
        for mode in ('create', 'read-ids', 'reuse-key', 'exit-attached', 'after-exit',
                     'namespace-isolation', 'fork', 'exec', 'after-exit',
                     'cgroup-create', 'cgroup-read', 'remove', 'absent'):
            run(mode, ['/usr/bin/python3', '-c', source, mode, fixture, str(key)], 'PASS_' + mode)
        for name, marker in [
            ('UnixRightsProbe.py', 'UNIX_RIGHTS_LIFETIME_OK'),
            ('EventFdWaitSemanticsProbe.py', 'EVENTFD_DUP_FORK_MULTIWAITER_SEMAPHORE_WRITABLE_AFD_OK'),
            ('EpollPollReadinessProbe.py', 'EPOLL_POLL_NONCONSUMING_EDGE_ONESHOT_WAKE_OK'),
            ('ShmForkProbe.py', 'SYSV_MIT_SHM_FORK_SHARED_RMID_DETACH_OK'),
        ]:
            run(name, ['/usr/bin/python3', '-c', (probes / name).read_text(encoding='utf-8')], marker)
    report.write_text(json.dumps(dict(passed=True, checks=checks), indent=2), encoding='utf-8')
    print(report.read_text(encoding='utf-8'))


if __name__ == '__main__':
    main()
