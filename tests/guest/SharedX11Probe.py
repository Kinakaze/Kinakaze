"""Independent ELF processes share window state, events and live Composite pixels."""
import ctypes as c
import json
import os
import subprocess
import sys
import time
from pathlib import Path
x=c.CDLL('libX11.so.6');p=c.c_void_p;u=c.c_ulong;i=c.c_int
def api(lib,name,result,*args):
 f=getattr(lib,name);f.restype=result;f.argtypes=args;return f
d=api(x,'XOpenDisplay',p,p)(None)
create=api(x,'XCreateSimpleWindow',u,p,u,i,i,c.c_uint,c.c_uint,c.c_uint,u,u)
atom=api(x,'XInternAtom',u,p,c.c_char_p,i)
change=api(x,'XChangeProperty',i,p,u,u,u,i,i,p,i)
select=api(x,'XSelectInput',i,p,u,u)
sync=api(x,'XSync',i,p,i)
pending=api(x,'XPending',i,p);next_event=api(x,'XNextEvent',i,p,p)
def events():
 result=[]
 while pending(d):
  e=(u*24)();next_event(d,e);result.append(list(e))
 return result
errors=[]
@c.CFUNCTYPE(i,p,p)
def error(display,event):errors.append(c.string_at(event,40).hex());return 0
api(x,'XSetErrorHandler',p,p)(error)
if len(sys.argv)>1:
 atom(d,b'PRODUCER_DIFFERENT_ORDER',0)
 a=atom(d,b'KINAKAZE_SHARED_TEST',0)
 w=create(d,1,150,150,320,240,0,0,0)
 api(x,'XStoreName',i,p,u,c.c_char_p)(d,w,b'Shared producer')
 select(d,w,(1<<22)|(1<<17)|(1<<15))
 change(d,w,a,31,8,0,c.c_char_p(b'from producer'),13)
 api(x,'XMapWindow',i,p,u)(d,w)
 gc=api(x,'XCreateGC',p,p,u,u,p)(d,w,0,None)
 def draw(color):
  api(x,'XSetForeground',i,p,p,u)(d,gc,color)
  api(x,'XFillRectangle',i,p,u,p,i,i,c.c_uint,c.c_uint)(d,w,gc,0,0,320,240);sync(d,0)
 draw(0x3377bb)
 print(json.dumps(dict(window=w,atom=a)),flush=True)
 for line in sys.stdin:
  command=json.loads(line)
  if command['op']=='draw':draw(command['color']);print('{}',flush=True)
  elif command['op']=='events':print(json.dumps(events()),flush=True)
  elif command['op']=='resize':api(x,'XResizeWindow',i,p,u,c.c_uint,c.c_uint)(d,w,400,260);draw(0xabcdef);print('{}',flush=True)
  elif command['op']=='destroy':api(x,'XDestroyWindow',i,p,u)(d,w);print('{}',flush=True);break
  elif command['op']=='selection':api(x,'XSetSelectionOwner',i,p,u,u,u)(d,command['atom'],w,0);print('{}',flush=True)
 api(x,'XCloseDisplay',i,p)(d);sys.exit(0)

checks=[];timings={}
def check(name,condition):
 assert condition,(name,errors)
 checks.append(name)
atom(d,b'CONSUMER_DIFFERENT_ORDER',0)
a=atom(d,b'KINAKAZE_SHARED_TEST',0)
select(d,1,(1<<19)|(1<<20))
child=subprocess.Popen(['/usr/bin/python3.11',__file__,'producer'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
def command(**kw):
 child.stdin.write(json.dumps(kw)+'\n');child.stdin.flush();return json.loads(child.stdout.readline())
try:
 ready=json.loads(child.stdout.readline());w=ready['window']
 check('global atom identity',ready['atom']==a)
 root=u();parent=u();children=c.POINTER(u)();count=c.c_uint()
 api(x,'XQueryTree',i,p,u,c.POINTER(u),c.POINTER(u),c.POINTER(c.POINTER(u)),c.POINTER(c.c_uint))(d,1,c.byref(root),c.byref(parent),c.byref(children),c.byref(count))
 check('foreign root child',w in children[:count.value]);api(x,'XFree',i,p)(children)
 found=[];deadline=time.monotonic()+3
 while time.monotonic()<deadline:
  found+=events()
  if any(e[0]&0xffffffff==20 and e[5]==w for e in found):break
  time.sleep(.01)
 check('cross-process MapRequest',any(e[0]&0xffffffff==20 and e[5]==w for e in found))
 api(x,'XMapWindow',i,p,u)(d,w);sync(d,0)
 typ=u();fmt=i();n=u();after=u();data=p()
 api(x,'XGetWindowProperty',i,p,u,u,c.c_long,c.c_long,i,u,c.POINTER(u),c.POINTER(i),c.POINTER(u),c.POINTER(u),c.POINTER(p))(d,w,a,0,1024,0,0,c.byref(typ),c.byref(fmt),c.byref(n),c.byref(after),c.byref(data))
 check('foreign property bytes',c.string_at(data,n.value)==b'from producer');api(x,'XFree',i,p)(data)
 select(d,w,1<<22)
 forked=os.fork()
 if forked==0:
  try:
   get_result=x.XGetWindowProperty(d,w,a,0,1024,0,0,c.byref(typ),c.byref(fmt),c.byref(n),c.byref(after),c.byref(data))
   ok=get_result==0 and c.string_at(data,n.value)==b'from producer'
   os._exit(0 if ok else 72)
  except BaseException:os._exit(73)
 check('fork preserves foreign window subscription',os.waitpid(forked,0)==(forked,0))
 getprop=x.XGetWindowProperty;free=x.XFree
 for batch in range(3):
  start=time.perf_counter()
  for _ in range(1000):
   getprop(d,w,a,0,1024,0,0,c.byref(typ),c.byref(fmt),c.byref(n),c.byref(after),c.byref(data));free(data)
  timings.setdefault('foreign_property_1000_ms',[]).append(round((time.perf_counter()-start)*1000,3))
 change(d,w,a,31,8,0,c.c_char_p(b'consumer'),8);sync(d,0);time.sleep(.03)
 check('foreign PropertyNotify',any(e[0]&0xffffffff==28 and e[4]==w and e[5]==a for e in command(op='events')))
 selection=atom(d,b'KINAKAZE_SHARED_SELECTION',0);command(op='selection',atom=selection)
 check('shared selection owner',api(x,'XGetSelectionOwner',u,p,u)(d,selection)==w)
 comp=c.CDLL('libXcomposite.so.1')
 api(comp,'XCompositeRedirectWindow',None,p,u,i)(d,w,1);sync(d,0)
 pixmap=api(comp,'XCompositeNameWindowPixmap',u,p,u)(d,w);sync(d,0)
 def color(drawable):
  image=api(x,'XGetImage',p,p,u,i,i,c.c_uint,c.c_uint,u,i)(d,drawable,0,0,1,1,0xffffffff,2)
  assert image,errors
  value=api(x,'XGetPixel',u,p,i,i)(image,0,0)&0xffffff
  api(x,'XDestroyImage',i,p)(image);return value
 actual=color(pixmap)
 check('foreign Composite frame '+hex(actual),actual==0x3377bb)
 name=comp.XCompositeNameWindowPixmap;freepix=api(x,'XFreePixmap',i,p,u)
 for batch in range(3):
  start=time.perf_counter()
  for _ in range(500):freepix(d,name(d,w))
  timings.setdefault('name_foreign_surface_500_ms',[]).append(round((time.perf_counter()-start)*1000,3))
 command(op='draw',color=0x44bb66)
 check('live shared pixels',color(pixmap)==0x44bb66)
 command(op='destroy');child.wait(timeout=5)
 check('pixmap survives owner exit',color(pixmap)==0x44bb66)
 check('dead selection owner cleared',api(x,'XGetSelectionOwner',u,p,u)(d,selection)==0)
 check('no X protocol errors',not errors)
 print('SHARED_X11_OK '+json.dumps(dict(checks=checks,timings=timings)),flush=True)
finally:
 if child.poll() is None:child.terminate();child.wait(timeout=5)
 api(x,'XCloseDisplay',i,p)(d)
