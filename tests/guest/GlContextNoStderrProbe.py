"""GL context setup must survive an absent/unwritable native stderr.

The host harness starts worker with a read-only standard error handle, both
with and without KINAKAZE_GL_TRACE. No GPU window is needed for this case.
"""
import ctypes

gl = ctypes.CDLL("libGL.so.1")
gl.glXCreateContextAttribsARB.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int, ctypes.POINTER(ctypes.c_int)]
gl.glXCreateContextAttribsARB.restype = ctypes.c_void_p
gl.glXDestroyContext.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
attributes = (ctypes.c_int * 5)(0x2091, 3, 0x2092, 2, 0)
context = gl.glXCreateContextAttribsARB(None, 1, None, 1, attributes)
assert context, "GL context allocation failed"
gl.glXDestroyContext(None, context)
print("GL_CONTEXT_UNWRITABLE_STDERR_OK", flush=True)
