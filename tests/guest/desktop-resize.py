"""Resize the real Qt clock and verify its client pixels, without forcing paint."""
from __future__ import annotations
import argparse
import ctypes as c
from ctypes import wintypes as w
import importlib.util
import json
from pathlib import Path
import subprocess
import time
from PIL import Image

spec = importlib.util.spec_from_file_location('desktop_window', Path(__file__).with_name('desktop-window.py'))
window = importlib.util.module_from_spec(spec)
spec.loader.exec_module(window)


class Capture:
    def __init__(self, desktop):
        self.user = desktop.user
        self.gdi = c.WinDLL('gdi32', use_last_error=True)
        def api(lib, name, result, *args):
            fn = getattr(lib, name)
            fn.restype, fn.argtypes = result, args
            return fn
        self.client = api(self.user, 'GetClientRect', w.BOOL, w.HWND, c.POINTER(w.RECT))
        self.bounds = api(self.user, 'GetWindowRect', w.BOOL, w.HWND, c.POINTER(w.RECT))
        self.show = api(self.user, 'ShowWindow', w.BOOL, w.HWND, c.c_int)
        self.iconic = api(self.user, 'IsIconic', w.BOOL, w.HWND)
        self.position = api(self.user, 'SetWindowPos', w.BOOL, w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT)
        self.getdc = api(self.user, 'GetDC', w.HDC, w.HWND)
        self.release = api(self.user, 'ReleaseDC', c.c_int, w.HWND, w.HDC)
        self.create = api(self.gdi, 'CreateCompatibleDC', w.HDC, w.HDC)
        self.deletedc = api(self.gdi, 'DeleteDC', w.BOOL, w.HDC)
        self.bitmap = api(self.gdi, 'CreateDIBSection', w.HANDLE, w.HDC, c.c_void_p, w.UINT, c.POINTER(c.c_void_p), w.HANDLE, w.DWORD)
        self.select = api(self.gdi, 'SelectObject', w.HANDLE, w.HDC, w.HANDLE)
        self.delete = api(self.gdi, 'DeleteObject', w.BOOL, w.HANDLE)
        self.blit = api(self.gdi, 'BitBlt', w.BOOL, w.HDC, c.c_int, c.c_int, c.c_int, c.c_int, w.HDC, c.c_int, c.c_int, w.DWORD)
        api(self.user, 'SetProcessDpiAwarenessContext', w.BOOL, w.HANDLE)(-4)

    def size(self, hwnd):
        rect = w.RECT()
        if not self.client(hwnd, c.byref(rect)):
            raise c.WinError(c.get_last_error())
        return rect.right, rect.bottom

    def resize(self, hwnd, size):
        # Adjust using actual client/nonclient extents, including the current DPI.
        client = self.size(hwnd)
        rect = w.RECT()
        if not self.bounds(hwnd, c.byref(rect)):
            raise c.WinError(c.get_last_error())
        width = size[0] + rect.right - rect.left - client[0]
        height = size[1] + rect.bottom - rect.top - client[1]
        # Only our test-owned window is raised, without activating or focusing it.
        if not self.position(hwnd, -1, 40, 40, width, height, 0x10):
            raise c.WinError(c.get_last_error())
        if self.size(hwnd) != size:
            raise RuntimeError(f'client size {self.size(hwnd)} differs from {size}')

    def pixels(self, hwnd):
        width, height = self.size(hwnd)
        dc = self.getdc(hwnd)
        if not dc:
            raise c.WinError(c.get_last_error())
        memory = self.create(dc)
        bitmap = previous = None
        try:
            import struct
            info = struct.pack('<IiiHHIIiiII', 40, width, -height, 1, 32, 0, 0, 0, 0, 0, 0)
            bits = c.c_void_p()
            bitmap = self.bitmap(dc, c.c_char_p(info), 0, c.byref(bits), None, 0)
            if not memory or not bitmap:
                raise c.WinError(c.get_last_error())
            previous = self.select(memory, bitmap)
            # Read displayed client pixels; PrintWindow/WM_PAINT would hide missed repaint events.
            if not self.blit(memory, 0, 0, width, height, dc, 0, 0, 0x00cc0020):
                raise c.WinError(c.get_last_error())
            self.gdi.GdiFlush()
            return Image.frombytes('RGB', (width, height), c.string_at(bits, width * height * 4), 'raw', 'BGRX')
        finally:
            if previous:
                self.select(memory, previous)
            if bitmap:
                self.delete(bitmap)
            if memory:
                self.deletedc(memory)
            self.release(hwnd, dc)


def clock_geometry(image):
    width, height = image.size
    # The fixture paints purple hour marks at +/-96 in a 200-unit square,
    # centered in the client area and scaled by its shorter dimension.
    purple = []
    data = image.tobytes()
    for index, (r, g, b) in enumerate(zip(data[0::3], data[1::3], data[2::3])):
        if min(r, b) > 60 and g + 30 < min(r, b) and abs(r - b) < 8:
            purple.append((index % width, index // width))
    if not purple:
        return dict(ok=False, reason='clock hour marks are absent')
    bounds = [min(x for x, y in purple), min(y for x, y in purple), max(x for x, y in purple), max(y for x, y in purple)]
    radius = min(width, height) * 0.48
    expected = [width // 2 - radius, height // 2 - radius, width // 2 + radius, height // 2 + radius]
    tolerance = max(3, min(width, height) / 400 + 1.5)
    geometry = all(abs(actual - wanted) <= tolerance for actual, wanted in zip(bounds, expected))
    pixels = image.load()
    corners = [pixels[x, y] for x, y in [(3, 3), (width-4, 3), (3, height-4), (width-4, height-4)]]
    clean = all(max(abs(a-b) for a, b in zip(color, corners[0])) <= 2 for color in corners)
    return dict(ok=geometry and clean, hour_mark_bounds=bounds, expected_bounds=expected, corners=corners)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    dist, report = args.dist.resolve(), args.report.resolve()
    logs = report.with_suffix('')
    logs.mkdir(parents=True, exist_ok=True)
    desktop = window.Desktop(dist / 'worker.exe')
    capture = Capture(desktop)
    command = [str(dist / 'worker.exe'), 'run', '--root', str(dist / 'rootfs'), '--dist', str(dist), '--', '/usr/bin/dbus-run-session', '--', '/usr/lib/x86_64-linux-gnu/qt5/examples/widgets/widgets/analogclock/analogclock']
    result = dict(status='failed', frames=[])
    started = time.monotonic()
    with (logs / 'stdout.log').open('wb') as out, (logs / 'stderr.log').open('wb') as err:
        process, job = desktop.start(command, out, err)
        try:
            deadline = started + 60
            hwnd = None
            while time.monotonic() < deadline and process.poll() is None and hwnd is None:
                for candidate, pid, title in desktop.windows():
                    if title == 'Analog Clock' and desktop.belongs(pid, job):
                        hwnd = candidate
                        result['window'] = dict(hwnd=hwnd, owner_pid=pid)
                        break
                if hwnd is None:
                    time.sleep(0.05)
            if hwnd is None:
                raise RuntimeError('Qt clock window not found')
            stages = [
                ('landscape', (320,240)), ('enlarge', (700,460)),
                ('portrait', (260,520)), ('shrink', (180,160)),
                ('burst', (480,320)), ('maximize', None),
                ('restore', (480,320)), ('minimize-restore', (480,320)),
            ]
            for index, (stage, size) in enumerate(stages):
                if index == 4:
                    # A burst leaves only the final dimensions authoritative.
                    for intermediate in [(640,420), (210,350), (500,180), (720,500), (230,220)]:
                        capture.resize(hwnd, intermediate)
                if stage == 'maximize':
                    capture.show(hwnd, 3)
                    size = capture.size(hwnd)
                    if size[0] <= 480 or size[1] <= 320:
                        raise RuntimeError(f'maximize did not enlarge the client: {size}')
                elif stage in ('restore', 'minimize-restore'):
                    if stage == 'minimize-restore':
                        capture.show(hwnd, 6)
                        if not capture.iconic(hwnd):
                            raise RuntimeError('window did not minimize')
                    capture.show(hwnd, 9)
                    if capture.size(hwnd) != size:
                        raise RuntimeError(f'restore did not recover the client: {capture.size(hwnd)}')
                else:
                    capture.resize(hwnd, size)
                frame_start = time.monotonic()
                frame_deadline = min(deadline, frame_start + 5)
                while True:
                    image = capture.pixels(hwnd)
                    geometry = clock_geometry(image)
                    if geometry['ok'] or time.monotonic() >= frame_deadline:
                        break
                    time.sleep(0.05)
                path = logs / f'{index}-{size[0]}x{size[1]}.png'
                image.save(path)
                row = dict(stage=stage, size=size, elapsed_ms=round((time.monotonic()-frame_start)*1000), image=str(path), **geometry)
                result['frames'].append(row)
                print(json.dumps(row), flush=True)
                if not geometry['ok']:
                    raise RuntimeError(f'repaint does not match client size {size}')
            if not desktop.user.PostMessageW(hwnd, 0x10, 0, 0):
                raise c.WinError(c.get_last_error())
            result['exit_code'] = process.wait(timeout=max(0.1, deadline-time.monotonic()))
            if result['exit_code'] != 0:
                raise RuntimeError(f"exit {result['exit_code']}")
            errors = [line for line in (logs / 'stderr.log').read_text(encoding='utf-8', errors='replace').splitlines() if 'XCB error:' in line]
            result['xcb_errors'] = errors
            if errors:
                raise RuntimeError('Qt reported XCB protocol errors')
            result['status'] = 'passed'
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as exc:
            result['reason'] = str(exc)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)
            desktop.kernel.CloseHandle(job)
    result['elapsed_ms'] = round((time.monotonic()-started)*1000)
    report.write_text(json.dumps(result, indent=2)+'\n', encoding='utf-8')
    print(result['status'], result.get('reason', ''), flush=True)
    return int(result['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
