"""Validate the native XKB protocol using the real xkbcommon-x11 client."""
import ctypes as c

xcb = c.CDLL('libxcb.so.1')
xkb = c.CDLL('libxkbcommon.so.0')
x11 = c.CDLL('libxkbcommon-x11.so.0')

def bind(lib, name, result, *args):
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args
    return fn

connect = bind(xcb, 'xcb_connect', c.c_void_p, c.c_char_p, c.c_void_p)
disconnect = bind(xcb, 'xcb_disconnect', None, c.c_void_p)
context_new = bind(xkb, 'xkb_context_new', c.c_void_p, c.c_int)
context_free = bind(xkb, 'xkb_context_unref', None, c.c_void_p)
setup = bind(x11, 'xkb_x11_setup_xkb_extension', c.c_int, c.c_void_p,
             c.c_uint16, c.c_uint16, c.c_int, c.c_void_p, c.c_void_p, c.c_void_p, c.c_void_p)
device_id = bind(x11, 'xkb_x11_get_core_keyboard_device_id', c.c_int, c.c_void_p)
map_new = bind(x11, 'xkb_x11_keymap_new_from_device', c.c_void_p,
               c.c_void_p, c.c_void_p, c.c_int, c.c_int)
map_free = bind(xkb, 'xkb_keymap_unref', None, c.c_void_p)
state_new = bind(x11, 'xkb_x11_state_new_from_device', c.c_void_p,
                 c.c_void_p, c.c_void_p, c.c_int)
state_free = bind(xkb, 'xkb_state_unref', None, c.c_void_p)
state_mask = bind(xkb, 'xkb_state_update_mask', c.c_uint, c.c_void_p,
                  c.c_uint, c.c_uint, c.c_uint, c.c_uint, c.c_uint, c.c_uint)
keysym = bind(xkb, 'xkb_state_key_get_one_sym', c.c_uint, c.c_void_p, c.c_uint)
connection = connect(None, None)
context = context_new(0)
keymap = state = None
try:
    assert connection and context
    assert setup(connection, 1, 0, 0, None, None, None, None) == 1
    device = device_id(connection)
    assert device == 3, device
    keymap = map_new(context, connection, device, 0)
    assert keymap, 'xkbcommon could not decode the native keyboard map'
    state = state_new(keymap, connection, device)
    assert state
    for depressed, locked, expected in [(0, 0, ord('a')), (1, 0, ord('A')),
                                       (0, 2, ord('A')), (1, 2, ord('a'))]:
        state_mask(state, depressed, 0, locked, 0, 0, 0)
        assert keysym(state, 38) == expected, (depressed, locked, keysym(state, 38))
    state_mask(state, 1, 0, 0, 0, 0, 0)
    assert keysym(state, 10) == ord('!')
    assert keysym(state, 105) == 0xffe4  # Right Control, same keycode as native input.
    print('XKB_DEVICE_KEYMAP_STATE_OK', flush=True)
finally:
    if state:
        state_free(state)
    if keymap:
        map_free(keymap)
    if context:
        context_free(context)
    if connection:
        disconnect(connection)
