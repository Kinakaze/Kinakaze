"""Pixel-exact XCB drawing, wire image formats, errors and fork ownership."""
import ctypes as c
import os
import struct

b = c.CDLL('libxcb.so.1')
lib = c.CDLL('libc.so.6')
P, U, I, B, S, H = c.c_void_p, c.c_uint, c.c_int, c.c_ubyte, c.c_ushort, c.c_short


def bind(library, name, result, args):
    f = getattr(library, name)
    f.restype, f.argtypes = result, args
    return f


free = bind(lib, 'free', None, [P])
conn = bind(b, 'xcb_connect', P, [P, P])(None, None)
assert conn
generate = bind(b, 'xcb_generate_id', U, [P])
check = bind(b, 'xcb_request_check', P, [P, U])
pixmap = bind(b, 'xcb_create_pixmap_checked', U, [P, B, U, U, S, S])
drop = bind(b, 'xcb_free_pixmap_checked', U, [P, U])
gc = bind(b, 'xcb_create_gc_checked', U, [P, U, U, U, P])
change = bind(b, 'xcb_change_gc_checked', U, [P, U, U, P])
drop_gc = bind(b, 'xcb_free_gc_checked', U, [P, U])
put = bind(b, 'xcb_put_image_checked', U, [P, B, U, U, S, S, H, H, B, B, U, P])
get = bind(b, 'xcb_get_image', U, [P, B, U, H, H, S, S, U])
get_unchecked = bind(b, 'xcb_get_image_unchecked', U, [P, B, U, H, H, S, S, U])
reply = bind(b, 'xcb_get_image_reply', P, [P, U, P])
data_len = bind(b, 'xcb_get_image_data_length', I, [P])
data_ptr = bind(b, 'xcb_get_image_data', P, [P])
fill = bind(b, 'xcb_poly_fill_rectangle_checked', U, [P, U, U, U, P])
outline = bind(b, 'xcb_poly_rectangle_checked', U, [P, U, U, U, P])
clip = bind(b, 'xcb_set_clip_rectangles_checked', U, [P, B, U, H, H, U, P])
copy = bind(b, 'xcb_copy_area_checked', U, [P, U, U, U, H, H, H, H, S, S])
event = bind(b, 'xcb_poll_for_event', P, [P])


def checked(sequence, expected=0):
    address = check(conn, sequence)
    if not expected:
        assert not address, c.string_at(address, 36).hex() if address else ''
    else:
        assert address
        raw = c.string_at(address, 36)
        free(address)
        assert raw[1] == expected, (raw, expected)


def response(sequence, expected=0):
    error = P()
    address = reply(conn, sequence, c.byref(error))
    if expected:
        assert not address and error.value
        raw = c.string_at(error, 36)
        free(error)
        assert raw[1] == expected
        return
    assert address and not error.value
    header = c.string_at(address, 32)
    assert struct.unpack_from('<H', header, 2)[0] == sequence & 65535
    n = struct.unpack_from('<I', header, 4)[0] * 4
    assert data_len(address) == n and data_ptr(address) == address + 32
    raw = c.string_at(address + 32, n)
    free(address)
    return header, raw


def read_pixels(drawable, width=8, height=4, x=0, y=0, mask=0xffffffff):
    _, raw = response(get(conn, 2, drawable, x, y, width, height, mask))
    return list(struct.unpack('<' + 'I' * (width * height), raw))


def set_gc(g, **values):
    # Parameter keys are protocol bit numbers.
    bits = sorted((int(bit), value) for bit, value in values.items())
    packed = (U * len(bits))(*(v for _, v in bits))
    checked(change(conn, g, sum(1 << bit for bit, _ in bits), packed))


def write(drawable, g, pixels, width=8, height=4, depth=24):
    data = (U * len(pixels))(*pixels)
    checked(put(conn, 2, drawable, g, width, height, 0, 0, 0, depth, c.sizeof(data), data))


def rect(x, y, width, height):
    return c.create_string_buffer(struct.pack('<hhHH', x, y, width, height), 8)


pid, gid = generate(conn), generate(conn)
checked(pixmap(conn, 24, pid, 1, 8, 4))
# Mixed mask order: function, plane mask, foreground, background, exposures.
values = (U * 5)(3, 0xffffffff, 0x123456, 0xabcdef, 0)
checked(gc(conn, gid, pid, 0xf | (1 << 16), values))
checked(pixmap(conn, 24, pid, 1, 1, 1), 14)
checked(gc(conn, pid, 1, 0, None), 14)
pixels = [0x10203 * i for i in range(32)]
write(pid, gid, pixels)
assert read_pixels(pid) == pixels
pending = get(conn, 2, pid, 2, 1, 3, 2, 0xffffffff)
checked(fill(conn, pid, gid, 1, rect(0, 0, 8, 4)))
assert list(struct.unpack('<6I', response(pending)[1])) == pixels[10:13] + pixels[18:21]
assert read_pixels(pid) == [0x123456] * 32
assert read_pixels(pid, mask=0xff00) == [0x3400] * 32

# Clip rectangles are relative to the GC origin; an empty list clips all pixels.
checked(clip(conn, 0, gid, 2, 1, 1, rect(-1, 0, 3, 2)))
set_gc(gid, **{'2': 0xaabbcc})
checked(fill(conn, pid, gid, 1, rect(0, 0, 8, 4)))
expected = [0xaabbcc if 1 <= i % 8 < 4 and 1 <= i // 8 < 3 else 0x123456 for i in range(32)]
assert read_pixels(pid) == expected
checked(clip(conn, 0, gid, 0, 0, 0, None))
set_gc(gid, **{'2': 0})
checked(fill(conn, pid, gid, 1, rect(0, 0, 8, 4)))
assert read_pixels(pid) == expected
set_gc(gid, **{'19': 0})

# All sixteen raster operations, with a partial plane mask and unsigned pixels.
source, target, mask = 0x92b65a, 0x7d08c3, 0xf00ff0
truth = [0, source & target, source & ~target, source, ~source & target, target,
         source ^ target, source | target, ~(source | target), ~(source ^ target),
         ~target, source | ~target, ~source, ~source | target, ~(source & target), 0xffffff]
for operation, value in enumerate(truth):
    set_gc(gid, **{'0': 3, '1': 0xffffffff})
    write(pid, gid, [target] * 32)
    set_gc(gid, **{'0': operation, '1': mask, '2': source})
    checked(fill(conn, pid, gid, 1, rect(0, 0, 8, 4)))
    assert read_pixels(pid) == [((value & mask) | (target & ~mask)) & 0xffffff] * 32
set_gc(gid, **{'0': 3, '1': 0xffffffff, '2': 0, '3': 0xffffff})

# Bitmap left padding, byte/row padding, and foreground/background expansion.
bitmap = c.create_string_buffer(bytes([0b10101000, 0b10, 0, 0]) * 4, 16)
checked(put(conn, 0, pid, gid, 8, 4, 0, 0, 3, 1, 16, bitmap))
bitmap_expected = [0 if i % 2 == 0 else 0xffffff for i in range(32)]
assert read_pixels(pid) == bitmap_expected

# XYPixmap is most-significant plane first, LSB first inside each byte.
planes = b''.join(bytes([sum(((pixels[y*8+x] >> bit) & 1) << x for x in range(8)), 0, 0, 0])
                  for bit in reversed(range(24)) for y in range(4))
plane_buffer = c.create_string_buffer(planes, len(planes))
checked(put(conn, 1, pid, gid, 8, 4, 0, 0, 0, 24, len(planes), plane_buffer))
assert read_pixels(pid) == pixels
assert response(get(conn, 1, pid, 0, 0, 8, 4, 0xffffff))[1] == planes
selected = b''.join(planes[(23-bit)*16:(24-bit)*16] for bit in [20, 7, 0])
assert response(get(conn, 1, pid, 0, 0, 8, 4, (1 << 20) | (1 << 7) | 1))[1] == selected

# Copies use a snapshot of the source rectangle when storage overlaps.
checked(copy(conn, pid, pid, gid, 0, 0, 1, 1, 7, 3))
overlap = pixels[:]
for y in range(3):
    overlap[(y+1)*8+1:(y+2)*8] = pixels[y*8:y*8+7]
assert read_pixels(pid) == overlap
set_gc(gid, **{'16': 1})
checked(copy(conn, pid, pid, gid, 0, 0, 0, 0, 8, 4))
ev = event(conn)
assert ev and c.string_at(ev, 1) == b'\x0e'
free(ev)
checked(copy(conn, pid, pid, gid, -1, 0, 0, 0, 8, 4))
for row in range(4):
    ev = event(conn)
    assert ev
    raw = c.string_at(ev, 36)
    free(ev)
    assert raw[0] == 13 and struct.unpack_from('<hhHH', raw, 8) == (0, row, 1, 1)
assert not event(conn)
set_gc(gid, **{'16': 0, '0': 3, '2': 0})
checked(fill(conn, pid, gid, 1, rect(0, 0, 8, 4)))
set_gc(gid, **{'0': 6, '2': 0xa1b2c3})
checked(outline(conn, pid, gid, 1, rect(1, 1, 5, 2)))
assert read_pixels(pid) == [0xa1b2c3 if (y in (1, 3) and 1 <= x <= 6) or (y == 2 and x in (1, 6)) else 0
                            for y in range(4) for x in range(8)]

# Genuine bitmap and 32-bit pixmaps preserve all depth-specific bits.
for depth in (1, 32):
    p, g = generate(conn), generate(conn)
    checked(pixmap(conn, depth, p, 1, 33, 2))
    checked(gc(conn, g, p, 0, None))
    raw = bytes([0xa5, 0x5a, 0x01, 0x80, 1, 0, 0, 0]) * 2 if depth == 1 else struct.pack('<66I', *[0x80000000+i for i in range(66)])
    data = c.create_string_buffer(raw, len(raw))
    checked(put(conn, 2, p, g, 33, 2, 0, 0, 0, depth, len(raw), data))
    header, actual = response(get(conn, 2, p, 0, 0, 33, 2, 0xffffffff))
    assert header[1] == depth and struct.unpack_from('<I', header, 8)[0] == 0 and actual == raw
    checked(drop_gc(conn, g)); checked(drop(conn, p))

# Pending image data, GC clipping/raster state and pixmap data survive fork independently.
set_gc(gid, **{'0': 3, '1': 0xffffffff, '2': 0x818283})
write(pid, gid, pixels)
checked(clip(conn, 0, gid, 2, 1, 1, rect(0, 0, 3, 2)))
pending = get(conn, 2, pid, 0, 0, 8, 4, 0xffffffff)
child = os.fork()
if child == 0:
    try:
        assert list(struct.unpack('<32I', response(pending)[1])) == pixels
        assert read_pixels(pid) == pixels
        checked(fill(conn, pid, gid, 1, rect(0, 0, 8, 4)))
        assert read_pixels(pid) == [0x818283 if 2 <= i % 8 < 5 and 1 <= i // 8 < 3 else p for i, p in enumerate(pixels)]
        checked(drop_gc(conn, gid)); checked(drop(conn, pid))
    except BaseException:
        import traceback
        traceback.print_exc()
        os._exit(1)
    os._exit(0)
assert os.waitpid(child, 0) == (child, 0)
assert list(struct.unpack('<32I', response(pending)[1])) == pixels
assert read_pixels(pid) == pixels

# Real window backing storage is also readable, independently of desktop occlusion.
window, window_gc = generate(conn), generate(conn)
create_window = bind(b, 'xcb_create_window_checked', U, [P, B, U, U, H, H, S, S, S, S, U, U, P])
checked(create_window(conn, 24, window, 1, 0, 0, 8, 4, 0, 1, 1, 0, None))
checked(gc(conn, window_gc, window, 0, None))
write(window, window_gc, pixels)
assert read_pixels(window) == pixels
checked(drop_gc(conn, window_gc))
bind(b, 'xcb_destroy_window', U, [P, U])(conn, window)

# Protocol errors have real cookies and owned guest allocations.
response(get(conn, 2, pid, -1, 0, 8, 4, 0xffffffff), 8)
response(get(conn, 2, 0xdead, 0, 0, 1, 1, 0xffffffff), 9)
checked(put(conn, 2, pid, gid, 8, 4, 0, 0, 0, 24, 1, bitmap), 16)
checked(fill(conn, pid, 0xdead, 0, None), 13)
unchecked = get_unchecked(conn, 2, 0xdead, 0, 0, 1, 1, 0xffffffff)
error = P()
assert not reply(conn, unchecked, c.byref(error)) and not error.value
ev = event(conn)
assert ev
raw = c.string_at(ev, 36); free(ev)
assert raw[0:2] == b'\0\x09' and struct.unpack_from('<I', raw, 32)[0] == unchecked
checked(drop_gc(conn, gid)); checked(drop(conn, pid)); checked(drop(conn, pid), 4)
bind(b, 'xcb_disconnect', None, [P])(conn)
print('XCB_PIXELS_FORMATS_CLIP_RASTER_COPY_FORK_OK')
