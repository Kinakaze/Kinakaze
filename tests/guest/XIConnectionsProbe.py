"""XI2 routing and grab lifetime with independent GDK/backend connections."""
import ctypes as c
import time
x=c.CDLL('libX11.so.6');xi=c.CDLL('libXi.so.6');p=c.c_void_p;u=c.c_ulong;i=c.c_int

def api(lib,name,result,*args):
 f=getattr(lib,name);f.restype=result;f.argtypes=args;return f
class Mask(c.Structure):_fields_=[('device',i),('length',i),('mask',p)]
class Cookie(c.Structure):_fields_=[('kind',i),('serial',u),('send',i),('display',p),('extension',i),('evtype',i),('cookie',c.c_uint),('data',p)]
open_display=api(x,'XOpenDisplay',p,p);close=api(x,'XCloseDisplay',i,p)
a=open_display(None);b=open_display(None);assert a!=b
w=api(x,'XCreateSimpleWindow',u,p,u,i,i,c.c_uint,c.c_uint,c.c_uint,u,u)(a,1,1800,120,240,160,0,0,0)
api(x,'XStoreName',i,p,u,c.c_char_p)(a,w,b'XI connection regression')
pending=api(x,'XPending',i,p);next_event=api(x,'XNextEvent',i,p,p);free=api(x,'XFreeEventData',None,p,p)
select=api(xi,'XISelectEvents',i,p,u,p,i);selected=api(xi,'XIGetSelectedEvents',p,p,u,p)
bits=(c.c_ubyte*1)(0x40);mask=Mask(2,1,c.cast(bits,p));assert select(b,w,c.byref(mask),1)==0
count=i();assert not selected(a,w,c.byref(count)) and count.value==0
api(x,'XMapWindow',i,p,u)(a,w);api(x,'XSync',i,p,i)(a,0);time.sleep(.2)
def events(d):
 rows=[]
 while pending(d):
  e=(u*24)();next_event(d,e);cookie=c.cast(e,c.POINTER(Cookie)).contents
  if cookie.kind==35:
   rows.append(cookie.evtype);assert cookie.display==d;free(d,e)
 return rows
warp=api(x,'XWarpPointer',i,p,u,u,i,i,c.c_uint,c.c_uint,i,i)
events(a);events(b)
warp(a,0,w,0,0,0,0,30,40);time.sleep(.15)
assert not events(a),'first polling connection stole XI events'
assert 6 in events(b),'selecting connection lost mouse motion'
grab=api(xi,'XIGrabDevice',i,p,i,u,u,u,i,i,i,p)
assert grab(b,2,w,0,0,1,1,0,c.byref(mask))==0
close(a);a=None
warp(b,0,w,0,0,0,0,45,55);time.sleep(.15)
assert 6 in events(b),'closing another connection cleared XI selection/grab'
api(xi,'XIUngrabDevice',i,p,i,u)(b,2,0)
api(x,'XDestroyWindow',i,p,u)(b,w);close(b)
print('XI_CONNECTION_ROUTING_AND_GRAB_LIFETIME_OK',flush=True)
