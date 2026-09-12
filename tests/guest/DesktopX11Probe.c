#include <assert.h>
#include <string.h>
#include <stdio.h>
#include <locale.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/XKBlib.h>
#include <X11/keysym.h>
#include <X11/Xatom.h>

int main(void) {
    assert(setlocale(LC_CTYPE,"C.UTF-8"));
    Display *d=XOpenDisplay(NULL); assert(d);
    assert(XDefaultRootWindow(d)==DefaultRootWindow(d));
    XModifierKeymap *map=XGetModifierMapping(d); assert(map && map->max_keypermod==2);
    assert(map->modifiermap[0]==XKeysymToKeycode(d,XK_Shift_L)); XFreeModifiermap(map);
    assert(XkbKeysymToModifiers(d,XK_Shift_R)==ShiftMask);
    assert(XkbKeysymToModifiers(d,XK_Control_L)==ControlMask);
    assert(XkbKeysymToModifiers(d,XK_Num_Lock)==Mod2Mask);
    assert(XkbKeysymToModifiers(d,XK_a)==0);
    int major=1,minor=0; assert(XkbLibraryVersion(&major,&minor));
    int count=-1; assert(!XGetMotionEvents(d,DefaultRootWindow(d),0,CurrentTime,&count) && count==0);
    Window w=XCreateSimpleWindow(d,DefaultRootWindow(d),0,0,32,32,0,0,0);
    assert(w);
    Atom protocols[]={XInternAtom(d,"WM_DELETE_WINDOW",False),XInternAtom(d,"WM_TAKE_FOCUS",False)},*protocol_copy=NULL;
    assert(XSetWMProtocols(d,w,protocols,2));
    assert(XGetWMProtocols(d,w,&protocol_copy,&count) && count==2 && !memcmp(protocols,protocol_copy,sizeof protocols)); XFree(protocol_copy);
    XSizeHints size={.flags=PMinSize|PMaxSize|PBaseSize|PResizeInc|PWinGravity,.x=-4,.y=-8,.min_width=10,.min_height=11,.max_width=500,.max_height=400,.width_inc=2,.height_inc=3,.base_width=12,.base_height=15,.win_gravity=NorthWestGravity},read_size;
    long supplied=0;
    assert(!XGetWMNormalHints(d,w,&read_size,&supplied));
    XSetWMNormalHints(d,w,&size);
    assert(XGetWMNormalHints(d,w,&read_size,&supplied));
    assert(read_size.flags==size.flags && read_size.x==-4 && read_size.y==-8 && read_size.min_width==10 && read_size.max_height==400);
    assert(read_size.base_width==12 && read_size.height_inc==3 && (supplied&PWinGravity));
    assert(XGetNormalHints(d,w,&read_size));
    Atom custom_size=XInternAtom(d,"PROBE_SIZE_HINTS",False);
    XSetWMSizeHints(d,w,&size,custom_size);
    assert(XGetWMSizeHints(d,w,&read_size,&supplied,custom_size) && read_size.base_height==15);
    long old_size[19]={0x3ff,-9,7,80,60,3,4,300,400,2,3,1,2,3,4,30,40,NorthWestGravity,99};
    XChangeProperty(d,w,custom_size,XA_WM_SIZE_HINTS,32,PropModeReplace,(unsigned char*)old_size,15);
    assert(XGetWMSizeHints(d,w,&read_size,&supplied,custom_size) && supplied==0xff && read_size.flags==0xff && read_size.x==-9);
    XChangeProperty(d,w,custom_size,XA_WM_SIZE_HINTS,32,PropModeReplace,(unsigned char*)old_size,19);
    assert(XGetWMSizeHints(d,w,&read_size,&supplied,custom_size) && read_size.base_width==30 && supplied==0x3ff);
    XClassHint own_class={"wm-instance","WMClass"},read_class;
    assert(XSetClassHint(d,w,&own_class) && XGetClassHint(d,w,&read_class));
    assert(!strcmp(read_class.res_name,own_class.res_name) && !strcmp(read_class.res_class,own_class.res_class));
    XFree(read_class.res_name); XFree(read_class.res_class);
    XWindowAttributes before,after_size; assert(XGetWindowAttributes(d,w,&before));
    XWindowChanges change={.width=before.width+16};
    assert(XReconfigureWMWindow(d,w,DefaultScreen(d),CWWidth,&change));
    assert(XGetWindowAttributes(d,w,&after_size));
    assert(after_size.width==before.width+16 && after_size.height==before.height);
    GC gc=XCreateGC(d,w,0,NULL); assert(gc);
    Pixmap tile=XCreatePixmap(d,w,2,2,24); assert(tile);
    XSetForeground(d,gc,0x112233); XFillRectangle(d,tile,gc,0,0,2,2);
    XSetForeground(d,gc,0xff0000); XDrawPoint(d,tile,gc,1,0);
    XSetFunction(d,gc,GXxor); XSetForeground(d,gc,0x00ffff); XDrawPoint(d,tile,gc,1,0);
    XSetFunction(d,gc,GXcopy);
    XSetWindowBackgroundPixmap(d,w,tile); XFreePixmap(d,tile); XClearWindow(d,w);
    XImage *image=XGetImage(d,w,0,0,4,4,AllPlanes,ZPixmap); assert(image);
    assert(XGetPixel(image,0,0)==0x112233 && XGetPixel(image,1,0)==0xffffff);
    assert(XGetPixel(image,3,2)==0xffffff); XDestroyImage(image);
    XSetWindowBackgroundPixmap(d,w,None); XClearWindow(d,w);
    image=XGetImage(d,w,0,0,4,4,AllPlanes,ZPixmap); assert(image && XGetPixel(image,1,0)==0xffffff); XDestroyImage(image);
    XSetForeground(d,gc,0x556677); XPoint points[]={{0,1},{1,1},{1,1}};
    XDrawPoints(d,w,gc,points,3,CoordModePrevious);
    image=XGetImage(d,w,0,0,4,4,AllPlanes,ZPixmap); assert(image);
    assert(XGetPixel(image,0,1)==0x556677 && XGetPixel(image,1,2)==0x556677 && XGetPixel(image,2,3)==0x556677); XDestroyImage(image);
    char name[]="desktop probe", icon[]="probe"; char *args[]={name};
    XClassHint class={"sample","DesktopProbe"}; XWMHints hints={.flags=InputHint,.input=True};
    XmbSetWMProperties(d,w,name,icon,args,1,NULL,&hints,&class);
    Atom type; int format; unsigned long n,after; unsigned char *value;
    assert(XGetWindowProperty(d,w,XA_WM_CLASS,0,128,False,XA_STRING,&type,&format,&n,&after,&value)==Success);
    assert(n==20 && !memcmp(value,"sample\0DesktopProbe\0",20)); XFree(value);
    XWMHints *returned=XGetWMHints(d,w); assert(returned && returned->input); XFree(returned);
    unsigned char latin[]={0xe9,0,'b'}; XTextProperty text={latin,XA_STRING,8,3}; char **list;
    assert(XmbTextPropertyToTextList(d,&text,&list,&count)==Success && count==2);
    assert(!strcmp(list[0],"\xc3\xa9") && !strcmp(list[1],"b")); XFreeStringList(list);
    XFreeGC(d,gc); XDestroyWindow(d,w); XCloseDisplay(d);
    puts("DESKTOP_X11_MODIFIERS_POINTS_TILES_PROPERTIES_OK");
}
