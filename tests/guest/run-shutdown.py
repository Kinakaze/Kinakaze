"""Measure tray responsiveness, terminal closure and bounded process-tree cleanup."""
import argparse
import ctypes
from ctypes import wintypes
import importlib.util
import json
from pathlib import Path
import subprocess
import time
import urllib.request
import psutil

spec = importlib.util.spec_from_file_location('terminal_helpers', Path(__file__).with_name('run-default-sshd.py'))
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('dist', 'root', 'manifest', 'report'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    worker = str(args.dist.resolve() / 'worker.exe')
    source = json.loads(args.manifest.read_text(encoding='utf-8'))
    source['startup']['tray'] = True
    user = ctypes.WinDLL('user32', use_last_error=True)
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    user.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
    user.SendMessageTimeoutW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM,
                                        wintypes.LPARAM, wintypes.UINT, wintypes.UINT,
                                        ctypes.POINTER(ctypes.c_size_t)]
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
    result = {'status': 'failed', 'cases': []}

    def alive(identity):
        try:
            return psutil.Process(identity[0]).create_time() == identity[1]
        except psutil.NoSuchProcess:
            return False

    for case in ('normal', 'init_does_not_exit', 'broker_not_responding'):
        manifest = json.loads(json.dumps(source))
        if case == 'init_does_not_exit':
            manifest['startup']['shutdown'] = ['/bin/true']
        fixture = args.report.parent / ('shutdown-' + case + '.manifest.json')
        fixture.write_text(json.dumps(manifest), encoding='utf-8')
        options = ['--root', str(args.root.resolve()), '--rootfs-manifest', str(fixture.resolve())]
        term = None

        def run(arguments):
            return subprocess.check_output([worker, *arguments, *options], timeout=60,
                                           creationflags=subprocess.CREATE_NO_WINDOW)

        try:
            run(['session', 'start'])
            state = json.loads(run(['session', 'status']))
            bash = next(t for t in state['terminals'] if t['name'] == 'bash')
            with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(state['web'] + 'api/state', timeout=10) as response:
                web = json.load(response)
            owned = [(pid, psutil.Process(pid).create_time()) for pid in
                     [state['pid'], *[p['host_pid'] for p in web['processes']]]]
            windows = []

            @callback_type
            def enum(hwnd, _):
                pid = wintypes.DWORD()
                user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                name = ctypes.create_unicode_buffer(128)
                user.GetClassNameW(hwnd, name, 128)
                if pid.value == state['pid'] and name.value == 'Kinakaze.Environment.Tray':
                    windows.append(hwnd)
                return True

            deadline = time.monotonic() + 5
            while not windows and time.monotonic() < deadline:
                user.EnumWindows(enum, 0)
                time.sleep(.02)
            assert len(windows) == 1, windows
            term = helpers.Terminal([worker, 'session', 'attach', *options, '--name', 'bash'], args.report.parent)
            term.wait_for('Ctrl+]')
            term.send("printf '\\nSHUTDOWN_%s\\n' READY\r")
            term.wait_for('SHUTDOWN_READY')
            if case == 'broker_not_responding':
                # Stop only this fixture's guest session service after start has replied.
                subprocess.check_call([worker, 'session', 'start', *options, '--name', 'freeze', '--',
                                       '/bin/sh', '-c', f'sleep .2; kill -STOP {bash["ppid"]}'],
                                      creationflags=subprocess.CREATE_NO_WINDOW, timeout=20)
                time.sleep(.4)
            started = time.monotonic()
            assert user.PostMessageW(windows[0], 0x111, 101, 0)
            reply = ctypes.c_size_t()
            assert user.SendMessageTimeoutW(windows[0], 0, 0, 0, 2, 500, ctypes.byref(reply))
            tray_ms = round((time.monotonic() - started) * 1000, 1)
            deadline = started + 2
            while term.proc.isalive() and time.monotonic() < deadline:
                time.sleep(.01)
            assert not term.proc.isalive(), 'terminal did not close promptly'
            terminal_ms = round((time.monotonic() - started) * 1000, 1)
            deadline = started + manifest['startup']['shutdown_timeout_seconds'] + 2
            while any(alive(p) for p in owned) and time.monotonic() < deadline:
                time.sleep(.02)
            assert not any(alive(p) for p in owned), ('remaining owned processes', owned)
            tree_seconds = round(time.monotonic()-started, 3)
            term.finish(timeout=.1)
            assert 'environment shutting down' in term.text, term.text[-2000:]
            result['cases'].append(dict(case=case, tray_response_ms=tray_ms,
                                        terminal_close_ms=terminal_ms,
                                        tree_exit_seconds=tree_seconds))
        finally:
            if term:
                (args.report.parent / ('shutdown-' + case + '.log')).write_text(term.text, encoding='utf-8')
                term.close()
            subprocess.run([worker, 'session', 'stop', *options], capture_output=True, timeout=10,
                           creationflags=subprocess.CREATE_NO_WINDOW)
            args.report.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    result['status'] = 'passed'
    args.report.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
