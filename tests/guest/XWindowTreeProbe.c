#include <X11/Xlib.h>
#include <assert.h>
#include <stdio.h>
#include <unistd.h>

static int errors, code;
static int error(Display *d, XErrorEvent *e) {(void)d; errors++; code=e->error_code; return 0;}
static void tree(Display *d, Window w, Window parent, Window child) {
    Window root,owner,*children=0; unsigned n;
    assert(XQueryTree(d,w,&root,&owner,&children,&n));
    assert(root==DefaultRootWindow(d) && owner==parent);
    assert(n==(child?1:0)); if(child) assert(children[0]==child); XFree(children);
}
static void coordinates(Display *d, Window from, Window to, int ex, int ey) {
    int x,y; Window child;
    assert(XTranslateCoordinates(d,from,to,2,3,&x,&y,&child));
    assert(x==ex && y==ey);
}
static void size(Display *d,Window w) {
    Window root;int x,y;unsigned width,height,border,depth;
    assert(XGetGeometry(d,w,&root,&x,&y,&width,&height,&border,&depth));
    if(width!=64 || height!=48) {
        char message[80];int n=snprintf(message,sizeof message,"client geometry: %u x %u\n",width,height);write(2,message,n);
    }
    assert(width==64 && height==48);
}
int main(void) {
    Display *d=XOpenDisplay(0); assert(d); XSetErrorHandler(error);
    Window root=DefaultRootWindow(d);
    Window a=XCreateSimpleWindow(d,root,0,0,200,160,0,0,0);
    Window b=XCreateSimpleWindow(d,root,0,0,200,160,0,0,0);
    Window c=XCreateSimpleWindow(d,a,7,9,64,48,0,0,0);
    assert(a!=root && b!=root && c!=root);
    tree(d,a,root,c); tree(d,c,a,0); coordinates(d,c,a,9,12);size(d,c);
    assert(XReparentWindow(d,c,b,11,13));
    tree(d,a,root,0); tree(d,b,root,c); tree(d,c,b,0); coordinates(d,c,b,13,16);size(d,c);
    XEvent event; assert(XCheckTypedWindowEvent(d,c,ReparentNotify,&event));
    assert(event.xreparent.parent==b && event.xreparent.x==11 && event.xreparent.y==13);
    assert(!XReparentWindow(d,b,c,0,0)); assert(errors==1 && code==BadMatch);
    tree(d,c,b,0);
    assert(XReparentWindow(d,c,root,20,30)); tree(d,b,root,0); tree(d,c,root,0);
    coordinates(d,c,root,22,33);size(d,c);
    Window *children=0,rt,owner; unsigned count;
    assert(!XQueryTree(d,0,&rt,&owner,&children,&count));
    assert(errors==2 && code==BadWindow && !children && !count);
    Window u=XCreateSimpleWindow(d,a,0,0,20,20,0,0,0);
    Window v=XCreateSimpleWindow(d,a,0,0,20,20,0,0,0);
    Window order[]={u,v};assert(XRestackWindows(d,order,2));
    assert(XQueryTree(d,a,&rt,&owner,&children,&count));
    assert(count==2 && children[0]==v && children[1]==u);XFree(children);
    assert(XLowerWindow(d,u));assert(XQueryTree(d,a,&rt,&owner,&children,&count));
    assert(count==2 && children[0]==u && children[1]==v);XFree(children);
    XDestroyWindow(d,v);XDestroyWindow(d,u);
    XDestroyWindow(d,c); XDestroyWindow(d,b); XDestroyWindow(d,a); XCloseDisplay(d);
    puts("XWINDOW_REPARENT_TREE_COORDINATES_ERRORS_OK"); return 0;
}
