"""Real Linux SPA vtable callers; no audio playback or desktop activity."""
import ctypes as C
import json
from pathlib import Path
pipewire = C.CDLL('libpipewire-0.3.so.0', mode=C.RTLD_GLOBAL)
fixture = C.CDLL(str(Path(__file__).with_suffix('.so')))
fixture.probe.argtypes = [C.POINTER(C.c_double)]
value = C.c_double()
result = fixture.probe(C.byref(value))
assert result == 0, f'C assertion failed at source line {result}'
print(json.dumps(dict(idle_wait_ms=value.value)), flush=True)
print('PipeWireLoopProbe: PASS', flush=True)
