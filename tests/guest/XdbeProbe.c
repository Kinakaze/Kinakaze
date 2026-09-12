#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/Xdbe.h>

static int last_error;
static int error(Display *d, XErrorEvent *event) {
    (void)d; last_error=event->error_code; return 0;
}
static unsigned long pixel(Display *d, Drawable drawable, int x, int y) {
    XImage *image=XGetImage(d,drawable,x,y,1,1,AllPlanes,ZPixmap);
    assert(image);unsigned long result=XGetPixel(image,0,0);XDestroyImage(image);return result;
}
static void paint(Display *d,Drawable drawable,GC gc,unsigned long color) {
    XSetForeground(d,gc,color);assert(XFillRectangle(d,drawable,gc,0,0,8,8));
}
static void swap(Display *d,Window w,unsigned char action) {
    XdbeSwapInfo info={w,action};assert(XdbeSwapBuffers(d,&info,1));
}
static void attributes(Display *d,XdbeBackBuffer name,Window expected) {
    XdbeBackBufferAttributes *a=XdbeGetBackBufferAttributes(d,name);assert(a && a->window==expected);XFree(a);
}
static void geometry(Display *d,Drawable drawable,unsigned *width,unsigned *height) {
    Window root;int x,y;unsigned border,depth;
    assert(XGetGeometry(d,drawable,&root,&x,&y,width,height,&border,&depth));
    assert(root==DefaultRootWindow(d) && x==0 && y==0 && border==0 && depth==24);
}
int main(void) {
    Display *d=XOpenDisplay(NULL);assert(d);XSetErrorHandler(error);
    int major=-1,minor=-1,opcode=0,event=-1,base=0;
    assert(XdbeQueryExtension(d,&major,&minor) && major==1 && minor==0);
    assert(XQueryExtension(d,"DOUBLE-BUFFER",&opcode,&event,&base) && opcode>127 && event==0);
    int count=0;XdbeScreenVisualInfo *visuals=XdbeGetVisualInfo(d,NULL,&count);
    assert(visuals && count==1 && visuals[0].count==1 && visuals[0].visinfo[0].depth==24);
    assert(visuals[0].visinfo[0].visual==XVisualIDFromVisual(DefaultVisual(d,0)));
    XdbeFreeVisualInfo(visuals);
    Window w=XCreateSimpleWindow(d,DefaultRootWindow(d),0,0,128,96,0,0,0x123456);
    assert(w && w!=DefaultRootWindow(d));XClearWindow(d,w);
    assert(XSetWindowBackground(d,w,0x765432));XClearWindow(d,w);
    assert(pixel(d,w,1,1)==0x765432);
    assert(XSetWindowBackground(d,w,0x123456));XClearWindow(d,w);
    XdbeBackBuffer b=XdbeAllocateBackBufferName(d,w,XdbeUntouched);
    XdbeBackBuffer alias=XdbeAllocateBackBufferName(d,w,XdbeCopied);assert(b && alias && b!=alias);
    attributes(d,b,w);attributes(d,alias,w);
    XWindowAttributes wa;last_error=0;
    assert(!XGetWindowAttributes(d,b,&wa) && last_error==BadWindow);
    unsigned rw,rh;geometry(d,DefaultRootWindow(d),&rw,&rh);assert(rw>0 && rh>0);
    assert(pixel(d,b,1,1)==0x123456 && pixel(d,w,1,1)==0x123456);
    GC gc=XCreateGC(d,w,0,NULL);assert(gc);
    paint(d,b,gc,0xab24e1);assert(pixel(d,alias,1,1)==0xab24e1 && pixel(d,w,1,1)==0x123456);
    assert(XdbeBeginIdiom(d));swap(d,w,XdbeUntouched);assert(XdbeEndIdiom(d));
    assert(pixel(d,w,1,1)==0xab24e1 && pixel(d,b,1,1)==0x123456);
    paint(d,alias,gc,0x56789a);swap(d,w,XdbeCopied);
    assert(pixel(d,w,1,1)==0x56789a && pixel(d,b,1,1)==0x56789a);
    paint(d,b,gc,0x789abc);swap(d,w,XdbeBackground);
    assert(pixel(d,w,1,1)==0x789abc && pixel(d,alias,1,1)==0x123456);
    paint(d,b,gc,0x0a4b9c);swap(d,w,XdbeUndefined);assert(pixel(d,w,1,1)==0x0a4b9c);
    paint(d,b,gc,0xffffff);paint(d,w,gc,0xffffff);
    XClearArea(d,w,2,2,0,0,False);
    assert(pixel(d,b,1,1)==0xffffff && pixel(d,w,1,1)==0xffffff);
    assert(pixel(d,b,3,3)==0x123456 && pixel(d,w,3,3)==0x123456);
    Pixmap p=XCreatePixmap(d,w,8,8,24);paint(d,p,gc,0x6248f1);
    assert(XCopyArea(d,p,b,gc,0,0,8,8,0,0));assert(pixel(d,alias,2,2)==0x6248f1);
    assert(XCopyArea(d,b,p,gc,0,0,8,8,0,0));assert(pixel(d,p,2,2)==0x6248f1);
    XFreePixmap(d,p);
    unsigned ww,wh,bw,bh;geometry(d,w,&ww,&wh);geometry(d,b,&bw,&bh);assert(ww==bw && wh==bh);
    XResizeWindow(d,w,256,192);geometry(d,w,&ww,&wh);geometry(d,alias,&bw,&bh);
    assert(ww==bw && wh==bh && ww>128 && wh>96);
    assert(pixel(d,b,1,1)==0x123456 && pixel(d,w,1,1)==0x123456);
    Drawable screens[2]={w,b};count=2;visuals=XdbeGetVisualInfo(d,screens,&count);
    assert(visuals && count==2 && visuals[1].count==1);XdbeFreeVisualInfo(visuals);
    last_error=0;assert(!XdbeAllocateBackBufferName(d,w,99) && last_error==BadValue);
    paint(d,b,gc,0x31a9c6);unsigned long front=pixel(d,w,1,1);
    XdbeSwapInfo duplicate[2]={{w,XdbeUntouched},{w,XdbeUntouched}};
    last_error=0;assert(!XdbeSwapBuffers(d,duplicate,2) && last_error==BadMatch);
    assert(pixel(d,w,1,1)==front && pixel(d,b,1,1)==0x31a9c6);
    errno=0;pid_t child=fork();assert(child==-1 && errno==EAGAIN);
    assert(pixel(d,b,1,1)==0x31a9c6); // rejected fork releases all frozen participants
    assert(XdbeDeallocateBackBufferName(d,b));attributes(d,b,None);attributes(d,alias,w);
    last_error=0;assert(!XdbeDeallocateBackBufferName(d,b) && last_error==base+XdbeBadBuffer);
    assert(pixel(d,alias,1,1)==0x31a9c6);
    Window w2=XCreateSimpleWindow(d,DefaultRootWindow(d),0,0,128,96,0,0,0xabcdef);
    XdbeBackBuffer b2=XdbeAllocateBackBufferName(d,w2,XdbeUntouched);assert(b2);
    paint(d,b2,gc,0xa2b3c4);
    XdbeSwapInfo both[2]={{w,XdbeCopied},{w2,XdbeCopied}};
    assert(XdbeSwapBuffers(d,both,2));assert(pixel(d,w,1,1)==0x31a9c6 && pixel(d,w2,1,1)==0xa2b3c4);
    XDestroyWindow(d,w2);attributes(d,b2,None);
    XFreeGC(d,gc);XDestroyWindow(d,w);attributes(d,alias,None);
    // Client-visible query memory survives fork when no GDI resources remain.
    count=0;visuals=XdbeGetVisualInfo(d,NULL,&count);assert(visuals);
    child=fork();assert(child>=0);
    if(!child) {assert(visuals[0].count==1 && visuals[0].visinfo[0].depth==24);XdbeFreeVisualInfo(visuals);_exit(0);}
    int status;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && !WEXITSTATUS(status));
    XdbeFreeVisualInfo(visuals);
    w=XCreateSimpleWindow(d,DefaultRootWindow(d),0,0,128,96,0,0,0);
    b=XdbeAllocateBackBufferName(d,w,XdbeCopied);assert(b);XCloseDisplay(d);
    d=XOpenDisplay(NULL);attributes(d,b,None);XDestroyWindow(d,w);XCloseDisplay(d);
    puts("XDBE_PIXELS_SWAP_ALIAS_LIFETIME_OK");return 0;
}
