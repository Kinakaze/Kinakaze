"""Linux ioctl command arguments are unsigned 32-bit, even on x86-64."""
import ctypes as c
import errno
import os


libc = c.CDLL(None, use_errno=True)
libc.syscall.restype = c.c_long
libc.syscall.argtypes = [c.c_long]


def ioctl(fd, request, argument):
    c.set_errno(0)
    return libc.syscall(16, c.c_long(fd), c.c_ulonglong(request), argument)


master = os.open('/dev/ptmx', os.O_RDWR | os.O_NOCTTY | os.O_CLOEXEC)
try:
    number = c.c_uint()
    assert ioctl(master, 0x80045430, c.byref(number)) == 0, c.get_errno()
    expected = number.value
    # musl callers can sign extend an int request into the syscall register.
    # Linux also ignores unrelated high bits, rather than matching all 64 bits.
    for high in (0xFFFFFFFF00000000, 0x1234567800000000):
        number.value = 0xDEADBEEF
        assert ioctl(master, high | 0x80045430, c.byref(number)) == 0, (hex(high), c.get_errno())
        assert number.value == expected
        unlocked = c.c_int(0)
        assert ioctl(master, high | 0x40045431, c.byref(unlocked)) == 0, c.get_errno()
        locked = c.c_int(-1)
        assert ioctl(master, high | 0x80045439, c.byref(locked)) == 0 and locked.value == 0
        peer = ioctl(master, high | 0x5441, c.c_ulong(os.O_RDWR | os.O_NOCTTY | os.O_CLOEXEC))
        assert peer >= 0, c.get_errno()
        try:
            assert os.isatty(peer)
            # Kernel TCGETS writes 36 bytes, never the glibc 60-byte termios.
            termios = (c.c_ubyte * 40)(*([0xA5] * 40))
            assert ioctl(peer, high | 0x5401, c.byref(termios)) == 0, c.get_errno()
            assert bytes(termios)[36:] == b'\xA5' * 4
            assert ioctl(peer, high | 0x54FF, None) == -1 and c.get_errno() == errno.ENOTTY
            assert ioctl(-1, high | 0x80045430, None) == -1 and c.get_errno() == errno.EBADF
        finally:
            os.close(peer)
finally:
    os.close(master)
print('IOCTL_REQUEST_WIDTH_PTY_OK')
