#include <X11/Xlib.h>
#include <X11/keysym.h>
#include <X11/Xutil.h>
#include <X11/Xatom.h>
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static void values(XIC ic,Window expected) {
    XIMStyle style=0;Window client=0,focus=0;long filter=-1;
    assert(!XGetICValues(ic,XNInputStyle,&style,XNClientWindow,&client,
        XNFocusWindow,&focus,XNFilterEvents,&filter,NULL));
    assert(style==(XIMPreeditNothing|XIMStatusNothing) && client==1 && focus==expected && filter==0);
}
static void text(XIC ic,Display *d) {
    XKeyEvent event={0};event.type=KeyPress;event.display=d;event.window=1;
    event.keycode=XKeysymToKeycode(d,XK_a);assert(event.keycode);
    char output[8];memset(output,0x5a,sizeof output);KeySym sym=0xdead;Status status=99;
    assert(Xutf8LookupString(ic,&event,output,0,&sym,&status)==1 && status==XBufferOverflow);
    assert(output[0]==0x5a && sym==0xdead);
    assert(Xutf8LookupString(ic,&event,output,1,&sym,&status)==1);
    assert(output[0]=='a' && output[1]==0x5a && sym==XK_a && status==XLookupBoth);
    event.state=ShiftMask;assert(XmbLookupString(ic,&event,output,8,&sym,&status)==1 && output[0]=='A' && sym==XK_A);
    event.state=LockMask;wchar_t wide[2]={0,0x1234};
    assert(XwcLookupString(ic,&event,wide,1,&sym,&status)==1 && wide[0]==L'A' && wide[1]==0x1234);
    event.state=ShiftMask|LockMask;assert(Xutf8LookupString(ic,&event,output,8,&sym,&status)==1 && output[0]=='a');
    event.state=ControlMask;assert(Xutf8LookupString(ic,&event,output,8,&sym,&status)==1 && output[0]==1);
    event.state=0;event.keycode=XKeysymToKeycode(d,XK_Left);
    assert(Xutf8LookupString(ic,&event,output,8,&sym,&status)==0 && sym==XK_Left && status==XLookupKeySym);
    event.type=KeyRelease;assert(!Xutf8LookupString(ic,&event,output,8,&sym,&status) && status==XLookupNone);
    assert(!Xutf8ResetIC(ic) && !XmbResetIC(ic) && !XwcResetIC(ic));
}
int main(void) {
    Display *d=XOpenDisplay(NULL);assert(d);assert(!XOpenIM(NULL,NULL,NULL,NULL));
    char keymap[34];memset(keymap,0x5a,sizeof keymap);
    assert(XQueryKeymap(d,keymap+1));assert(keymap[0]==0x5a && keymap[33]==0x5a && keymap[1]==0);
    assert(XInternAtom(d,"STRING",True)==XA_STRING);
    char first[]="one",second[]="two";char *list[]={first,second};XTextProperty property;
    assert(XStringListToTextProperty(list,2,&property) && property.value!=(unsigned char *)first);
    assert(property.encoding==XA_STRING && property.format==8 && property.nitems==7 && !memcmp(property.value,"one\0two\0",8));
    first[0]='X';assert(property.value[0]=='o');XFree(property.value);
    char unicode[]="\xc3\xa9\xe4\xb8\xad";list[0]=unicode;list[1]=(char *)"";
    assert(!Xutf8TextListToTextProperty(d,list,2,XUTF8StringStyle,&property));
    assert(property.encoding==XInternAtom(d,"UTF8_STRING",True) && property.nitems==6 && !memcmp(property.value,unicode,5));XFree(property.value);
    list[0]=(char *)"\xc3\xa9";
    assert(!XmbTextListToTextProperty(d,list,1,XStringStyle,&property));
    assert(property.encoding==XA_STRING && property.nitems==1 && property.value[0]==0xe9 && property.value[1]==0);XFree(property.value);
    assert(Xutf8TextListToTextProperty(d,list,1,XCompoundTextStyle,&property)==XConverterNotFound);
    assert(XStringListToTextProperty(NULL,0,&property) && property.nitems==0 && property.value[0]==0);XFree(property.value);
    XIM im=XOpenIM(d,NULL,"probe","Probe");assert(im && XDisplayOfIM(im)==d);
    XIMStyles *styles=NULL;assert(!XGetIMValues(im,XNQueryInputStyle,&styles,NULL) && styles && styles->count_styles==2);
    for(int i=0;i<styles->count_styles;i++)assert(!(styles->supported_styles[i]&(XIMPreeditCallbacks|XIMPreeditPosition|XIMPreeditArea|XIMStatusArea|XIMStatusCallbacks)));
    XFree(styles);
    char unsupported[]="unsupportedAttribute";
    assert(XSetIMValues(im,unsupported,0,NULL)==unsupported);
    assert(!XCreateIC(im,XNClientWindow,(Window)1,NULL));
    assert(!XCreateIC(im,XNInputStyle,XIMPreeditCallbacks|XIMStatusNothing,NULL));
    XIC ic=XCreateIC(im,XNInputStyle,XIMPreeditNothing|XIMStatusNothing,
        XNClientWindow,(Window)1,XNFocusWindow,(Window)2,NULL);
    assert(ic && (void *)ic!=(void *)1 && XIMOfIC(ic)==im);values(ic,2);
    XSetICFocus(ic);XUnsetICFocus(ic);
    assert(!XSetICValues(ic,XNFocusWindow,(Window)3,NULL));values(ic,3);
    assert(XSetICValues(ic,unsupported,0,NULL)==unsupported);
    assert(XGetICValues(ic,unsupported,NULL,NULL)==unsupported);
    text(ic,d);
    pid_t child=fork();assert(child>=0);
    if(!child) {
        assert(XDisplayOfIM(im)==XOpenDisplay(NULL));values(ic,3);
        assert(!XSetICValues(ic,XNFocusWindow,(Window)4,NULL));values(ic,4);
        XDestroyIC(ic);assert(XCloseIM(im));_exit(19);
    }
    int status;assert(waitpid(child,&status,0)==child && WIFEXITED(status) && WEXITSTATUS(status)==19);
    values(ic,3);XDestroyIC(ic);
    assert(XCreateIC(im,XNInputStyle,XIMPreeditNone|XIMStatusNone,NULL));
    assert(XCloseIM(im));XCloseDisplay(d);
    puts("XIM_ATTRIBUTES_LOOKUP_OWNERSHIP_FORK_OK");return 0;
}
