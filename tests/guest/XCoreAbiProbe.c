#include <X11/Xlib.h>
#include <X11/Xlibint.h>
#include <X11/Xutil.h>
#include <X11/Xatom.h>
#include <assert.h>
#include <stdio.h>
#include <string.h>
int main(void) {
    Display *d=XOpenDisplay(0);assert(d);Screen *s=ScreenOfDisplay(d,DefaultScreen(d));
    assert(XDefaultScreenOfDisplay(d)==s && XConnectionNumber(d)==ConnectionNumber(d));
    assert(XWidthOfScreen(s)==WidthOfScreen(s) && XHeightOfScreen(s)==HeightOfScreen(s));
    assert(XDefaultVisualOfScreen(s)==DefaultVisualOfScreen(s) && XDefaultColormapOfScreen(s)==DefaultColormapOfScreen(s));
    assert(XRootWindowOfScreen(s)==RootWindowOfScreen(s) && XDefaultDepthOfScreen(s)==DefaultDepthOfScreen(s));
    assert(XImageByteOrder(d)==ImageByteOrder(d) && XBitmapUnit(d)==BitmapUnit(d));
    assert(XDisplayHeightMM(d,0)==HeightMMOfScreen(s) && XDisplayWidthMM(d,0)==WidthMMOfScreen(s));
    assert(XDefaultGCOfScreen(s)==DefaultGCOfScreen(s) && XCellsOfScreen(s)==CellsOfScreen(s));
    assert(!XExtendedMaxRequestSize(d));
    unsigned char buttons[12];memset(buttons,0xff,sizeof buttons);
    assert(XGetPointerMapping(d,buttons,4)==9);
    for(int i=0;i<4;i++)assert(buttons[i]==i+1);assert(buttons[4]==0xff);
    unsigned bestw,besth;assert(XQueryBestCursor(d,DefaultRootWindow(d),128,128,&bestw,&besth));
    assert(bestw>0 && bestw<=128 && besth>0 && besth<=128);
    XIconSize *icon=XAllocIconSize();assert(icon && !icon->min_width);XFree(icon);
    XIconSize icons[2]={{16,16,128,128,16,16},{24,24,48,48,24,24}},*returned=0;int icon_count;
    assert(XSetIconSizes(d,DefaultRootWindow(d),icons,2));
    assert(XGetIconSizes(d,DefaultRootWindow(d),&returned,&icon_count));
    assert(icon_count==2 && !memcmp(returned,icons,sizeof icons));XFree(returned);
    char a[]="first",b[]={ (char)0xe9,0},c[]="";char *list[]={a,b,c};XTextProperty property;
    assert(XStringListToTextProperty(list,3,&property));char **copy=0;int count=-1;
    assert(XTextPropertyToStringList(&property,&copy,&count));assert(count==3 && copy && !copy[3]);
    assert(!strcmp(copy[0],a) && !strcmp(copy[1],b) && !strcmp(copy[2],c));
    a[0]='x';XFree(property.value);assert(!strcmp(copy[0],"first"));XFreeStringList(copy);
    property=(XTextProperty){.encoding=XA_ATOM,.format=8,.nitems=0};
    assert(!XTextPropertyToStringList(&property,&copy,&count) && !copy && !count);
    property.encoding=XA_STRING;assert(XTextPropertyToStringList(&property,&copy,&count) && !copy && !count);
    XCloseDisplay(d);puts("XCORE_ACCESSORS_TEXTLIST_OWNERSHIP_OK");return 0;
}
