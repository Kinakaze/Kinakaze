"""Real-process inotify and epoll open-description inheritance."""
import argparse
import json
import uuid
import time
import ctypes as c
from ctypes import wintypes as w
import psutil
from pathlib import Path
from init_pool import InitPool

parser = argparse.ArgumentParser(description=__doc__)
for name in ('root', 'dist', 'output'):
    parser.add_argument('--' + name, type=Path, required=True)
parser.add_argument('--modes', nargs='+', default=['inotify-fork', 'inotify-memory', 'inotify-rights', 'inotify-exec', 'epoll-shared', 'epoll-race', 'epoll-rights', 'epoll-threaded-fork', 'epoll-remote-eventfd', 'epoll-remote-inotify', 'epoll-remote-netlink', 'epoll-only-rights'])
args = parser.parse_args()
source = (Path(__file__).resolve().parents[1] / 'tests/guest/SharedWatchProbe.py').read_text(encoding='utf-8')
rows = []
with InitPool(args.root, args.dist, args.output, size=1, timeout=60) as pool:
    for mode in args.modes:
        path = '/var/tmp/shared-watch-' + uuid.uuid4().hex
        row = pool.run(['/usr/bin/python3', '-c', source, mode, path, source], expect=['PASS_' + mode])
        row['name'] = mode
        rows.append(row)
        (args.output / 'result.json').write_text(json.dumps(rows, indent=2), encoding='utf-8')
        assert row['status'] == 'passed', row
    # Actual native resources must disappear even while the caller remains
    # alive, and after a process dies without running any close hooks.
    native = psutil.Process(pool.child.process.pid)
    kernel = c.WinDLL('kernel32', use_last_error=True)
    kernel.GetProcessId.argtypes = [w.HANDLE]
    kernel.GetProcessId.restype = w.DWORD
    for mode in ('inotify-last-close', 'inotify-crash-owner'):
        path = '/var/tmp/shared-watch-' + uuid.uuid4().hex
        with pool.lock:
            offset = len(pool.buffers[0])
        before = dict(threads=native.num_threads(), handles=native.num_handles())
        application = pool.launch(['/usr/bin/python3', '-c', source, mode, path, source])
        deadline = time.monotonic() + 10
        while True:
            with pool.lock:
                ready = b'WATCH_BATCH_READY' in pool.buffers[0][offset:]
            if ready:
                break
            assert time.monotonic() < deadline, mode
            time.sleep(.01)
        peak = dict(threads=native.num_threads(), handles=native.num_handles())
        assert peak['threads'] >= before['threads'] + 48, (before, peak)
        if mode == 'inotify-last-close':
            (args.root / path.lstrip('/') / 'close').touch()
        else:
            psutil.Process(kernel.GetProcessId(application.sample.handle)).kill()
            application.wait()
        started = time.monotonic()
        while True:
            after = dict(threads=native.num_threads(), handles=native.num_handles())
            if after['threads'] <= before['threads'] + 6 and after['handles'] <= before['handles'] + 20:
                break
            assert time.monotonic() - started < 8, (mode, before, peak, after)
            time.sleep(.01)
        if mode == 'inotify-last-close':
            (args.root / path.lstrip('/') / 'stop').touch()
            assert application.wait() == 0
        else:
            (args.root / path.lstrip('/')).rmdir()
        rows.append(dict(name=mode, status='passed', before=before, peak=peak, after=after,
                         cleanup_ms=round((time.monotonic() - started) * 1000, 1)))
        (args.output / 'result.json').write_text(json.dumps(rows, indent=2), encoding='utf-8')
    path = '/var/tmp/shared-clock-' + uuid.uuid4().hex
    with pool.lock:
        offset = len(pool.buffers[0])
    application = pool.launch(['/usr/bin/python3', '-c', source, 'timer-rights', path, source])
    deadline = time.monotonic() + 10
    while True:
        with pool.lock:
            ready = b'TIMER_CLOCK_READY' in pool.buffers[0][offset:]
        if ready:
            break
        assert time.monotonic() < deadline, 'timer owner did not exit'
        time.sleep(.01)
    user = c.WinDLL('user32', use_last_error=True)
    callback_type = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    user.EnumWindows.argtypes = [callback_type, w.LPARAM]
    user.GetClassNameW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
    user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
    user.SendMessageTimeoutW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM, w.UINT, w.UINT, c.POINTER(c.c_size_t)]
    windows = []
    @callback_type
    def find_receiver(window, _):
        name = c.create_unicode_buffer(256)
        user.GetClassNameW(window, name, len(name))
        if name.value.startswith('KinakazeClockChange'):
            pid = w.DWORD()
            user.GetWindowThreadProcessId(window, c.byref(pid))
            if pool.child.owns_process(pid.value):
                windows.append(window)
        return True
    deadline = time.monotonic() + 5
    while True:
        windows.clear()
        user.EnumWindows(find_receiver, 0)
        if windows or time.monotonic() >= deadline:
            break
        # A surviving receiver takes over asynchronously when its owner exits.
        time.sleep(.01)
    assert len(windows) == 1, f'queued timer has {len(windows)} clock-change receivers in its session'
    result = c.c_size_t()
    # Deliver the notification to this test's hidden receiver only. The host
    # clock and other sessions are never changed.
    assert user.SendMessageTimeoutW(windows[0], 0x1E, 0, 0, 2, 2000, c.byref(result))
    (args.root / path.lstrip('/')).touch()
    assert application.wait() == 0
    rows.append(dict(name='timer-rights', status='passed'))
    (args.output / 'result.json').write_text(json.dumps(rows, indent=2), encoding='utf-8')
print(json.dumps(dict(passed=True, checks=[r['name'] for r in rows]), indent=2))
