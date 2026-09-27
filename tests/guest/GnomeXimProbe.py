"""Verify the XIM server's selection and actual XCONNECT reply on its X queue.

Run as a GNOME --app in the same runtime, with a bounded owned session.
This does not type physical keys or test composition in an application.
"""
import ctypes as c
import time


class Data(c.Union):
    _fields_ = [('b', c.c_char * 20), ('s', c.c_short * 10), ('l', c.c_long * 5)]


class Client(c.Structure):
    _fields_ = [('type', c.c_int), ('serial', c.c_ulong), ('send_event', c.c_int),
                ('display', c.c_void_p), ('window', c.c_ulong),
                ('message_type', c.c_ulong), ('format', c.c_int), ('data', Data)]


class Event(c.Union):
    _fields_ = [('client', Client), ('pad', c.c_long * 24)]


def check():
    assert c.sizeof(c.c_long) == 8, 'Run under guest Linux Python'
    x = c.CDLL('libX11.so.6')
    def bind(name, result, *args):
        f = getattr(x, name)
        f.restype, f.argtypes = result, args
        return f
    display = bind('XOpenDisplay', c.c_void_p, c.c_char_p)(None)
    assert display
    atom = bind('XInternAtom', c.c_ulong, c.c_void_p, c.c_char_p, c.c_int)
    owner = bind('XGetSelectionOwner', c.c_ulong, c.c_void_p, c.c_ulong)
    create = bind('XCreateSimpleWindow', c.c_ulong, c.c_void_p, c.c_ulong,
                  c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_uint, c.c_ulong, c.c_ulong)
    send = bind('XSendEvent', c.c_int, c.c_void_p, c.c_ulong, c.c_int, c.c_long, c.POINTER(Event))
    flush = bind('XFlush', c.c_int, c.c_void_p)
    pending = bind('XPending', c.c_int, c.c_void_p)
    next_event = bind('XNextEvent', c.c_int, c.c_void_p, c.POINTER(Event))
    destroy = bind('XDestroyWindow', c.c_int, c.c_void_p, c.c_ulong)
    close = bind('XCloseDisplay', c.c_int, c.c_void_p)
    window = 0
    try:
        selection = atom(display, b'@server=ibus', 0)
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            server = owner(display, selection)
            if server:
                break
            time.sleep(.02)
        else:
            raise RuntimeError('IBus XIM selection has no owner')
        connect = atom(display, b'_XIM_XCONNECT', 0)
        window = create(display, 1, 0, 0, 1, 1, 0, 0, 0)
        assert window
        event = Event()
        event.client.type = 33
        event.client.display = display
        event.client.window = server
        event.client.message_type = connect
        event.client.format = 32
        event.client.data.l[0] = window
        assert send(display, server, 0, 0, c.byref(event))
        flush(display)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            while pending(display):
                reply = Event()
                next_event(display, c.byref(reply))
                if reply.client.type == 33 and reply.client.message_type == connect:
                    assert reply.client.window == window and reply.client.data.l[0] != 0
                    print('XIM_XCONNECT_OK ' + str(reply.client.data.l[0]), flush=True)
                    return
            time.sleep(.01)
        raise RuntimeError('IBus XIM did not reply to XCONNECT')
    finally:
        if window:
            destroy(display, window)
        close(display)


if __name__ == '__main__':
    check()
