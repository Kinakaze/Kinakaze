"""Render and read native GL pixels through an XCB window, also after fork."""
import ctypes as c
import os

P, I, U = c.c_void_p, c.c_int, c.c_uint
b = c.CDLL('libxcb.so.1')
e = c.CDLL('libEGL.so.1')
g = c.CDLL('libGL.so.1')


def bind(lib, name, result, args):
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args
    return fn


conn = bind(b, 'xcb_connect', P, [P, P])(None, None)

class Iterator(c.Structure):
    _fields_ = [('data', P), ('rem', I), ('index', I)]

setup = bind(b, 'xcb_get_setup', P, [P])(conn)
screen = bind(b, 'xcb_setup_roots_iterator', Iterator, [P])(setup)
depths = bind(b, 'xcb_screen_allowed_depths_iterator', Iterator, [P])(screen.data)
visuals = bind(b, 'xcb_depth_visuals_iterator', Iterator, [P])
next_depth = bind(b, 'xcb_depth_next', None, [P])
visual_depths = {}
while depths.rem:
    depth = c.c_ubyte.from_address(depths.data).value
    visual = visuals(depths.data)
    for n in range(visual.rem):
        visual_id = U.from_address(visual.data + 24 * n).value
        assert visual_id not in visual_depths  # A visual has exactly one depth.
        visual_depths[visual_id] = depth
    offset = depths.index
    next_depth(c.byref(depths))
    assert depths.index == offset + 8 + 24 * visual.rem
assert visual_depths[1] == 24
window = bind(b, 'xcb_generate_id', U, [P])(conn)
create = bind(b, 'xcb_create_window_checked', U,
              [P, c.c_ubyte, U, U, c.c_short, c.c_short, c.c_ushort,
               c.c_ushort, c.c_ushort, c.c_ushort, U, U, P])
cookie = create(conn, 24, window, 1, 0, 0, 32, 32, 0, 1, 1, 0, None)
assert not bind(b, 'xcb_request_check', P, [P, U])(conn, cookie)
get_display = bind(e, 'eglGetDisplay', P, [P])
initialize = bind(e, 'eglInitialize', U, [P, P, P])
configs = bind(e, 'eglGetConfigs', U, [P, P, I, P])
api = bind(e, 'eglBindAPI', U, [U])
create_surface = bind(e, 'eglCreateWindowSurface', P, [P, P, c.c_ulong, P])
create_context = bind(e, 'eglCreateContext', P, [P, P, P, P])
make_current = bind(e, 'eglMakeCurrent', U, [P, P, P, P])
destroy_context = bind(e, 'eglDestroyContext', U, [P, P])
destroy_surface = bind(e, 'eglDestroySurface', U, [P, P])
get_error = bind(e, 'eglGetError', I, [])
clear_color = bind(g, 'glClearColor', None, [c.c_float] * 4)
clear = bind(g, 'glClear', None, [U])
finish = bind(g, 'glFinish', None, [])
read = bind(g, 'glReadPixels', None, [I, I, I, I, U, U, P])
gl_error = bind(g, 'glGetError', U, [])


def render(rgb):
    display = get_display(None)
    assert display and initialize(display, None, None)
    config, count = P(), I()
    assert configs(display, c.byref(config), 1, c.byref(count)) and count.value
    assert api(0x30A2)  # EGL_OPENGL_API
    surface = create_surface(display, config, window, None)
    assert surface, hex(get_error())
    context = create_context(display, config, None, None)
    assert context and make_current(display, surface, surface, context)
    clear_color(*(v / 255 for v in rgb), 1)
    clear(0x4000)
    finish()
    pixel = (c.c_ubyte * 4)()
    read(0, 0, 1, 1, 0x1908, 0x1401, pixel)
    assert gl_error() == 0 and tuple(pixel[:3]) == rgb, tuple(pixel)
    assert make_current(display, None, None, None)
    assert destroy_context(display, context) and destroy_surface(display, surface)


# Only the XCB window exists at fork; each process creates its own native GL state.
child = os.fork()
if child == 0:
    try:
        render((0, 255, 0))
    except BaseException:
        import traceback
        traceback.print_exc()
        os._exit(1)
    os._exit(0)
assert os.waitpid(child, 0) == (child, 0)
render((255, 0, 0))
bind(b, 'xcb_destroy_window', U, [P, U])(conn, window)
display = get_display(None)
config, count = P(), I()
assert configs(display, c.byref(config), 1, c.byref(count))
assert not create_surface(display, config, window, None)
assert get_error() == 0x300B  # EGL_BAD_NATIVE_WINDOW: no stale XID mapping
bind(b, 'xcb_disconnect', None, [P])(conn)
print('XCB_EGL_PIXELS_NATIVE_WINDOW_FORK_DESTROY_OK')
