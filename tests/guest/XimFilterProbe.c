/* Xlib filter ABI and fork probe, linked as a small guest ELF shared object. */
typedef unsigned long Window;
typedef struct Display Display;
typedef union Event {
    struct { int type; unsigned long serial; int send_event; Display *display;
             Window window; } any;
    long pad[24];
} Event;
typedef int (*Filter)(Display *, Window, Event *, void *);
extern Display *XOpenDisplay(const char *);
extern int XCloseDisplay(Display *);
extern int XFilterEvent(Event *, Window);
extern void _XRegisterFilterByType(Display *, Window, int, int, Filter, void *);
extern void _XUnregisterFilter(Display *, Window, Filter, void *);
extern int fork(void);
extern int waitpid(int, int *, int);
extern void _exit(int);

static int consume(Display *display, Window window, Event *event, void *data) {
    int *calls = data;
    if (!display || window != 1 || event->any.type != 33) return 0;
    ++*calls;
    _XUnregisterFilter(display, window, consume, data);
    return 1;
}

int xim_filter_fork_probe(void) {
    Display *display = XOpenDisplay((void *)0);
    if (!display) return 1;
    int calls = 0;
    Event event = {0};
    event.any.type = 33;
    event.any.display = display;
    event.any.window = 1;
    _XRegisterFilterByType(display, 1, 33, 33, consume, &calls);
    int child = fork();
    if (child == 0) {
        int ok = XFilterEvent(&event, 0) == 1 && calls == 1
              && XFilterEvent(&event, 0) == 0;
        _exit(ok ? 0 : 2);
    }
    int status = -1;
    int ok = child > 0 && waitpid(child, &status, 0) == child && status == 0;
    /* The child's unregistration and data write must not alter the parent. */
    ok = ok && calls == 0 && XFilterEvent(&event, 0) == 1 && calls == 1
         && XFilterEvent(&event, 0) == 0;
    _XUnregisterFilter(display, 1, consume, &calls);
    XCloseDisplay(display);
    return ok ? 0 : 3;
}
