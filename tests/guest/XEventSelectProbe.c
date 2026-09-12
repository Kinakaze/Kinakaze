#define _GNU_SOURCE
#include <X11/Xlib.h>
#include <X11/Xlibint.h>
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static Display *display;
static atomic_int waiting;
static void put(int type,Window window,unsigned state) {
    XEvent e={0};e.type=type;e.xany.display=display;e.xany.window=window;
    if(type==MotionNotify)e.xmotion.state=state;
    XPutBackEvent(display,&e);
}
static void *receive(void *value) {
    Window window=(Window)value;atomic_fetch_add(&waiting,1);XEvent e;
    assert(!XWindowEvent(display,window,KeyPressMask,&e));
    assert(e.type==KeyPress && e.xany.window==window);return NULL;
}
static void *peek(void *value) {
    (void)value;atomic_fetch_add(&waiting,1);XEvent e;
    assert(!XPeekEvent(display,&e));assert(e.type==ClientMessage && e.xany.window==37);
    assert(QLength(display)==1);return NULL;
}
int main(void) {
    assert(XInitThreads());display=XOpenDisplay(NULL);assert(display);XEvent e;
    while(XPending(display))XNextEvent(display,&e);
    put(ClientMessage,1,0);put(KeyRelease,2,0);put(KeyPress,1,0);
    assert(XPending(display)==3 && QLength(display)==3);
    assert(!XCheckWindowEvent(display,3,KeyPressMask,&e) && QLength(display)==3);
    assert(XCheckWindowEvent(display,2,KeyReleaseMask,&e) && e.type==KeyRelease);
    assert(QLength(display)==2);
    assert(!XMaskEvent(display,KeyPressMask,&e) && e.xany.window==1);
    assert(!XCheckMaskEvent(display,~0L,&e) && QLength(display)==1);
    assert(XCheckTypedEvent(display,ClientMessage,&e) && !QLength(display));
    put(MotionNotify,1,0);put(MotionNotify,2,Button2Mask);
    assert(!XCheckWindowEvent(display,1,Button1MotionMask,&e));
    assert(XCheckMaskEvent(display,Button2MotionMask,&e) && e.xany.window==2);
    assert(XCheckTypedWindowEvent(display,1,MotionNotify,&e) && !QLength(display));
    for(int round=0;round<16;round++) {
        pthread_t a,b;atomic_store(&waiting,0);
        assert(!pthread_create(&a,NULL,receive,(void *)11));assert(!pthread_create(&b,NULL,receive,(void *)12));
        while(atomic_load(&waiting)!=2)usleep(1000);
        usleep(10000);
        if(round==0) { errno=0;assert(fork()==-1 && errno==EAGAIN); }
        put(ClientMessage,20,0);put(KeyPress,11,0);XPending(display);
        put(KeyPress,12,0);XPending(display);
        assert(!pthread_join(a,NULL) && !pthread_join(b,NULL));
        assert(XCheckTypedWindowEvent(display,20,ClientMessage,&e));assert(!XPending(display));
    }
    pthread_t peeker;atomic_store(&waiting,0);assert(!pthread_create(&peeker,NULL,peek,NULL));
    while(!atomic_load(&waiting))usleep(1000);usleep(10000);
    put(ClientMessage,37,0);assert(!pthread_join(peeker,NULL));
    assert(QLength(display)==1);assert(!XNextEvent(display,&e));assert(e.xany.window==37 && !QLength(display));
    pid_t child=fork();assert(child>=0);
    if(!child)_exit(23);
    int status;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==23);
    XCloseDisplay(display);puts("XEVENT_SELECT_QUEUE_CONCURRENT_WAKE_OK");return 0;
}
