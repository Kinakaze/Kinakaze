import ctypes as C
import locale
from pathlib import Path

locale.setlocale(locale.LC_ALL, 'C.UTF-8')
probe = C.CDLL(str(Path(__file__).with_suffix('.so')))
result = probe.probe_wide_format()
assert result == 0, result
print('WIDE_FORMAT_ABI_UNICODE_BOUNDS_OK', flush=True)
