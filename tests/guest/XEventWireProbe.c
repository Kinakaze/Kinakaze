#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#include <X11/Xlib.h>
#include <X11/Xlibint.h>

static int encoded,decoded,last_error;
static int error(Display *d,XErrorEvent *e) {(void)d;last_error=e->error_code;return 0;}
static Status encode(Display *d,XEvent *event,xEvent *wire) {
    assert(event->type==90);++encoded;
    // Replacing a hook from inside that hook must not hold a registry mutex.
    assert(XESetEventToWire(d,90,encode)==encode);
    errno=0;assert(fork()==-1 && errno==EAGAIN);
    memset(wire,0,32);unsigned char *p=(void *)wire;p[0]=90;
    uint32_t value=(uint32_t)event->xclient.data.l[0]+7;memcpy(p+12,&value,4);return 1;
}
static Bool decode(Display *d,XEvent *event,xEvent *wire) {
    ++decoded;assert(XESetWireToEvent(d,90,decode)==decode);
    const unsigned char *p=(void *)wire;uint32_t value;memcpy(&value,p+12,4);
    memset(event,0,sizeof(*event));event->type=p[0]&127;event->xany.display=d;
    event->xany.send_event=(p[0]&128)!=0;event->xclient.data.l[0]=value;return True;
}
static void custom(Display *d) {
    XEvent e={0},got;e.type=90;e.xclient.data.l[0]=35;
    assert(XSendEvent(d,DefaultRootWindow(d),False,NoEventMask,&e));
    assert(XPending(d)>0);XNextEvent(d,&got);
    assert(got.type==90 && got.xany.send_event && got.xany.display==d && got.xclient.data.l[0]==42);
}
int main(void) {
    Display *d=XOpenDisplay(NULL);assert(d);XSetErrorHandler(error);
    for(int format=8;format<=32;format*=2) {
        XEvent e={0},got;e.xclient.type=ClientMessage;e.xclient.display=d;
        e.xclient.window=DefaultRootWindow(d);e.xclient.serial=0x1234;
        e.xclient.message_type=XInternAtom(d,"WIRE_PROBE",False);e.xclient.format=format;
        if(format==32)for(int i=0;i<5;i++)e.xclient.data.l[i]=(int32_t)(0xabcdef01u+i);
        else for(int i=0;i<20;i++)e.xclient.data.b[i]=(char)(0x80+i);
        assert(XSendEvent(d,DefaultRootWindow(d),False,0,&e));assert(XPending(d)>0);XNextEvent(d,&got);
        assert(got.type==ClientMessage && got.xclient.send_event && got.xclient.serial==0x1234);
        assert(got.xclient.format==format && got.xclient.message_type==e.xclient.message_type);
        if(format==32)assert(!memcmp(got.xclient.data.l,e.xclient.data.l,5*sizeof(long)));
        else assert(!memcmp(got.xclient.data.b,e.xclient.data.b,20));
    }
    XEvent key={0},got;key.xkey.type=KeyPress;key.xkey.window=DefaultRootWindow(d);
    key.xkey.root=DefaultRootWindow(d);key.xkey.x=-13;key.xkey.y=14;key.xkey.keycode=38;
    key.xkey.x_root=15;key.xkey.y_root=-16;key.xkey.state=ControlMask;key.xkey.time=0x10203040;
    key.xkey.same_screen=True;
    assert(XSendEvent(d,DefaultRootWindow(d),False,0,&key));XNextEvent(d,&got);
    assert(got.type==KeyPress && got.xkey.x==-13 && got.xkey.y_root==-16 && got.xkey.keycode==38);
    assert(got.xkey.state==ControlMask && got.xkey.time==0x10203040 && got.xkey.same_screen && got.xkey.send_event);
    last_error=0;assert(!XSendEvent(d,DefaultRootWindow(d),True,KeyPressMask,&key) && last_error==BadImplementation);
    assert(XESetEventToWire(d,90,encode)==NULL);
    assert(XESetWireToEvent(d,90,decode)!=NULL);
    custom(d);assert(encoded==1 && decoded==1);
    xEvent wire={0};unsigned char *p=(void *)&wire;p[0]=90;uint32_t value=123;memcpy(p+12,&value,4);
    _XEnq(d,&wire);XNextEvent(d,&got);assert(got.type==90 && !got.xany.send_event && got.xclient.data.l[0]==123);
    pid_t child=fork();assert(child>=0);
    if(!child) {custom(d);_exit(0);}
    int status;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && !WEXITSTATUS(status));
    custom(d);
    assert(XESetEventToWire(d,90,NULL)==encode);
    XEvent e={0};e.type=90;assert(!XSendEvent(d,DefaultRootWindow(d),False,0,&e));assert(!XPending(d));
    assert(XESetEventToWire(d,90,encode)!=NULL);
    assert(XESetWireToEvent(d,90,NULL)==decode);
    assert(XSendEvent(d,DefaultRootWindow(d),False,0,&e));assert(!XPending(d));
    XCloseDisplay(d);d=XOpenDisplay(NULL);assert(d);
    assert(!XSendEvent(d,DefaultRootWindow(d),False,0,&e));
    assert(XESetEventToWire(d,90,encode)==NULL);XCloseDisplay(d);
    puts("XEVENT_WIRE_CALLBACK_FORK_OK");return 0;
}
