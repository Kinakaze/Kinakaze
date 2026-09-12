"""A growing InputOutput placeholder becomes an app; InputOnly remains a helper."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'tools'))
from session_process import SessionProcess

SOURCE = r'''
import ctypes as c, json, time
x = c.CDLL('libX11.so.6')
p, i, u, w = c.c_void_p, c.c_int, c.c_uint, c.c_ulong
def bind(name, result, *args):
    f = getattr(x, name); f.restype, f.argtypes = result, args; return f
d = bind('XOpenDisplay', p, c.c_char_p)(None)
create = bind('XCreateWindow', w, p, w, i, i, u, u, u, i, u, p, w, p)
windows = [create(d, 1, 100, 100, 1, 1, 0, 0, cls, None, 0, None) for cls in (1, 2)]
assert all(windows)
print(json.dumps(dict(phase='small', windows=windows)), flush=True)
time.sleep(1)
resize = bind('XResizeWindow', i, p, w, u, u)
for window in windows: assert resize(d, window, 320, 240)
bind('XSync', i, p, i)(d, 0)
print(json.dumps(dict(phase='large', windows=windows)), flush=True)
time.sleep(1)
destroy = bind('XDestroyWindow', i, p, w)
for window in windows: destroy(d, window)
bind('XCloseDisplay', i, p)(d)
'''

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', required=True, type=Path)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--report', required=True, type=Path)
    args = parser.parse_args()
    dist, root = args.dist.resolve(), args.root.resolve()
    u = c.WinDLL('user32')
    u.GetWindowLongPtrW.argtypes = [w.HWND, c.c_int]; u.GetWindowLongPtrW.restype = c.c_ssize_t
    u.GetPropW.argtypes = [w.HWND, w.LPCWSTR]; u.GetPropW.restype = w.HANDLE
    command = [str(dist/'worker.exe'), 'run', '--root', str(root), '--dist', str(dist),
               '--', '/usr/bin/python3.11', '-c', SOURCE]
    result = dict(status='failed', phases=[])
    with args.report.with_suffix('.stderr.log').open('wb') as err:
        session = SessionProcess(command, stdout=subprocess.PIPE, stderr=err)
        try:
            for expected in ('small', 'large'):
                row = json.loads(session.process.stdout.readline())
                assert row['phase'] == expected
                row['tool_flags'] = [bool(u.GetWindowLongPtrW(h, -20) & 0x80) for h in row['windows']]
                row['input_only'] = [bool(u.GetPropW(h, 'KinakazeInputOnly')) for h in row['windows']]
                result['phases'].append(row)
                assert row['input_only'] == [False, True], row
                assert row['tool_flags'] == ([True, True] if expected == 'small' else [False, True]), row
            assert session.process.wait(timeout=10) == 0
            result['status'] = 'passed'
        except Exception as error:
            result['reason'] = repr(error)
        finally:
            session.close()
    args.report.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))
    return result['status'] != 'passed'

if __name__ == '__main__':
    raise SystemExit(main())
