"""Counters for one pinned native process; unrelated pool refill is excluded."""
import ctypes as c
from ctypes import wintypes as w
import threading


class ProcessSample:
    def __init__(self, pid, birth):
        self.lock = threading.Lock()
        self.kernel = c.WinDLL('kernel32', use_last_error=True)
        self.kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
        self.kernel.OpenProcess.restype = w.HANDLE
        self.kernel.CloseHandle.argtypes = [w.HANDLE]
        self.kernel.GetProcessTimes.argtypes = [w.HANDLE, *([c.POINTER(w.FILETIME)]*4)]
        self.kernel.QueryProcessCycleTime.argtypes = [w.HANDLE, c.POINTER(c.c_uint64)]
        self.kernel.K32GetProcessMemoryInfo.argtypes = [w.HANDLE, c.c_void_p, w.DWORD]
        self.handle = self.kernel.OpenProcess(0x0400 | 0x0010, False, pid)
        if not self.handle:
            raise c.WinError(c.get_last_error())
        try:
            initial = self.read()
            if initial['birth'] != birth:
                raise RuntimeError('pool worker native identity changed')
        except BaseException:
            self.close()
            raise

    def read(self):
        with self.lock:
            if not self.handle:
                return None
            created, ended, kernel, user = (w.FILETIME() for _ in range(4))
            if not self.kernel.GetProcessTimes(self.handle, c.byref(created), c.byref(ended), c.byref(kernel), c.byref(user)):
                raise c.WinError(c.get_last_error())
            ticks = lambda value: (value.dwHighDateTime << 32) | value.dwLowDateTime
            result = dict(birth=ticks(created), user_ms=ticks(user)/10000, kernel_ms=ticks(kernel)/10000,
                          total_cpu_ms=(ticks(user)+ticks(kernel))/10000)
            cycles = c.c_uint64()
            if self.kernel.QueryProcessCycleTime(self.handle, c.byref(cycles)):
                result['cycles'] = cycles.value
            class Memory(c.Structure):
                _fields_ = [('size', w.DWORD), ('page_faults', w.DWORD),
                    *[(name, c.c_size_t) for name in ['peak_working_set', 'working_set', 'peak_paged_pool',
                        'paged_pool', 'peak_nonpaged_pool', 'nonpaged_pool', 'pagefile', 'peak_pagefile']]]
            memory = Memory()
            memory.size = c.sizeof(memory)
            if self.kernel.K32GetProcessMemoryInfo(self.handle, c.byref(memory), c.sizeof(memory)):
                result.update(page_faults=memory.page_faults, working_set=memory.working_set,
                              peak_working_set=memory.peak_working_set, private_commit=memory.pagefile)
            return result

    def close(self):
        with self.lock:
            if self.handle:
                self.kernel.CloseHandle(self.handle)
                self.handle = None
