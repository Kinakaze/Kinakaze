"""One persistent init and a replenished pool of unused native workers.

Applications are chosen after readiness; each gets a fresh worker and EOF stdin.
The owner supplies the session root/distribution. Output is captured continuously.
"""
import hashlib
import os
from pathlib import Path
import subprocess
import threading
import time
import uuid

from init_controller import Controller
from session_process import SessionProcess
from process_sample import ProcessSample


def distribution_hashes(dist):
    paths = [dist / 'worker.exe', dist / 'init.exe', *sorted((dist / 'rootfs/lib').glob('*'))]
    result = {}
    for path in paths:
        if path.is_file():
            with path.open('rb') as stream:
                result[str(path.relative_to(dist))] = hashlib.file_digest(stream, 'sha256').hexdigest()
    return result


class InitPool:
    def __init__(self, root, dist, output, size=2, timeout=None, profile=False):
        if timeout is not None and timeout <= 0:
            raise ValueError('session timeout must be positive or None')
        self.root, self.dist, self.output = Path(root).resolve(), Path(dist).resolve(), Path(output).resolve()
        self.output.mkdir(parents=True, exist_ok=True)
        self.endpoint = r'\\.\pipe\kinakaze-pool-' + uuid.uuid4().hex
        self.token = uuid.uuid4().hex + uuid.uuid4().hex
        env = os.environ.copy()
        env['KINAKAZE_V2_TOKEN'] = self.token
        for name in ['KINAKAZE_STARTUP_PROFILE', 'KINAKAZE_LOADER_PROFILE']:
            env.pop(name, None)
            if profile:
                directory = self.output / 'profile'
                directory.mkdir(exist_ok=True)
                env[name] = str(directory)
        self.buffers = [bytearray(), bytearray()]
        self.lock = threading.Lock()
        self.control_lock = threading.Lock()
        self.watches = []
        self.controller = None
        self.timed_out = threading.Event()
        self.drainers = []
        self.samples = []
        self.sample_lock = threading.Lock()
        self.started = time.perf_counter_ns()
        self.child = SessionProcess([str(self.dist / 'init.exe'), '--pipe', self.endpoint,
            '--controller-pid', str(os.getpid()), '--prewarm-root', str(self.root),
            '--prewarm-dist', str(self.dist), '--prewarm-pool', str(size)], env=env,
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.watchdog = None
        if timeout is not None:
            self.watchdog = threading.Timer(timeout, self._timeout)
            self.watchdog.daemon = True
            self.watchdog.start()
        try:
            for index, (stream, name) in enumerate([(self.child.process.stdout, 'stdout'), (self.child.process.stderr, 'stderr')]):
                thread = threading.Thread(target=self._drain, args=(index, stream, name), daemon=True)
                thread.start()
                self.drainers.append(thread)
            self.controller = Controller(self.endpoint, self.token, self.child, time.monotonic() + (timeout or 30))
            self.controller.call({'AwaitPoolReady': {'minimum': size}})
            self.preparation_ms = (time.perf_counter_ns()-self.started)/1e6
        except BaseException:
            self.close()
            raise

    def _timeout(self):
        self.timed_out.set()
        self.child.close()

    def _drain(self, index, stream, name):
        with (self.output / f'pool.{name}.log').open('wb') as log:
            while data := stream.read1(65536):
                observed = time.perf_counter_ns()
                log.write(data)
                with self.lock:
                    self.buffers[index].extend(data)
                    for watch in self.watches:
                        if watch['timestamp'] is None:
                            start = watch['offsets'][index]
                            if watch['marker'] in self.buffers[index][start:]:
                                watch['timestamp'] = observed
                            else:
                                # Retain just enough overlap to match a marker
                                # split across reads, without rescanning old output.
                                watch['offsets'][index] = max(start, len(self.buffers[index])-len(watch['marker'])+1)
                    if len(self.buffers[index]) > 64*1024*1024:
                        # Long-lived applications may produce unlimited output.
                        # Keep complete disk logs and bounded in-memory history.
                        discarded = len(self.buffers[index])-32*1024*1024
                        del self.buffers[index][:discarded]
                        for watch in self.watches:
                            watch['offsets'][index] = max(0, watch['offsets'][index]-discarded)

    def launch(self, command, *, cwd='/', environment=None):
        """Start any application and return immediately after init releases it.

        Returned applications wait on independent control connections, so a
        long-running application does not prevent launching another one.
        """
        with self.control_lock:
            started = time.perf_counter_ns()
            worker = self.controller.call('ReservePoolWorker')['PoolWorker']
            pid = worker['pid']
            sample = None
            baseline = None
            activated = time.perf_counter_ns()
            try:
                if 'host_pid' in worker:
                    sample = ProcessSample(worker['host_pid'], worker['birth'])
                    with self.sample_lock:
                        self.samples.append(sample)
                    baseline = sample.read()
                activated = time.perf_counter_ns()
                self.controller.call({'ActivatePoolWorker': {'pid': pid, 'launch': dict(
                    arguments=command, cwd=cwd, environment=environment)}})
            except BaseException:
                if sample:
                    sample.close()
                    self._forget_sample(sample)
                try:
                    self.controller.call({'ReleasePoolWorker': {'pid': pid}})
                except Exception:
                    pass
                raise
            return PooledApplication(self, pid, started, activated, sample, baseline)

    def run(self, command, *, cwd='/', environment=None, expect=(), expected_exit=0, ready_marker=None):
        with self.lock:
            offsets = [len(buffer) for buffer in self.buffers]
            watch = dict(marker=ready_marker.encode() if ready_marker else b'', offsets=offsets.copy(), timestamp=None)
            expected = [(marker, dict(marker=marker.encode(), offsets=offsets.copy(), timestamp=None)) for marker in expect]
            active_watches = [value for _, value in expected]
            if ready_marker:
                active_watches.append(watch)
            self.watches.extend(active_watches)
        before = self.child.cpu_metrics()
        started = time.perf_counter_ns()
        try:
            application = self.launch(command, cwd=cwd, environment=environment)
            reserved, activated = application.pid, application.activated
            status = application.wait()
        except BaseException:
            with self.lock:
                for value in active_watches:
                    self.watches.remove(value)
            raise
        ended = time.perf_counter_ns()
        after = self.child.cpu_metrics()
        # Output drains are asynchronous. Marker checks may wait, but the native
        # exit timer above does not. Ready timestamps come from the drain itself.
        deadline = time.monotonic() + 1
        while True:
            with self.lock:
                missing = [marker for marker, value in expected if value['timestamp'] is None]
                observed = watch['timestamp']
            if (not missing and (not ready_marker or observed is not None)) or time.monotonic() >= deadline:
                break
            time.sleep(0.001)
        with self.lock:
            for value in active_watches:
                self.watches.remove(value)
        return dict(pid=reserved, command=command, exit_code=status, expected_exit=expected_exit,
            status='passed' if status == expected_exit and not missing and (not ready_marker or observed is not None) else 'failed',
            missing_markers=missing, reservation_ms=(activated-started)/1e6,
            request_to_exit_ms=(ended-started)/1e6, activation_to_exit_ms=(ended-activated)/1e6,
            request_to_ready_ms=(observed-started)/1e6 if observed is not None else None,
            application_cpu_metrics=application.cpu_metrics,
            prepared_working_set_bytes=application.baseline.get('working_set') if application.baseline else None,
            cpu_metrics={key: after[key]-before[key] for key in after} if before and after else None)

    def shutdown(self):
        with self.control_lock:
            self.controller.call('Shutdown')
        self.child.process.wait(timeout=10)
        for thread in self.drainers:
            thread.join(timeout=5)
        if self.child.process.returncode:
            raise RuntimeError(f'init exited with {self.child.process.returncode}')

    def close(self):
        if self.watchdog:
            self.watchdog.cancel()
        self.child.close()
        if self.controller:
            self.controller.pipe.close()
        self.child.process.wait(timeout=10)
        for thread in self.drainers:
            thread.join(timeout=5)
        with self.sample_lock:
            samples, self.samples = self.samples, []
        for sample in samples:
            sample.close()

    def _forget_sample(self, sample):
        with self.sample_lock:
            if sample in self.samples:
                self.samples.remove(sample)

    def __enter__(self):
        return self

    def __exit__(self, *exception):
        self.close()


class PooledApplication:
    def __init__(self, pool, pid, started, activated, sample, baseline):
        self.pool, self.pid, self.started, self.activated = pool, pid, started, activated
        self.wait_lock = threading.Lock()
        self.exit_code = None
        self.sample, self.baseline = sample, baseline
        self.cpu_metrics = None

    def wait(self):
        with self.wait_lock:
            if self.exit_code is None:
                controller = Controller(self.pool.endpoint, self.pool.token, self.pool.child, time.monotonic()+10)
                try:
                    self.exit_code = controller.call({'AwaitExit': {'pid': self.pid}})['Exit']['status']
                    if self.sample:
                        final = self.sample.read()
                        if final:
                            self.cpu_metrics = {key: final[key]-self.baseline[key] for key in
                                ['user_ms','kernel_ms','total_cpu_ms','cycles','page_faults'] if key in final and key in self.baseline}
                finally:
                    controller.pipe.close()
                    if self.sample:
                        self.sample.close()
                        self.pool._forget_sample(self.sample)
            return self.exit_code
