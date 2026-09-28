"""The default Unix PAM stack must reject an incorrect password."""
import ctypes as c


class Message(c.Structure):
    _fields_ = [('style', c.c_int), ('text', c.c_char_p)]


class Response(c.Structure):
    _fields_ = [('text', c.c_void_p), ('code', c.c_int)]


Callback = c.CFUNCTYPE(c.c_int, c.c_int, c.POINTER(c.POINTER(Message)),
                      c.POINTER(c.POINTER(Response)), c.c_void_p)
libc = c.CDLL('libc.so.6')
libc.calloc.argtypes, libc.calloc.restype = [c.c_size_t, c.c_size_t], c.c_void_p
libc.strdup.argtypes, libc.strdup.restype = [c.c_char_p], c.c_void_p
libc.openlog(b'pam-runtime-probe', 32, 0)


@Callback
def converse(count, messages, result, unused):
    responses = c.cast(libc.calloc(count, c.sizeof(Response)), c.POINTER(Response))
    if not responses:
        return 5  # PAM_BUF_ERR
    for index in range(count):
        if messages[index].contents.style in (1, 2):
            responses[index].text = libc.strdup(b'kinakaze-deliberately-incorrect-password')
    result[0] = responses  # PAM owns and frees these libc allocations.
    return 0


class Conversation(c.Structure):
    _fields_ = [('callback', Callback), ('data', c.c_void_p)]


pam = c.CDLL('libpam.so.0')
handle = c.c_void_p()
conversation = Conversation(converse, None)
assert pam.pam_start(b'login', b'root', c.byref(conversation), c.byref(handle)) == 0
try:
    result = pam.pam_authenticate(handle, 0)
    assert result == 7, ('expected PAM_AUTH_ERR, got', result)
finally:
    pam.pam_end(handle, 0)
print('PAM_PASSWORD_REJECTION_OK')
