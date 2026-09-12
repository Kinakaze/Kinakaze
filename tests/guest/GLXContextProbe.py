"""Check GLX context metadata through both native guest library entry points."""
import ctypes as c

p, i, u = c.c_void_p, c.c_int, c.c_ulong


def bind(lib, name, result, *args):
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args
    return fn


x = c.CDLL('libX11.so.6')
gl = c.CDLL('libGL.so.1')
glx = c.CDLL('libGLX.so.0')
d = bind(x, 'XOpenDisplay', p, c.c_char_p)(None)
assert d
free = bind(x, 'XFree', i, p)
count = i()
configs = bind(gl, 'glXChooseFBConfig', c.POINTER(p), p, i, p, c.POINTER(i))(
    d, 0, None, c.byref(count))
assert configs and count.value > 0
config = configs[0]
config_id = i()
assert bind(gl, 'glXGetFBConfigAttrib', i, p, p, i, c.POINTER(i))(
    d, config, 0x8013, c.byref(config_id)) == 0
visual = bind(gl, 'glXGetVisualFromFBConfig', p, p, p)(d, config)
assert visual
create = bind(gl, 'glXCreateNewContext', p, p, p, i, p, i)
contexts = [
    bind(gl, 'glXCreateContext', p, p, p, p, i)(d, visual, None, 1),
    create(d, config, 0x8014, None, 1),
    bind(gl, 'glXCreateContextAttribsARB', p, p, p, p, i, p)(d, config, None, 1, None),
]
assert all(contexts)
assert not create(d, config, 0x8015, None, 1)  # Unsupported color-index rendering.
assert not create(d, p(-1), 0x8014, None, 1)
queries = []
for lib in (gl, glx):
    extensions = bind(lib, 'glXQueryExtensionsString', c.c_char_p, p, i)(d, 0)
    assert b'GLX_ARB_get_proc_address' in extensions.split()
    address = bind(lib, 'glXGetProcAddress', p, c.c_char_p)(b'glXQueryContext')
    assert address
    assert bind(lib, 'glXGetProcAddressARB', p, c.c_char_p)(b'glXQueryContext') == address
    queries.append(bind(lib, 'glXQueryContext', i, p, p, i, c.POINTER(i)))


def check(ctx):
    for query in queries:
        for attribute, expected in ((0x8013, config_id.value), (0x8011, 0x8014), (0x800c, 0)):
            value = i(-1)
            assert query(d, ctx, attribute, c.byref(value)) == 0
            assert value.value == expected, (attribute, value.value, expected)
        value = i(123)
        assert query(d, ctx, -1, c.byref(value)) == 2 and value.value == 123
        assert query(None, ctx, 0x800c, c.byref(value)) == 5 and value.value == 123
        assert query(d, None, 0x800c, c.byref(value)) == 5 and value.value == 123
        assert query(d, ctx, 0x800c, None) == 6


w = bind(x, 'XCreateSimpleWindow', u, p, u, i, i, c.c_uint, c.c_uint, c.c_uint, u, u)(
    d, 1, 0, 0, 32, 24, 0, 0, 0)
assert w
make_current = bind(gl, 'glXMakeCurrent', i, p, u, p)
destroy = bind(gl, 'glXDestroyContext', None, p, p)
for ctx in contexts:
    check(ctx)
    assert make_current(d, w, ctx)
    check(ctx)
    assert make_current(d, 0, None)
    destroy(d, ctx)
    value = i(123)
    assert queries[0](d, ctx, 0x800c, c.byref(value)) == 5 and value.value == 123
bind(gl, 'glXDestroyWindow', None, p, u)(d, w)
bind(x, 'XDestroyWindow', i, p, u)(d, w)
free(visual)
free(configs)
bind(x, 'XCloseDisplay', i, p)(d)
print('GLX_CONTEXT_QUERY_AND_LIFETIME_OK', flush=True)
