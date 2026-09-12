#include <X11/Xlib.h>
#include <assert.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

static int errors, code, request;
static int error(Display *d, XErrorEvent *e) {
    (void)d; errors++; code=e->error_code; request=e->request_code; return 0;
}
static void check(Display *d, int n, int denominator, int threshold) {
    int a,b,c;
    assert(XGetPointerControl(d,&a,&b,&c));
    assert(a==n && b==denominator && c==threshold);
}
int main(void) {
    Display *d=XOpenDisplay(0); assert(d); XSetErrorHandler(error);
    int n,den,t; assert(XGetPointerControl(d,&n,&den,&t));
    assert(XChangePointerControl(d,False,False,-99,0,-99)); check(d,n,den,t);
    assert(!XChangePointerControl(d,True,False,2,0,0));
    assert(errors==1 && code==BadValue && request==105); check(d,n,den,t);
    assert(!XChangePointerControl(d,False,True,0,0,-2)); assert(errors==2);
    assert(XChangePointerControl(d,True,True,1,1,3)); check(d,1,1,3);
    assert(XChangePointerControl(d,True,False,7,2,-99));
    int rounded,rden,rt;assert(XGetPointerControl(d,&rounded,&rden,&rt));
    /* Modern Windows normalizes legacy acceleration level 2 to level 1. */
    assert((rounded==2 || rounded==4) && rden==1 && rt==3);
    assert(XChangePointerControl(d,False,True,-99,0,9)); check(d,rounded,1,9);
    pid_t child=fork(); assert(child>=0);
    if (!child) {
        check(d,rounded,1,9);
        assert(XChangePointerControl(d,True,True,-1,-1,-1)); check(d,n,den,t);
        _exit(27);
    }
    int status; assert(waitpid(child,&status,0)==child);
    assert(WIFEXITED(status) && WEXITSTATUS(status)==27);
    check(d,n,den,t); assert(errors==2);
    XCloseDisplay(d); puts("XPOINTER_NATIVE_MASKS_ERRORS_DEFAULTS_FORK_OK");
    return 0;
}
