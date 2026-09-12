"""Real libXfixes client: version, selections, cursor pixels/names and regions."""
import ctypes as c
p,u,i=c.c_void_p,c.c_ulong,c.c_int
def bind(lib,name,result,*args):
    f=getattr(lib,name);f.restype,f.argtypes=result,args;return f
x=c.CDLL('libX11.so.6'); f=c.CDLL('libXfixes.so.3')
class Rect(c.Structure):
    _fields_=[('x',c.c_short),('y',c.c_short),('width',c.c_ushort),('height',c.c_ushort)]
class Selection(c.Structure):
    _fields_=[('type',i),('serial',u),('send',i),('display',p),('window',u),('subtype',i),('owner',u),('selection',u),('time',u),('selection_time',u)]
class Cursor(c.Structure):
    _fields_=[('x',c.c_short),('y',c.c_short),('width',c.c_ushort),('height',c.c_ushort),('xhot',c.c_ushort),('yhot',c.c_ushort),('serial',u),('pixels',c.POINTER(u)),('atom',u),('name',c.c_char_p)]
d=bind(x,'XOpenDisplay',p,p)(None);assert d
errors=[]
@c.CFUNCTYPE(i,p,p)
def on_error(display,event):
    errors.append(bytes((c.c_ubyte*40).from_address(event)));return 0
bind(x,'XSetErrorHandler',p,p)(c.cast(on_error,p))
sync=bind(x,'XSync',i,p,i);free=bind(x,'XFree',i,p)
w=bind(x,'XCreateSimpleWindow',u,p,u,i,i,c.c_uint,c.c_uint,c.c_uint,u,u)(d,1,0,0,80,60,0,0,0)
major,minor=i(5),i()
assert bind(f,'XFixesQueryVersion',i,p,c.POINTER(i),c.POINTER(i))(d,c.byref(major),c.byref(minor)) and major.value==5
atom=bind(x,'XInternAtom',u,p,c.c_char_p,i)(d,b'KINAKAZE_XFIXES_PROBE',0)
bind(f,'XFixesSelectSelectionInput',None,p,u,u,u)(d,w,atom,7);sync(d,0)
bind(x,'XSetSelectionOwner',i,p,u,u,u)(d,atom,w,0);sync(d,0)
pending=bind(x,'XPending',i,p);next_event=bind(x,'XNextEvent',i,p,p)
found=False
while pending(d):
    e=(u*24)();next_event(d,e);s=c.cast(e,c.POINTER(Selection)).contents
    if s.type==92:
        assert (s.window,s.owner,s.selection,s.subtype)==(w,w,atom,0);found=True
assert found,'selection ownership notification missing'
cursor=bind(x,'XCreateFontCursor',u,p,c.c_uint)(d,68);assert cursor
bind(x,'XDefineCursor',i,p,u,u)(d,1,cursor)
bind(f,'XFixesSetCursorName',None,p,u,c.c_char_p)(d,cursor,b'probe-cursor')
name_atom=u();name=bind(f,'XFixesGetCursorName',p,p,u,c.POINTER(u))(d,cursor,c.byref(name_atom))
assert name and c.string_at(name)==b'probe-cursor' and name_atom.value;free(name)
image=bind(f,'XFixesGetCursorImage',c.POINTER(Cursor),p)(d);assert image
assert image.contents.width>0 and image.contents.height>0 and image.contents.pixels
assert any(image.contents.pixels[n]>>24 for n in range(image.contents.width*image.contents.height));free(image)
hide=bind(f,'XFixesHideCursor',None,p,u);show=bind(f,'XFixesShowCursor',None,p,u)
hide(d,w);hide(d,w);show(d,w);show(d,w);sync(d,0)
create=bind(f,'XFixesCreateRegion',u,p,c.POINTER(Rect),i)
r=create(d,(Rect*1)(Rect(10,20,4,6)),1);out=create(d,None,0)
bind(f,'XFixesExpandRegion',None,p,u,u,c.c_uint,c.c_uint,c.c_uint,c.c_uint)(d,out,r,1,2,3,4)
count=i();rects=bind(f,'XFixesFetchRegion',c.POINTER(Rect),p,u,c.POINTER(i))(d,out,c.byref(count))
assert count.value==1 and (rects[0].x,rects[0].y,rects[0].width,rects[0].height)==(9,17,7,13);free(rects)
shape=bind(f,'XFixesSetWindowShapeRegion',None,p,u,i,i,i,u)
shape(d,w,0,2,3,r);sync(d,0)
copied=bind(f,'XFixesCreateRegionFromWindow',u,p,u,i)(d,w,0)
rects=bind(f,'XFixesFetchRegion',c.POINTER(Rect),p,u,c.POINTER(i))(d,copied,c.byref(count))
assert count.value==1 and (rects[0].x,rects[0].y)==(12,23);free(rects)
shape(d,w,0,0,0,0);shape(d,w,2,0,0,out);shape(d,w,2,0,0,0)
barrier=bind(f,'XFixesCreatePointerBarrier',u,p,u,i,i,i,i,i,i,c.POINTER(i))(d,1,10,0,10,100,1,0,None);assert barrier
bind(f,'XFixesDestroyPointerBarrier',None,p,u)(d,barrier);sync(d,0)
for region in (r,out,copied):bind(f,'XFixesDestroyRegion',None,p,u)(d,region)
bind(x,'XFreeCursor',i,p,u)(d,cursor);bind(x,'XDestroyWindow',i,p,u)(d,w);sync(d,0)
assert not errors,errors
bind(x,'XCloseDisplay',i,p)(d)
print('XFIXES_SELECTION_CURSOR_SHAPE_BARRIER_OK',flush=True)
