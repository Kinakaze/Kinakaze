"""Binary80 log1p accuracy, special values and fortified cwd buffer boundaries."""
import ctypes
from decimal import Decimal, getcontext
import errno
import os
from pathlib import Path
import signal
import struct
import subprocess
import sys


libc = ctypes.CDLL(None, use_errno=True)
getcwd = libc.__getcwd_chk
getcwd.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_size_t]
getcwd.restype = ctypes.c_void_p
if sys.argv[1:] == ['overflow']:
    buffer = ctypes.create_string_buffer(4)
    getcwd(buffer, 5, 4)
    raise AssertionError('fortified getcwd did not abort')

buffer = ctypes.create_string_buffer(4096)
assert getcwd(buffer, len(buffer), len(buffer)) == ctypes.addressof(buffer)
assert os.fsdecode(buffer.value) == os.getcwd()
ctypes.set_errno(0)
assert getcwd(buffer, 1, len(buffer)) is None
assert ctypes.get_errno() == errno.ERANGE
child = subprocess.run([sys.executable, __file__, 'overflow'], capture_output=True, timeout=15)
assert child.returncode == -signal.SIGABRT, (child.returncode, child.stderr)

fixture = ctypes.CDLL(str(Path(__file__).with_suffix('.so')))
fixture.evaluate.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_ushort)]
getcontext().prec = 120
two = Decimal(2)


def raw(significand, exponent):
    return struct.pack('<QH6x', significand, exponent)


def calculate(value):
    output, status = ctypes.create_string_buffer(16), ctypes.c_ushort()
    error = fixture.evaluate(value, output, ctypes.byref(status))
    assert error != -1000, 'x87 rounding mode changed'
    return output.raw, error, status.value


def unpack(value):
    significand, exponent = struct.unpack('<QH', value[:10])
    sign = -1 if exponent & 32768 else 1
    exponent &= 32767
    return sign * Decimal(significand) * two ** ((exponent or 1) - 16383 - 63)


for sign in (0, 32768):
    result, error, status = calculate(raw(0, sign))
    assert result[:10] == raw(0, sign)[:10] and error == 0
    for significand in (1, 2**63 - 1):
        value = raw(significand, sign)
        result, error, status = calculate(value)
        assert result[:10] == value[:10] and error == 0
    for exponent in (-10000, -100, -65):
        value = raw(2**63, exponent + 16383 + sign)
        result, error, status = calculate(value)
        assert result[:10] == value[:10] and error == 0

for exponent in (-20, -5, -3, -2, -1, 0, 1, 10, 100, 1000):
    for significand in (2**63, 2**63 + 2**61, 2**64 - 1):
        for sign in (0, 32768) if exponent < 0 else (0,):
            value = raw(significand, exponent + 16383 + sign)
            result, error, status = calculate(value)
            reference = (1 + unpack(value)).ln()
            relative = abs((unpack(result) - reference) / reference)
            assert error == 0 and relative < two ** -61, (exponent, significand, sign, relative)

result, error, status = calculate(raw(2**63, 0xbfff))
assert result[:10] == raw(2**63, 0xffff)[:10] and error == errno.ERANGE and status & 4
for value in (raw(2**63, 0xc000), raw(2**63, 0xffff)):
    result, error, status = calculate(value)
    significand, exponent = struct.unpack('<QH', result[:10])
    assert exponent & 32767 == 32767 and significand != 2**63
    assert error == errno.EDOM and status & 1
result, error, status = calculate(raw(2**63, 0x7fff))
assert result[:10] == raw(2**63, 0x7fff)[:10] and error == 0
for sign in (0, 32768):
    result, error, status = calculate(raw(2**63 + 2**62, 32767 + sign))
    significand, exponent = struct.unpack('<QH', result[:10])
    assert exponent & 32767 == 32767 and significand != 2**63 and error == 0
print('ORACLE_ABI_LOG1PL_FORTIFY_OK', flush=True)
