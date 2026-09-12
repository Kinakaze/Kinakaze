"""Mutter's background pointer grab receives motion over a foreign app HWND."""
import ctypes as c
import subprocess,sys,time
x=c.CDLL('libX11.so.6');xi=c.CDLL('libXi.so.6');p=c.c_void_p;u=c.c_ulong;i=c.c_int

def api(lib,name,result,*args):
 f=getattr(lib,name);f.restype=result;f.argtypes=args;return f
class Mask(c.Structure):_fields_=[('device',i),('length',i),('mask',p)]
class Cookie(c.Structure):_fields_=[('kind',i),('serial',u),('send',i),('display',p),('extension',i),('evtype',i),('cookie',c.c_uint),('data',p)]
d=api(x,'XOpenDisplay',p,p)(None)
w=api(x,'XCreateSimpleWindow',u,p,u,i,i,c.c_uint,c.c_uint,c.c_uint,u,u)(d,1,1500 if len(sys.argv)>1 else 1200,100,240,180,0,0,0)
api(x,'XStoreName',i,p,u,c.c_char_p)(d,w,b'Cross process pointer regression')
api(x,'XClearWindow',i,p,u)(d,w);api(x,'XMapWindow',i,p,u)(d,w);api(x,'XSync',i,p,i)(d,0)
if len(sys.argv)>1:
 print(w,flush=True);sys.stdin.readline();sys.exit(0)
child=subprocess.Popen(['/usr/bin/python3.11',__file__,'child'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
try:
 foreign=int(child.stdout.readline());time.sleep(.3);print('grab',w,'foreign',foreign,flush=True)
 bits=(c.c_ubyte*1)(0x70);mask=Mask(2,1,c.cast(bits,p))
 assert api(xi,'XIGrabDevice',i,p,i,u,u,u,i,i,i,p)(d,2,w,0,0,1,1,0,c.byref(mask))==0
 api(x,'XWarpPointer',i,p,u,u,i,i,c.c_uint,c.c_uint,i,i)(d,0,foreign,0,0,0,0,50,60)
 pending=api(x,'XPending',i,p);next_event=api(x,'XNextEvent',i,p,p);free=api(x,'XFreeEventData',None,p,p)
 received=[];deadline=time.monotonic()+2
 while time.monotonic()<deadline:
  while pending(d):
   e=(u*24)();next_event(d,e);cookie=c.cast(e,c.POINTER(Cookie)).contents
   if cookie.kind==35:
    received.append(cookie.evtype);free(d,e)
  if 6 in received:break
  time.sleep(.01)
 assert 6 in received,('foreign foreground HWND retained captured motion',received)
 print('XI_CROSS_PROCESS_POINTER_GRAB_OK',flush=True)
finally:
 api(xi,'XIUngrabDevice',i,p,i,u)(d,2,0)
 child.stdin.write('done\n');child.stdin.flush();child.wait(timeout=10)
 api(x,'XDestroyWindow',i,p,u)(d,w);api(x,'XCloseDisplay',i,p)(d)
