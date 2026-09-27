"""Windows tray, WebUI and forced-exit cleanup for the owned test environment."""
import argparse
import ctypes
from ctypes import wintypes
import json
from pathlib import Path
import subprocess
import time
import urllib.request
import psutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('dist', 'root', 'manifest', 'report'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    command = [str(args.dist.resolve() / 'worker.exe'), 'session']
    options = ['--root', str(args.root.resolve()), '--rootfs-manifest', str(args.manifest.resolve())]
    checks = []

    def run(action):
        p = subprocess.run(command + [action, *options], capture_output=True, timeout=35, creationflags=subprocess.CREATE_NO_WINDOW)
        if p.returncode:
            raise AssertionError(p.stderr.decode(errors='replace'))
        return json.loads(p.stdout) if action == 'status' else None

    def boot():
        run('start')
        state = run('status')
        with urllib.request.urlopen(state['web'] + 'api/state', timeout=10) as response:
            web = json.load(response)
        assert web['desktop']['ready']
        children = [(p['host_pid'], psutil.Process(p['host_pid']).create_time()) for p in web['processes']]
        return state, web, children

    def dead(pid, birth):
        try:
            return psutil.Process(pid).create_time() != birth
        except psutil.NoSuchProcess:
            return True

    def gone(state, children):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            if not psutil.pid_exists(state['pid']) and all(dead(pid, birth) for pid, birth in children):
                return
            time.sleep(.05)
        raise AssertionError(('surviving tree', state['pid'], [p for p in children if not dead(*p)]))

    try:
        state, web, children = boot()
        user = ctypes.WinDLL('user32', use_last_error=True)
        user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
        user.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
        user.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
        windows = []
        callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
        @callback_type
        def callback(hwnd, _):
            pid = wintypes.DWORD()
            user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            name = ctypes.create_unicode_buffer(128)
            user.GetClassNameW(hwnd, name, 128)
            if pid.value == state['pid'] and name.value == 'Kinakaze.Environment.Tray':
                windows.append(hwnd)
            return True
        user.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
        deadline = time.monotonic() + 5
        while not windows and time.monotonic() < deadline:
            user.EnumWindows(callback, 0)
            time.sleep(.05)
        assert len(windows) == 1, windows
        assert user.PostMessageW(windows[0], 0x111, 101, 0)  # The actual tray Stop command.
        gone(state, children)
        checks.append({'tray_window_and_stop_command': True, 'webui_live_tree': True})

        state, web, children = boot()
        psutil.Process(state['pid']).kill()
        gone(state, children)
        checks.append({'manager_crash_kills_workers': len(children)})

        # Reusing a stale descriptor must start a new generation, never attach
        # to a recycled host PID or retain the former boot's terminals.
        state, web, children = boot()
        root = next(p for p in web['processes'] if p['identity']['pid'] == 1)
        psutil.Process(root['host_pid']).kill()
        gone(state, children)
        checks.append({'pid1_death_closes_tree': True, 'stale_descriptor_recovery': True})
        result = {'status': 'passed', 'checks': checks}
        args.report.write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps(result, indent=2))
    finally:
        subprocess.run(command + ['stop', *options], capture_output=True, timeout=40, creationflags=subprocess.CREATE_NO_WINDOW)


if __name__ == '__main__':
    main()
