"""Exercise texture generation through the guest ABI and the native GL driver."""
import ctypes as C
import json


def bind(library, name, result, *arguments):
    function = getattr(library, name)
    function.restype, function.argtypes = result, arguments
    return function


p, i, u, xid = C.c_void_p, C.c_int, C.c_uint, C.c_ulong
x = C.CDLL('libX11.so.6')
gl = C.CDLL('libGL.so.1')
display = bind(x, 'XOpenDisplay', p, C.c_char_p)(None)
assert display
count = i()
configs = bind(gl, 'glXChooseFBConfig', C.POINTER(p), p, i, p, C.POINTER(i))(
    display, 0, None, C.byref(count))
assert configs and count.value
context = bind(gl, 'glXCreateNewContext', p, p, p, i, p, i)(
    display, configs[0], 0x8014, None, 1)
assert context
window = bind(x, 'XCreateSimpleWindow', xid, p, xid, i, i, u, u, u, xid, xid)(
    display, 1, 0, 0, 32, 24, 0, 0, 0)
current = bind(gl, 'glXMakeCurrent', i, p, xid, p)
assert window and current(display, window, context)
get_error = bind(gl, 'glGetError', u)
get_proc = bind(gl, 'glXGetProcAddress', p, C.c_char_p)
driver = bind(gl, 'glGetString', C.c_char_p, u)(0x1F01).decode()
S, T, MODE, OBJECT_PLANE, EYE_PLANE = 0x2000, 0x2001, 0x2500, 0x2501, 0x2502
OBJECT_LINEAR, EYE_LINEAR = 0x2401, 0x2400
checked = []
try:
    bind(gl, 'glMatrixMode', None, u)(0x1700)
    bind(gl, 'glLoadIdentity', None)()
    assert get_error() == 0
    for suffix, scalar in [('d', C.c_double), ('f', C.c_float), ('i', i)]:
        getter_name = 'glGetTexGen' + suffix + 'v'
        getter = bind(gl, getter_name, None, u, u, C.POINTER(scalar))
        address = get_proc(getter_name.encode())
        assert address
        indirect_get = C.CFUNCTYPE(None, u, u, C.POINTER(scalar))(address)
        for vector in (False, True):
            name = 'glTexGen' + suffix + ('v' if vector else '')
            arg = C.POINTER(scalar) if vector else scalar
            direct = bind(gl, name, None, u, u, arg)
            address = get_proc(name.encode())
            assert address
            indirect = C.CFUNCTYPE(None, u, u, arg)(address)
            for mode, setter, query in [(OBJECT_LINEAR, direct, indirect_get),
                                         (EYE_LINEAR, indirect, getter)]:
                value = (scalar * 1)(mode) if vector else scalar(mode)
                setter(S, MODE, value)
                result = (scalar * 1)(-1)
                query(S, MODE, result)
                assert result[0] == mode and get_error() == 0, name
            if vector:
                values = [3, -2, 7, 1] if scalar == i else [0.25, -1.5, 3.75, 1.0]
                for plane in (OBJECT_PLANE, EYE_PLANE):
                    indirect(T, plane, (scalar * 4)(*values))
                    result = (scalar * 4)()
                    getter(T, plane, result)
                    assert list(result) == values and get_error() == 0, name
            checked.append(name)
        untouched = (scalar * 4)(11, 12, 13, 14)
        indirect_get(0xDEAD, OBJECT_PLANE, untouched)
        assert get_error() == 0x0500 and list(untouched) == [11, 12, 13, 14]
        checked.append(getter_name)
finally:
    current(display, 0, None)
    bind(gl, 'glXDestroyContext', None, p, p)(display, context)
    bind(x, 'XDestroyWindow', i, p, xid)(display, window)
    bind(x, 'XFree', i, p)(configs)
    bind(x, 'XCloseDisplay', i, p)(display)
print(json.dumps({'renderer': driver, 'checked': checked}), flush=True)
print('GL_TEXGEN_NATIVE_STATE_OK', flush=True)
