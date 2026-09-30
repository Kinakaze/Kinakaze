"""Snapshot an owned Windows process's thread registers and stack candidates.

Candidates are executable-looking words, not a reconstructed call stack.
Threads are resumed before symbol lookup or writing output.
"""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import struct

import psutil


def sample(pid):
    if pid == os.getpid():
        raise ValueError('Cannot suspend this sampler')
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *([ctypes.POINTER(wintypes.FILETIME)] * 4)]
    kernel.OpenThread.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.OpenThread.restype = wintypes.HANDLE
    for name in ('SuspendThread', 'ResumeThread'):
        getattr(kernel, name).argtypes = [wintypes.HANDLE]
        getattr(kernel, name).restype = wintypes.DWORD
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetThreadContext.argtypes = [wintypes.HANDLE, ctypes.c_void_p]
    kernel.GetProcessIdOfThread.argtypes = [wintypes.HANDLE]
    kernel.GetProcessIdOfThread.restype = wintypes.DWORD
    kernel.ReadProcessMemory.argtypes = [wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p,
                                       ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
    kernel.VirtualQueryEx.argtypes = [wintypes.HANDLE, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t]
    kernel.VirtualQueryEx.restype = ctypes.c_size_t
    kernel.K32GetMappedFileNameW.argtypes = [wintypes.HANDLE, ctypes.c_void_p, wintypes.LPWSTR, wintypes.DWORD]
    ntdll = ctypes.WinDLL('ntdll')
    create_stub = ctypes.cast(ntdll.NtCreateFile, ctypes.c_void_p).value
    process = psutil.Process(pid)
    birth = process.create_time()
    handle = kernel.OpenProcess(0x410, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    report = dict(pid=pid, birth=birth, command=process.cmdline(), threads=[], addresses={})
    try:
        times = [wintypes.FILETIME() for _ in range(4)]
        if not kernel.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
            raise ctypes.WinError(ctypes.get_last_error())
        ticks = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
        if abs((ticks - 116444736000000000) / 10000000 - birth) > .00001:
            raise RuntimeError('Process identity changed')
        for thread in process.threads():
            native = kernel.OpenThread(0x4a, False, thread.id)
            if not native:
                report['threads'].append(dict(tid=thread.id, error=ctypes.get_last_error()))
                continue
            if kernel.GetProcessIdOfThread(native) != pid:
                kernel.CloseHandle(native)
                report['threads'].append(dict(tid=thread.id, error='thread identity changed'))
                continue
            suspended = False
            row = dict(tid=thread.id, user_time=thread.user_time, system_time=thread.system_time)
            context_storage = ctypes.create_string_buffer(1248)
            context = (ctypes.addressof(context_storage) + 15) & ~15
            ctypes.c_uint32.from_address(context + 48).value = 0x10000b
            stack = ctypes.create_string_buffer(8192)
            copied = ctypes.c_size_t()
            try:
                suspended = kernel.SuspendThread(native) != 0xffffffff
                if not suspended or not kernel.GetThreadContext(native, context):
                    row['error'] = ctypes.get_last_error()
                else:
                    row['rip'] = ctypes.c_uint64.from_address(context + 248).value
                    row['rsp'] = ctypes.c_uint64.from_address(context + 152).value
                    row['registers'] = dict(zip(
                        ('rax', 'rcx', 'rdx', 'rbx', 'rsp', 'rbp', 'rsi', 'rdi',
                         'r8', 'r9', 'r10', 'r11', 'r12', 'r13', 'r14', 'r15', 'rip'),
                        struct.unpack('<17Q', ctypes.string_at(context + 120, 136))))
                    kernel.ReadProcessMemory(handle, row['rsp'], stack, len(stack), ctypes.byref(copied))
                    # Decode the actual object name at NtCreateFile, rather
                    # than infer filesystem activity from stale stack words.
                    # Require the same mapped image base before comparing the
                    # local export address; read only bounded remote buffers.
                    if 0 <= row['rip'] - create_stub < 32:
                        memory = ctypes.create_string_buffer(48)
                        if (kernel.VirtualQueryEx(handle, row['rip'], memory, len(memory))
                                and struct.unpack_from('<Q', memory.raw, 8)[0] == ntdll._handle):
                            def read_remote(address, length):
                                buffer = ctypes.create_string_buffer(length)
                                count = ctypes.c_size_t()
                                if (kernel.ReadProcessMemory(handle, address, buffer, length, ctypes.byref(count))
                                        and count.value == length):
                                    return buffer.raw
                            attrs = read_remote(row['registers']['r8'], 48)
                            if attrs and struct.unpack_from('<I', attrs)[0] == 48:
                                name = read_remote(struct.unpack_from('<Q', attrs, 16)[0], 16)
                                if name:
                                    length = struct.unpack_from('<H', name)[0]
                                    if length <= 8192 and length % 2 == 0:
                                        raw_name = (read_remote(struct.unpack_from('<Q', name, 8)[0], length)
                                                    if length else b'')
                                        if raw_name is not None:
                                            row['native_create'] = dict(
                                                name=raw_name.decode('utf-16-le', errors='replace'),
                                                root=hex(struct.unpack_from('<Q', attrs, 8)[0]),
                                                access=hex(row['registers']['rdx']))
            finally:
                try:
                    if suspended and kernel.ResumeThread(native) == 0xffffffff:
                        raise ctypes.WinError(ctypes.get_last_error())
                finally:
                    kernel.CloseHandle(native)
            if 'rip' in row:
                row['stack_hex'] = stack.raw[:copied.value].hex()
                words = [(-1, row['rip']), *enumerate(struct.unpack(
                    '<' + 'Q' * (copied.value // 8), stack.raw[:copied.value // 8 * 8]))]
                row['candidates'] = []
                for index, address in words:
                    if not 65536 <= address < 0x800000000000:
                        continue
                    memory = ctypes.create_string_buffer(48)
                    if not kernel.VirtualQueryEx(handle, address, memory, len(memory)):
                        continue
                    base, allocation = struct.unpack_from('<QQ', memory.raw)
                    state, protection = struct.unpack_from('<II', memory.raw, 32)
                    if state != 0x1000 or not protection & 0xf0:
                        continue
                    row['candidates'].append(dict(slot=index, address=hex(address)))
                    if hex(address) not in report['addresses']:
                        filename = ctypes.create_unicode_buffer(32768)
                        kernel.K32GetMappedFileNameW(handle, address, filename, len(filename))
                        report['addresses'][hex(address)] = dict(
                            allocation=hex(allocation), region=hex(base),
                            offset=hex(address - allocation), path=filename.value)
            report['threads'].append(row)
    finally:
        kernel.CloseHandle(handle)
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = sample(args.pid)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(dict(pid=args.pid, threads=len(result['threads']), addresses=len(result['addresses']))))
