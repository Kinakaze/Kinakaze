"""Verify GTK, GNOME and Qt windows receive native close events and exit cleanly."""
from __future__ import annotations
import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
from pathlib import Path
import subprocess
import time


class Desktop:
    def __init__(self, worker):
        self.worker = worker
        self.user = c.WinDLL('user32', use_last_error=True)
        self.kernel = c.WinDLL('kernel32', use_last_error=True)
        self.callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
        self.user.EnumWindows.argtypes = [self.callback, w.LPARAM]
        self.user.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
        self.user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
        self.user.IsWindowVisible.argtypes = [w.HWND]
        self.user.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
        self.kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
        self.kernel.OpenProcess.restype = w.HANDLE
        self.kernel.QueryFullProcessImageNameW.argtypes = [w.HANDLE, w.DWORD, w.LPWSTR, c.POINTER(w.DWORD)]
        self.kernel.CloseHandle.argtypes = [w.HANDLE]
        self.nt = c.WinDLL('ntdll')
        self.nt.NtResumeProcess.argtypes = [w.HANDLE]
        self.nt.NtResumeProcess.restype = w.LONG
        self.kernel.CreateJobObjectW.argtypes = [c.c_void_p, w.LPCWSTR]
        self.kernel.CreateJobObjectW.restype = w.HANDLE
        self.kernel.AssignProcessToJobObject.argtypes = [w.HANDLE, w.HANDLE]
        self.kernel.IsProcessInJob.argtypes = [w.HANDLE, w.HANDLE, c.POINTER(w.BOOL)]

    def windows(self):
        result = []
        @self.callback
        def visit(hwnd, _):
            title = c.create_unicode_buffer(512)
            self.user.GetWindowTextW(hwnd, title, len(title))
            if self.user.IsWindowVisible(hwnd) and title.value:
                pid = w.DWORD()
                self.user.GetWindowThreadProcessId(hwnd, c.byref(pid))
                result.append((hwnd, pid.value, title.value))
            return True
        if not self.user.EnumWindows(visit, 0):
            raise c.WinError(c.get_last_error())
        return result

    def start(self, command, output, error):
        job = self.kernel.CreateJobObjectW(None, None)
        if not job:
            raise c.WinError(c.get_last_error())
        process = None
        try:
            # Assign before any guest can spawn. Native exec replaces a process,
            # so walking host parent PIDs after startup loses dead ancestors.
            process = subprocess.Popen(command, stdout=output, stderr=error,
                creationflags=subprocess.CREATE_NO_WINDOW | 4)  # CREATE_SUSPENDED
            if not self.kernel.AssignProcessToJobObject(job, int(process._handle)):
                raise c.WinError(c.get_last_error())
            if self.nt.NtResumeProcess(int(process._handle)) < 0:
                raise RuntimeError('could not resume supervisor')
            return process, job
        except BaseException:
            if process is not None:
                process.kill()
                process.wait(timeout=5)
            self.kernel.CloseHandle(job)
            raise

    def belongs(self, pid, job):
        handle = self.kernel.OpenProcess(0x1000, False, pid)
        if not handle:
            return False
        try:
            member = w.BOOL()
            name = c.create_unicode_buffer(32768)
            size = w.DWORD(len(name))
            return (self.kernel.IsProcessInJob(handle, job, c.byref(member)) and member.value
                and self.kernel.QueryFullProcessImageNameW(handle, 0, name, c.byref(size))
                and Path(name.value) == self.worker)
        finally:
            self.kernel.CloseHandle(handle)

    def close(self, process, job, title, deadline):
        while time.monotonic() < deadline and process.poll() is None:
            for hwnd, pid, text in self.windows():
                if text == title and self.belongs(pid, job):
                    # Let startup idles drain. No guest timer or XPending call
                    # is permitted to wake the main loop for this test.
                    time.sleep(2)
                    if not self.user.PostMessageW(hwnd, 0x10, 0, 0):  # WM_CLOSE
                        raise c.WinError(c.get_last_error())
                    return dict(hwnd=hwnd, owner_pid=pid, title=text)
            time.sleep(0.05)
        raise RuntimeError(f'window not found: {title}; exit={process.poll()}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=45)
    args = parser.parse_args()
    if not 5 <= args.timeout <= 120:
        parser.error('timeout must be between 5 and 120 seconds')
    worker, root, dist, report = (p.resolve() for p in (args.worker, args.root, args.dist, args.report))
    desktop = Desktop(worker)
    source = Path(__file__).with_name('DesktopGtkProbe.py').read_text(encoding='utf-8')
    cases = [
        ('gtk-idle-close', 'Kinakaze GTK probe', ['/usr/bin/python3.11', '-c', source, '--idle-close'], 'DESKTOP_GTK_IDLE_CLOSE_OK'),
        ('gnome-dictionary-close', 'Dictionary', ['/usr/bin/gnome-dictionary'], None),
        ('qt-analogclock-close', 'Analog Clock', ['/usr/lib/x86_64-linux-gnu/qt5/examples/widgets/widgets/analogclock/analogclock'], None),
    ]
    logs = report.with_suffix('')
    logs.mkdir(parents=True, exist_ok=True)
    results = []
    for name, title, argv, marker in cases:
        row = dict(id=name, status='failed')
        output, error = logs / (name + '.stdout.log'), logs / (name + '.stderr.log')
        command = [str(worker), 'run', '--root', str(root), '--dist', str(dist), '--', '/usr/bin/dbus-run-session', '--', *argv]
        started = time.monotonic()
        with output.open('wb') as out, error.open('wb') as err:
            process, job = desktop.start(command, out, err)
            try:
                deadline = started + args.timeout
                row['window'] = desktop.close(process, job, title, deadline)
                row['exit_code'] = process.wait(timeout=max(0.1, deadline-time.monotonic()))
                if row['exit_code'] != 0:
                    raise RuntimeError(f"exit {row['exit_code']}")
                if marker and marker.encode() not in output.read_bytes():
                    raise RuntimeError('guest close callback did not complete')
                row['status'] = 'passed'
            except (RuntimeError, OSError, subprocess.TimeoutExpired) as exc:
                row['reason'] = str(exc)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
                desktop.kernel.CloseHandle(job)
        row.update(elapsed_ms=round((time.monotonic()-started)*1000), stdout=str(output), stderr=str(error))
        results.append(row)
        print(name, row['status'], flush=True)
    report.write_text(json.dumps(dict(results=results, gtk_source_sha256=hashlib.sha256(source.encode()).hexdigest()), indent=2)+'\n', encoding='utf-8')
    return int(any(row['status'] != 'passed' for row in results))


if __name__ == '__main__':
    raise SystemExit(main())
