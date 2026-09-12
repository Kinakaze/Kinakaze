"""VLC's ALSA output imports: owned text buffers, PCM dumps and channel maps."""
import ctypes as C

alsa = C.CDLL('libasound.so.2')
P, I, U = C.c_void_p, C.c_int, C.c_uint


def api(name, result, *args):
    function = getattr(alsa, name)
    function.restype, function.argtypes = result, args
    return function


out, pcm, hw, sw, status, info = [P() for _ in range(6)]
assert api('snd_output_buffer_open', I, C.POINTER(P))(C.byref(out)) == 0
string = api('snd_output_buffer_string', C.c_size_t, P, C.POINTER(P))
puts = api('snd_output_puts', I, P, C.c_char_p)
flush = api('snd_output_flush', I, P)
text = P()
assert string(out, C.byref(text)) == 0
for value in [b'one', b'two' * 1000]:
    assert puts(out, value) >= 0
assert string(out, C.byref(text)) == 3003
assert C.string_at(text) == b'one' + b'two' * 1000
assert flush(out) == 0 and string(out, C.byref(text)) == 0
assert puts(out, b'reused') >= 0
assert string(out, C.byref(text)) == 6 and C.string_at(text) == b'reused'
assert flush(out) == 0
assert api('snd_pcm_open', I, C.POINTER(P), C.c_char_p, I, I)(C.byref(pcm), b'default', 0, 0) == 0
for kind, handle in [('hw_params', hw), ('sw_params', sw), ('status', status), ('info', info)]:
    assert api('snd_pcm_' + kind + '_malloc', I, C.POINTER(P))(C.byref(handle)) == 0
assert api('snd_pcm_hw_params_any', I, P, P)(pcm, hw) == 0
assert api('snd_pcm_hw_params_can_pause', I, P)(hw) == 1
assert api('snd_pcm_sw_params_current', I, P, P)(pcm, sw) == 0
assert api('snd_pcm_status', I, P, P)(pcm, status) == 0
assert api('snd_pcm_info', I, P, P)(pcm, info) == 0
assert api('snd_pcm_info_get_subdevice_name', C.c_char_p, P)(info)
for kind, handle, marker in [('hw_params', hw, b'RATE:'), ('sw_params', sw, b'avail_min:'), ('status', status, b'state:')]:
    assert api('snd_pcm_' + kind + '_dump', I, P, P)(handle, out) == 0
    assert string(out, C.byref(text)) > 0 and marker in C.string_at(text)
    assert flush(out) == 0
maps = api('snd_pcm_query_chmaps', C.POINTER(P), P)(pcm)
assert maps and maps[0] and maps[1] and not maps[2]
for index, positions in enumerate(([2], [3, 4])):
    fields = C.cast(maps[index], C.POINTER(U))
    assert fields[0] == 1 and fields[1] == len(positions)
    assert list(fields[2:2 + len(positions)]) == positions
api('snd_pcm_free_chmaps', None, P)(maps)
for kind, handle in [('hw_params', hw), ('sw_params', sw), ('status', status), ('info', info)]:
    api('snd_pcm_' + kind + '_free', None, P)(handle)
assert api('snd_pcm_close', I, P)(pcm) == 0
assert api('snd_output_close', I, P)(out) == 0
print('NETEASE_VLC_ALSA_ABI_OK', flush=True)
