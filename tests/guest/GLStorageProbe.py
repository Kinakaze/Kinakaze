"""Exercise persistent GPU uploads and storage through the Linux GL ABI."""
import ctypes as C
import json

p, i, u, size, xid = C.c_void_p, C.c_int, C.c_uint, C.c_ssize_t, C.c_ulong


def bind(lib, name, result, *args):
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args
    return fn


x, gl = C.CDLL('libX11.so.6'), C.CDLL('libGL.so.1')
display = bind(x, 'XOpenDisplay', p, C.c_char_p)(None)
assert display
count = i()
configs = bind(gl, 'glXChooseFBConfig', C.POINTER(p), p, i, p, C.POINTER(i))(
    display, 0, None, C.byref(count))
assert configs and count.value
context = bind(gl, 'glXCreateNewContext', p, p, p, i, p, i)(
    display, configs[0], 0x8014, None, 1)
window = bind(x, 'XCreateSimpleWindow', xid, p, xid, i, i, u, u, u, xid, xid)(
    display, 1, 0, 0, 32, 24, 0, 0, 0)
current = bind(gl, 'glXMakeCurrent', i, p, xid, p)
assert context and window and current(display, window, context)
error = bind(gl, 'glGetError', u)
get_proc = bind(gl, 'glXGetProcAddress', p, C.c_char_p)
driver = bind(gl, 'glGetString', C.c_char_p, u)(0x1F01).decode()
buffers, textures = (u * 3)(), (u * 2)()
feedback, vao, framebuffer = u(), u(), u()
shader = program = 0
checked = []


def indirect(name, result, *args):
    address = get_proc(name.encode())
    assert address, name
    checked.append(name)
    return C.CFUNCTYPE(result, *args)(address)


def integer(name):
    value = i(-1)
    bind(gl, 'glGetIntegerv', None, u, C.POINTER(i))(name, C.byref(value))
    assert error() == 0
    return value.value


try:
    storage = bind(gl, 'glBufferStorage', None, u, size, p, u)
    storage_indirect = indirect('glBufferStorage', None, u, size, p, u)
    bind(gl, 'glGenBuffers', None, i, C.POINTER(u))(3, buffers)
    bind_buffer = bind(gl, 'glBindBuffer', None, u, u)
    buffer_query = bind(gl, 'glGetBufferParameteriv', None, u, u, C.POINTER(i))
    bind_buffer(0x88EC, buffers[0])  # GL_PIXEL_UNPACK_BUFFER
    storage(0x88EC, 64, None, 0x42)  # WRITE | PERSISTENT
    for name, expected in [(0x821F, 1), (0x8220, 0x42), (0x8764, 64)]:
        value = i(-1)
        buffer_query(0x88EC, name, C.byref(value))
        assert value.value == expected and error() == 0
    # Immutable storage cannot be silently reallocated.
    storage_indirect(0x88EC, 128, None, 0x42)
    assert error() == 0x0502
    mapped = bind(gl, 'glMapBufferRange', p, u, size, size, u)(0x88EC, 0, 64, 0x52)
    assert mapped and error() == 0  # WRITE | PERSISTENT | FLUSH_EXPLICIT
    pixels = bytes([25, 59, 87, 255]) * 16
    C.memmove(mapped, pixels, len(pixels))
    bind(gl, 'glFlushMappedBufferRange', None, u, size, size)(0x88EC, 0, 64)
    bind(gl, 'glGenTextures', None, i, C.POINTER(u))(2, textures)
    bind_texture = bind(gl, 'glBindTexture', None, u, u)
    bind_texture(0x0DE1, textures[0])
    indirect('glTexStorage2D', None, u, i, u, i, i)(0x0DE1, 2, 0x8058, 4, 4)
    bind(gl, 'glTexSubImage2D', None, u, i, i, i, i, i, u, u, p)(
        0x0DE1, 0, 0, 0, 4, 4, 0x1908, 0x1401, None)
    # Upload while the buffer remains mapped; this is WebRender's required path.
    actual = (C.c_ubyte * 64)()
    bind(gl, 'glGetTexImage', None, u, i, u, u, p)(0x0DE1, 0, 0x1908, 0x1401, actual)
    assert bytes(actual) == pixels and error() == 0
    assert bind(gl, 'glUnmapBuffer', C.c_ubyte, u)(0x88EC) == 1
    bind_buffer(0x88EC, 0)
    immutable = i()
    bind(gl, 'glGetTexParameteriv', None, u, u, C.POINTER(i))(0x0DE1, 0x912F, C.byref(immutable))
    assert immutable.value == 1
    supported = i()
    indirect('glGetInternalformativ', None, u, u, u, i, C.POINTER(i))(
        0x0DE1, 0x8058, 0x826F, 1, C.byref(supported))
    assert supported.value == 1 and error() == 0
    bind_texture(0x8C1A, textures[1])  # GL_TEXTURE_2D_ARRAY
    indirect('glTexStorage3D', None, u, i, u, i, i, i)(0x8C1A, 2, 0x8058, 4, 4, 2)
    depth = i()
    bind(gl, 'glGetTexLevelParameteriv', None, u, i, u, C.POINTER(i))(
        0x8C1A, 1, 0x8071, C.byref(depth))
    assert depth.value == 2 and error() == 0

    bind_buffer(0x8892, buffers[1])
    contents = (C.c_ubyte * 4)(17, 31, 97, 251)
    indirect('glNamedBufferStorage', None, u, size, p, u)(buffers[1], 4, contents, 0)
    copied = (C.c_ubyte * 4)()
    get_buffer = bind(gl, 'glGetBufferSubData', None, u, size, size, p)
    get_buffer(0x8892, 0, 4, copied)
    assert bytes(copied) == bytes(contents) and error() == 0

    # A vertex-only pipeline with rasterization discarded validates real
    # transform-feedback output as well as pause/resume state transitions.
    bind(gl, 'glGenVertexArrays', None, i, C.POINTER(u))(1, C.byref(vao))
    bind(gl, 'glBindVertexArray', None, u)(vao.value)
    indirect('glVertexAttribDivisorARB', None, u, u)(0, 3)
    divisor = i()
    bind(gl, 'glGetVertexAttribiv', None, u, u, C.POINTER(i))(0, 0x88FE, C.byref(divisor))
    assert divisor.value == 3 and error() == 0
    shader = bind(gl, 'glCreateShader', u, u)(0x8B31)
    source = C.c_char_p(b'#version 330 core\nout float result;\nvoid main() { result = float(gl_VertexID) * 2.0; gl_Position = vec4(0.0); }')
    bind(gl, 'glShaderSource', None, u, i, C.POINTER(C.c_char_p), C.POINTER(i))(
        shader, 1, C.byref(source), None)
    bind(gl, 'glCompileShader', None, u)(shader)
    status = i()
    bind(gl, 'glGetShaderiv', None, u, u, C.POINTER(i))(shader, 0x8B81, C.byref(status))
    assert status.value == 1
    program = bind(gl, 'glCreateProgram', u)()
    bind(gl, 'glAttachShader', None, u, u)(program, shader)
    varying = C.c_char_p(b'result')
    bind(gl, 'glTransformFeedbackVaryings', None, u, i, C.POINTER(C.c_char_p), u)(
        program, 1, C.byref(varying), 0x8C8C)
    bind(gl, 'glLinkProgram', None, u)(program)
    bind(gl, 'glGetProgramiv', None, u, u, C.POINTER(i))(program, 0x8B82, C.byref(status))
    assert status.value == 1
    bind(gl, 'glUseProgram', None, u)(program)
    indirect('glGenTransformFeedbacks', None, i, C.POINTER(u))(1, C.byref(feedback))
    bind_feedback = indirect('glBindTransformFeedback', None, u, u)
    bind_feedback(0x8E22, feedback.value)
    assert indirect('glIsTransformFeedback', C.c_ubyte, u)(feedback.value) == 1
    bind_buffer(0x8C8E, buffers[2])
    bind(gl, 'glBufferData', None, u, size, p, u)(0x8C8E, 12, None, 0x88E1)
    bind(gl, 'glBindBufferBase', None, u, u, u)(0x8C8E, 0, buffers[2])
    bind(gl, 'glEnable', None, u)(0x8C89)
    bind(gl, 'glBeginTransformFeedback', None, u)(0)
    assert integer(0x8E24) == 1
    indirect('glPauseTransformFeedback', None)()
    assert integer(0x8E23) == 1
    indirect('glResumeTransformFeedback', None)()
    assert integer(0x8E23) == 0
    bind(gl, 'glDrawArrays', None, u, i, i)(0, 0, 3)
    bind(gl, 'glEndTransformFeedback', None)()
    bind(gl, 'glDisable', None, u)(0x8C89)
    values = (C.c_float * 3)()
    get_buffer(0x8C8E, 0, 12, values)
    assert list(values) == [0.0, 2.0, 4.0] and error() == 0
    bind_feedback(0x8E22, 0)
    indirect('glDeleteTransformFeedbacks', None, i, C.POINTER(u))(1, C.byref(feedback))
    assert bind(gl, 'glIsTransformFeedback', C.c_ubyte, u)(feedback.value) == 0
    feedback.value = 0

    indirect('glBlendEquationSeparatei', None, u, u, u)(0, 0x800A, 0x8006)
    indirect('glBlendFuncSeparatei', None, u, u, u, u, u)(0, 1, 0, 0, 1)
    for name, expected in [(0x8009, 0x800A), (0x883D, 0x8006), (0x80C9, 1), (0x80C8, 0)]:
        value = i()
        bind(gl, 'glGetIntegeri_v', None, u, u, C.POINTER(i))(name, 0, C.byref(value))
        assert value.value == expected and error() == 0
    bind(gl, 'glGenFramebuffers', None, i, C.POINTER(u))(1, C.byref(framebuffer))
    bind(gl, 'glBindFramebuffer', None, u, u)(0x8D40, framebuffer.value)
    bind(gl, 'glFramebufferTexture2D', None, u, u, u, u, i)(0x8D40, 0x8CE0, 0x0DE1, textures[0], 0)
    assert bind(gl, 'glCheckFramebufferStatus', u, u)(0x8D40) == 0x8CD5
    attachment = u(0x8CE0)
    indirect('glInvalidateFramebuffer', None, u, i, C.POINTER(u))(0x8D40, 1, C.byref(attachment))
    indirect('glInvalidateSubFramebuffer', None, u, i, C.POINTER(u), i, i, i, i)(
        0x8D40, 1, C.byref(attachment), 0, 0, 2, 2)
    indirect('glMemoryBarrier', None, u)(0x4000)
    assert error() == 0
finally:
    if framebuffer.value:
        bind(gl, 'glDeleteFramebuffers', None, i, C.POINTER(u))(1, C.byref(framebuffer))
    if feedback.value:
        bind(gl, 'glDeleteTransformFeedbacks', None, i, C.POINTER(u))(1, C.byref(feedback))
    if program:
        bind(gl, 'glUseProgram', None, u)(0)
        bind(gl, 'glDeleteProgram', None, u)(program)
    if shader:
        bind(gl, 'glDeleteShader', None, u)(shader)
    if vao.value:
        bind(gl, 'glDeleteVertexArrays', None, i, C.POINTER(u))(1, C.byref(vao))
    bind(gl, 'glDeleteTextures', None, i, C.POINTER(u))(2, textures)
    bind(gl, 'glDeleteBuffers', None, i, C.POINTER(u))(3, buffers)
    current(display, 0, None)
    bind(gl, 'glXDestroyContext', None, p, p)(display, context)
    bind(x, 'XDestroyWindow', i, p, xid)(display, window)
    bind(x, 'XFree', i, p)(configs)
    bind(x, 'XCloseDisplay', i, p)(display)
print(json.dumps({'renderer': driver, 'checked': checked}), flush=True)
print('GL_PERSISTENT_STORAGE_GPU_OK', flush=True)
