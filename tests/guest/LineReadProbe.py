import ctypes as C
import json
from pathlib import Path
import time

probe = C.CDLL(str(Path(__file__).with_suffix('.so')))
result = probe.probe_lines()
assert result == 0, result
probe.benchmark_lines.argtypes = [C.c_uint]
probe.benchmark_lines.restype = C.c_long
samples = []
for _ in range(4):
    started = time.perf_counter()
    assert probe.benchmark_lines(8 * 1024 * 1024) == 8 * 1024 * 1024
    samples.append((time.perf_counter() - started) * 1000)
print(json.dumps(dict(probe='LineReadProbe', status='passed', bytes=8 * 1024 * 1024,
                      elapsed_ms=samples, scope='cookie read, allocation and line parsing')), flush=True)
path = Path(__file__).with_suffix('.data')
path.write_bytes((b'x' * 127 + b'\n') * 65536)
probe.benchmark_file_lines.argtypes = [C.c_char_p]
probe.benchmark_file_lines.restype = C.c_long
samples = []
for _ in range(4):
    started = time.perf_counter()
    assert probe.benchmark_file_lines(str(path).encode()) == 8 * 1024 * 1024
    samples.append((time.perf_counter() - started) * 1000)
print(json.dumps(dict(probe='FileLineRead', status='passed', bytes=8 * 1024 * 1024,
                      elapsed_ms=samples, scope='warm filesystem, open/read/close and line parsing')), flush=True)
