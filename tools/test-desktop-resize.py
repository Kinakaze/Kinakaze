"""Windows host test. Uses physical mouse input only over its own GTK fixture.

Requires tools/run-gnome.py and the resident desktop-test-driver.py. The probe
closes only its own window; run while the desktop is available for input tests.
"""
import ctypes as c,json,time
from ctypes import wintypes as w
from pathlib import Path
u=c.WinDLL('user32');u.GetWindowTextW.argtypes=[w.HWND,w.LPWSTR,c.c_int];u.GetClassNameW.argtypes=[w.HWND,w.LPWSTR,c.c_int];u.GetWindowThreadProcessId.argtypes=[w.HWND,c.POINTER(w.DWORD)];u.GetForegroundWindow.restype=w.HWND;u.SetForegroundWindow.argtypes=[w.HWND];u.GetPropW.argtypes=[w.HWND,w.LPCWSTR];u.GetPropW.restype=w.HANDLE;u.GetWindowRect.argtypes=[w.HWND,c.POINTER(w.RECT)];u.IsWindowVisible.argtypes=[w.HWND]
def windows():
 rows=[]
 @c.WINFUNCTYPE(w.BOOL,w.HWND,w.LPARAM)
 def each(h,l):
  s=c.create_unicode_buffer(256);u.GetClassNameW(h,s,256)
  if s.value!='kinakaze.display.window':return True
  u.GetWindowTextW(h,s,256);pid=w.DWORD();u.GetWindowThreadProcessId(h,c.byref(pid));r=w.RECT();u.GetWindowRect(h,c.byref(r))
  rows.append(dict(hwnd=h,pid=pid.value,title=s.value,visible=bool(u.IsWindowVisible(h)),rect=[r.left,r.top,r.right,r.bottom]));return True
 u.EnumWindows(each,0)
 return rows

import argparse, shutil
parser = argparse.ArgumentParser(description='Exercise all resize edges, release, and Escape in a live GNOME session.')
parser.add_argument('--root', type=Path, required=True, help='Prepared guest root with the resident desktop test driver running')
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
args.output.parent.mkdir(parents=True, exist_ok=True)
shutil.copyfile(Path(__file__).resolve().parents[1] / 'tests/guest/GtkGrabRecoveryProbe.py', args.root / 'tmp/GtkGrabRecoveryProbe.py')
top_windows=windows
u.GetAncestor.argtypes=[w.HWND,w.UINT];u.GetAncestor.restype=w.HWND
def windows():
 rows=top_windows();known={r['hwnd'] for r in rows}
 @c.WINFUNCTYPE(w.BOOL,w.HWND,w.LPARAM)
 def child(h,l):
  if h in known:return True
  name=c.create_unicode_buffer(256);u.GetClassNameW(h,name,256)
  if name.value!='kinakaze.display.window':return True
  known.add(h);u.GetWindowTextW(h,name,256);pid=w.DWORD();u.GetWindowThreadProcessId(h,c.byref(pid));rect=w.RECT();u.GetWindowRect(h,c.byref(rect));rows.append(dict(hwnd=h,pid=pid.value,title=name.value,visible=bool(u.IsWindowVisible(h)),rect=[rect.left,rect.top,rect.right,rect.bottom]));return True
 for root in list(rows):u.EnumChildWindows(root['hwnd'],child,0)
 return rows

import uuid,os
folder=args.root / 'tmp/desktop-test-driver'
assert folder.is_dir(), 'resident desktop test driver is not running'
(folder.parent/'shell-test-command').write_text('desktop')
time.sleep(1)
end=time.monotonic()+12
while any(r['visible'] and r['title']=='Kinakaze GTK grab recovery probe' for r in windows()) and time.monotonic()<end:time.sleep(.05)
assert not any(r['visible'] and r['title']=='Kinakaze GTK grab recovery probe' for r in windows()),'previous owned probe still active'
job=folder/(uuid.uuid4().hex+'.json');tmp=job.with_suffix('.tmp');tmp.write_text(json.dumps({'argv':['/usr/bin/python3.11','/tmp/GtkGrabRecoveryProbe.py']}));os.replace(tmp,job)
end=time.monotonic()+25
while time.monotonic()<end:
 rows=windows();app=next((r for r in rows if r['visible'] and r['title']=='Kinakaze GTK grab recovery probe'),None)
 if app:break
 time.sleep(.02)
assert app,'probe did not map'
h=app['hwnd'];u.SetForegroundWindow(h);time.sleep(.2)
u.SetWindowPos.argtypes=[w.HWND,w.HWND,c.c_int,c.c_int,c.c_int,c.c_int,w.UINT]
u.WindowFromPoint.argtypes=[w.POINT];u.WindowFromPoint.restype=w.HWND
u.SendMessageW.argtypes=[w.HWND,w.UINT,w.WPARAM,w.LPARAM]
def bounds():
 r=w.RECT();u.GetWindowRect(h,c.byref(r));return r
u.GetPropW.argtypes=[w.HWND,w.LPCWSTR];u.GetPropW.restype=w.HANDLE
shell=next(r['hwnd'] for r in windows() if r['title']=='Kinakaze GNOME Shell')
def box():
 r=bounds();return [r.left,r.top,r.right,r.bottom]
def esc():
 assert u.GetAncestor(u.GetForegroundWindow(),2)==h, 'test fixture lost foreground'
 scan=u.MapVirtualKeyW(27,0)
 u.keybd_event(27,scan,0,0)
 u.keybd_event(27,scan,2,0)
reports=[]
try:
 for edge,hold in [('right',.05),('left',.05),('bottom',.05),('top',.05),('se',.05),('sw',.05),('ne',.05),('nw',.05),('quick',0),('quick',.005),('escape',.05)]:
  before=box();l,t,r,b=before
  x=l+7 if edge=='left' else l+12 if edge in ('sw','nw') else r-12 if edge in ('se','ne') else (l+r)//2 if edge in ('top','bottom') else r-7
  y=t+7 if edge=='top' else t+3 if edge in ('ne','nw') else b-7 if edge=='bottom' else b-3 if edge in ('se','sw','quick','escape') else (t+b)//2
  expected={'left':10,'right':11,'top':12,'bottom':15,'nw':13,'ne':14,'sw':16,'se':17,'quick':17,'escape':17}[edge]
  hit=u.SendMessageW(h,0x84,0,(x&65535)|((y&65535)<<16))
  assert hit==expected, ('resize hit',edge,hit,expected)
  assert u.GetAncestor(u.WindowFromPoint(w.POINT(x,y)),2)==h,('covered',edge)
  u.SetCursorPos(x,y);time.sleep(.06);u.mouse_event(2,0,0,0,0);time.sleep(hold)
  cancelled=None
  try:
   steps=1 if edge=='quick' else 12
   for n in range(1,steps+1):u.SetCursorPos(x+3*n,y+2*n);time.sleep(.008 if edge!='quick' else 0)
   if edge=='escape':
    during=box();esc();time.sleep(.2);cancelled=box();u.SetCursorPos(x+90,y+60);time.sleep(.15)
  finally:u.mouse_event(4,0,0,0,0)
  time.sleep(.15);released=box();mask=u.GetPropW(shell,'KinakazeCompositorGrab') or 0
  u.SetCursorPos(x+110,y+80);time.sleep(.12);after=box()
  report=dict(edge=edge,hold=hold,before=before,released=released,after=after,grab=mask,cancelled=cancelled)
  reports.append(report);print(report,flush=True)
  if mask:esc();time.sleep(.2)
finally:
 args.output.write_text(json.dumps(reports,indent=2),encoding='utf-8')
 u.PostMessageW(h,0x10,0,0)
assert all(s['grab']==0 and s['released']==s['after'] for s in reports),'capture persisted after release'
for sample in reports:
 edge=sample['edge']
 if edge in ('quick','escape'):continue
 before,after=sample['before'],sample['released']
 dw=(after[2]-after[0])-(before[2]-before[0])
 dh=(after[3]-after[1])-(before[3]-before[1])
 assert (dw != 0) == (edge not in ('top','bottom')), ('width',sample)
 assert (dh != 0) == (edge not in ('left','right')), ('height',sample)
assert reports[-1]['cancelled']==reports[-1]['before'],'Escape did not restore original geometry'
print('RESIZE_RELEASE_OK',flush=True)
