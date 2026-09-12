"""Own every Windows child of a desktop session, including reparented services."""
import ctypes as c
from ctypes import wintypes as w
import os
import subprocess
import threading

class SessionProcess:
    def __init__(self, command, suspended=False, **kwargs):
        self.job = None
        self._job_lock = threading.Lock()
        if os.name != 'nt':
            self.process = subprocess.Popen(command, **kwargs)
            return
        kernel = c.WinDLL('kernel32', use_last_error=True)
        kernel.CreateJobObjectW.argtypes = [c.c_void_p, w.LPCWSTR]
        kernel.CreateJobObjectW.restype = w.HANDLE
        kernel.SetInformationJobObject.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD]
        kernel.AssignProcessToJobObject.argtypes = [w.HANDLE, w.HANDLE]
        kernel.CloseHandle.argtypes = [w.HANDLE]
        class Basic(c.Structure):
            _fields_ = [('process_time', c.c_int64), ('job_time', c.c_int64), ('flags', w.DWORD),
                        ('min_working_set', c.c_size_t), ('max_working_set', c.c_size_t),
                        ('active_limit', w.DWORD), ('affinity', c.c_size_t),
                        ('priority', w.DWORD), ('scheduling', w.DWORD)]
        class Extended(c.Structure):
            _fields_ = [('basic', Basic), ('io', c.c_uint64 * 6),
                        ('process_memory', c.c_size_t), ('job_memory', c.c_size_t),
                        ('peak_process_memory', c.c_size_t), ('peak_job_memory', c.c_size_t)]
        self.kernel = kernel
        self.job = kernel.CreateJobObjectW(None, None)
        if not self.job: raise c.WinError(c.get_last_error())
        limits = Extended(); limits.basic.flags = 0x2000  # KILL_ON_JOB_CLOSE
        self.process = None
        try:
            if not kernel.SetInformationJobObject(self.job, 9, c.byref(limits), c.sizeof(limits)):
                raise c.WinError(c.get_last_error())
            # CREATE_NO_WINDOW still starts a hidden conhost for console images.
            # Pipes/files need no console. Keep attachment when any actual
            # standard stream is a console so interactive IO remains usable.
            import msvcrt
            kernel.GetStdHandle.argtypes = [w.DWORD]
            kernel.GetStdHandle.restype = w.HANDLE
            kernel.GetConsoleMode.argtypes = [w.HANDLE, c.POINTER(w.DWORD)]
            kernel.GetConsoleMode.restype = w.BOOL
            def console_stream(name, std_id):
                value = kwargs.get(name)
                if value in (subprocess.PIPE, subprocess.DEVNULL, subprocess.STDOUT):
                    return False
                if value is None:
                    handle = kernel.GetStdHandle(std_id & 0xffffffff)
                else:
                    descriptor = value if isinstance(value, int) else value.fileno()
                    handle = msvcrt.get_osfhandle(descriptor)
                mode = w.DWORD()
                return bool(kernel.GetConsoleMode(handle, c.byref(mode)))
            console = any(console_stream(name, std_id) for name, std_id in
                          [('stdin', -10), ('stdout', -11), ('stderr', -12)])
            background = subprocess.CREATE_NO_WINDOW if console else subprocess.DETACHED_PROCESS
            self.process = subprocess.Popen(command, creationflags=background | 4, **kwargs)
            if not kernel.AssignProcessToJobObject(self.job, int(self.process._handle)):
                raise c.WinError(c.get_last_error())
            if not suspended: self.resume()
        except BaseException:
            if self.process is not None:
                self.process.kill(); self.process.wait(timeout=10)
            self.close()
            raise

    def resume(self):
        if os.name != 'nt': return
        nt = c.WinDLL('ntdll')
        nt.NtResumeProcess.argtypes = [w.HANDLE]
        nt.NtResumeProcess.restype = c.c_long
        if nt.NtResumeProcess(int(self.process._handle)) < 0:
            raise RuntimeError('Could not resume desktop session')

    def cpu_metrics(self):
        """Aggregate CPU/IO accounting for this owned process tree, including exited children.

        CPU sums across threads and can exceed wall time. IO bytes include cached
        operations; neither wall minus CPU nor IO counts establish disk wait time.
        """
        if not self.job:
            return None
        class Basic(c.Structure):
            _fields_ = [('user', c.c_int64), ('kernel', c.c_int64),
                        ('period_user', c.c_int64), ('period_kernel', c.c_int64),
                        ('page_faults', w.DWORD), ('processes', w.DWORD),
                        ('active', w.DWORD), ('terminated', w.DWORD)]
        class Io(c.Structure):
            _fields_ = [(name, c.c_uint64) for name in
                        ['read_ops', 'write_ops', 'other_ops', 'read_bytes', 'write_bytes', 'other_bytes']]
        class Accounting(c.Structure):
            _fields_ = [('basic', Basic), ('io', Io)]
        value = Accounting()
        self.kernel.QueryInformationJobObject.argtypes = [w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.c_void_p]
        self.kernel.QueryInformationJobObject.restype = w.BOOL
        with self._job_lock:
            if not self.job:
                return None
            if not self.kernel.QueryInformationJobObject(self.job, 8, c.byref(value), c.sizeof(value), None):
                raise c.WinError(c.get_last_error())
        return dict(user_ms=value.basic.user / 10000, kernel_ms=value.basic.kernel / 10000,
                    total_cpu_ms=(value.basic.user + value.basic.kernel) / 10000,
                    processes=value.basic.processes, page_faults=value.basic.page_faults,
                    read_bytes=value.io.read_bytes, write_bytes=value.io.write_bytes)

    def close(self):
        # A timeout watchdog and the caller's finally block can arrive together.
        with self._job_lock:
            if self.job:
                self.kernel.CloseHandle(self.job)
                self.job = None
