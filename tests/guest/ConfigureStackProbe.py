"""XConfigureWindow sibling/conditional stacking and owned-window coordinates."""
import ctypes as c
import json
x=c.CDLL('libX11.so.6');p=c.c_void_p;i=c.c_int;u=c.c_uint;w=c.c_ulong
def bind(name,result,*args):
    f=getattr(x,name);f.restype=result;f.argtypes=args;return f
d=bind('XOpenDisplay',p,c.c_char_p)(None)
class Changes(c.Structure):
    _fields_=[('x',i),('y',i),('width',i),('height',i),('border_width',i),('sibling',w),('stack_mode',i)]
create=bind('XCreateSimpleWindow',w,p,w,i,i,u,u,u,w,w)
top=create(d,1,1200,650,400,300,0,0,0)
windows=[create(d,top,10,10,160,120,0,0,0) for _ in range(3)]
map_window=bind('XMapWindow',i,p,w)
for window in [top,*windows]:map_window(d,window)
errors=[]
callback=c.CFUNCTYPE(i,p,p)
@callback
def error(_display,event):errors.append(bytes(c.string_at(event,40)).hex());return 0
bind('XSetErrorHandler',p,p)(error)
configure=bind('XConfigureWindow',i,p,w,u,c.POINTER(Changes))
query=bind('XQueryTree',i,p,w,c.POINTER(w),c.POINTER(w),c.POINTER(c.POINTER(w)),c.POINTER(u))
def order():
    root=w();parent=w();children=c.POINTER(w)();count=u()
    assert query(d,top,c.byref(root),c.byref(parent),c.byref(children),c.byref(count))
    result=list(children[:count.value]);bind('XFree',i,p)(children);return result
def stack(target,mode,sibling=None,**geometry):
    changes=Changes(stack_mode=mode,sibling=sibling or 0,**geometry)
    mask=64|(32 if sibling else 0)|(1 if 'x' in geometry else 0)|(2 if 'y' in geometry else 0)
    assert configure(d,target,mask,c.byref(changes))
    bind('XSync',i,p,i)(d,0)
    assert not errors, errors
a,b,z=windows
stack(a,0,b);result=order();assert result.index(a)==result.index(b)+1,result
stack(a,1,b);result=order();assert result.index(a)+1==result.index(b),result
stack(a,2,b);assert order()[-1]==a
stack(a,3,b);assert order()[0]==a
stack(a,4,b);assert order()[-1]==a
stack(a,4,b);assert order()[0]==a
stack(a,2,b,x=230,y=160);assert order()[0]==a,'Nonoverlapping window incorrectly raised'
stack(a,2,b,x=10,y=10);assert order()[-1]==a,'Conditional mode ignored final geometry'
bind('XDestroyWindow',i,p,w)(d,top)
bind('XCloseDisplay',i,p)(d)
print('CONFIGURE_STACK_OK '+json.dumps(dict(checks=8,errors=errors)))
