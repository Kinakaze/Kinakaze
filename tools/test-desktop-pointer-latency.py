"""Sample physical pointer input to changed screen pixels on an owned GTK window.

Requires the resident desktop test driver; measures an end-to-end software
observation, including GetPixel/polling overhead, not hardware photon latency.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import shutil
import statistics
import time
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--root', required=True, type=Path)
parser.add_argument('--output', required=True, type=Path)
args = parser.parse_args()
title = 'Kinakaze pointer latency probe'
u, g = c.WinDLL('user32'), c.WinDLL('gdi32')
callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
u.EnumWindows.argtypes = [callback, w.LPARAM]
u.EnumChildWindows.argtypes = [w.HWND, callback, w.LPARAM]
u.GetClassNameW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
u.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
u.IsWindowVisible.argtypes = [w.HWND]
u.GetPropW.argtypes = [w.HWND, w.LPCWSTR]
u.GetPropW.restype = w.HANDLE
u.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
u.SetForegroundWindow.argtypes = [w.HWND]
u.WindowFromPoint.argtypes = [w.POINT]
u.WindowFromPoint.restype = w.HWND
u.GetAncestor.argtypes = [w.HWND, w.UINT]
u.GetAncestor.restype = w.HWND
u.GetDC.argtypes = [w.HWND]
u.GetDC.restype = w.HDC
u.ReleaseDC.argtypes = [w.HWND, w.HDC]
u.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
g.GetPixel.argtypes = [w.HDC, c.c_int, c.c_int]
g.GetPixel.restype = w.DWORD


def windows():
    result = []
    @c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    def visit(handle, _):
        name = c.create_unicode_buffer(256)
        u.GetClassNameW(handle, name, 256)
        if name.value == 'kinakaze.display.window':
            u.GetWindowTextW(handle, name, 256)
            if (name.value == title and u.IsWindowVisible(handle)
                    and u.GetPropW(handle, 'KinakazeFirstFrame') == 1):
                result.append(handle)
        return True
    @c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    def top(handle, data):
        visit(handle, data)
        u.EnumChildWindows(handle, visit, 0)
        return True
    u.EnumWindows(top, 0)
    return set(result)


folder = args.root / 'tmp/desktop-test-driver'
assert folder.is_dir()
(folder.parent / 'shell-test-command').write_text('desktop')
time.sleep(.5)
before = windows()
source = Path(__file__).resolve().parents[1] / 'tests/guest/GtkPointerLatencyProbe.py'
shutil.copyfile(source, folder.parent / source.name)
job = folder / (uuid.uuid4().hex + '.json')
temporary = job.with_suffix('.tmp')
temporary.write_text(json.dumps({'argv': ['/usr/bin/python3.11', '/tmp/' + source.name]}))
os.replace(temporary, job)
deadline = time.monotonic() + 25
handle = None
while time.monotonic() < deadline:
    new = windows() - before
    if new:
        assert len(new) == 1
        handle = new.pop()
        break
    time.sleep(.02)
assert handle, 'owned latency fixture did not map'
try:
    top = u.GetAncestor(handle, 2)
    u.SetForegroundWindow(top)
    time.sleep(.3)
    rect = w.RECT()
    u.GetWindowRect(handle, c.byref(rect))
    width, height = rect.right - rect.left, rect.bottom - rect.top
    assert width > 300 and height > 250
    y = rect.top + height * 2 // 3
    positions = [rect.left + width // 4, rect.left + width * 3 // 4]
    sample_point = w.POINT(rect.left + width // 2, rect.bottom - 80)
    samples = []
    for index in range(34):
        side = index % 2
        x = positions[side]
        for point in [w.POINT(x, y), sample_point]:
            assert u.GetAncestor(u.WindowFromPoint(point), 2) == top, 'fixture covered'
        expected = 0xff if side else 0xff00  # COLORREF red/green
        dc = u.GetDC(None)
        try:
            started = time.perf_counter_ns()
            assert u.SetCursorPos(x, y), 'pointer move failed'
            deadline = time.monotonic() + 1.5
            while g.GetPixel(dc, sample_point.x, sample_point.y) != expected:
                assert time.monotonic() < deadline, 'GTK did not repaint after pointer motion'
                time.sleep(.001)
            elapsed = (time.perf_counter_ns() - started) / 1e6
        finally:
            u.ReleaseDC(None, dc)
        assert u.GetAncestor(u.WindowFromPoint(sample_point), 2) == top, 'sample covered'
        if index >= 4:
            samples.append(elapsed)
        time.sleep(.045)
    ordered = sorted(samples)
    report = dict(samples_ms=[round(value, 3) for value in samples],
                  median_ms=round(statistics.median(samples), 3),
                  p95_ms=round(ordered[28], 3), maximum_ms=round(max(samples), 3),
                  observation='SetCursorPos to changed GTK content at owned screen pixel; includes polling/GetPixel overhead')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report), flush=True)
finally:
    u.PostMessageW(handle, 0x10, 0, 0)
