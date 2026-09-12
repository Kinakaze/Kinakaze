"""Exercise native CSD geometry, title-bar movement and GTK mouse selection."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import importlib.util
import json
from pathlib import Path
import subprocess
import time

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--report', required=True, type=Path)
    parser.add_argument('--rapid', action='store_true', help='Queue drag motion without waiting for the press repaint')
    args = parser.parse_args()
    dist, root, report = args.dist.resolve(), args.root.resolve(), args.report.resolve()
    spec = importlib.util.spec_from_file_location('desktop_window', Path(__file__).with_name('desktop-window.py'))
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    desktop = module.Desktop(dist / 'worker.exe')
    user = desktop.user
    user.GetClientRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
    user.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
    user.ClientToScreen.argtypes = [w.HWND, c.POINTER(w.POINT)]
    user.GetWindowLongW.argtypes = [w.HWND, c.c_int]
    user.SetForegroundWindow.argtypes = [w.HWND]
    user.SetCursorPos.argtypes = [c.c_int, c.c_int]
    user.GetCursorPos.argtypes = [c.POINTER(w.POINT)]
    user.mouse_event.argtypes = [w.DWORD, w.DWORD, w.DWORD, w.DWORD, c.c_size_t]
    user.keybd_event.argtypes = [c.c_ubyte, c.c_ubyte, w.DWORD, c.c_size_t]
    user.VkKeyScanW.argtypes = [c.c_wchar]
    user.VkKeyScanW.restype = c.c_short
    user.GetForegroundWindow.restype = w.HWND
    user.WindowFromPoint.argtypes = [w.POINT]
    user.WindowFromPoint.restype = w.HWND
    user.SetWindowPos.argtypes = [w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT]
    original_cursor = w.POINT(); user.GetCursorPos(c.byref(original_cursor))
    original_foreground = user.GetForegroundWindow()
    output, error = report.with_suffix('.stdout.log'), report.with_suffix('.stderr.log')
    report.parent.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).with_name('DesktopInteractionProbe.py').read_text(encoding='utf-8')
    command = [str(dist / 'worker.exe'), 'run', '--root', str(root), '--dist', str(dist), '--',
               '/usr/bin/dbus-run-session', '--', '/usr/bin/python3.11', '-c', source, '--trace-input']
    result = dict(status='failed')
    def bounds(hwnd):
        rect = w.RECT(); assert user.GetWindowRect(hwnd, c.byref(rect))
        return [rect.left, rect.top, rect.right, rect.bottom]
    def settled(previous):
        """Waits for a QUIET marker printed after the host's last action."""
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and process.poll() is None:
            count = output.read_text(encoding='utf-8', errors='replace').count('QUIET')
            if count > previous:
                return count
            time.sleep(0.1)
        raise AssertionError('GTK loop did not settle')
    def drag(x, y, dx, dy, quiet):
        """Presses, waits for the press to be handled, then moves and releases.

        The press repaints (focus, selection start); the guest reports QUIET
        again once that is done, so the motion meets a responsive loop.
        """
        assert user.SetCursorPos(x, y)
        time.sleep(0.15)
        user.mouse_event(2, 0, 0, 0, 0)
        try:
            quiet = settled(quiet)
            for step in range(1, 21):
                assert user.SetCursorPos(x + dx * step // 20, y + dy * step // 20)
                time.sleep(0.025)
        finally:
            user.mouse_event(4, 0, 0, 0, 0)
        return settled(quiet)
    with output.open('wb') as out, error.open('wb') as err:
        launched = time.monotonic()
        process, job = desktop.start(command, out, err)
        try:
            deadline = time.monotonic() + 55
            ready = None
            max_visible_copies = 0
            while time.monotonic() < deadline and process.poll() is None:
                copies = sum(title == 'Kinakaze drag and selection probe' and desktop.belongs(pid, job)
                             for _, pid, title in desktop.windows())
                max_visible_copies = max(max_visible_copies, copies)
                for line in output.read_text(encoding='utf-8', errors='replace').splitlines():
                    if line.startswith('{'):
                        ready = json.loads(line); break
                if ready: break
                time.sleep(0.1)
            assert ready, 'GTK probe did not become ready'
            result['max_visible_copies_during_fork'] = max_visible_copies
            assert max_visible_copies <= 1, 'Fork displayed a duplicate application window'
            result['ready_seconds'] = round(time.monotonic() - launched, 3)
            hwnd = ready['hwnd']
            pid = w.DWORD(); user.GetWindowThreadProcessId(hwnd, c.byref(pid))
            assert desktop.belongs(pid.value, job), 'Window not owned by this test'
            style, extended = user.GetWindowLongW(hwnd, -16), user.GetWindowLongW(hwnd, -20)
            result.update(hwnd=hwnd, style=hex(style & 0xffffffff), ex_style=hex(extended & 0xffffffff))
            assert style & 0xcf0000 == 0, 'CSD has a second Windows frame'
            assert extended & 8 == 0, 'CSD became globally topmost'
            # The host may hold the foreground lock; keep the probe above other
            # desktop windows for the drag only, after the style assertions.
            # Activation repaints the header bar in its focused state, which
            # takes seconds under emulation; wait for the loop to settle again.
            quiet = output.read_text(encoding='utf-8', errors='replace').count('QUIET')
            user.SetWindowPos(hwnd, -1, 200, 160, 0, 0, 0x11)
            user.SetForegroundWindow(hwnd)
            quiet = settled(quiet)
            before = bounds(hwnd)
            point = w.POINT(before[0] + (before[2] - before[0]) // 2, before[1] + 25)
            result.update(foreground=user.GetForegroundWindow(), hit_window=user.WindowFromPoint(point))
            assert result['hit_window'] == hwnd, result
            user.PostMessageW(hwnd, 0x200, 0, (25 << 16) | 325)
            quiet = drag(before[0] + (before[2] - before[0]) // 2, before[1] + 25, 120, 80, quiet)
            after = bounds(hwnd)
            result.update(before_drag=before, after_drag=after)
            assert after[0] - before[0] >= 90 and after[1] - before[1] >= 60, (before, after)
            origin = w.POINT(); assert user.ClientToScreen(hwnd, c.byref(origin))
            sx, sy = ready['scroll']
            user.SetCursorPos(origin.x + sx, origin.y + sy)
            # Six partial deltas must accumulate into two downward notches.
            for _ in range(6):
                user.mouse_event(0x0800, 0, 0, c.c_uint(-40).value, 0)
                time.sleep(0.04)
            if not args.rapid:
                quiet = settled(quiet)
            for _ in range(6):
                user.mouse_event(0x1000, 0, 0, 40, 0)
                time.sleep(0.04)
            quiet = settled(quiet)
            x, y, height = ready['entry']
            # Verify the popup stays at the click and its visible action works.
            anchor = [origin.x + x + 35, origin.y + y + height // 2]
            user.SetCursorPos(*anchor)
            user.mouse_event(8, 0, 0, 0, 0); user.mouse_event(16, 0, 0, 0, 0)
            quiet = settled(quiet)
            popups = [json.loads(line) for line in output.read_text().splitlines()
                      if line.startswith('{') and '"menu"' in line]
            assert popups, 'The context menu did not draw its action'
            popup = popups[-1]
            menu_bounds = bounds(popup['menu'])
            result.update(popup_anchor=anchor, popup_bounds=menu_bounds)
            assert abs(menu_bounds[0] - anchor[0]) <= 16 and abs(menu_bounds[1] - anchor[1]) <= 16, result
            menu_origin = w.POINT(); assert user.ClientToScreen(popup['menu'], c.byref(menu_origin))
            user.SetCursorPos(menu_origin.x + popup['action'][0], menu_origin.y + popup['action'][1])
            user.mouse_event(2, 0, 0, 0, 0); user.mouse_event(4, 0, 0, 0, 0)
            quiet = settled(quiet)
            kx, ky = ready['keyboard']
            user.SetCursorPos(origin.x + kx, origin.y + ky)
            user.mouse_event(2, 0, 0, 0, 0); user.mouse_event(4, 0, 0, 0, 0)
            quiet = settled(quiet)
            def key(code, released=False):
                user.keybd_event(code, user.MapVirtualKeyW(code, 0), 2 if released else 0, 0)
            def type_text(text):
                for char in text:
                    mapping = user.VkKeyScanW(char)
                    assert mapping != -1
                    shifted = bool(mapping & 0x100)
                    if shifted: key(16)
                    key(mapping & 255); key(mapping & 255, True)
                    if shifted: key(16, True)
                    time.sleep(0.025)
            type_text('replace-me')
            key(17); key(65); key(65, True); key(17, True)
            type_text('Aa&|<>!_')
            quiet = settled(quiet)
            quiet = drag(origin.x + x + 10, origin.y + y + height // 2, 280, 0, quiet)
            try:
                from PIL import ImageGrab
                screenshot = report.with_suffix('.png'); ImageGrab.grab(window=hwnd).save(screenshot)
                result['screenshot'] = str(screenshot)
            except ImportError:
                pass
            user.PostMessageW(hwnd, 0x10, 0, 0)
            result['exit_code'] = process.wait(timeout=15)
            text = output.read_text(encoding='utf-8', errors='replace')
            assert result['exit_code'] == 0 and all(marker in text for marker in
                ('GTK_CSD_NATIVE_DRAG_TEXT_SELECTION_OK', 'GTK_WHEEL_SCROLL_OK', 'GTK_KEYBOARD_MODIFIERS_OK',
                 'GTK_CONTEXT_MENU_OK', 'GTK_POPUP_ANCHOR_AND_ACTION_OK')), text
            result['status'] = 'passed'
        except (AssertionError, OSError, subprocess.TimeoutExpired) as exception:
            result['reason'] = str(exception)
        finally:
            user.mouse_event(4, 0, 0, 0, 0)
            user.SetCursorPos(original_cursor.x, original_cursor.y)
            user.SetForegroundWindow(original_foreground)
            if process.poll() is None:
                subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], capture_output=True,
                               creationflags=subprocess.CREATE_NO_WINDOW, timeout=10)
                process.wait(timeout=10)
            desktop.kernel.CloseHandle(job)
    report.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, indent=2))
    return result['status'] != 'passed'

if __name__ == '__main__':
    raise SystemExit(main())
