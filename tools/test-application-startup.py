"""Observe the first submitted frame of an isolated, owned guest application.

Includes worker launch and a DwmFlush boundary, plus polling overhead; this is
not photon latency or GNOME desktop activation. Captures only the owned window.
Optional settling, title checks and native close observation extend the probe
past startup; they do not assert complete document rendering or interaction.
"""
import argparse
import ctypes as C
from ctypes import wintypes as W
import json
from pathlib import Path
import subprocess
import time

from session_process import SessionProcess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--dist', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--timeout', type=float, default=45)
    parser.add_argument('--capture', action='store_true')
    parser.add_argument('--settle-seconds', type=float, default=0,
                        help='observe the window after its first frame before capturing')
    parser.add_argument('--close-timeout', type=float, default=0,
                        help='request native window close and wait for normal process exit')
    parser.add_argument('--title-contains', help='require this text in a settled window title')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('supply a guest executable after --')
    if args.timeout <= 0 or not 0 <= args.settle_seconds <= 60 or not 0 <= args.close_timeout <= 60:
        parser.error('timeout must be positive; settle and close waits must be within [0, 60]')
    user, kernel, dwm = C.WinDLL('user32'), C.WinDLL('kernel32'), C.WinDLL('dwmapi')
    callback = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
    user.EnumWindows.argtypes = [callback, W.LPARAM]
    user.EnumChildWindows.argtypes = [W.HWND, callback, W.LPARAM]
    user.GetClassNameW.argtypes = [W.HWND, W.LPWSTR, C.c_int]
    user.GetWindowTextW.argtypes = [W.HWND, W.LPWSTR, C.c_int]
    user.GetWindowThreadProcessId.argtypes = [W.HWND, C.POINTER(W.DWORD)]
    user.GetWindowRect.argtypes = [W.HWND, C.POINTER(W.RECT)]
    user.IsWindowVisible.argtypes = [W.HWND]
    user.GetPropW.argtypes = [W.HWND, W.LPCWSTR]
    user.GetPropW.restype = W.HANDLE
    user.PostMessageW.argtypes = [W.HWND, W.UINT, W.WPARAM, W.LPARAM]
    user.SetProcessDpiAwarenessContext.argtypes = [W.HANDLE]
    user.SetProcessDpiAwarenessContext(C.c_void_p(-4))
    kernel.OpenProcess.argtypes = [W.DWORD, W.BOOL, W.DWORD]
    kernel.OpenProcess.restype = W.HANDLE
    kernel.IsProcessInJob.argtypes = [W.HANDLE, W.HANDLE, C.POINTER(W.BOOL)]
    kernel.CloseHandle.argtypes = [W.HANDLE]

    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = dict(command=command, dist=str(args.dist.resolve()), root=str(args.root.resolve()),
                  scope=__doc__.strip(), polling_interval_ms=5, status='timeout')
    with args.output.with_suffix('.log').open('wb') as log:
        started = time.perf_counter_ns()
        child = SessionProcess([str(args.dist.resolve() / 'worker.exe'), 'run',
                                '--root', str(args.root.resolve()), '--dist', str(args.dist.resolve()),
                                '--', *command], stdout=log, stderr=log)

        def owned(hwnd):
            pid = W.DWORD()
            user.GetWindowThreadProcessId(hwnd, C.byref(pid))
            process = kernel.OpenProcess(0x1000, False, pid.value)
            if not process:
                return False
            try:
                member = W.BOOL()
                return bool(kernel.IsProcessInJob(process, child.job, C.byref(member)) and member.value)
            finally:
                kernel.CloseHandle(process)

        try:
            while (time.perf_counter_ns() - started) / 1e9 < args.timeout:
                frames = []

                @callback
                def visit(hwnd, _):
                    name = C.create_unicode_buffer(256)
                    user.GetClassNameW(hwnd, name, len(name))
                    if name.value != 'kinakaze.display.window' or not user.IsWindowVisible(hwnd):
                        return True
                    if not owned(hwnd) or user.GetPropW(hwnd, 'KinakazeInputOnly'):
                        return True
                    rect = W.RECT()
                    user.GetWindowRect(hwnd, C.byref(rect))
                    if rect.right - rect.left <= 32 or rect.bottom - rect.top <= 32:
                        return True
                    report.setdefault('first_visible_ms', (time.perf_counter_ns() - started) / 1e6)
                    if user.GetPropW(hwnd, 'KinakazeFirstFrame') == 1:
                        user.GetWindowTextW(hwnd, name, len(name))
                        frames.append(dict(hwnd=hwnd, title=name.value,
                                           size=[rect.right-rect.left, rect.bottom-rect.top]))
                    return True

                @callback
                def top(hwnd, data):
                    visit(hwnd, data)
                    user.EnumChildWindows(hwnd, visit, 0)
                    return True

                user.EnumWindows(top, 0)
                if frames:
                    report['dwm_flush_hresult'] = dwm.DwmFlush()
                    report['first_frame_ms'] = (time.perf_counter_ns() - started) / 1e6
                    report.update(status='frame_observed', windows=frames)
                    if args.settle_seconds:
                        until = time.monotonic() + args.settle_seconds
                        while child.process.poll() is None and time.monotonic() < until:
                            time.sleep(.05)
                        frames = []
                        user.EnumWindows(top, 0)
                        report['settled_windows'] = frames
                        if not frames or child.process.poll() is not None:
                            report.update(status='exited_after_frame', exit_code=child.process.poll())
                            break
                    if args.title_contains:
                        report['title_matched'] = any(args.title_contains in f['title'] for f in frames)
                        if not report['title_matched']:
                            report['status'] = 'unexpected_title'
                    if args.capture and owned(frames[0]['hwnd']):
                        from PIL import ImageGrab
                        picture = args.output.with_suffix('.png')
                        ImageGrab.grab(window=frames[0]['hwnd']).save(picture)
                        report['capture'] = str(picture.resolve())
                    if args.close_timeout:
                        report['close_requests'] = [dict(hwnd=f['hwnd'], posted=bool(
                            user.PostMessageW(f['hwnd'], 0x0010, 0, 0))) for f in frames if owned(f['hwnd'])]
                        closing = time.perf_counter_ns()
                        try:
                            report['exit_code'] = child.process.wait(timeout=args.close_timeout)
                            report['close_status'] = 'exited' if report['exit_code'] == 0 else 'failed'
                        except subprocess.TimeoutExpired:
                            report['close_status'] = 'timeout'
                        report['close_ms'] = (time.perf_counter_ns() - closing) / 1e6
                        frames = []
                        user.EnumWindows(top, 0)
                        report['remaining_windows_after_close'] = frames
                    break
                if child.process.poll() is not None:
                    report.update(status='exited_before_frame', exit_code=child.process.returncode)
                    break
                time.sleep(.005)
        finally:
            report['observation_ms'] = (time.perf_counter_ns() - started) / 1e6
            child.close()
    args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report), flush=True)
    return int(report['status'] != 'frame_observed' or
               (args.close_timeout > 0 and report.get('close_status') != 'exited'))


if __name__ == '__main__':
    raise SystemExit(main())
