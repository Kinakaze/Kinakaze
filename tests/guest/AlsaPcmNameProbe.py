"""Borrowed ALSA PCM names own their original bytes until PCM close."""
import ctypes as C

alsa = C.CDLL('libasound.so.2')
alsa.snd_pcm_open.argtypes = [C.POINTER(C.c_void_p), C.c_char_p, C.c_int, C.c_int]
alsa.snd_pcm_name.argtypes = [C.c_void_p]
alsa.snd_pcm_name.restype = C.c_void_p
alsa.snd_pcm_close.argtypes = [C.c_void_p]

for name in (None, b'default', b'hw:0'):
    pcm = C.c_void_p()
    source = C.create_string_buffer(name) if name is not None else None
    assert alsa.snd_pcm_open(C.byref(pcm), source, 0, 0) == 0
    try:
        if source is not None:
            source[0] = b'x'
        pointer = alsa.snd_pcm_name(pcm)
        assert pointer and C.string_at(pointer) == (name or b'default')
        assert alsa.snd_pcm_name(pcm) == pointer
    finally:
        assert alsa.snd_pcm_close(pcm) == 0
assert not alsa.snd_pcm_name(None)
print('ALSA_PCM_NAME_LIFETIME_OK', flush=True)
