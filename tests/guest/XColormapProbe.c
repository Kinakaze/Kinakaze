#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/Xatom.h>
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
static int code,errors;
static int error(Display *d,XErrorEvent *e) {(void)d;code=e->error_code;errors++;return 0;}
static void installed(Display *d,Colormap expected) {
    int n; Colormap *maps=XListInstalledColormaps(d,DefaultRootWindow(d),&n);
    assert(maps && n==1 && maps[0]==expected); XFree(maps);
}
int main(void) {
    Display *d=XOpenDisplay(0);assert(d);XSetErrorHandler(error);
    Window root=DefaultRootWindow(d);Visual *v=DefaultVisual(d,0);
    Colormap map=XCreateColormap(d,root,v,AllocNone);assert(map && map!=DefaultColormap(d,0));
    assert(XInstallColormap(d,map));installed(d,map);
    XColor c={.red=0x1234,.green=0x5678,.blue=0x9abc};
    assert(XAllocColor(d,map,&c));assert(c.pixel==0x12569a && c.red==0x1212 && c.green==0x5656 && c.blue==0x9a9a);
    c.red=c.green=c.blue=0;assert(XQueryColor(d,map,&c));assert(c.red==0x1212 && c.green==0x5656 && c.blue==0x9a9a);
    c.pixel=123;assert(XParseColor(d,map,"#abc",&c));assert(c.red==0xa000 && c.green==0xb000 && c.blue==0xc000 && c.pixel==123);
    assert(XParseColor(d,map,"#123456789",&c));assert(c.red==0x1230 && c.green==0x4560 && c.blue==0x7890);
    assert(XParseColor(d,map,"rgb:a/bb/cccc",&c));assert(c.red==0xaaaa && c.green==0xbbbb && c.blue==0xcccc);
    XColor before=c;assert(!XParseColor(d,map,"#xx00ff",&c));assert(!memcmp(&before,&c,sizeof c));
    assert(!XParseColor(d,map,"definitely-not-a-color",&c));
    XColor exact;assert(XAllocNamedColor(d,map,"RED",&c,&exact));assert(c.pixel==0xff0000 && exact.red==65535);
    assert(!XStoreColor(d,map,&c));assert(errors==1 && code==BadAccess);
    unsigned long pixel=0;assert(!XAllocColorCells(d,map,False,0,0,&pixel,1));assert(errors==2 && code==BadAlloc);
    assert(!XCreateColormap(d,root,v,AllocAll));assert(errors==3 && code==BadMatch);
    XStandardColormap *allocated=XAllocStandardColormap();assert(allocated && !allocated->red_max);XFree(allocated);
    XStandardColormap maps[2]={{.colormap=map,.red_max=255,.red_mult=65536,.green_max=255,.green_mult=256,.blue_max=255,.blue_mult=1,.visualid=v->visualid,.killid=0xffffffffUL},
        {.colormap=map,.base_pixel=0x87654321UL,.visualid=v->visualid}};
    XSetRGBColormaps(d,root,maps,2,XA_RGB_DEFAULT_MAP);XStandardColormap *copy=0;int count;
    assert(XGetRGBColormaps(d,root,&copy,&count,XA_RGB_DEFAULT_MAP));assert(count==2 && !memcmp(maps,copy,sizeof maps));XFree(copy);
    pid_t child=fork();assert(child>=0);
    if(!child) {
        installed(d,map);assert(XUninstallColormap(d,map));installed(d,DefaultColormap(d,0));
        assert(XFreeColormap(d,map));Colormap fresh=XCreateColormap(d,root,v,AllocNone);assert(fresh>map);_exit(19);
    }
    int status;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==19);
    installed(d,map);assert(XFreeColormap(d,map));installed(d,DefaultColormap(d,0));
    assert(!XQueryColor(d,map,&c));assert(errors==4 && code==BadColor);
    XCloseDisplay(d);puts("XCOLORMAP_TRUECOLOR_PROPERTIES_OWNERSHIP_FORK_OK");return 0;
}
