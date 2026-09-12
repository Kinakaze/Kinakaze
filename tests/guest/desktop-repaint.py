"""Verify native exposure restores Xlib/XCB pixels without a resize."""
import argparse
import ctypes as c
from ctypes import wintypes as w
import importlib.util
import json
from pathlib import Path
import sys
import time
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'tools'))
from session_process import SessionProcess

GUEST = r'''
import ctypes as c,sys,time
x=c.CDLL('libX11.so.6');p=c.c_void_p;i=c.c_int;u=c.c_uint;l=c.c_ulong
def api(name,result,*args):
 f=getattr(x,name);f.restype=result;f.argtypes=args;return f
d=api('XOpenDisplay',p,p)(None)
w=api('XCreateSimpleWindow',l,p,l,i,i,u,u,u,l,l)(d,1,90,90,360,240,0,0,0)
api('XStoreName',i,p,l,c.c_char_p)(d,w,('Repaint '+sys.argv[1]).encode())
api('XSelectInput',i,p,l,l)(d,w,(1<<15) if sys.argv[1]!='retained' else 0)
gc=api('XCreateGC',p,p,l,l,p)(d,w,0,None)
if sys.argv[1]=='redirected':
 co=c.CDLL('libXcomposite.so.1');co.XCompositeRedirectWindow.argtypes=[p,l,i]
 co.XCompositeRedirectWindow(d,w,1)
api('XMapWindow',i,p,l)(d,w)
api('XSetForeground',i,p,p,l)(d,gc,0x2673b8)
fill=api('XFillRectangle',i,p,l,p,i,i,u,u)
flush=api('XFlush',i,p)
fill(d,w,gc,0,0,360,240);flush(d)
if sys.argv[1]=='xcb':
 b=c.CDLL('libX11-xcb.so.1');b.XGetXCBConnection.argtypes=[p];b.XGetXCBConnection.restype=p
 connection=b.XGetXCBConnection(d)
 b.XSetEventQueueOwner.argtypes=[p,i];b.XSetEventQueueOwner.restype=None
 b.XSetEventQueueOwner(d,1)
 xc=c.CDLL('libxcb.so.1');xc.xcb_poll_for_event.argtypes=[p];xc.xcb_poll_for_event.restype=p
 free=c.CDLL('libc.so.6').free;free.argtypes=[p]
else: pending=api('XPending',i,p);next_event=api('XNextEvent',i,p,p)
while True:
 if sys.argv[1]=='xcb':
  event=xc.xcb_poll_for_event(connection)
  if event:
   kind=c.c_ubyte.from_address(event).value&127;free(event)
   if kind==12:print('EXPOSE',flush=True)
 else:
  while pending(d):
   event=(l*24)();next_event(d,event)
   if event[0]&0xffffffff==12:print('EXPOSE',flush=True)
 time.sleep(.005)
'''

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('dist','root','report'):parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--mode',choices=('xlib','retained','xcb','redirected'))
    args=parser.parse_args();dist=args.dist.resolve();root=args.root.resolve()
    spec=importlib.util.spec_from_file_location('desktop_window',Path(__file__).with_name('desktop-window.py'))
    m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
    desktop=m.Desktop(dist/'worker.exe');u=desktop.user;g=c.WinDLL('gdi32')
    def api(lib,name,result,*types):
        f=getattr(lib,name);f.restype=result;f.argtypes=types;return f
    getdc=api(u,'GetDC',w.HDC,w.HWND);release=api(u,'ReleaseDC',i:=c.c_int,w.HWND,w.HDC)
    pixel=api(g,'GetPixel',w.DWORD,w.HDC,i,i)
    brush=api(g,'CreateSolidBrush',w.HANDLE,w.DWORD)
    fill=api(u,'FillRect',i,w.HDC,c.POINTER(w.RECT),w.HANDLE)
    delete=api(g,'DeleteObject',w.BOOL,w.HANDLE)
    invalidate=api(u,'InvalidateRect',w.BOOL,w.HWND,c.POINTER(w.RECT),w.BOOL)
    position=api(u,'SetWindowPos',w.BOOL,w.HWND,w.HWND,i,i,i,i,w.UINT)
    show=api(u,'ShowWindow',w.BOOL,w.HWND,i)
    bounds=api(u,'GetClientRect',w.BOOL,w.HWND,c.POINTER(w.RECT))
    def color(hwnd):
        dc=getdc(hwnd)
        try:return pixel(dc,150,100)
        finally:release(hwnd,dc)
    logs=args.report.with_suffix('');logs.mkdir(parents=True,exist_ok=True)
    result=dict(status='failed',checks=[])
    try:
        for mode in ([args.mode] if args.mode else ('xlib','retained','xcb','redirected')):
            outpath=logs/(mode+'.stdout.log')
            with outpath.open('wb') as out,(logs/(mode+'.stderr.log')).open('wb') as err:
                session=SessionProcess([str(dist/'worker.exe'),'run','--root',str(root),'--dist',str(dist),'--',
                    '/usr/bin/python3.11','-c',GUEST,mode],stdout=out,stderr=err)
                try:
                    deadline=time.monotonic()+15;hwnd=None
                    while time.monotonic()<deadline:
                        assert session.process.poll() is None, (logs/(mode+'.stderr.log')).read_text(errors='replace')
                        hwnd=next((h for h,p,t in desktop.windows() if t=='Repaint '+mode and desktop.belongs(p,session.job)),None)
                        if hwnd:break
                        time.sleep(.02)
                    assert hwnd,'fixture window missing'
                    position(hwnd,-1,90,90,0,0,0x11)
                    while color(hwnd)!=0xb87326 and time.monotonic()<deadline:time.sleep(.01)
                    assert color(hwnd)==0xb87326,'initial pixels missing'
                    time.sleep(.1)
                    rect=w.RECT();bounds(hwnd,c.byref(rect));initial=(rect.right,rect.bottom)
                    for stage in ('invalidate','hide-show','minimize-restore'):
                        time.sleep(.35)
                        assert session.process.poll() is None, (logs/(mode+'.stderr.log')).read_text(errors='replace')
                        before=outpath.read_text().count('EXPOSE')
                        dc=getdc(hwnd);b=brush(0xff00ff)
                        fill(dc,c.byref(rect),b);g.GdiFlush();delete(b);release(hwnd,dc)
                        assert color(hwnd)==0xff00ff,'failed to invalidate visible pixels'
                        start=time.monotonic()
                        if stage=='hide-show':show(hwnd,0);show(hwnd,4)
                        elif stage=='minimize-restore':show(hwnd,6);show(hwnd,9)
                        else:invalidate(hwnd,None,False)
                        # A second, live desktop may activate as this test hides.
                        # Raise only this owned fixture; never alter its geometry.
                        position(hwnd,-1,0,0,0,0,0x13)
                        while color(hwnd)!=0xb87326 and time.monotonic()-start<2:time.sleep(.005)
                        actual=color(hwnd)
                        if actual!=0xb87326:
                            print(dict(pixel=hex(actual),visible=u.IsWindowVisible(hwnd),alive=session.process.poll(),bounds=[rect.left,rect.top,rect.right,rect.bottom]),flush=True)
                            from PIL import ImageGrab
                            ImageGrab.grab(window=hwnd).save(logs/'failed-window.png')
                        assert actual==0xb87326,f'{mode} {stage}: requires resize to repaint (pixel={actual:#x})'
                        bounds(hwnd,c.byref(rect));assert (rect.right,rect.bottom)==initial
                        if mode!='retained':
                            until=time.monotonic()+.3
                            while outpath.read_text().count('EXPOSE')==before and time.monotonic()<until:time.sleep(.005)
                            assert outpath.read_text().count('EXPOSE')>before,'ExposureMask event missing'
                        result['checks'].append(dict(mode=mode,stage=stage,ms=round((time.monotonic()-start)*1000,2)))
                finally:session.close()
        result['status']='passed'
    finally:
        args.report.write_text(json.dumps(result,indent=2));print(json.dumps(result))

if __name__=='__main__':main()
