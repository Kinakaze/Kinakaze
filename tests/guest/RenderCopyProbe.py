"""Pixel-correct Cairo-style buffer copies, with a repeatable native benchmark."""
import ctypes as c, json, time
x,r=c.CDLL('libX11.so.6'),c.CDLL('libXrender.so.1')
p,u,i=c.c_void_p,c.c_ulong,c.c_int
def bind(lib,name,result,*args):
    f=getattr(lib,name);f.restype,f.argtypes=result,args;return f
class Color(c.Structure):
    _fields_=[(n,c.c_ushort) for n in ('red','green','blue','alpha')]
class Rect(c.Structure):
    _fields_=[('x',c.c_short),('y',c.c_short),('width',c.c_ushort),('height',c.c_ushort)]
d=bind(x,'XOpenDisplay',p,c.c_char_p)(None)
surfaces=[]
def surface(depth):
    pix=bind(x,'XCreatePixmap',u,p,u,c.c_uint,c.c_uint,c.c_uint)(d,1,1024,768,depth)
    fmt=bind(r,'XRenderFindStandardFormat',p,p,i)(d,0 if depth==32 else 1)
    pic=bind(r,'XRenderCreatePicture',u,p,u,p,u,p)(d,pix,fmt,0,None)
    surfaces.append((pix,pic));return pix,pic
def pixel(pix,px,py):
    image=bind(x,'XGetImage',p,p,u,i,i,c.c_uint,c.c_uint,u,i)(d,pix,px,py,1,1,0xffffffff,2)
    value=bind(x,'XGetPixel',u,p,i,i)(image,0,0)
    bind(x,'XDestroyImage',i,p)(image);return value
fill=bind(r,'XRenderFillRectangle',None,p,i,u,p,i,i,c.c_uint,c.c_uint)
copy=bind(r,'XRenderComposite',None,p,i,u,u,u,i,i,i,i,i,i,c.c_uint,c.c_uint)
clip=bind(r,'XRenderSetPictureClipRectangles',None,p,u,i,i,p,i)
try:
    a,ap=surface(32);b,bp=surface(24);z,zp=surface(32)
    fill(d,1,ap,c.byref(Color(65535,0,0,65535)),0,0,1024,768)
    fill(d,1,ap,c.byref(Color(0,65535,0,32768)),10,20,4,3)
    copy(d,1,ap,0,bp,10,20,0,0,40,50,4,3)
    assert pixel(b,40,50)==0x008000 and pixel(b,44,50)==0
    copy(d,3,bp,0,zp,40,50,0,0,60,70,4,3)
    assert pixel(z,60,70)==0xff008000 and pixel(z,64,70)==0
    rect=Rect(65,75,2,2);clip(d,zp,0,0,c.byref(rect),1)
    copy(d,1,ap,0,zp,0,0,0,0,60,70,10,10)
    assert pixel(z,65,75)==0xffff0000 and pixel(z,64,75)==0
    # Same drawable, overlapping source/destination, must read the old pixels.
    copy(d,1,ap,0,ap,8,20,0,0,10,20,8,1)
    assert pixel(a,10,20)==0xffff0000 and pixel(a,12,20)==0x80008000 and pixel(a,15,20)==0x80008000
    begin=time.perf_counter()
    for _ in range(20):copy(d,1,ap,0,bp,0,0,0,0,0,0,1024,768)
    elapsed=(time.perf_counter()-begin)*1000
    assert pixel(b,1000,700)==0xff0000
    print('RENDER_COPY_FORMAT_CLIP_OVERLAP_OK',json.dumps(dict(frames=20,milliseconds=elapsed)),flush=True)
finally:
    for pix,pic in surfaces:
        bind(r,'XRenderFreePicture',None,p,u)(d,pic);bind(x,'XFreePixmap',i,p,u)(d,pix)
    bind(x,'XCloseDisplay',i,p)(d)
