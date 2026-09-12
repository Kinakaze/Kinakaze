"""SHAPE state, boolean operations and notifications across Display/process boundaries."""
import ctypes as c
import select
import subprocess
import sys
import time
p,u,i=c.c_void_p,c.c_ulong,c.c_int
x=c.CDLL('libX11.so.6');s=c.CDLL('libXext.so.6')
def bind(lib,name,result,*args):
 f=getattr(lib,name);f.restype=result;f.argtypes=args;return f
class Rect(c.Structure):_fields_=[('x',c.c_short),('y',c.c_short),('width',c.c_ushort),('height',c.c_ushort)]
class Event(c.Structure):_fields_=[('type',i),('serial',u),('sent',i),('display',p),('window',u),('kind',i),('x',i),('y',i),('width',c.c_uint),('height',c.c_uint),('time',u),('shaped',i)]
open_display=bind(x,'XOpenDisplay',p,p);d=open_display(None);assert d
rectangles=bind(s,'XShapeCombineRectangles',None,p,u,i,i,i,c.POINTER(Rect),i,i,i)
fetch=bind(s,'XShapeGetRectangles',c.POINTER(Rect),p,u,i,c.POINTER(i),c.POINTER(i))
free=bind(x,'XFree',i,p);pending=bind(x,'XPending',i,p);next_event=bind(x,'XNextEvent',i,p,p)
if len(sys.argv)>1:
 window=int(sys.argv[1]);rectangles(d,window,0,0,0,(Rect*1)(Rect(4,8,60,40)),1,0,0)
 bind(x,'XCloseDisplay',i,p)(d);sys.exit(0)
base=i();error=i();assert bind(s,'XShapeQueryExtension',i,p,p,p)(d,c.byref(base),c.byref(error))
other=open_display(None);assert other
window=bind(x,'XCreateSimpleWindow',u,p,u,i,i,c.c_uint,c.c_uint,c.c_uint,u,u)(d,1,40,40,160,100,0,0,0)
assert window
subscribe=bind(s,'XShapeSelectInput',None,p,u,u);subscribe(d,window,1)
selected=bind(s,'XShapeInputSelected',u,p,u);assert selected(d,window)==1 and selected(other,window)==0

def read(display,kind=0):
 count=i();order=i();result=fetch(display,window,kind,c.byref(count),c.byref(order))
 try:return [(result[n].x,result[n].y,result[n].width,result[n].height) for n in range(count.value)]
 finally:free(result)
def events(display):
 result=[]
 while pending(display):
  data=(u*24)();next_event(display,data);event=c.cast(data,c.POINTER(Event)).contents
  if event.type==base.value:result.append((event.window,event.kind,event.x,event.y,event.width,event.height,event.shaped))
 return result
rectangles(d,window,0,0,0,(Rect*1)(Rect(0,0,80,100)),1,0,0)
assert read(other)==[(0,0,80,100)]
assert events(d)==[(window,0,0,0,80,100,1)];assert not events(other)
child=subprocess.Popen([sys.executable,__file__,str(window)]);assert child.wait(timeout=15)==0
fd=bind(x,'XConnectionNumber',i,p)(d);deadline=time.monotonic()+3;got=[]
while not got and time.monotonic()<deadline:
 select.select([fd],[],[],max(0,deadline-time.monotonic()));got=events(d)
assert got==[(window,0,4,8,60,40,1)],got
assert read(other)==[(4,8,60,40)]
rectangles(d,window,0,0,0,(Rect*1)(Rect(14,18,10,10)),1,3,0)
assert sum(w*h for _,_,w,h in read(other))==2300
bind(s,'XShapeOffsetShape',None,p,u,i,i,i)(d,window,0,3,5)
assert min(r[0] for r in read(other))==7 and min(r[1] for r in read(other))==13
bind(s,'XShapeCombineMask',None,p,u,i,i,i,u,i)(d,window,0,0,0,0,0)
assert read(other)==[(0,0,160,100)]
rectangles(d,window,2,0,0,None,0,0,0);assert read(other,2)==[]
subscribe(d,window,0);events(d)
rectangles(other,window,0,0,0,(Rect*1)(Rect(0,0,60,40)),1,0,0);assert not events(d)
bind(x,'XDestroyWindow',i,p,u)(d,window)
bind(x,'XCloseDisplay',i,p)(other);bind(x,'XCloseDisplay',i,p)(d)
print('SHAPE_CROSS_PROCESS_CONNECTION_BOOLEAN_NOTIFY_OK',flush=True)
