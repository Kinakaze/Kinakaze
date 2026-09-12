"""Real generic-pool lifecycle, mixed applications, reservation and refill checks."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import time
import uuid

import psutil
from init_controller import Controller
from init_pool import InitPool


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--java', default='/usr/lib/jvm/jdk-25.0.4.1+1-jre/bin/java')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    rows = []
    kernel = c.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
    kernel.OpenProcess.restype = w.HANDLE
    kernel.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    kernel.TerminateProcess.argtypes = [w.HANDLE, w.UINT]
    kernel.CloseHandle.argtypes = [w.HANDLE]
    kernel.GetProcessTimes.argtypes = [w.HANDLE, *([c.POINTER(w.FILETIME)]*4)]
    kernel.QueryInformationJobObject.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.c_void_p]
    class Accounting(c.Structure):
        _fields_ = [('user', c.c_int64), ('kernel', c.c_int64), ('period_user', c.c_int64),
                    ('period_kernel', c.c_int64), ('page_faults', w.DWORD), ('processes', w.DWORD),
                    ('active', w.DWORD), ('terminated', w.DWORD)]
    def assert_job_empty(child):
        # Includes a replacement created concurrently with shutdown, even when
        # it did not exist in the earlier parent.children() snapshot.
        deadline = time.monotonic() + 5
        while True:
            value = Accounting()
            with child._job_lock:
                assert child.job, 'outer test Job must still be open'
                assert kernel.QueryInformationJobObject(child.job, 1, c.byref(value), c.sizeof(value), None)
            if value.active == 0:
                return
            assert time.monotonic() < deadline, 'a worker escaped init shutdown'
            time.sleep(0.001)
    def pin(process):
        birth = process.create_time()
        handle = kernel.OpenProcess(0x00100000 | 0x1000 | 1, False, process.pid)
        assert handle, 'failed to pin owned worker'
        created, exited, system, user = (w.FILETIME() for _ in range(4))
        assert kernel.GetProcessTimes(handle, c.byref(created), c.byref(exited), c.byref(system), c.byref(user))
        native_birth = (((created.dwHighDateTime << 32) | created.dwLowDateTime)-116444736000000000)/10000000
        if abs(native_birth-birth) > 0.00001:
            kernel.CloseHandle(handle)
            raise AssertionError('worker identity changed before pinning')
        return handle
    marker = args.root.resolve()/'tmp'/('pool-side-effect-'+uuid.uuid4().hex)
    handles = []
    try:
        with InitPool(args.root, args.dist, args.output, size=2, timeout=90) as pool:
            parent = psutil.Process(pool.child.process.pid)
            children = parent.children()
            assert len(children) == 2
            assert all(child.nice() == parent.nice() for child in children), 'preparation priority leaked into ready workers'
            before = pool.child.cpu_metrics()
            time.sleep(0.2)
            idle_cpu = pool.child.cpu_metrics()['total_cpu_ms']-before['total_cpu_ms']
            assert not marker.exists()
            first = pool.controller.call('ReservePoolWorker')['PoolWorker']['pid']
            alternate = Controller(pool.endpoint, pool.token, pool.child, time.monotonic()+10)
            second = alternate.call('ReservePoolWorker')['PoolWorker']['pid']
            assert first != second
            alternate.call({'ReleasePoolWorker': {'pid': second}})
            pool.controller.pipe.close()
            pool.controller = alternate
            alternate.call({'AwaitPoolReady': {'minimum': 2}})
            assert not marker.exists(), 'reservation/EOF executed application code'
            rows.append(dict(case='exclusive-reservation-release-eof', status='passed', idle_cpu_ms=idle_cpu))
            cases = [dict(command=['/bin/sh','-c',
                f'printf activated > /tmp/{marker.name}; test "$PWD" = /tmp && test "$POOL_VALUE" = one && echo POOL_ENV_ONE'],
                cwd='/tmp', environment=['PATH=/bin:/usr/bin','POOL_VALUE=one'], expect=['POOL_ENV_ONE']),
                dict(command=[args.java,'-cp','/tmp/startup4-java','JavaRuntimeProbe','spawn'],
                    expect=['JAVA_MAIN_ENTERED','JAVA_JIT_THREADS_FILES_OK','JAVA_SPAWN_OK'], ready_marker='JAVA_MAIN_ENTERED'),
                dict(command=['/bin/sh','-c','test "${POOL_VALUE-unset}" = unset && printf POOL_ENV_CLEAN; printf POOL_STDERR >&2; exit 7'],
                    expect=['POOL_ENV_CLEAN','POOL_STDERR'], expected_exit=7)]
            for index, case in enumerate(cases):
                row = dict(case=f'mixed-application-{index}', **pool.run(**case))
                rows.append(row)
                assert row['status']=='passed', row
            assert marker.read_text(encoding='utf-8') == 'activated'
            # An application may stay alive while another independent one starts.
            pool.controller.call({'AwaitPoolReady': {'minimum': 2}})
            release = marker.with_name(marker.name+'-release')
            running = pool.launch(['/bin/sh','-c',f'while test ! -f /tmp/{release.name}; do :; done; exit 9'])
            other = pool.launch(['/bin/sh','-c','exit 3'])
            assert other.wait() == 3
            release.write_text('release',encoding='utf-8')
            try:
                assert running.wait() == 9 and running.wait() == 9
            finally:
                release.unlink(missing_ok=True)
            rows.append(dict(case='independent-applications-can-overlap',status='passed'))
            # More launches than pool capacity force real replenishment and queueing.
            for index in range(6):
                token = f'POOL_REFILL_{index}'
                row = dict(case=f'refill-{index}', **pool.run(['/bin/sh','-c',f'printf {token}'], expect=[token]))
                rows.append(row)
                assert row['status']=='passed', row
            pids = [row['pid'] for row in rows if 'pid' in row]
            assert len(set(pids)) == len(pids), 'an executed worker was recycled'
            pool.controller.call({'AwaitPoolReady': {'minimum': 2}})
            children = parent.children()
            assert len(children) == 2, 'pool exceeded its standby bound'
            crashed = pin(children[0])
            handles.append(crashed)
            assert kernel.TerminateProcess(crashed, 91)
            assert kernel.WaitForSingleObject(crashed, 5000) == 0
            # Allow the native exit watcher to revoke readiness, then require refill.
            time.sleep(0.1)
            pool.controller.call({'AwaitPoolReady': {'minimum': 2}})
            children = parent.children()
            assert len(children) == 2 and children[0].pid != 0
            handles.extend(pin(process) for process in children)
            rows.append(dict(case='idle-worker-crash-refilled', status='passed'))
            pool.shutdown()
            assert_job_empty(pool.child)
            for handle in handles:
                assert kernel.WaitForSingleObject(handle, 5000) == 0, 'init left an owned worker alive'
            rows.append(dict(case='shutdown-cleans-idle-workers', status='passed'))
        for delay in [0.22, 0.25, 0.28]:
            with InitPool(args.root, args.dist, args.output/f'shutdown-refill-{delay}', size=1, timeout=15) as pool:
                started = time.monotonic()
                assert pool.launch(['/bin/sh', '-c', 'exit 0']).wait() == 0
                time.sleep(max(0, started + delay - time.monotonic()))
                pool.shutdown()
                assert_job_empty(pool.child)
            rows.append(dict(case=f'shutdown-during-refill-{delay}', status='passed'))
    except Exception as error:
        rows.append(dict(case='pool-lifecycle', status='failed', error=str(error)))
    finally:
        for handle in handles:
            kernel.CloseHandle(handle)
        marker.unlink(missing_ok=True)
        (args.output/'results.json').write_text(json.dumps(rows, indent=2),encoding='utf-8')
    for row in rows:
        print(json.dumps(row),flush=True)
    return int(any(row['status']!='passed' for row in rows))


if __name__ == '__main__':
    raise SystemExit(main())
