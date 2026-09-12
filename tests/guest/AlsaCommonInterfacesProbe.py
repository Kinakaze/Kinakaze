"""ALSA metadata and MIDI lifecycle; never plays audio or changes host volume."""
import ctypes as C
import struct

alsa = C.CDLL('libasound.so.2')
libc = C.CDLL('libc.so.6')
P, I, U, L = C.c_void_p, C.c_int, C.c_uint, C.c_long
def api(name, result, *args):
    f = getattr(alsa, name)
    f.restype, f.argtypes = result, args
    return f
libc.free.argtypes = [P]
for symbol in ('snd_card_get_name', 'snd_card_get_longname'):
    get = api(symbol, I, I, C.POINTER(P))
    a, b = P(), P()
    assert get(0, C.byref(a)) == get(0, C.byref(b)) == 0
    assert a.value != b.value and C.string_at(a) == C.string_at(b)
    libc.free(a)
    libc.free(b)
    assert get(1, C.byref(a)) == -19 and not a.value
open_ctl = api('snd_ctl_open', I, C.POINTER(P), C.c_char_p, I)
close_ctl = api('snd_ctl_close', I, P)
a, b = P(), P()
assert open_ctl(C.byref(a), b'default', 0) == open_ctl(C.byref(b), b'hw:0', 1) == 0
assert a.value != b.value and a.value != 0x1234
size = api('snd_ctl_card_info_sizeof', C.c_size_t)()
info = C.create_string_buffer(size)
assert api('snd_ctl_card_info', I, P, P)(a, info) == 0
for name in ('driver', 'longname'):
    assert api('snd_ctl_card_info_get_' + name, C.c_char_p, P)(info)
device = I(-1)
assert api('snd_ctl_hwdep_next_device', I, P, C.POINTER(I))(a, C.byref(device)) == 0 and device.value == -1
assert close_ctl(a) == close_ctl(b) == 0
assert open_ctl(C.byref(a), b'hw:99', 0) == -19 and not a.value
size = api('snd_pcm_format_size', L, I, C.c_size_t)
assert size(2, 5) == 10 and size(6, 5) == 20 and size(32, 5) == 15
assert size(-1, 5) == -22 and size(2, (1 << 64) - 1) == -75

pcm, hw = P(), P()
assert api('snd_pcm_open', I, C.POINTER(P), C.c_char_p, I, I)(C.byref(pcm), b'default', 0, 0) == 0
assert api('snd_pcm_hw_params_malloc', I, C.POINTER(P))(C.byref(hw)) == 0
try:
    buffer, period = C.c_ulong(), C.c_ulong()
    get = api('snd_pcm_get_params', I, P, C.POINTER(C.c_ulong), C.POINTER(C.c_ulong))
    assert get(pcm, C.byref(buffer), C.byref(period)) == -77
    assert api('snd_pcm_hw_params_any', I, P, P)(pcm, hw) == 0
    buffer.value, period.value = 2048, 256
    assert api('snd_pcm_hw_params_set_buffer_size_near', I, P, P, C.POINTER(C.c_ulong))(pcm, hw, C.byref(buffer)) == 0
    assert api('snd_pcm_hw_params_set_period_size_near', I, P, P, C.POINTER(C.c_ulong), P)(pcm, hw, C.byref(period), None) == 0
    assert api('snd_pcm_hw_params', I, P, P)(pcm, hw) == 0
    buffer.value = period.value = 0
    assert get(pcm, C.byref(buffer), C.byref(period)) == 0
    assert (buffer.value, period.value) == (2048, 256)
    assert api('snd_pcm_hw_params_can_resume', I, P)(hw) == 1
finally:
    api('snd_pcm_hw_params_free', None, P)(hw)
    assert api('snd_pcm_close', I, P)(pcm) == 0

mixer, identity = P(), P()
assert api('snd_mixer_open', I, C.POINTER(P), I)(C.byref(mixer), 0) == 0
assert api('snd_mixer_selem_id_malloc', I, C.POINTER(P))(C.byref(identity)) == 0
try:
    api('snd_mixer_selem_id_set_name', None, P, C.c_char_p)(identity, b'PCM')
    api('snd_mixer_selem_id_set_index', None, P, U)(identity, 0)
    find = api('snd_mixer_find_selem', P, P, P)
    assert not find(mixer, identity)
    assert api('snd_mixer_attach', I, P, C.c_char_p)(mixer, b'default') == 0
    assert api('snd_mixer_selem_register', I, P, P, P)(mixer, None, None) == 0
    assert api('snd_mixer_load', I, P)(mixer) == 0
    element = find(mixer, identity)
    if element:
        api('snd_mixer_elem_set_callback_private', None, P, P)(element, 0x12345)
        assert api('snd_mixer_elem_get_callback_private', P, P)(element) == 0x12345
        db, volume = L(), L()
        assert api('snd_mixer_selem_ask_playback_vol_dB', I, P, L, C.POINTER(L))(element, 65535, C.byref(db)) == 0 and db.value == 0
        assert api('snd_mixer_selem_ask_playback_dB_vol', I, P, L, I, C.POINTER(L))(element, 0, 0, C.byref(volume)) == 0 and volume.value == 65535
        assert api('snd_mixer_handle_events', I, P)(mixer) == 0
    assert api('snd_mixer_detach', I, P, C.c_char_p)(mixer, b'default') == 0
    assert not find(mixer, identity)
finally:
    api('snd_mixer_selem_id_free', None, P)(identity)
    assert api('snd_mixer_close', I, P)(mixer) == 0

encoder = P()
assert api('snd_midi_event_new', I, C.c_size_t, C.POINTER(P))(4, C.byref(encoder)) == 0
encode = api('snd_midi_event_encode', L, P, P, L, P)
decode = api('snd_midi_event_decode', L, P, P, L, P)
reset = api('snd_midi_event_init', None, P)
event, output = C.create_string_buffer(28), C.create_string_buffer(16)
try:
    assert encode(encoder, b'\x90\x3c\x40\x3d\x41', 5, event) == 3
    assert event.raw[0] == 6
    assert decode(encoder, output, 2, event) == -12
    assert decode(encoder, output, 16, event) == 3 and output.raw[:3] == b'\x90\x3c\x40'
    assert encode(encoder, b'\xf8', 1, event) == 1
    assert decode(encoder, output, 16, event) == 1 and output.raw[0] == 0xf8
    assert encode(encoder, b'\x3d\x41', 2, event) == 2
    assert decode(encoder, output, 16, event) == 2 and output.raw[:2] == b'\x3d\x41'
    api('snd_midi_event_no_status', None, P, I)(encoder, 1)
    assert decode(encoder, output, 16, event) == 3 and output.raw[:3] == b'\x90\x3d\x41'
    reset(encoder)
    assert encode(encoder, b'\x01\x02\xf4', 3, event) == 3 and event.raw[0] == 255
    assert encode(encoder, b'\xf0\x01\x02\x03\x04\xf7', 6, event) == 4
    assert decode(encoder, output, 16, event) == 4 and output.raw[:4] == b'\xf0\x01\x02\x03'
    assert encode(encoder, b'\x04\xf7', 2, event) == 2
    assert decode(encoder, output, 16, event) == 2 and output.raw[:2] == b'\x04\xf7'
    assert api('snd_midi_event_resize_buffer', I, P, C.c_size_t)(encoder, 8) == 0
    assert encode(encoder, b'\xf0\x01\x02\x03\x04\xf7', 6, event) == 6
    assert decode(encoder, output, 16, event) == 6 and output.raw[:6] == b'\xf0\x01\x02\x03\x04\xf7'
    # RPN expansion fits exactly 4 three-byte messages when status is forced.
    event.raw = bytes([16]) + bytes(15) + struct.pack('<B3xIi', 2, 0x102, 0x203)
    assert decode(encoder, output, 16, event) == 12
    assert output.raw[:12] == bytes([0xb2,101,2, 0xb2,100,2, 0xb2,6,4, 0xb2,38,3])
finally:
    api('snd_midi_event_free', None, P)(encoder)
print('ALSA_COMMON_INTERFACES_AND_MIDI_OK', flush=True)
