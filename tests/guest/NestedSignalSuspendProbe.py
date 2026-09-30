"""JSC suspends a thread inside a handler, then resumes it with the same signal."""
import ctypes as c
import errno

lib = c.CDLL(None, use_errno=True)
callback = c.CFUNCTYPE(None, c.c_int)
lib.signal.argtypes = [c.c_int, callback]
lib.signal.restype = c.c_void_p
lib.pthread_self.restype = c.c_size_t
lib.pthread_kill.argtypes = [c.c_size_t, c.c_int]
lib.sigsuspend.argtypes = [c.c_void_p]
lib.sigprocmask.argtypes = [c.c_int, c.c_void_p, c.c_void_p]
signal = 12
events = []
errors = []

@callback
def handler(number):
    try:
        events.append(number)
        if len(events) == 1:
            assert lib.pthread_kill(lib.pthread_self(), signal) == 0
            assert len(events) == 1  # Normally deferred by the handler mask.
            empty = (c.c_ulong * 16)()
            assert lib.sigsuspend(empty) == -1 and c.get_errno() == errno.EINTR
            assert len(events) == 2
            mask = (c.c_ulong * 16)()
            assert lib.sigprocmask(2, None, mask) == 0
            assert mask[0] & (1 << (signal - 1))  # Restore the handler mask.
            events.append('resumed')
    except BaseException as error:
        errors.append(str(error))

assert lib.signal(signal, handler) != c.c_void_p(-1).value
assert lib.pthread_kill(lib.pthread_self(), signal) == 0
assert not errors, errors
assert events == [signal, signal, 'resumed'], events

class SigAction(c.Structure):
    _fields_ = [('handler', c.c_void_p), ('mask', c.c_ulong * 16),
                ('flags', c.c_int), ('restorer', c.c_void_p)]

lib.sigaction.argtypes = [c.c_int, c.POINTER(SigAction), c.c_void_p]
events.clear()

@callback
def nodefer_handler(number):
    try:
        events.append(number)
        mask = (c.c_ulong * 16)()
        assert lib.sigprocmask(2, None, mask) == 0
        assert mask[0] & (1 << 9)  # sa_mask applies even with SA_NODEFER.
        assert not mask[0] & (1 << (signal - 1))
        if len(events) == 1:
            assert lib.pthread_kill(lib.pthread_self(), signal) == 0
            assert len(events) == 2
    except BaseException as error:
        errors.append(str(error))

action = SigAction(handler=c.cast(nodefer_handler, c.c_void_p), flags=0x40000000)
action.mask[0] = 1 << 9
assert lib.sigaction(signal, c.byref(action), None) == 0
assert lib.pthread_kill(lib.pthread_self(), signal) == 0
assert not errors, errors
assert events == [signal, signal], events
print('NESTED_SIGNAL_SUSPEND_OK', flush=True)
