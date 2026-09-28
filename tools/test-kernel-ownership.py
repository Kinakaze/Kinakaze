"""Real worker exits, shared mounts, namespaces and init-owned temporary data."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
import time
import uuid
from pathlib import Path

import psutil
from init_pool import InitPool


def job_workers(pool):
    # Exec can outlive its Windows parent. Enumerate the owned Job rather than
    # relying on Windows PPIDs to discover Linux process descendants.
    class Processes(c.Structure):
        _fields_ = [('assigned', w.DWORD), ('count', w.DWORD), ('pids', c.c_size_t * 64)]
    kernel = c.WinDLL('kernel32', use_last_error=True)
    kernel.QueryInformationJobObject.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.c_void_p]
    value = Processes()
    with pool.child._job_lock:
        assert kernel.QueryInformationJobObject(pool.child.job, 3, c.byref(value), c.sizeof(value), None)
    return [psutil.Process(pid) for pid in value.pids[:value.count]
            if pid != pool.child.process.pid]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    target = '/tmp/kernel-owner-' + uuid.uuid4().hex
    rows = []
    with InitPool(args.root, args.dist, args.output, size=1, timeout=120) as pool:
        def run(name, script, marker):
            row = pool.run(['/bin/bash', '-e', '-c', script], expect=[marker])
            row['name'] = name
            rows.append(row)
            assert row['status'] == 'passed', row

        run('mount creator exits',
            f'mkdir -p {target}; mount -t tmpfs -o size=4m none {target}; '
            f'printf preserved > {target}/value; echo CREATED', 'CREATED')
        time.sleep(1.05)  # Cross an idle collection with no active application.
        run('new worker reads mount after idle',
            f'test "$(cat {target}/value)" = preserved; echo PRESERVED', 'PRESERVED')
        run('sysfs catalogue creator exits',
            f'mkdir {target}/sys-a {target}/sys-b; mount -t sysfs none {target}/sys-a; '
            f'stat -c %d {target}/sys-a > {target}/sys-device; echo SYS_CREATED', 'SYS_CREATED')
        run('new worker reuses the same sysfs instance',
            f'mount -t sysfs none {target}/sys-b; '
            f'test "$(stat -c %d {target}/sys-b)" = "$(cat {target}/sys-device)"; '
            f'umount {target}/sys-a; umount {target}/sys-b; '
            f'rmdir {target}/sys-a {target}/sys-b; echo CATALOG_OK', 'CATALOG_OK')
        run('fork and exec retain shared mount',
            f'/bin/bash -c \'test "$(cat {target}/value)" = preserved\'; '
            f'exec /bin/bash -c "echo FORK_EXEC_OK"', 'FORK_EXEC_OK')
        run('private namespace does not change parent mount',
            f'unshare -m /bin/bash -e -c "mount -t tmpfs none {target}; '
            f'test ! -e {target}/value; echo PRIVATE_OK"; '
            f'test "$(cat {target}/value)" = preserved; echo ISOLATED', 'ISOLATED')
        run('confined root rejects user namespace creation without changing mounts',
            f'if unshare -Ur -m /bin/true; then exit 1; fi; '
            f'test "$(cat {target}/value)" = preserved; echo CONFINEMENT_OK', 'CONFINEMENT_OK')
        signal = target + '-parent-exit'
        with pool.lock:
            parent_offset = len(pool.buffers[0])
        parent = pool.launch(['/usr/bin/unshare', '-m', '/bin/bash', '-e', '-c',
            f'mount -t tmpfs none {target}; echo inherited > {target}/value; '
            f'echo CONTEXT_READY; while test ! -e {signal}; do sleep .02; done'])
        deadline = time.monotonic() + 15
        while True:
            with pool.lock:
                ready = b'CONTEXT_READY' in pool.buffers[0][parent_offset:]
            if ready:
                break
            assert time.monotonic() < deadline, 'namespace parent did not become ready'
            time.sleep(.01)
        child = pool.controller.call('ReservePoolWorker')['PoolWorker']['pid']
        pool.controller.call({'ActivatePoolWorkerUnderParent': dict(pid=child, parent_pid=parent.pid,
            launch=dict(arguments=['/bin/bash', '-e', '-c',
                f'test "$(cat {target}/value)" = inherited; : > {signal}'],
                cwd='/', environment=None))})
        status = pool.controller.call({'AwaitExit': {'pid': child}})['Exit']['status']
        assert status == 0, 'fresh command did not inherit parent mount namespace'
        assert parent.wait() == 0
        rows.append(dict(name='fresh command inherits parent mount context', status='passed'))
        run('parent namespace exit preserves original context',
            f'rm {signal}; test "$(cat {target}/value)" = preserved; echo PARENT_CONTEXT_OK',
            'PARENT_CONTEXT_OK')
        run('shared propagation commits across namespaces',
            f'mount --make-shared {target}; mkdir {target}/child; '
            f'unshare -m --propagation unchanged /bin/bash -e -c "'
            f'mount -t tmpfs none {target}/child; echo propagated > {target}/child/value"; '
            f'test "$(cat {target}/child/value)" = propagated; echo PROPAGATED', 'PROPAGATED')
        with pool.lock:
            offset = len(pool.buffers[0])
        application = pool.launch(['/bin/bash', '-e', '-c',
            f'echo crash-safe > {target}/value; echo KILL_READY; while :; do :; done'])
        deadline = time.monotonic() + 15
        while True:
            with pool.lock:
                ready = b'KILL_READY' in pool.buffers[0][offset:]
            if ready:
                break
            assert time.monotonic() < deadline, 'worker did not reach crash point'
            time.sleep(.01)
        # This shell does not exec after the marker; its pinned original native
        # process is the target. Activated pool workers retain their host argv.
        kernel = c.WinDLL('kernel32', use_last_error=True)
        kernel.GetProcessId.argtypes = [w.HANDLE]
        kernel.GetProcessId.restype = w.DWORD
        native_pid = kernel.GetProcessId(application.sample.handle)
        victim = next(p for p in job_workers(pool) if p.pid == native_pid)
        victim.kill()
        application.wait()
        run('native worker death preserves mounted data',
            f'test "$(cat {target}/value)" = crash-safe; echo CRASH_SAFE', 'CRASH_SAFE')
        run('unmount removes visibility and allows cleanup',
            f'umount {target}/child; umount {target}; '
            f'test ! -e {target}/value; rmdir {target}; echo UNMOUNTED', 'UNMOUNTED')
        pool.shutdown()
        assert not job_workers(pool), 'a worker outlived init shutdown'
    result = dict(passed=True, checks=rows)
    (args.output / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
