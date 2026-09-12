"""Click a GTK grid before/after move and resize, including a transient window."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'tools'))
from session_process import SessionProcess

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--report', required=True, type=Path)
    args = parser.parse_args()
    dist, root, report = (p.resolve() for p in (args.dist, args.root, args.report))
    u, k = c.WinDLL('user32'), c.WinDLL('kernel32')
    u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
    u.GetForegroundWindow.restype = w.HWND
    u.SetForegroundWindow.argtypes = [w.HWND]
    u.SetWindowPos.argtypes = [w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT]
    u.ClientToScreen.argtypes = [w.HWND, c.POINTER(w.POINT)]
    u.WindowFromPoint.argtypes = [w.POINT]; u.WindowFromPoint.restype = w.HWND
    u.GetAncestor.argtypes = [w.HWND, w.UINT]; u.GetAncestor.restype = w.HWND
    u.mouse_event.argtypes = [w.DWORD, w.DWORD, w.DWORD, w.DWORD, c.c_size_t]
    original, foreground = w.POINT(), u.GetForegroundWindow()
    u.GetCursorPos(c.byref(original))
    output, error = report.with_suffix('.stdout.log'), report.with_suffix('.stderr.log')
    report.parent.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).with_name('DesktopCoordinatesProbe.py').read_text()
    command = [str(dist / 'worker.exe'), 'run', '--root', str(root), '--dist', str(dist), '--',
               '/usr/bin/dbus-run-session', '--', '/usr/bin/python3.11', '-c', source]
    result = dict(status='failed', clicks=[])
    def events():
        rows = []
        for line in output.read_text(errors='replace').splitlines():
            if line.startswith('{'):
                try: rows.append(json.loads(line))
                except json.JSONDecodeError: pass
        return rows
    def wait(predicate, timeout=25):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and session.process.poll() is None:
            rows = events()
            if predicate(rows): return rows
            time.sleep(.05)
        raise AssertionError('Timed out waiting for GTK coordinates: ' + str(events()[-3:]))
    def focus(hwnd, x, y, width, height):
        me = k.GetCurrentThreadId()
        threads = {u.GetWindowThreadProcessId(hwnd, None),
                   u.GetWindowThreadProcessId(u.GetForegroundWindow(), None)} - {me, 0}
        for tid in threads: u.AttachThreadInput(me, tid, 1)
        try:
            # Tests own both windows and temporarily keep them above the Shell.
            assert u.SetWindowPos(hwnd, -1, x, y, width, height, 0)
            u.SetForegroundWindow(hwnd)
        finally:
            for tid in threads: u.AttachThreadInput(me, tid, 0)
    with output.open('wb') as out, error.open('wb') as err:
        session = SessionProcess(command, stdout=out, stderr=err)
        try:
            rows = wait(lambda rows: len({r['role'] for r in rows if r['kind'] == 'ready'}) == 2, 60)
            handles = {r['role']: r['hwnd'] for r in rows if r['kind'] == 'ready'}
            for role, hwnd in handles.items():
                for phase, (x, y, width, height) in enumerate(((180, 180, 480, 330),
                                                            (960, 580, 480, 330),
                                                            (900, 480, 640, 420))):
                    # Park the other window clear of this target, including the
                    # always-above-owner ordering of a native transient.
                    for other in handles.values():
                        if other != hwnd: u.SetWindowPos(other, None, 1800, 250, 0, 0, 0x15)
                    focus(hwnd, x, y, width, height)
                    time.sleep(.5)
                    rows = wait(lambda rows: any(r['kind'] == 'ready' and r['role'] == role
                        and r['area'][2] == width for r in rows))
                    ready = next(r for r in reversed(rows) if r['kind'] == 'ready' and r['role'] == role)
                    ax, ay, aw, ah = ready['area']
                    origin = w.POINT(); assert u.ClientToScreen(hwnd, c.byref(origin))
                    for lx, ly in ((18, 18), (aw-18, 18), (aw//2, ah//2), (18, ah-18), (aw-18, ah-18)):
                        sx, sy = origin.x + ax + lx, origin.y + ay + ly
                        assert u.GetAncestor(u.WindowFromPoint(w.POINT(sx, sy)), 2) == hwnd
                        count = sum(r['kind'] == 'click' for r in events())
                        assert u.SetCursorPos(sx, sy)
                        time.sleep(.08)
                        u.mouse_event(2, 0, 0, 0, 0); u.mouse_event(4, 0, 0, 0, 0)
                        rows = wait(lambda rows: sum(r['kind'] == 'click' for r in rows) > count)
                        click = next(r for r in reversed(rows) if r['kind'] == 'click')
                        result['clicks'].append(dict(phase=phase, expected_local=[lx, ly],
                                                    expected_root=[sx, sy], **click))
                        assert click['role'] == role, click
                        assert click['local'] == [lx, ly], result['clicks'][-1]
                        assert click['root'] == [sx, sy], result['clicks'][-1]
            result['status'] = 'passed'
        except Exception as error:
            result['reason'] = repr(error)
        finally:
            session.close()
            u.SetCursorPos(original.x, original.y)
            u.SetForegroundWindow(foreground)
    report.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(dict(status=result['status'], clicks=len(result['clicks']), reason=result.get('reason'))))
    return result['status'] != 'passed'

if __name__ == '__main__':
    raise SystemExit(main())
