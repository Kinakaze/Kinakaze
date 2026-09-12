#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <sys/wait.h>
#include <unistd.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/Xproto.h>
#include <X11/extensions/XShm.h>
#include <X11/extensions/extutil.h>
#undef XDestroyImage
extern int XDestroyImage(XImage *);
static int last_error, closed;
static XExtensionInfo *extension;
static int error(Display *display, XErrorEvent *event) {
    (void)display; last_error=event->error_code; return 0;
}
static int close_extension(Display *display,XExtCodes *codes) {
    assert(codes && codes->major_opcode==130);
    ++closed;
    return XextRemoveDisplay(extension,display);
}
static void client_state(Display *d) {
    extension=XextCreateExtension();assert(extension && !extension->ndisplays);
    assert(!XextFindDisplay(extension,d));
    XExtensionHooks hooks={0};hooks.close_display=close_extension;
    XExtDisplayInfo *entry=XextAddDisplay(extension,d,"MIT-SHM",&hooks,0,(void *)0x1234);
    assert(entry && entry->codes && entry->data==(void *)0x1234 && extension->head==entry);
    assert(XextFindDisplay(extension,d)==entry && extension->ndisplays==1);
    XExtDisplayInfo *missing=XextAddDisplay(extension,NULL,"missing-extension",NULL,0,(void *)0x4321);
    assert(missing && !missing->codes && missing->data==(void *)0x4321 && extension->ndisplays==2);
    assert(XextFindDisplay(extension,d)==entry);
    assert(XextRemoveDisplay(extension,NULL) && !XextRemoveDisplay(extension,NULL));
    char *storage=malloc(64);assert(storage);memset(storage,0x5a,64);
    XShmSegmentInfo info={0};
    XImage *image=XShmCreateImage(d,DefaultVisual(d,0),24,ZPixmap,storage,&info,4,4);
    assert(image && image->obdata==(void *)&info);
    pid_t child=fork();assert(child>=0);
    if(!child){
        assert(XextFindDisplay(extension,d)==entry && entry->data==(void *)0x1234);
        assert(XDestroyImage(image));assert(storage[0]==0x5a);free(storage);
        XCloseDisplay(d);assert(closed==1 && extension->ndisplays==0);
        XextDestroyExtension(extension);_exit(0);
    }
    int status;assert(waitpid(child,&status,0)==child && status==0);
    assert(extension->ndisplays==1 && closed==0 && storage[63]==0x5a);
    assert(XDestroyImage(image));free(storage);
    puts("XEXT_GUEST_STATE_FORK_OK");
}
int main(void) {
    Display *d=XOpenDisplay(NULL);assert(d);XSetErrorHandler(error);
    int major=-1,minor=-1;Bool pixmaps=True;
    assert(XShmQueryExtension(d) && XShmQueryVersion(d,&major,&minor,&pixmaps));
    assert(major==1 && minor==1 && !pixmaps);
    client_state(d);
    XShmSegmentInfo info={0};
    XImage *image=XShmCreateImage(d,DefaultVisual(d,0),24,ZPixmap,NULL,&info,8,4);assert(image);
    size_t bytes=image->bytes_per_line*image->height;
    info.shmid=shmget(IPC_PRIVATE,bytes+64,IPC_CREAT|0600);assert(info.shmid>=0);
    info.shmaddr=shmat(info.shmid,NULL,0);assert(info.shmaddr!=(void *)-1);
    memset(info.shmaddr,0xcd,bytes+64);image->data=info.shmaddr+32;
    assert(XShmAttach(d,&info));
    struct shmid_ds metadata;assert(!shmctl(info.shmid,IPC_STAT,&metadata) && metadata.shm_nattch==2);
    Pixmap pixmap=XCreatePixmap(d,DefaultRootWindow(d),8,4,24);assert(pixmap);
    GC gc=XCreateGC(d,pixmap,0,NULL);assert(gc);
    for(int y=0;y<4;y++)for(int x=0;x<8;x++)assert(XPutPixel(image,x,y,0x102030+x+y*8));
    int (*destroy)(XImage *)=image->f.destroy_image;
    assert(XShmPutImage(d,pixmap,gc,image,0,0,0,0,8,4,True));
    assert(image->f.destroy_image==destroy && !last_error);
    XEvent event;assert(XPending(d)>0);XNextEvent(d,&event);
    XShmCompletionEvent *completion=(void *)&event;
    assert(completion->type==XShmGetEventBase(d) && completion->drawable==pixmap);
    assert(completion->offset==32 && completion->shmseg==info.shmseg && !completion->send_event);
    memset(image->data,0,bytes);assert(XShmGetImage(d,pixmap,image,0,0,AllPlanes));
    for(int y=0;y<4;y++)for(int x=0;x<8;x++)assert(XGetPixel(image,x,y)==0x102030UL+x+y*8);
    XImage *region=XGetImage(d,pixmap,2,1,3,2,0xff00,ZPixmap);assert(region);
    for(int y=0;y<2;y++)for(int x=0;x<3;x++)assert(XGetPixel(region,x,y)==((0x102030UL+x+2+(y+1)*8)&0xff00));
    XDestroyImage(region);
    for(int i=0;i<32;i++){assert((unsigned char)info.shmaddr[i]==0xcd);assert((unsigned char)info.shmaddr[bytes+32+i]==0xcd);}
    int stride=image->bytes_per_line;image->bytes_per_line=100000;
    assert(!XShmPutImage(d,pixmap,gc,image,0,0,0,0,8,4,False) && last_error==BadValue);
    image->bytes_per_line=stride;
    image->width=100000;image->bytes_per_line=0;last_error=0;
    assert(!XShmPutImage(d,pixmap,gc,image,0,0,0,0,8,4,False) && last_error==BadValue);
    image->width=8;image->bytes_per_line=stride;
    XShmSegmentInfo readonly=info;readonly.readOnly=True;assert(XShmAttach(d,&readonly));
    image->obdata=(void *)&readonly;last_error=0;
    assert(!XShmGetImage(d,pixmap,image,0,0,AllPlanes) && last_error==BadAccess);
    assert(XShmPutImage(d,pixmap,gc,image,0,0,0,0,8,4,False));assert(XShmDetach(d,&readonly));
    image->obdata=(void *)&info;
    assert(XShmDetach(d,&info));last_error=0;
    assert(!XShmGetImage(d,pixmap,image,0,0,AllPlanes) && last_error==136);
    assert(XDestroyImage(image));assert((unsigned char)info.shmaddr[0]==0xcd);
    assert(!shmctl(info.shmid,IPC_STAT,&metadata) && metadata.shm_nattch==1);
    assert(!shmdt(info.shmaddr));assert(!shmctl(info.shmid,IPC_RMID,NULL));
    XFreeGC(d,gc);XFreePixmap(d,pixmap);
    XCloseDisplay(d);assert(closed==1 && !extension->ndisplays);XextDestroyExtension(extension);
    puts("XSHM_PIXELS_COMPLETION_OWNERSHIP_OK");return 0;
}
