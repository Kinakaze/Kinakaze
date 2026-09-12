"""Binary80 ABI, special values, wide exponents and independent Decimal reference."""
import ctypes as C
import decimal as D
import json
from pathlib import Path
import random
import struct

D.getcontext().prec = 110
T = D.Decimal(2)
fixture = C.CDLL(str(Path(__file__).with_suffix('.so')))
fixture.evaluate.argtypes = [C.c_void_p, C.c_void_p, C.c_void_p, C.POINTER(C.c_ushort)]
def raw(significand, exponent):
    return struct.pack('<QH6x', significand, exponent)
def pack(x):
    x = D.Decimal(x)
    if not x: return bytes(16)
    negative = x < 0
    x = abs(x)
    exponent = int(x.ln() / T.ln())
    while x < T ** exponent: exponent -= 1
    while x >= T ** (exponent + 1): exponent += 1
    significand = int((x / T ** (exponent - 63)).to_integral_value(rounding=D.ROUND_HALF_EVEN))
    if significand == 2**64:
        significand //= 2
        exponent += 1
    return raw(significand, exponent + 16383 + (32768 if negative else 0))
def unpack(value):
    significand, exponent = struct.unpack('<QH', value[:10])
    sign = -1 if exponent & 32768 else 1
    exponent &= 32767
    if exponent == 32767:
        return D.Decimal('NaN') if significand != 2**63 else D.Decimal('-Infinity' if sign < 0 else 'Infinity')
    return sign * D.Decimal(significand) * T ** ((exponent or 1) - 16383 - 63)
def evaluate(x, y):
    output, status = C.create_string_buffer(16), C.c_ushort()
    error = fixture.evaluate(x, y, output, C.byref(status))
    assert error != -1000, 'x87 control word changed'
    return output.raw, error, status.value & 63
for exponent in (10000, -10000, -16445):
    value, error, _ = evaluate(pack(2), pack(exponent))
    expected = raw(1, 0) if exponent == -16445 else raw(1 << 63, exponent + 16383)
    assert value[:10] == expected[:10], (exponent, value.hex(), expected.hex())
near_one = raw((1 << 63) + 8, 16383)  # 1 + 2^-60
assert evaluate(near_one, pack(2))[0][:10] == raw((1 << 63) + 16, 16383)[:10]
assert unpack(evaluate(pack(-1), raw((1 << 63) + 1, 16383 + 63))[0]) == -1
negative_zero, infinity, nan = raw(0, 32768), raw(1 << 63, 32767), raw(0xc000000000000000, 32767)
assert evaluate(negative_zero, pack(3))[0][:10] == negative_zero[:10]
value, error, flags = evaluate(negative_zero, pack(-3))
assert value[:10] == raw(1 << 63, 65535)[:10] and error == 34 and flags & 4, (value.hex(), error, flags)
value, error, flags = evaluate(pack(-2), pack('0.5'))
assert unpack(value).is_nan() and error == 33 and flags & 1
assert unpack(evaluate(nan, pack(0))[0]) == 1
assert unpack(evaluate(pack(1), nan)[0]) == 1
assert unpack(evaluate(pack(-1), infinity)[0]) == 1
value, error, flags = evaluate(pack(2), pack(20000))
assert unpack(value).is_infinite() and error == 34 and flags & 8
value, error, flags = evaluate(pack(2), pack(-20000))
assert unpack(value) == 0 and error == 34 and flags & 16
worst = D.Decimal(0)
for index in range(250):
    x = T ** (D.Decimal(random.Random(index).randint(-10000, 10000)) / 100)
    y = D.Decimal(random.Random(index + 1000).randint(-1000, 1000)) / 10
    xb, yb = pack(x), pack(y)
    expected = unpack(xb) ** unpack(yb)
    actual, error, _ = evaluate(xb, yb)
    relative = abs((unpack(actual) - expected) / expected)
    worst = max(worst, relative)
    assert error == 0 and relative < D.Decimal('2e-17'), (index, error, relative)
print(json.dumps(dict(decimal_reference_cases=250, max_relative_error=str(worst))), flush=True)
print('Power80Probe: PASS', flush=True)
