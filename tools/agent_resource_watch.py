"""Bounded sampling of a test's owned Windows processes and their cleanup."""
import ctypes as c
from ctypes import wintypes as w
import threading
import time

from process_sample import ProcessSample


class AgentResourceWatch:
    def __init__(self, pool, max_private_commit=8 * 1024**3, min_available_physical=8 * 1024**3,
                 min_available_commit=8 * 1024**3):
        self.owner = pool.child
        self.processes = {}
        self.errors = []
        self.samples = 0
        self.peak_working_set = 0
        self.peak_private_commit = 0
        self.max_private_commit = max_private_commit
        self.min_available_physical = min_available_physical
        self.minimum_observed_available = None
        self.min_available_commit = min_available_commit
        self.minimum_observed_commit_available = None
        self.peak_job_commit = 0
        self.limit_reason = None
        self.stop = threading.Event()
        self.kernel = c.WinDLL('kernel32', use_last_error=True)
        self.kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
        self.kernel.OpenProcess.restype = w.HANDLE
        self.kernel.CloseHandle.argtypes = [w.HANDLE]
        self.kernel.GetProcessTimes.argtypes = [w.HANDLE, *([c.POINTER(w.FILETIME)] * 4)]
        self.kernel.QueryInformationJobObject.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.c_void_p]
        self.kernel.GetExitCodeProcess.argtypes = [w.HANDLE, c.POINTER(w.DWORD)]
        self.kernel.IsProcessInJob.argtypes = [w.HANDLE, w.HANDLE, c.POINTER(w.BOOL)]
        self.kernel.GlobalMemoryStatusEx.argtypes = [c.c_void_p]
        self.thread = threading.Thread(target=self._sample_loop, daemon=True)
        self.thread.start()

    def _sample_loop(self):
        # Fixed-size counters and one process handle per observed owned PID.
        # Do not retain an unbounded series or duplicate the job handle: the
        # latter would delay KILL_ON_JOB_CLOSE and keep children alive.
        while not self.stop.is_set():
            try:
                buffer = c.create_string_buffer(8192)
                with self.owner._job_lock:
                    job = self.owner.job
                    if not job:
                        break
                    if not self.kernel.QueryInformationJobObject(job, 3, buffer, len(buffer), None):
                        raise c.WinError(c.get_last_error())
                    count = c.c_uint32.from_buffer(buffer, 4).value
                    if count > (len(buffer) - 8) // c.sizeof(c.c_size_t):
                        raise RuntimeError('owned process list exceeds bounded sampler capacity')
                    pids = list((c.c_size_t * count).from_buffer(buffer, 8))
                for pid in pids:
                    if pid in self.processes:
                        continue
                    if len(self.processes) >= 512:
                        raise RuntimeError('owned process handle limit reached')
                    handle = self.kernel.OpenProcess(0x0400 | 0x0010, False, pid)
                    if not handle:
                        continue
                    created, ended, kernel, user = (w.FILETIME() for _ in range(4))
                    try:
                        if not self.kernel.GetProcessTimes(handle, c.byref(created), c.byref(ended), c.byref(kernel), c.byref(user)):
                            continue
                        birth = (created.dwHighDateTime << 32) | created.dwLowDateTime
                        sample = ProcessSample(pid, birth)
                        belongs = w.BOOL()
                        with self.owner._job_lock:
                            job = self.owner.job
                            owned = bool(job and self.kernel.IsProcessInJob(sample.handle, job, c.byref(belongs)) and belongs.value)
                        if owned:
                            self.processes[pid] = sample
                        else:
                            sample.close()
                    finally:
                        self.kernel.CloseHandle(handle)
                working_set, private_commit = 0, 0
                for sample in self.processes.values():
                    code = w.DWORD()
                    if self.kernel.GetExitCodeProcess(sample.handle, c.byref(code)) and code.value == 259:
                        row = sample.read()
                        working_set += row.get('working_set', 0)
                        private_commit += row.get('private_commit', 0)
                self.peak_working_set = max(self.peak_working_set, working_set)
                self.peak_private_commit = max(self.peak_private_commit, private_commit)
                self.samples += 1
                job_memory = self.owner.memory_metrics()
                if job_memory:
                    self.peak_job_commit = max(self.peak_job_commit, job_memory['peak_job_commit_bytes'])
                class MemoryStatus(c.Structure):
                    _fields_ = [('length', w.DWORD), ('load', w.DWORD),
                                *[(name, c.c_uint64) for name in ('total_physical', 'available_physical',
                                   'total_pagefile', 'available_pagefile', 'total_virtual',
                                   'available_virtual', 'available_extended')]]
                memory = MemoryStatus()
                memory.length = c.sizeof(memory)
                if not self.kernel.GlobalMemoryStatusEx(c.byref(memory)):
                    raise c.WinError(c.get_last_error())
                self.minimum_observed_available = min(memory.available_physical,
                    self.minimum_observed_available if self.minimum_observed_available is not None else memory.available_physical)
                self.minimum_observed_commit_available = min(memory.available_pagefile,
                    self.minimum_observed_commit_available if self.minimum_observed_commit_available is not None else memory.available_pagefile)
                if private_commit > self.max_private_commit:
                    self.limit_reason = 'owned private commit exceeded its limit'
                elif memory.available_physical < self.min_available_physical:
                    self.limit_reason = 'system available physical memory fell below its test threshold'
                elif memory.available_pagefile < self.min_available_commit:
                    self.limit_reason = 'system available commit fell below its test threshold'
                if self.limit_reason:
                    self.owner.close()
                    break
            except Exception as error:
                self.errors.append(str(error))
                self.owner.close()
                break
            self.stop.wait(0.5)

    def finish(self):
        """Call after the pool closes; process handles pin identity, not lifetime."""
        self.stop.set()
        self.thread.join(timeout=5)
        alive = []
        try:
            deadline = time.monotonic() + 5
            while True:
                alive = []
                for pid, sample in self.processes.items():
                    code = w.DWORD()
                    if self.kernel.GetExitCodeProcess(sample.handle, c.byref(code)) and code.value == 259:
                        alive.append(pid)
                if not alive or time.monotonic() >= deadline:
                    break
                time.sleep(0.05)
            return dict(observed_owned_processes=len(self.processes), samples=self.samples,
                        sampled_peak_working_set_bytes=self.peak_working_set,
                        sampled_peak_private_commit_bytes=self.peak_private_commit,
                        sampled_peak_job_commit_bytes=self.peak_job_commit,
                        job_peak_metric_note='Windows Job peak can include rejected commitment requests; '
                                             'it is not the sampled live private commit above.',
                        private_commit_limit_bytes=self.max_private_commit,
                        minimum_available_physical_bytes=self.min_available_physical,
                        sampled_minimum_available_physical_bytes=self.minimum_observed_available,
                        minimum_available_commit_bytes=self.min_available_commit,
                        sampled_minimum_available_commit_bytes=self.minimum_observed_commit_available,
                        limit_exceeded=self.limit_reason,
                        alive_after_close=alive, errors=self.errors,
                        cleanup_passed=not alive and not self.errors and self.samples > 0,
                        scope='Sampled aggregate owned processes; shared resident pages may be counted in multiple processes. This does not prove absence of long-term memory leaks.')
        finally:
            for sample in self.processes.values():
                sample.close()
            self.processes.clear()
