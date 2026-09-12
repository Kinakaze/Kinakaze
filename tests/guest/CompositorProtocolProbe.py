"""Exercise real ELF XDamage/XFixes clients and GLX texture readback."""
import ctypes as c
p,u,i=c.c_void_p,c.c_ulong,c.c_int
def bind(lib,name,result,*args):
    f=getattr(lib,name);f.restype,f.argtypes=result,args;return f
x=c.CDLL('libX11.so.6');fix=c.CDLL('libXfixes.so.3');damage=c.CDLL('libXdamage.so.1');gl=c.CDLL('libGL.so.1')
get_proc=bind(gl,'glXGetProcAddress',p,c.c_char_p)
for name in ('glXDestroyContext','glXSwapBuffers','glXIsDirect','glXGetFBConfigAttrib','glXCreateWindow','glXDestroyWindow','glXCreatePixmap','glXDestroyPixmap','glXCreateNewContext','glXMakeContextCurrent','glXSelectEvent','glXGetFBConfigs','glXChooseFBConfig','glXGetVisualFromFBConfig'):
    assert get_proc(name.encode()),name
class Rect(c.Structure):
    _fields_=[('x',c.c_short),('y',c.c_short),('width',c.c_ushort),('height',c.c_ushort)]
class DamageEvent(c.Structure):
    _fields_=[('type',i),('serial',u),('send',i),('display',p),('drawable',u),('damage',u),('level',i),('more',i),('timestamp',u),('area',Rect),('geometry',Rect)]
open_display=bind(x,'XOpenDisplay',p,c.c_char_p);free=bind(x,'XFree',i,p)
create=bind(x,'XCreateSimpleWindow',u,p,u,i,i,c.c_uint,c.c_uint,c.c_uint,u,u)
sync=bind(x,'XSync',i,p,i);next_event=bind(x,'XNextEvent',i,p,p);pending=bind(x,'XPending',i,p)
gc_new=bind(x,'XCreateGC',p,p,u,u,p);fg=bind(x,'XSetForeground',i,p,p,u)
fill=bind(x,'XFillRectangle',i,p,u,p,i,i,c.c_uint,c.c_uint)
region_new=bind(fix,'XFixesCreateRegion',u,p,c.POINTER(Rect),i)
region_fetch=bind(fix,'XFixesFetchRegion',c.POINTER(Rect),p,u,c.POINTER(i))
subtract=bind(fix,'XFixesSubtractRegion',None,p,u,u,u)
destroy_region=bind(fix,'XFixesDestroyRegion',None,p,u)
damage_new=bind(damage,'XDamageCreate',u,p,u,i)
damage_sub=bind(damage,'XDamageSubtract',None,p,u,u,u)
d=open_display(None);assert d
w=create(d,1,0,0,32,24,0,0,0);assert w
g=gc_new(d,w,0,None);assert g
major,minor=i(),i()
for lib,name in [(fix,'XFixesQueryVersion'),(damage,'XDamageQueryVersion')]:
    assert bind(lib,name,i,p,c.POINTER(i),c.POINTER(i))(d,c.byref(major),c.byref(minor))
a=region_new(d,(Rect*1)(Rect(0,0,32,24)),1)
b=region_new(d,(Rect*1)(Rect(8,6,16,12)),1)
r=region_new(d,None,0);subtract(d,r,a,b)
count=i();rects=region_fetch(d,r,c.byref(count));assert rects
assert sum(rects[n].width*rects[n].height for n in range(count.value))==32*24-16*12
free(rects)
watch=damage_new(d,w,3);assert watch
sync(d,0)
def damage_events():
    found=[]
    while pending(d):
        buf=(u*24)();next_event(d,c.byref(buf))
        e=c.cast(buf,c.POINTER(DamageEvent)).contents
        if e.type==96:found.append((e.drawable,e.damage,e.area.x,e.area.y,e.area.width,e.area.height))
    return found
assert any(e[:2]==(w,watch) for e in damage_events())
damage_sub(d,watch,0,r);sync(d,0)
rects=region_fetch(d,r,c.byref(count));assert count.value and rects;free(rects)
fg(d,g,0xff0000);fill(d,w,g,4,3,8,6);sync(d,0)
assert any(e[:2]==(w,watch) for e in damage_events())
damage_sub(d,watch,0,r);sync(d,0)
rects=region_fetch(d,r,c.byref(count));assert count.value and rects
assert sum(rects[n].width*rects[n].height for n in range(count.value))==48
free(rects)
print('DAMAGE_EVENT_AND_REGION_SUBTRACT_OK',flush=True)
choose=bind(gl,'glXChooseVisual',p,p,i,c.POINTER(i))
context_new=bind(gl,'glXCreateContext',p,p,p,p,i)
make_current=bind(gl,'glXMakeCurrent',i,p,u,p)
visual=choose(d,0,(i*3)(4,5,0));assert visual
ctx=context_new(d,visual,None,1);assert ctx and make_current(d,w,ctx)
renderer=bind(gl,'glGetString',c.c_char_p,c.c_uint)(0x1f01);assert renderer
pixmap=bind(x,'XCreatePixmap',u,p,u,c.c_uint,c.c_uint,c.c_uint)(d,w,4,4,24);assert pixmap
fg(d,g,0x12ab34);fill(d,pixmap,g,0,0,4,4);sync(d,0)
gp=bind(gl,'glXCreatePixmap',u,p,p,u,c.POINTER(i))(d,p(1),pixmap,(i*5)(0x20d5,0x20d9,0x20d6,0x20dc,0));assert gp
tex=c.c_uint();bind(gl,'glGenTextures',None,i,c.POINTER(c.c_uint))(1,c.byref(tex))
bind(gl,'glBindTexture',None,c.c_uint,c.c_uint)(0xde1,tex)
bindtex=bind(gl,'glXBindTexImageEXT',None,p,u,i,c.POINTER(i))
gettex=bind(gl,'glGetTexImage',None,c.c_uint,i,c.c_uint,c.c_uint,p)
geterror=bind(gl,'glGetError',c.c_uint)
for color in (0x12ab34,0x5634ef):
    fg(d,g,color);fill(d,pixmap,g,0,0,4,4);sync(d,0)
    bindtex(d,gp,0x20de,None)
    pixels=(c.c_uint*16)();gettex(0xde1,0,0x80e1,0x1401,pixels)
    assert geterror()==0
    assert all(value&0xffffff==color for value in pixels),[hex(v) for v in pixels]
    bind(gl,'glXReleaseTexImageEXT',None,p,u,i)(d,gp,0x20de)
bind(x,'XFreePixmap',i,p,u)(d,pixmap)
bindtex(d,gp,0x20de,None)
gettex(0xde1,0,0x80e1,0x1401,pixels)
assert geterror()==0 and all(value&0xffffff==0x5634ef for value in pixels)
bind(gl,'glXDestroyPixmap',None,p,u)(d,gp)
# Redirected GPU clients must update the named XComposite surface at swap.
composite=c.CDLL('libXcomposite.so.1')
bind(composite,'XCompositeRedirectWindow',None,p,u,i)(d,w,1)
sync(d,0)
clear_color=bind(gl,'glClearColor',None,c.c_float,c.c_float,c.c_float,c.c_float)
clear=bind(gl,'glClear',None,c.c_uint)
pixel_store=bind(gl,'glPixelStorei',None,c.c_uint,i)
get_integer=bind(gl,'glGetIntegerv',None,c.c_uint,c.POINTER(i))
swap=bind(gl,'glXSwapBuffers',None,p,u)
root=u();gx=i();gy=i();width=c.c_uint();height=c.c_uint();border=c.c_uint();depth=c.c_uint()
assert bind(x,'XGetGeometry',i,p,u,p,p,p,p,p,p,p)(d,w,c.byref(root),c.byref(gx),c.byref(gy),c.byref(width),c.byref(height),c.byref(border),c.byref(depth))
width,height=width.value,height.value
for frame,color in enumerate((0x00ff00,0xff0000)):
    clear_color(0,0,1,1);clear(0x4000)
    bind(gl,'glEnable',None,c.c_uint)(0x0c11)
    bind(gl,'glScissor',None,i,i,i,i)(0,height//2,width,height-height//2)
    clear_color(1 if frame else 0,0 if frame else 1,0,1);clear(0x4000)
    bind(gl,'glDisable',None,c.c_uint)(0x0c11)
    pixel_store(0x0d02,40)
    swap(d,w)
    preserved=i();get_integer(0x0d02,c.byref(preserved));assert preserved.value==40
    pixel_store(0x0d02,0)
    if frame==0:
        named=bind(composite,'XCompositeNameWindowPixmap',u,p,u)(d,w);assert named
        gp=bind(gl,'glXCreatePixmap',u,p,p,u,p)(d,p(1),named,None);assert gp
    bindtex(d,gp,0x20de,None)
    tw=i();th=i();level=bind(gl,'glGetTexLevelParameteriv',None,c.c_uint,i,c.c_uint,c.POINTER(i))
    level(0xde1,0,0x1000,c.byref(tw));level(0xde1,0,0x1001,c.byref(th))
    assert (tw.value,th.value)==(width,height)
    published=(c.c_uint*(width*height))();gettex(0xde1,0,0x80e1,0x1401,published)
    assert geterror()==0
    split=width*(height-height//2)
    assert all(v&0xffffff==color for v in published[:split]), [hex(v) for v in published[:4]]
    assert all(v&0xffffff==0x0000ff for v in published[split:])
bind(gl,'glXDestroyPixmap',None,p,u)(d,gp)
bind(x,'XFreePixmap',i,p,u)(d,named)
print('GLX_REDIRECTED_SWAP_PUBLICATION_OK',flush=True)
bind(gl,'glDeleteTextures',None,i,c.POINTER(c.c_uint))(1,c.byref(tex))
make_current(d,0,None);bind(gl,'glXDestroyContext',None,p,p)(d,ctx);free(visual)
bind(damage,'XDamageDestroy',None,p,u)(d,watch)
for region in (a,b,r):destroy_region(d,region)
bind(x,'XFreeGC',i,p,p)(d,g);bind(x,'XDestroyWindow',i,p,u)(d,w);bind(x,'XCloseDisplay',i,p)(d)
print('GLX_PIXMAP_TEXTURE_READBACK_OK',renderer.decode(),flush=True)
