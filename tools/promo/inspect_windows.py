"""Inspect only the application's own video subjects and save a raw window capture."""
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
from PIL import ImageGrab

user = c.WinDLL('user32', use_last_error=True)
try:
    user.SetProcessDpiAwarenessContext(c.c_void_p(-4))
except Exception:
    pass
user.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
user.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
user.IsWindowVisible.argtypes = [w.HWND]
user.SetForegroundWindow.argtypes = [w.HWND]
user.MoveWindow.argtypes = [w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.BOOL]
user.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
user.SendMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]


def windows():
    found = []
    @c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    def callback(hwnd, _):
        name = c.create_unicode_buffer(512)
        user.GetWindowTextW(hwnd, name, 512)
        if user.IsWindowVisible(hwnd) and any(word in name.value.lower() for word in ('minecraft', 'gnome', 'kinakaze promo')):
            rect = w.RECT()
            user.GetWindowRect(hwnd, c.byref(rect))
            pid = w.DWORD()
            user.GetWindowThreadProcessId(hwnd, c.byref(pid))
            found.append(dict(hwnd=hwnd, title=name.value, pid=pid.value,
                              rect=[rect.left, rect.top, rect.right, rect.bottom]))
        return True
    user.EnumWindows(callback, 0)
    return found


if __name__ == '__main__':
    out = Path('artifacts/promo-v1/captures')
    out.mkdir(parents=True, exist_ok=True)
    found = windows()
    print(json.dumps(found, ensure_ascii=False))
    for item in found:
        if 'minecraft' in item['title'].lower():
            ImageGrab.grab(window=item['hwnd']).save(out / 'minecraft-current.png')
