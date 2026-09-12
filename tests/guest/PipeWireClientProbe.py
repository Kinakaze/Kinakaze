"""PipeWire client lifecycle and native SPA callers used by desktop Portal."""
import ctypes as C
from pathlib import Path
pipewire=C.CDLL('libpipewire-0.3.so.0',mode=C.RTLD_GLOBAL)
fixture=C.CDLL(str(Path(__file__).with_suffix('.so')))
result=fixture.probe()
assert result==0,f'C assertion failed at line {result}'
print('PipeWireClientProbe: PASS',flush=True)
