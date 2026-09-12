"""A WM process must be able to restack windows belonging to another client."""
import ctypes as C
import os

x = C.CDLL('libX11.so.6')
p, u, i = C.c_void_p, C.c_ulong, C.c_int
def bind(name, result, *args):
    function = getattr(x, name)
    function.restype, function.argtypes = result, args
    return function
open_display = bind('XOpenDisplay', p, p)
create = bind('XCreateSimpleWindow', u, p, u, i, i, C.c_uint, C.c_uint, C.c_uint, u, u)
flush = bind('XSync', i, p, i)
restack = bind('XRestackWindows', i, p, C.POINTER(u), i)
query = bind('XQueryTree', i, p, u, p, p, p, p)
free = bind('XFree', i, p)
errors = []
@C.CFUNCTYPE(i, p, p)
def error_handler(display, error):
    errors.append(C.string_at(error, 40).hex())
    return 0
bind('XSetErrorHandler', p, p)(error_handler)

reader, writer = os.pipe()
done_reader, done_writer = os.pipe()
child = os.fork()
if child == 0:
    try:
        display = open_display(None)
        first = create(display, 1, 80, 80, 80, 70, 0, 0, 0)
        second = create(display, 1, 90, 90, 80, 70, 0, 0, 0)
        os.write(writer, f'{first},{second}'.encode())
        os.read(done_reader, 1)
    finally:
        os._exit(0)
os.close(writer)
os.close(done_reader)
try:
    first, second = map(int, os.read(reader, 100).decode().split(','))
    display = open_display(None)
    for ordered in ((first, second), (second, first)):
        assert restack(display, (u * 2)(*ordered), 2) != 0, errors
        flush(display, 0)
        assert not errors, errors
        root, parent, children, count = u(), u(), C.POINTER(u)(), C.c_uint()
        assert query(display, 1, C.byref(root), C.byref(parent), C.byref(children), C.byref(count))
        try:
            members = list(children[:count.value])
            assert members.index(ordered[0]) > members.index(ordered[1]), members
        finally:
            free(children)
finally:
    os.write(done_writer, b'1')
    os.close(reader)
    os.close(done_writer)
    assert os.waitpid(child, 0)[1] == 0
print('FOREIGN_WINDOW_RESTACK_OK', flush=True)
