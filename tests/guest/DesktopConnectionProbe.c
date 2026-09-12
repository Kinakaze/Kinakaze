#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <sys/resource.h>
#include <unistd.h>
#include <X11/Xlib.h>
#include <X11/Xatom.h>
#include <X11/XKBlib.h>

static int error_code, request_code;
static int on_error(Display *d, XErrorEvent *event) {
    (void)d; error_code=event->error_code; request_code=event->request_code; return 0;
}
static int ready(int fd) {
    struct pollfd pollfd={fd,POLLIN,0};
    int n=poll(&pollfd,1,0); assert(n>=0 && !(pollfd.revents&POLLNVAL));
    return !!(pollfd.revents&POLLIN);
}
int main(void) {
    Display *d=XOpenDisplay(NULL); assert(d);
    int fd=ConnectionNumber(d); assert(fd>=0 && (fcntl(fd,F_GETFD)&FD_CLOEXEC));
    Atom atom=XInternAtom(d,"KINAKAZE_CONNECTION_LIFETIME",False);
    const unsigned char text[]="retained";
    XChangeProperty(d,DefaultRootWindow(d),atom,XA_STRING,8,PropModeReplace,text,sizeof(text));
    Display *temporary=XOpenDisplay(NULL); assert(temporary); XCloseDisplay(temporary);
    Atom type; int format; unsigned long count,after; unsigned char *value;
    assert(XGetWindowProperty(d,DefaultRootWindow(d),atom,0,32,False,XA_STRING,
           &type,&format,&count,&after,&value)==Success);
    assert(type==XA_STRING && count==sizeof(text) && !memcmp(value,text,count)); XFree(value);
    XSetErrorHandler(on_error);
    for(int mode=0;mode<=7;mode++) XAllowEvents(d,mode,CurrentTime);
    assert(!error_code); XAllowEvents(d,8,CurrentTime); XSync(d,False);
    assert(error_code==BadValue && request_code==35);
    int opcode,event,error,major=1,minor=0;
    assert(!XkbQueryExtension(d,&opcode,&event,&error,&major,&minor));
    assert(!XkbSetControls(d,0,NULL) && !XkbSetMap(d,0,NULL));
    assert(!XkbLockModifiers(d,XkbUseCoreKbd,LockMask,LockMask));
    assert(!XkbLatchModifiers(d,XkbUseCoreKbd,ShiftMask,ShiftMask));
    XEvent queued; while(XPending(d)) XNextEvent(d,&queued);
    assert(!ready(fd));
    pid_t child=fork(); assert(child>=0);
    if(!child) {
        assert(ConnectionNumber(d)==fd && (fcntl(fd,F_GETFD)&FD_CLOEXEC));
        while(XPending(d)) XNextEvent(d,&queued);
        memset(&queued,0,sizeof(queued)); queued.xclient.type=ClientMessage;
        queued.xclient.window=DefaultRootWindow(d); XPutBackEvent(d,&queued);
        assert(ready(fd));
        XCloseDisplay(d); assert(fcntl(fd,F_GETFD)==-1 && errno==EBADF);
        _exit(0);
    }
    int status; assert(waitpid(child,&status,0)==child && WIFEXITED(status) && !WEXITSTATUS(status));
    assert(!ready(fd)); /* Child input must not signal the parent's connection. */
    XCloseDisplay(d); assert(fcntl(fd,F_GETFD)==-1 && errno==EBADF);
    for(int i=0;i<32;i++) {
        d=XOpenDisplay(NULL); assert(d && ConnectionNumber(d)==fd);
        temporary=XOpenDisplay(NULL); assert(temporary); XCloseDisplay(temporary);
        assert(fcntl(fd,F_GETFD)>=0); XCloseDisplay(d);
        assert(fcntl(fd,F_GETFD)==-1 && errno==EBADF);
    }
    struct rlimit limit,zero;
    assert(!getrlimit(RLIMIT_NOFILE,&limit)); zero=limit; zero.rlim_cur=0;
    assert(!setrlimit(RLIMIT_NOFILE,&zero));
    assert(!XOpenDisplay(NULL) && errno==EMFILE);
    assert(!setrlimit(RLIMIT_NOFILE,&limit));
    d=XOpenDisplay(NULL); assert(d && ConnectionNumber(d)==fd); XCloseDisplay(d);
    puts("DESKTOP_CONNECTION_REFS_POLL_FORK_CLOSE_OK");
}
