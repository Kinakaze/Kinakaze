"""Check Pulse mixer ABI, asynchronous lifecycle and GLib mainloop integration."""
import ctypes as c
import math

p, i, u = c.c_void_p, c.c_int, c.c_uint
pa = c.CDLL('libpulse.so.0')

def bind(lib, name, result, *args):
    f = getattr(lib, name)
    f.restype, f.argtypes = result, args
    return f

class Volume(c.Structure):
    _fields_ = [('channels', c.c_ubyte), ('values', u * 32)]

class ChannelMap(c.Structure):
    _fields_ = [('channels', c.c_ubyte), ('positions', i * 32)]

props = bind(pa, 'pa_proplist_new', p)()
sets = bind(pa, 'pa_proplist_sets', i, p, c.c_char_p, c.c_char_p)
gets = bind(pa, 'pa_proplist_gets', c.c_char_p, p, c.c_char_p)
assert sets(props, b'application.name', b'Mixer probe') == 0
assert sets(props, b'media.role', b'test') == 0
assert gets(props, b'application.name') == b'Mixer probe'
assert sets(props, b'bad key', b'value') == -1
iterate = bind(pa, 'pa_proplist_iterate', c.c_char_p, p, c.POINTER(p))
state = p()
keys = []
while True:
    key = iterate(props, c.byref(state))
    if key is None:
        break
    keys.append(key)
assert set(keys) == {b'application.name', b'media.role'}
v = Volume(2, (u * 32)(65536, 32768))
m = ChannelMap(2, (i * 32)(1, 2))
balance = bind(pa, 'pa_cvolume_get_balance', c.c_float, c.POINTER(Volume), c.POINTER(ChannelMap))
assert balance(c.byref(v), c.byref(m)) == -0.5
assert bind(pa, 'pa_channel_map_can_balance', i, c.POINTER(ChannelMap))(c.byref(m)) == 1
assert bind(pa, 'pa_channel_map_can_fade', i, c.POINTER(ChannelMap))(c.byref(m)) == 0
assert bind(pa, 'pa_cvolume_scale', p, c.POINTER(Volume), u)(c.byref(v), 32768)
assert list(v.values)[:2] == [32768, 16384]
db = bind(pa, 'pa_sw_volume_to_dB', c.c_double, u)(32768)
assert math.isclose(db, -18.06179973983887)
assert bind(pa, 'pa_sw_volume_from_dB', u, c.c_double)(db) == 32768

new = bind(pa, 'pa_context_new_with_proplist', p, p, c.c_char_p, p)
connect = bind(pa, 'pa_context_connect', i, p, p, i, p)
get_state = bind(pa, 'pa_context_get_state', i, p)
op_state = bind(pa, 'pa_operation_get_state', i, p)
unref = bind(pa, 'pa_operation_unref', None, p)
cancel = bind(pa, 'pa_operation_cancel', None, p)
query = bind(pa, 'pa_context_get_card_info_list', p, p, p, p)
query_source = bind(pa, 'pa_context_get_source_info_list', p, p, p, p)
events = []

@c.CFUNCTYPE(None, p, p, i, p)
def on_info(context, info, end, data):
    assert not info and end == 1
    events.append(('empty', data))

@c.CFUNCTYPE(None, p, p)
def on_state(context, data):
    events.append(('state', get_state(context)))

for foreign in (False, True):
    events.clear()
    if foreign:
        # This loads Debian's real Pulse GLib adapter and libpulsecommon.
        adapter = c.CDLL('libpulse-mainloop-glib.so.0')
        glib = c.CDLL('libglib-2.0.so.0')
        loop = bind(adapter, 'pa_glib_mainloop_new', p, p)(None)
        api = bind(adapter, 'pa_glib_mainloop_get_api', p, p)(loop)
        dispatch = lambda: bind(glib, 'g_main_context_iteration', i, p, i)(None, 0)
        free_loop = bind(adapter, 'pa_glib_mainloop_free', None, p)
    else:
        loop = bind(pa, 'pa_mainloop_new', p)()
        api = bind(pa, 'pa_mainloop_get_api', p, p)(loop)
        dispatch = lambda: bind(pa, 'pa_mainloop_iterate', i, p, i, p)(loop, 0, None)
        free_loop = bind(pa, 'pa_mainloop_free', None, p)
    context = new(api, b'probe', props)
    bind(pa, 'pa_context_set_state_callback', None, p, p, p)(context, on_state, None)
    assert connect(context, None, 0, None) == 0
    assert get_state(context) == 1 and not events, 'connect callback ran synchronously'
    for _ in range(20):
        dispatch()
        if get_state(context) == 4:
            break
    assert events == [('state', 4)]
    op = query(context, on_info, 100)
    assert op_state(op) == 0 and events == [('state', 4)]
    cancel(op)
    assert op_state(op) == 2
    unref(op)
    # Dropping the caller's reference must not destroy a queued operation.
    op = query_source(context, on_info, 200)
    unref(op)
    for _ in range(20):
        dispatch()
        if ('empty', 200) in events:
            break
    assert events == [('state', 4), ('empty', 200)], events
    is_pending = bind(pa, 'pa_context_is_pending', i, p)
    # GLib may dispatch the cancelled query after the later source query.
    # Drain that retained request before asserting the context is idle.
    for _ in range(20):
        if not is_pending(context): break
        dispatch()
    assert is_pending(context) == 0
    op = bind(pa, 'pa_context_get_sample_info_list', p, p, p, p)(context, on_info, 400)
    assert op_state(op) == 0 and is_pending(context) == 1
    for _ in range(20):
        dispatch()
        if ('empty', 400) in events: break
    assert ('empty', 400) in events and is_pending(context) == 0
    unref(op)
    # Freeing a loop with pending callbacks must release retained references.
    op = query(context, on_info, 300)
    unref(op)
    bind(pa, 'pa_context_disconnect', None, p)(context)
    bind(pa, 'pa_context_unref', None, p)(context)
    free_loop(loop)
    assert ('empty', 300) not in events
bind(pa, 'pa_proplist_free', None, p)(props)
print('PULSE_MIXER_NATIVE_GLIB_ASYNC_OK')
