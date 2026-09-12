#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/Xatom.h>
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static int error_code, error_count;
static int error(Display *d, XErrorEvent *e) { (void)d; error_code=e->error_code; error_count++; return 0; }
static unsigned char *get(Display *d, Atom p, long off, long len, int del, Atom wanted,
                          Atom *kind, int *format, unsigned long *n, unsigned long *after) {
    unsigned char *bytes=0;
    assert(XGetWindowProperty(d,DefaultRootWindow(d),p,off,len,del,wanted,kind,format,n,after,&bytes)==Success);
    return bytes;
}
int main(void) {
    Display *d=XOpenDisplay(0); assert(d);
    Window w=DefaultRootWindow(d);
    XSetErrorHandler(error);
    XWMHints *allocated=XAllocWMHints();
    XClassHint *class_hint=XAllocClassHint();
    XSizeHints *size_hint=XAllocSizeHints();
    assert(allocated && !allocated->flags && !allocated->input && !allocated->initial_state);
    assert(class_hint && !class_hint->res_name && !class_hint->res_class);
    assert(size_hint && !size_hint->flags && !size_hint->width && !size_hint->win_gravity);
    XFree(allocated); XFree(class_hint); XFree(size_hint);
    for(Atom a=1;a<=XA_LAST_PREDEFINED;a++) {
        char *name=XGetAtomName(d,a); assert(name && XInternAtom(d,name,True)==a); XFree(name);
    }
    assert(XInternAtom(d,"missing-property-probe",True)==None);
    Atom p=XInternAtom(d,"property-probe",False), kind;
    assert(p>XA_LAST_PREDEFINED && XInternAtom(d,"property-probe",True)==p);
    int format; unsigned long n,after;
    unsigned char *b=get(d,p,0,4,0,AnyPropertyType,&kind,&format,&n,&after);
    assert(kind==None && !format && !n && !after && !b);
    XChangeProperty(d,w,p,XA_STRING,8,PropModeReplace,(const unsigned char *)"cdef",4);
    XChangeProperty(d,w,p,XA_STRING,8,PropModePrepend,(const unsigned char *)"ab",2);
    XChangeProperty(d,w,p,XA_STRING,8,PropModeAppend,(const unsigned char *)"ghi",3);
    b=get(d,p,1,1,True,XA_STRING,&kind,&format,&n,&after);
    assert(kind==XA_STRING && format==8 && n==4 && after==1 && !memcmp(b,"efgh",4) && !b[4]); XFree(b);
    b=get(d,p,0,4,True,XA_INTEGER,&kind,&format,&n,&after);
    assert(kind==XA_STRING && format==8 && n==0 && after==9 && b && !b[0]); XFree(b);
    b=get(d,p,0,4,True,XA_STRING,&kind,&format,&n,&after);
    assert(n==9 && !after && !memcmp(b,"abcdefghi",9) && !b[9]); XFree(b);
    b=get(d,p,0,4,False,AnyPropertyType,&kind,&format,&n,&after); assert(kind==None && !b);
    long words[]={0x11223344L,-1L,0x180000000L,17};
    XChangeProperty(d,w,p,XA_INTEGER,32,PropModeReplace,(const unsigned char *)words,4);
    b=get(d,p,1,2,False,XA_INTEGER,&kind,&format,&n,&after);
    assert(format==32 && n==2 && after==4 && ((long *)b)[0]==-1L && ((long *)b)[1]==(long)INT32_MIN && !b[16]); XFree(b);
    XChangeProperty(d,w,p,XA_STRING,8,PropModeAppend,(const unsigned char *)"x",1);
    assert(error_code==BadMatch && error_count==1);
    unsigned char *bad=0;
    assert(XGetWindowProperty(d,w,p,5,1,0,AnyPropertyType,&kind,&format,&n,&after,&bad)==BadValue);
    assert(error_count==2 && !bad);
    assert(!XChangeProperty(d,0,p,XA_STRING,8,PropModeReplace,(const unsigned char *)"bad",3));
    assert(error_count==3 && error_code==BadWindow);
    short shorts[]={-1,0x1234,7};
    XChangeProperty(d,w,p,XA_INTEGER,16,PropModeReplace,(const unsigned char *)shorts,3);
    b=get(d,p,0,10,False,XA_INTEGER,&kind,&format,&n,&after);
    assert(format==16 && n==3 && !after && !memcmp(b,shorts,6) && !b[6]); XFree(b);
    unsigned char textbytes[]={'a',0,'b',0x7f};
    XTextProperty text={textbytes,XA_STRING,8,4},copy;
    XSetWMName(d,w,&text); assert(XGetWMName(d,w,&copy) && copy.nitems==4 && !memcmp(copy.value,textbytes,4)); XFree(copy.value);
    XSetWMIconName(d,w,&text); assert(XGetWMIconName(d,w,&copy) && copy.nitems==4 && !memcmp(copy.value,textbytes,4)); XFree(copy.value);
    XSetTextProperty(d,w,&text,p); assert(XGetTextProperty(d,w,&copy,p));
    assert(copy.encoding==XA_STRING && copy.nitems==4 && copy.format==8 && copy.value!=textbytes && !memcmp(copy.value,textbytes,4) && !copy.value[4]); XFree(copy.value);
    XWMHints hints={0}; hints.flags=InputHint|StateHint|IconPositionHint|WindowGroupHint;
    hints.input=1; hints.initial_state=IconicState; hints.icon_x=-123; hints.icon_y=321; hints.window_group=0x1234;
    assert(XSetWMHints(d,w,&hints));
    XWMHints *h=XGetWMHints(d,w); assert(h && h->flags==hints.flags && h->input && h->initial_state==IconicState && h->icon_x==-123 && h->icon_y==321 && h->window_group==0x1234); XFree(h);
    pid_t child=fork(); assert(child>=0);
    if (!child) {
        assert(XInternAtom(d,"property-probe",True)==p);
        assert(XGetTextProperty(d,w,&copy,p) && copy.nitems==4 && !memcmp(copy.value,textbytes,4)); XFree(copy.value);
        h=XGetWMHints(d,w); assert(h && h->icon_x==-123); XFree(h);
        XDeleteProperty(d,w,p); assert(!XGetTextProperty(d,w,&copy,p));
        _exit(23);
    }
    int status; assert(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==23);
    assert(XGetTextProperty(d,w,&copy,p) && copy.nitems==4); XFree(copy.value);
    XDeleteProperty(d,w,p); XDeleteProperty(d,w,XA_WM_HINTS);
    assert(!XGetWMHints(d,w) && !XGetTextProperty(d,w,&copy,p));
    XCloseDisplay(d);
    puts("XPROPERTY_ATOMS_LP64_HINTS_FORK_OK"); return 0;
}
