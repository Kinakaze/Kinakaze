"""Check guest/host taskbar transitions without changing Windows preferences."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import time

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--desktop', required=True, type=int)
    parser.add_argument('--application', required=True, type=int, action='append')
    parser.add_argument('--report', required=True, type=Path)
    args = parser.parse_args()
    u, k, shell = c.WinDLL('user32'), c.WinDLL('kernel32'), c.WinDLL('shell32')
    u.GetForegroundWindow.restype = w.HWND
    u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
    u.SetForegroundWindow.argtypes = [w.HWND]
    u.SetWindowPos.argtypes = [w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT]
    u.GetPropW.argtypes = [w.HWND, w.LPCWSTR]; u.GetPropW.restype = w.HANDLE
    u.FindWindowW.argtypes = [w.LPCWSTR, w.LPCWSTR]; u.FindWindowW.restype = w.HWND
    u.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
    u.WindowFromPoint.argtypes = [w.POINT]; u.WindowFromPoint.restype = w.HWND
    u.GetAncestor.argtypes = [w.HWND, w.UINT]; u.GetAncestor.restype = w.HWND
    u.GetWindow.argtypes = [w.HWND, w.UINT]; u.GetWindow.restype = w.HWND
    u.CreateWindowExW.argtypes = [w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD,
        c.c_int, c.c_int, c.c_int, c.c_int, w.HWND, w.HMENU, w.HINSTANCE, c.c_void_p]
    u.CreateWindowExW.restype = w.HWND
    u.DestroyWindow.argtypes = [w.HWND]; u.ShowWindow.argtypes = [w.HWND, c.c_int]
    u.DefWindowProcW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
    u.DefWindowProcW.restype = c.c_ssize_t
    procedure = c.WINFUNCTYPE(c.c_ssize_t, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
    @procedure
    def callback(hwnd, message, wp, lp):
        return u.DefWindowProcW(hwnd, message, wp, lp)
    class Class(c.Structure):
        _fields_ = [('size', w.UINT), ('style', w.UINT), ('proc', procedure),
            ('ce', c.c_int), ('we', c.c_int), ('instance', w.HINSTANCE), ('icon', w.HICON),
            ('cursor', w.HANDLE), ('background', w.HBRUSH), ('menu', w.LPCWSTR),
            ('name', w.LPCWSTR), ('small_icon', w.HICON)]
    k.GetModuleHandleW.restype = w.HMODULE
    cls = Class(); cls.size = c.sizeof(cls); cls.proc = callback
    cls.instance = k.GetModuleHandleW(None); cls.name = 'KinakazeTaskbarTestHost'; cls.background = 6
    assert u.RegisterClassExW(c.byref(cls))
    host = u.CreateWindowExW(0x40000, cls.name, 'Kinakaze taskbar test', 0x00cf0000,
                            1700, 180, 280, 160, None, None, cls.instance, None)
    assert host
    tray = u.FindWindowW('Shell_TrayWnd', None); assert tray
    bounds = w.RECT(); assert u.GetWindowRect(tray, c.byref(bounds))
    point = w.POINT((bounds.left+bounds.right)//2, (bounds.top+bounds.bottom)//2)
    class Appbar(c.Structure):
        _fields_ = [('size', w.DWORD), ('hwnd', w.HWND), ('message', w.UINT),
                    ('edge', w.UINT), ('rect', w.RECT), ('parameter', w.LPARAM)]
    appbar = Appbar(); appbar.size = c.sizeof(appbar)
    shell.SHAppBarMessage.argtypes = [w.DWORD, c.POINTER(Appbar)]
    shell.SHAppBarMessage.restype = c.c_size_t
    auto_hide = bool(shell.SHAppBarMessage(4, c.byref(appbar)) & 1)
    message = w.MSG()
    u.DispatchMessageW.argtypes = [c.POINTER(w.MSG)]; u.DispatchMessageW.restype = c.c_ssize_t
    def pump(seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            while u.PeekMessageW(c.byref(message), None, 0, 0, 1):
                u.TranslateMessage(c.byref(message)); u.DispatchMessageW(c.byref(message))
            time.sleep(.005)
    def focus(hwnd):
        if hwnd == host: u.ShowWindow(host, 5)
        me = k.GetCurrentThreadId()
        threads = {u.GetWindowThreadProcessId(hwnd, None),
                   u.GetWindowThreadProcessId(u.GetForegroundWindow(), None)} - {me, 0}
        for tid in threads: u.AttachThreadInput(me, tid, 1)
        try:
            u.SetWindowPos(hwnd, None, 0, 0, 0, 0, 3)
            u.SetForegroundWindow(hwnd)
        finally:
            for tid in threads: u.AttachThreadInput(me, tid, 0)
        assert u.GetForegroundWindow() == hwnd
    result = dict(status='failed', host_auto_hide=auto_hide, transitions=[])
    def check(label, hwnd):
        pump(.3)
        hit = u.WindowFromPoint(point)
        shown = u.GetAncestor(hit, 2) == tray
        row = dict(target=label, foreground=u.GetForegroundWindow(), taskbar_visible=shown,
                   hint=u.GetPropW(hwnd, 'KinakazeFullscreenSession'))
        result['transitions'].append(row)
        assert row['foreground'] == hwnd, row
        if hwnd == args.desktop:
            above, cursor = [], u.GetWindow(hwnd, 3)  # GW_HWNDPREV
            while cursor:
                above.append(cursor); cursor = u.GetWindow(cursor, 3)
            assert all(app in above for app in args.application), ('desktop covered an app', above)
        if hwnd != host: assert not shown and row['hint'] == 1, row
        elif not auto_hide: assert shown, row
    try:
        targets = [*args.application, args.desktop, args.application[0], host, args.application[-1]]
        for index, hwnd in enumerate(targets):
            focus(hwnd); check(str(index), hwnd)
        for index in range(3):
            for hwnd in targets:
                focus(hwnd); pump(.025)
            check('rapid-' + str(index), targets[-1])
        focus(host); check('host-final', host)
        focus(args.application[0]); check('guest-return', args.application[0])
        result['status'] = 'passed'
    except Exception as error:
        result['reason'] = repr(error)
    finally:
        u.DestroyWindow(host)
    args.report.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))
    return result['status'] != 'passed'

if __name__ == '__main__':
    raise SystemExit(main())
