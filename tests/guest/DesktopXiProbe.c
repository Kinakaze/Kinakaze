#include <assert.h>
#include <stdio.h>
#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <X11/extensions/XInput2.h>
static int errors;
static int unsupported(Display *d, XErrorEvent *e) {
    (void)d; assert(e->error_code==BadImplementation); errors++; return 0;
}
int main(void) {
    Display *d=XOpenDisplay(NULL); assert(d);
    Window w=XCreateSimpleWindow(d,DefaultRootWindow(d),0,0,32,32,0,0,0); assert(w);
    Window root,child; double rx,ry,x,y; XIButtonState buttons; XIModifierState mods; XIGroupState group;
    assert(XIQueryPointer(d,2,w,&root,&child,&rx,&ry,&x,&y,&buttons,&mods,&group));
    assert(root==DefaultRootWindow(d) && buttons.mask_len==4 && buttons.mask);
    assert(mods.effective==(mods.base|mods.latched|mods.locked)); XFree(buttons.mask);
    Atom kind; int format; unsigned long count,after; unsigned char *value;
    Atom enabled=XInternAtom(d,"Device Enabled",False);
    assert(XIGetProperty(d,3,enabled,0,1,False,XA_INTEGER,&kind,&format,&count,&after,&value)==Success);
    assert(kind==XA_INTEGER && format==8 && count==1 && after==0 && value[0]==1); XFree(value);
    assert(XIGetProperty(d,3,enabled,0,0,False,XA_INTEGER,&kind,&format,&count,&after,&value)==Success);
    assert(!count && after==1); XFree(value);
    Atom absent=XInternAtom(d,"probe.unset.property",False);
    assert(XIGetProperty(d,3,absent,0,1,False,AnyPropertyType,&kind,&format,&count,&after,&value)==Success);
    assert(kind==None && !format && !count && !after && !value);
    assert(XIDefineCursor(d,2,w,None)==Success); assert(XIUndefineCursor(d,2,w)==Success);
    XErrorHandler old=XSetErrorHandler(unsupported);
    unsigned char mask[1]={0}; XIEventMask selected={3,1,mask}; XIGrabModifiers modifier={0,0};
    assert(XIGrabDevice(d,3,w,CurrentTime,None,GrabModeAsync,GrabModeAsync,False,&selected)==BadImplementation);
    assert(XIUngrabDevice(d,3,CurrentTime)==BadImplementation);
    assert(XIGrabKeycode(d,3,38,w,GrabModeAsync,GrabModeAsync,False,&selected,1,&modifier)==-1);
    assert(XIUngrabKeycode(d,3,38,w,1,&modifier)==BadImplementation && errors==4);
    XSetErrorHandler(old); XDestroyWindow(d,w); XCloseDisplay(d);
    puts("DESKTOP_XI_QUERY_PROPERTY_UNSUPPORTED_GRABS_OK");
}
