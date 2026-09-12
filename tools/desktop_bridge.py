"""Session-scoped native window management for the hosted GNOME overview.

One authenticated loopback connection carries window deltas and actions. WinEvent
notifications drive enumeration; no guest processes are spawned to poll windows.
The Windows job, PID and HWND must all match before any window is manipulated.
"""
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import secrets
import select
import socket
import struct
import threading
import time

class CopyData(c.Structure):
    _fields_ = [('tag', c.c_size_t), ('size', w.DWORD), ('data', c.c_void_p)]

class DesktopBridge:
    def __init__(self, job, directory, guest_directory):
        self.job = job
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.guest_directory = guest_directory
        self.user = c.WinDLL('user32', use_last_error=True)
        self.kernel = c.WinDLL('kernel32', use_last_error=True)
        self._bind()
        self.listener = socket.socket()
        self.listener.bind(('127.0.0.1', 0))
        self.listener.listen(4)
        self.listener.setblocking(False)
        self.token = secrets.token_hex(32)
        self.config = dict(port=self.listener.getsockname()[1], token=self.token)
        self.windows = {}
        self.workspace = 0
        self.workspaces = 4
        self.overview = False
        self.native_previews = False
        self.desktop = None
        self.dirty = True
        self.clients = {}
        self.last_state = None
        self.stop_event = threading.Event()
        self.ready = threading.Event()
        self.thread = threading.Thread(target=self._run, name='desktop-window-bridge', daemon=True)

    def _bind(self):
        u, k = self.user, self.kernel
        self.enum_callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
        self.event_callback = c.WINFUNCTYPE(None, w.HANDLE, w.DWORD, w.HWND, w.LONG, w.LONG, w.DWORD, w.DWORD)
        for name, result, args in (
            ('EnumWindows', w.BOOL, [self.enum_callback, w.LPARAM]),
            ('GetWindowThreadProcessId', w.DWORD, [w.HWND,c.POINTER(w.DWORD)]),
            ('GetWindowTextW', c.c_int, [w.HWND,w.LPWSTR,c.c_int]),
            ('GetClassNameW', c.c_int, [w.HWND,w.LPWSTR,c.c_int]),
            ('GetWindowLongPtrW', c.c_ssize_t, [w.HWND,c.c_int]),
            ('GetWindow', w.HWND, [w.HWND,w.UINT]),
            ('GetPropW', w.HANDLE, [w.HWND,w.LPCWSTR]),
            ('SetPropW', w.BOOL, [w.HWND,w.LPCWSTR,w.HANDLE]),
            ('RemovePropW', w.HANDLE, [w.HWND,w.LPCWSTR]),
            ('GetWindowRect', w.BOOL, [w.HWND,c.POINTER(w.RECT)]),
            ('IsWindow', w.BOOL, [w.HWND]),
            ('IsWindowVisible', w.BOOL, [w.HWND]),
            ('IsIconic', w.BOOL, [w.HWND]),
            ('ShowWindow', w.BOOL, [w.HWND,c.c_int]),
            ('GetForegroundWindow', w.HWND, []),
            ('SetForegroundWindow', w.BOOL, [w.HWND]),
            ('SetFocus', w.HWND, [w.HWND]),
            ('AttachThreadInput', w.BOOL, [w.DWORD,w.DWORD,w.BOOL]),
            ('BringWindowToTop', w.BOOL, [w.HWND]),
            ('SetWindowPos', w.BOOL, [w.HWND,w.HWND,c.c_int,c.c_int,c.c_int,c.c_int,w.UINT]),
            ('PostMessageW', w.BOOL, [w.HWND,w.UINT,w.WPARAM,w.LPARAM]),
            ('SendMessageTimeoutW', c.c_ssize_t, [w.HWND,w.UINT,w.WPARAM,w.LPARAM,w.UINT,w.UINT,c.POINTER(c.c_size_t)]),
            ('SetWinEventHook', w.HANDLE, [w.DWORD,w.DWORD,w.HMODULE,self.event_callback,w.DWORD,w.DWORD,w.DWORD]),
            ('UnhookWinEvent', w.BOOL, [w.HANDLE]),
            ('PeekMessageW', w.BOOL, [c.POINTER(w.MSG),w.HWND,w.UINT,w.UINT,w.UINT]),
            ('DispatchMessageW', c.c_ssize_t, [c.POINTER(w.MSG)]),
        ):
            f = getattr(u, name); f.restype, f.argtypes = result, args
        k.OpenProcess.argtypes = [w.DWORD,w.BOOL,w.DWORD]; k.OpenProcess.restype = w.HANDLE
        k.IsProcessInJob.argtypes = [w.HANDLE,w.HANDLE,c.POINTER(w.BOOL)]
        k.CloseHandle.argtypes = [w.HANDLE]

    def _member(self, pid):
        process = self.kernel.OpenProcess(0x1000, False, pid)
        if not process:
            return False
        try:
            member = w.BOOL()
            return bool(self.kernel.IsProcessInJob(process, self.job, c.byref(member)) and member.value)
        finally:
            self.kernel.CloseHandle(process)

    def _valid(self, record):
        pid = w.DWORD()
        self.user.GetWindowThreadProcessId(record['hwnd'], c.byref(pid))
        return pid.value == record['pid'] and self._member(pid.value)

    def _enumerate(self):
        u = self.user
        found, membership = set(), {}
        @self.enum_callback
        def visit(hwnd, _):
            name = c.create_unicode_buffer(256)
            u.GetClassNameW(hwnd, name, len(name))
            if name.value != 'kinakaze.display.window': return True
            pid = w.DWORD(); u.GetWindowThreadProcessId(hwnd, c.byref(pid))
            if pid.value not in membership: membership[pid.value] = self._member(pid.value)
            if not membership[pid.value]: return True
            if u.GetPropW(hwnd, 'KinakazeDesktopSurface'):
                if u.IsWindowVisible(hwnd): self.desktop = hwnd
                return True
            if u.GetPropW(hwnd, 'KinakazeInputOnly'): return True
            previous = self.windows.get(hwnd)
            if previous and previous['pid'] != pid.value: previous = None
            if not u.IsWindowVisible(hwnd) and not (previous and previous['hidden']): return True
            rect = w.RECT(); u.GetWindowRect(hwnd, c.byref(rect))
            if rect.right-rect.left <= 32 or rect.bottom-rect.top <= 32: return True
            # Owned modal dialogs travel with their main window. Override-redirect
            # menus/tooltips are native tool windows and never overview entries.
            owner = u.GetWindow(hwnd, 4)
            if u.GetWindowLongPtrW(hwnd, -20) & 0x80 and not owner: return True
            if u.GetPropW(hwnd, 'KinakazeOverrideRedirect'): return True
            title = c.create_unicode_buffer(1024); u.GetWindowTextW(hwnd,title,len(title))
            if not title.value or title.value in ('mutter guard window', 'X11 Window'): return True
            found.add(hwnd)
            record = previous or dict(hwnd=hwnd, pid=pid.value, workspace=self.workspace,
                                      hidden=False, minimized=bool(u.IsIconic(hwnd)), preview=None, placed=False)
            record.update(title=title.value, bounds=[rect.left,rect.top,rect.right,rect.bottom], owner=owner)
            if not record['hidden']: record['minimized'] = bool(u.IsIconic(hwnd))
            self.windows[hwnd] = record
            return True
        u.EnumWindows(visit, 0)
        for hwnd in list(self.windows):
            if hwnd not in found:
                record = self.windows.pop(hwnd)
                if record.get('preview'):
                    (self.directory / Path(record['preview']).name).unlink(missing_ok=True)
        for record in self.windows.values():
            owner = self.windows.get(record['owner'])
            if owner: record['workspace'] = owner['workspace']
            if not record['placed'] and self.desktop:
                bounds=w.RECT()
                if u.GetWindowRect(self.desktop,c.byref(bounds)):
                    left,top,right,bottom=record['bounds']
                    # Root-positioned GTK windows otherwise cover Activities and
                    # the workspace selector. Preserve explicit fullscreen sizes.
                    fullscreen = right-left >= bounds.right-bounds.left and bottom-top >= bounds.bottom-bounds.top
                    if not fullscreen and bounds.left <= left < bounds.right and bounds.top <= top < bounds.top+32:
                        top=bounds.top+32
                        u.SetWindowPos(record['hwnd'],None,left,top,0,0,0x15)
                        record['bounds']=[left,top,right,top+bottom-record['bounds'][1]]
                    record['placed']=True
        self._visibility()

    def _visibility(self):
        u = self.user
        for record in self.windows.values():
            hidden = self.overview or record['workspace'] != self.workspace
            if hidden == record['hidden'] or not self._valid(record): continue
            hwnd = record['hwnd']
            if hidden:
                u.SetPropW(hwnd, 'KinakazeWorkspaceHidden', 1)
                u.ShowWindow(hwnd, 0)
            else:
                u.RemovePropW(hwnd, 'KinakazeWorkspaceHidden')
                u.ShowWindow(hwnd, 7 if record['minimized'] else 4)
            record['hidden'] = hidden

    def _focus(self, hwnd):
        if not hwnd: return
        u = self.user
        if u.GetForegroundWindow() == hwnd: return
        pid = w.DWORD(); u.GetWindowThreadProcessId(hwnd,c.byref(pid))
        if not self._member(pid.value): return
        current = self.kernel.GetCurrentThreadId()
        threads = {u.GetWindowThreadProcessId(hwnd,None),
                   u.GetWindowThreadProcessId(u.GetForegroundWindow(),None)} - {0,current}
        attached = [thread for thread in threads if u.AttachThreadInput(current,thread,True)]
        try:
            u.BringWindowToTop(hwnd)
            u.SetForegroundWindow(hwnd)
            u.SetFocus(hwnd)
        finally:
            for thread in attached: u.AttachThreadInput(current,thread,False)

    def _capture(self):
        if self._thumbnails([]):
            self.native_previews = True
            return
        # Capture only when entering overview; retain the last preview for hidden
        # workspaces. GDI reads the application's retained frame, not the screen.
        from PIL import Image
        g = c.WinDLL('gdi32'); u = self.user
        u.GetWindowDC.argtypes=[w.HWND];u.GetWindowDC.restype=w.HDC
        u.ReleaseDC.argtypes=[w.HWND,w.HDC]
        u.PrintWindow.argtypes=[w.HWND,w.HDC,w.UINT]
        g.CreateCompatibleDC.argtypes=[w.HDC];g.CreateCompatibleDC.restype=w.HDC
        g.CreateCompatibleBitmap.argtypes=[w.HDC,c.c_int,c.c_int];g.CreateCompatibleBitmap.restype=w.HBITMAP
        g.SelectObject.argtypes=[w.HDC,w.HANDLE];g.SelectObject.restype=w.HANDLE
        g.GetDIBits.argtypes=[w.HDC,w.HBITMAP,w.UINT,w.UINT,c.c_void_p,c.c_void_p,w.UINT]
        g.DeleteObject.argtypes=[w.HANDLE];g.DeleteDC.argtypes=[w.HDC]
        for r in self.windows.values():
            if r['hidden'] or r['minimized'] or not self._valid(r): continue
            left,top,right,bottom = r['bounds']; width,height=right-left,bottom-top
            if width*height > 16_000_000: continue
            dc=u.GetWindowDC(r['hwnd']); mem=g.CreateCompatibleDC(dc)
            bitmap=g.CreateCompatibleBitmap(dc,width,height); old=g.SelectObject(mem,bitmap)
            try:
                if not u.PrintWindow(r['hwnd'],mem,2): continue
                # BITMAPINFOHEADER, top-down 32-bit DIB.
                import struct
                info=c.create_string_buffer(struct.pack('<IiiHHIIiiII',40,width,-height,1,32,0,0,0,0,0,0))
                data=c.create_string_buffer(width*height*4)
                if not g.GetDIBits(mem,bitmap,0,height,data,info,0):continue
                image=Image.frombuffer('RGB',(width,height),data.raw,'raw','BGRX',0,1)
                image.thumbnail((480,300), Image.Resampling.BILINEAR)
                # Some GL windows have no readable GDI frame while unmapped.
                # Retain the last real preview instead of replacing it with black.
                if image.getbbox() is None: continue
                name=f'window-{r["hwnd"]}-{time.monotonic_ns()}.png'
                image.save(self.directory/name,compress_level=1)
                previous=r['preview'];r['preview']=self.guest_directory+'/'+name
                if previous:(self.directory/Path(previous).name).unlink(missing_ok=True)
            finally:
                g.SelectObject(mem,old);g.DeleteObject(bitmap);g.DeleteDC(mem);u.ReleaseDC(r['hwnd'],dc)

    def _thumbnails(self, items):
        if not self.desktop: return False
        payload = bytearray(struct.pack('<II', 1, len(items)))
        for item in items:
            record = self.windows.get(int(item['id']))
            if not record or not self._valid(record) or record['workspace'] != self.workspace: return False
            rect = item.get('rect', [])
            if len(rect) != 4 or any(not isinstance(v, (int, float)) or not -32768 <= v <= 32767 for v in rect): return False
            payload.extend(struct.pack('<Qiiii', record['hwnd'], *(round(v) for v in rect)))
        data = c.create_string_buffer(bytes(payload))
        request = CopyData(0x43595448, len(payload), c.cast(data, c.c_void_p))
        result = c.c_size_t()
        return bool(self.user.SendMessageTimeoutW(self.desktop,0x4a,0,c.addressof(request),2,80,c.byref(result)) and result.value)

    def _command(self, command):
        op = command.get('op')
        if op == 'release':
            self._thumbnails([])
            self.overview = False
            for record in self.windows.values(): record['workspace'] = self.workspace
            self._visibility()
        elif op == 'overview':
            enabled = command.get('enabled') is True
            if not enabled: self._thumbnails([])
            if enabled and not self.overview:
                self._capture()
            self.overview = enabled
            self._visibility()
            if enabled: self._focus(self.desktop)
        elif op == 'workspace':
            self._thumbnails([])
            index = command.get('index')
            if not isinstance(index,int) or not 0 <= index < self.workspaces: raise ValueError('Invalid workspace')
            self.workspace = index; self._visibility()
            self._focus(self.desktop)
        elif op in ('activate','close','move','minimize'):
            record = self.windows.get(int(command.get('id',0)))
            if not record or not self._valid(record): raise ValueError('Window no longer belongs to session')
            if op == 'activate':
                self._thumbnails([])
                self.workspace=record['workspace'];self.overview=False;record['minimized']=False
                self._visibility(); self.user.ShowWindow(record['hwnd'],9);self._focus(record['hwnd'])
            elif op == 'close': self.user.PostMessageW(record['hwnd'],0x10,0,0)
            elif op == 'minimize':
                record['minimized']=True
                if not record['hidden']: self.user.ShowWindow(record['hwnd'],6)
            else:
                index = command.get('index')
                if not isinstance(index,int) or not 0 <= index < self.workspaces:raise ValueError('Invalid workspace')
                owner = record['owner'] if record['owner'] in self.windows else record['hwnd']
                for item in self.windows.values():
                    if item['hwnd']==owner or item['owner']==owner:item['workspace']=index
                self._visibility()
        elif op == 'thumbnails':
            items = command.get('items', [])
            if not isinstance(items, list) or len(items) > 64: raise ValueError('Invalid thumbnail layout')
            if self.overview: self._thumbnails(items)
            return
        elif op != 'refresh': raise ValueError('Unknown desktop command')
        self.dirty = True

    def _snapshot(self):
        foreground = self.user.GetForegroundWindow()
        return dict(type='state',workspace=self.workspace,workspaces=self.workspaces,overview=self.overview,
                    nativePreviews=self.native_previews,
                    windows=[dict(id=str(r['hwnd']),pid=r['pid'],title=r['title'],workspace=r['workspace'],
                                  minimized=r['minimized'],active=r['hwnd']==foreground,preview=r['preview'])
                             for r in self.windows.values()])

    def start(self):
        self.thread.start()
        if not self.ready.wait(5): raise RuntimeError('Desktop bridge did not start')

    def close(self):
        self.stop_event.set()
        if self.thread.is_alive(): self.thread.join(timeout=5)
        self.listener.close()

    def _drop_client(self, connection):
        connection.close()
        client = self.clients.pop(connection,None)
        if client and client['authenticated'] and not any(item['authenticated'] for item in self.clients.values()):
            self._command({'op': 'release'})

    def _run(self):
        u = self.user
        @self.event_callback
        def changed(_hook,_event,hwnd,obj,_child,_thread,_time):
            if not hwnd or obj != 0: return
            if hwnd in self.windows or hwnd == self.desktop:
                self.dirty = True
            elif _event == 3:
                # One foreground delta clears the previous app's active flag.
                self.dirty = True
            else:
                name = c.create_unicode_buffer(64)
                u.GetClassNameW(hwnd,name,len(name))
                if name.value == 'kinakaze.display.window': self.dirty = True
        hooks=[u.SetWinEventHook(0x8000,0x800c,None,changed,0,0,2),
               u.SetWinEventHook(3,3,None,changed,0,0,2)]
        self.ready.set(); last_scan=0
        try:
            while not self.stop_event.is_set():
                message=w.MSG()
                while u.PeekMessageW(c.byref(message),None,0,0,1):u.DispatchMessageW(c.byref(message))
                for connection in select.select([self.listener,*self.clients],[],[],0.04)[0]:
                    if connection is self.listener:
                        client,_=self.listener.accept(); client.settimeout(.2)
                        self.clients[client]=dict(buffer=b'',authenticated=False);continue
                    try:
                        data=connection.recv(65536)
                        if not data:raise ConnectionError()
                        client=self.clients[connection];client['buffer']+=data
                        if len(client['buffer'])>65536:raise ValueError('Request too large')
                        while b'\n' in client['buffer']:
                            line,client['buffer']=client['buffer'].split(b'\n',1);request=json.loads(line)
                            if not client['authenticated']:
                                if not secrets.compare_digest(str(request.get('token','')),self.token):raise ValueError('Authentication failed')
                                client['authenticated']=True
                                connection.sendall((json.dumps(self._snapshot(),ensure_ascii=False)+'\n').encode())
                                self.dirty=True
                            else:
                                try:self._command(request)
                                except (ValueError,TypeError) as error:
                                    connection.sendall((json.dumps(dict(type='error',message=str(error)))+'\n').encode())
                    except (OSError,ValueError,ConnectionError):
                        self._drop_client(connection)
                now=time.monotonic()
                if self.dirty and now-last_scan >= .08:
                    self.dirty=False;last_scan=now;self._enumerate()
                    state=json.dumps(self._snapshot(),ensure_ascii=False,separators=(',',':'))
                    if state!=self.last_state:
                        self.last_state=state
                        for connection,client in list(self.clients.items()):
                            if not client['authenticated']:continue
                            try:connection.sendall((state+'\n').encode())
                            except OSError:self._drop_client(connection)
        finally:
            # Fail open: disabling the bridge must never strand hidden apps.
            self._thumbnails([])
            self.overview=False
            for record in self.windows.values():record['workspace']=self.workspace
            self._visibility()
            for hook in hooks:
                if hook:u.UnhookWinEvent(hook)
            for connection in self.clients:connection.close()
