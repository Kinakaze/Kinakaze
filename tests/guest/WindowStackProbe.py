"""Already mapped windows must be raised on MapRaised and activation requests."""
import ctypes as c
x = c.CDLL('libX11.so.6')
p, u, i = c.c_void_p, c.c_ulong, c.c_int
def bind(name, result, *args):
    f = getattr(x,name); f.restype, f.argtypes = result,args; return f
d = bind('XOpenDisplay',p,c.c_char_p)(None)
create = bind('XCreateSimpleWindow',u,p,u,i,i,c.c_uint,c.c_uint,c.c_uint,u,u)
first, second = create(d,1,40,40,200,100,0,0,0), create(d,1,60,60,200,100,0,0,0)
assert first and second
tree = bind('XQueryTree',i,p,u,p,p,p,p)
def above(a,b):
    root,parent,children,count = u(),u(),p(),c.c_uint()
    assert tree(d,1,c.byref(root),c.byref(parent),c.byref(children),c.byref(count))
    values = list(c.cast(children,c.POINTER(u))[:count.value])
    bind('XFree',i,p)(children)
    assert values.index(a) > values.index(b), values
try:
    mapped = bind('XMapRaised',i,p,u)
    assert mapped(d,first) and mapped(d,second)
    above(second,first)
    assert mapped(d,first)
    above(first,second)
    class Message(c.Structure):
        _fields_=[('kind',i),('serial',u),('send',i),('display',p),('window',u),('message_type',u),('format',i),('data',c.c_long*5)]
    event=(u*24)()
    message=c.cast(event,c.POINTER(Message)).contents
    message.kind=33; message.display=d; message.window=second; message.format=32
    message.message_type=bind('XInternAtom',u,p,c.c_char_p,i)(d,b'_NET_ACTIVE_WINDOW',0)
    message.data[0]=1
    assert bind('XSendEvent',i,p,u,i,c.c_long,p)(d,1,0,(1<<20)|(1<<19),event)
    above(second,first)
    print('MAPPED_WINDOW_RAISE_ACTIVATION_STACKING_OK',flush=True)
finally:
    destroy=bind('XDestroyWindow',i,p,u)
    destroy(d,first);destroy(d,second)
    bind('XCloseDisplay',i,p)(d)
