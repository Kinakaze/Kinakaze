"""Named parent pixmaps include changed child pixels, also across processes."""
import ctypes as c
import os

x = c.CDLL('libX11.so.6')
co = c.CDLL('libXcomposite.so.1')
p, u, i = c.c_void_p, c.c_ulong, c.c_int
def bind(lib, name, result, *args):
    fn = getattr(lib, name); fn.restype = result; fn.argtypes = args; return fn
open_display = bind(x, 'XOpenDisplay', p, p)
create = bind(x, 'XCreateSimpleWindow', u, p, u, i, i, c.c_uint, c.c_uint, c.c_uint, u, u)
map_window = bind(x, 'XMapWindow', i, p, u)
gc_create = bind(x, 'XCreateGC', p, p, u, u, p)
foreground = bind(x, 'XSetForeground', i, p, p, u)
fill = bind(x, 'XFillRectangle', i, p, u, p, i, i, c.c_uint, c.c_uint)
flush = bind(x, 'XFlush', i, p)
get_image = bind(x, 'XGetImage', p, p, u, i, i, c.c_uint, c.c_uint, u, i)
pixel = bind(x, 'XGetPixel', u, p, i, i)
destroy_image = bind(x, 'XDestroyImage', i, p)
redirect = bind(co, 'XCompositeRedirectWindow', None, p, u, i)
name_pixmap = bind(co, 'XCompositeNameWindowPixmap', u, p, u)
d = open_display(None); assert d
parent = create(d, 1, 100, 100, 160, 120, 0, 0, 0)
redirect(d, parent, 1); map_window(d, parent)
named = name_pixmap(d, parent); assert named
def paint(display, parent, color):
    child = create(display, 1, 12, 10, 48, 40, 0, 0, 0)
    assert child
    assert bind(x, 'XReparentWindow', i, p, u, u, i, i)(display, child, parent, 12, 10)
    map_window(display, child); gc = gc_create(display, child, 0, None)
    foreground(display, gc, color); fill(display, child, gc, 0, 0, 48, 40); flush(display)
    return child
def check(color):
    image = get_image(d, named, 0, 0, 160, 120, u(-1).value, 2); assert image
    try: assert pixel(image, 20, 20) & 0xffffff == color, (hex(pixel(image, 20, 20)), hex(color))
    finally: destroy_image(image)
child = paint(d, parent, 0x22bb66); check(0x22bb66)
bind(x, 'XDestroyWindow', i, p, u)(d, child)
reader, writer = os.pipe(); done_reader, done_writer = os.pipe()
pid = os.fork()
if pid == 0:
    try:
        foreign = open_display(None); paint(foreign, parent, 0xcc4422)
        os.write(writer, b'1'); os.read(done_reader, 1)
    finally: os._exit(0)
os.close(writer); os.close(done_reader)
try:
    assert os.read(reader, 1) == b'1'; check(0xcc4422)
finally:
    os.write(done_writer, b'1'); assert os.waitpid(pid, 0)[1] == 0
    bind(x, 'XDestroyWindow', i, p, u)(d, parent)
    bind(x, 'XCloseDisplay', i, p)(d)
print('COMPOSITE_LOCAL_AND_FOREIGN_CHILD_PIXELS_OK', flush=True)
