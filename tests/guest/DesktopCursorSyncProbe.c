#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <sys/wait.h>
#include <X11/Xlib.h>
#include <X11/extensions/sync.h>
#include <X11/extensions/Xrandr.h>
#include <X11/extensions/shape.h>
typedef struct {unsigned version,size,width,height,xhot,yhot,delay; uint32_t *pixels;} XcursorImage;
typedef struct {int nimage; XcursorImage **images; char *name;} XcursorImages;
extern XcursorImage *XcursorImageCreate(int,int), *XcursorLibraryLoadImage(const char*,const char*,int);
extern void XcursorImageDestroy(XcursorImage*), XcursorImagesDestroy(XcursorImages*);
extern XcursorImages *XcursorLibraryLoadImages(const char*,const char*,int);
extern Cursor XcursorImageLoadCursor(Display*,const XcursorImage*), XcursorShapeLoadCursor(Display*,unsigned);
extern int XcursorSupportsARGB(Display*), XcursorSetTheme(Display*,const char*), XcursorSetDefaultSize(Display*,int), XcursorGetDefaultSize(Display*);
extern char *XcursorGetTheme(Display*);
extern void *_Xglobal_lock;
extern void (*_XLockMutex_fn)(void*), (*_XUnlockMutex_fn)(void*);
static Bool selected(Display *d, XEvent *e, XPointer arg) { (void)d; return e->type==ClientMessage&&e->xclient.data.l[0]==(long)arg; }
int main(void) {
 Display *d=XOpenDisplay(NULL); assert(d);
 Screen *screen=DefaultScreenOfDisplay(d); assert(screen->ndepths>0&&screen->depths);
 int found_visual=0; for(int i=0;i<screen->ndepths;i++) for(int j=0;j<screen->depths[i].nvisuals;j++) found_visual|=screen->depths[i].visuals[j].visualid==DefaultVisual(d,0)->visualid; assert(found_visual);
 Window w=XCreateSimpleWindow(d,DefaultRootWindow(d),0,0,64,32,0,0,0); assert(w);
 XWindowAttributes initial; assert(XGetWindowAttributes(d,w,&initial)&&initial.map_state==IsUnmapped);
 assert(XMapWindow(d,w)); assert(XGetWindowAttributes(d,w,&initial)&&initial.map_state==IsViewable);
 assert(XUnmapWindow(d,w)); assert(XGetWindowAttributes(d,w,&initial)&&initial.map_state==IsUnmapped);
 assert(XcursorSupportsARGB(d)); assert(!XcursorImageCreate(-1,2));
 assert(XcursorSetTheme(d,"Adwaita")); assert(!strcmp(XcursorGetTheme(d),"Adwaita"));
 assert(XcursorSetDefaultSize(d,32)); assert(XcursorGetDefaultSize(d)==32); assert(!XcursorSetDefaultSize(d,0));
 XcursorImages *images=XcursorLibraryLoadImages("left_ptr","Adwaita",32); assert(images&&images->nimage>0);
 XcursorImage *image=images->images[0]; assert(image->width>1&&image->height>1&&image->xhot<image->width);
 size_t visible=0; for(size_t i=0;i<(size_t)image->width*image->height;i++) visible+=!!(image->pixels[i]>>24);
 assert(visible>1); Cursor cursor=XcursorImageLoadCursor(d,image); assert(cursor); assert(XDefineCursor(d,w,cursor));
 XFreeCursor(d,cursor); /* Window retains its own native image. */
 XSyncValue value,one,out; XSyncIntsToValue(&value,0xabcdef12,0x12345678); XSyncIntToValue(&one,1);
 XSyncCounter counter=XSyncCreateCounter(d,value); assert(counter); assert(XSyncQueryCounter(d,counter,&out)); assert(XSyncValueEqual(value,out));
 assert(_Xglobal_lock&&_XLockMutex_fn&&_XUnlockMutex_fn);
 _XLockMutex_fn(_Xglobal_lock); _XLockMutex_fn(_Xglobal_lock);
 pid_t child=fork(); assert(child>=0);
 if(!child) { _XLockMutex_fn(_Xglobal_lock); _XUnlockMutex_fn(_Xglobal_lock); assert(images->images[0]->width>1); XcursorImagesDestroy(images); assert(XSyncChangeCounter(d,counter,one)); assert(XSyncQueryCounter(d,counter,&out)); assert(out.lo==0xabcdef13); assert(XSyncDestroyCounter(d,counter)); _exit(0); }
 _XUnlockMutex_fn(_Xglobal_lock); _XUnlockMutex_fn(_Xglobal_lock);
 int status; assert(waitpid(child,&status,0)==child&&WIFEXITED(status)&&WEXITSTATUS(status)==0);
 assert(XSyncQueryCounter(d,counter,&out)&&XSyncValueEqual(out,value)); assert(XSyncDestroyCounter(d,counter));
 int overflow=0; XSyncMaxValue(&value); XSyncValueAdd(&out,value,one,&overflow); assert(overflow&&out.hi==INT32_MIN&&out.lo==0);
 XcursorImagesDestroy(images); assert(XcursorSetTheme(d,NULL)); assert(!XcursorGetTheme(d));
 image=XcursorLibraryLoadImage("left_ptr",NULL,32); assert(image&&image->width>1&&image->pixels); XcursorImageDestroy(image);
 cursor=XcursorShapeLoadCursor(d,68); assert(cursor); assert(XDefineCursor(d,w,cursor)); XFreeCursor(d,cursor);
 assert(!XcursorLibraryLoadImage("does-not-exist","Adwaita",32));
 int n=0; XRRMonitorInfo *monitors=XRRGetMonitors(d,DefaultRootWindow(d),True,&n); assert(monitors&&n>0);
 int primary=0; for(int i=0;i<n;i++){assert(monitors[i].width>0&&monitors[i].height>0);primary+=monitors[i].primary;} assert(primary==1); XRRFreeMonitors(monitors);
 XWindowAttributes attrs; assert(XGetWindowAttributes(d,w,&attrs));
 int ordering=-1; XRectangle *rects=XShapeGetRectangles(d,w,ShapeBounding,&n,&ordering); assert(rects&&n==1&&rects[0].width==attrs.width+2*attrs.border_width&&rects[0].height==attrs.height+2*attrs.border_width); XFree(rects);
 XEvent event; while(XPending(d)) XNextEvent(d,&event);
 for(int i=3;i>=1;i--) { memset(&event,0,sizeof(event)); event.xclient.type=ClientMessage; event.xclient.window=w; event.xclient.data.l[0]=i; XPutBackEvent(d,&event); }
 assert(!XCheckIfEvent(d,&event,selected,(void*)4));
 assert(XCheckIfEvent(d,&event,selected,(void*)2)&&event.xclient.data.l[0]==2);
 assert(XPeekIfEvent(d,&event,selected,(void*)3)==0&&event.xclient.data.l[0]==3);
 XNextEvent(d,&event); assert(event.xclient.data.l[0]==1);
 XNextEvent(d,&event); assert(event.xclient.data.l[0]==3);
 XUndefineCursor(d,w); XDestroyWindow(d,w); XCloseDisplay(d);
 puts("DESKTOP_CURSOR_SYNC_MONITORS_FORK_OK"); return 0;
}
