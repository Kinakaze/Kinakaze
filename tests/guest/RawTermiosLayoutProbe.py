"""TCGETS/TCSETS use 36-byte x86-64 termios, with no termios2 speed tail."""
import ctypes as c
import os
import termios


libc = c.CDLL(None, use_errno=True)
libc.syscall.restype = c.c_long
libc.syscall.argtypes = [c.c_long]
libc.mmap.restype = c.c_void_p
libc.mmap.argtypes = [c.c_void_p, c.c_size_t, c.c_int, c.c_int, c.c_int, c.c_long]
libc.mprotect.argtypes = [c.c_void_p, c.c_size_t, c.c_int]
libc.munmap.argtypes = [c.c_void_p, c.c_size_t]


class KernelTermios(c.Structure):
    _fields_ = [('iflag', c.c_uint), ('oflag', c.c_uint), ('cflag', c.c_uint),
                ('lflag', c.c_uint), ('line', c.c_ubyte), ('cc', c.c_ubyte * 19)]


assert c.sizeof(KernelTermios) == 36
master, slave = os.openpty()
try:
    guarded = (c.c_ubyte * 44)(*([0xA5] * 44))
    assert libc.syscall(16, master, 0x5401, c.byref(guarded)) == 0, c.get_errno()
    assert bytes(guarded)[36:] == b'\xA5' * 8, bytes(guarded)[36:]
    current = KernelTermios.from_buffer_copy(bytes(guarded)[:36])
    # A TCSETS caller has no explicit speed fields; derive speeds from c_cflag.
    for output, input_speed in ((termios.B9600, termios.B4800), (termios.B0, termios.B0)):
        current.cflag = (current.cflag & ~(0x100F | (0x100F << 16))) | output | (input_speed << 16)
        assert libc.syscall(16, slave, 0x5402, c.byref(current)) == 0, c.get_errno()
        values = termios.tcgetattr(slave)
        assert values[4:6] == [input_speed, output], values[4:6]
    # Neither reading nor writing termios may touch an adjacent protected page.
    page = os.sysconf('SC_PAGESIZE')
    address = libc.mmap(None, page * 2, 3, 0x22, -1, 0)
    assert address not in (None, c.c_void_p(-1).value), c.get_errno()
    try:
        assert libc.mprotect(address + page, page, 0) == 0, c.get_errno()
        exact = address + page - c.sizeof(current)
        c.memmove(exact, c.byref(current), c.sizeof(current))
        assert libc.syscall(16, slave, 0x5402, c.c_void_p(exact)) == 0, c.get_errno()
        assert libc.syscall(16, slave, 0x5401, c.c_void_p(exact)) == 0, c.get_errno()
    finally:
        assert libc.munmap(address, page * 2) == 0
finally:
    os.close(slave)
    os.close(master)
print('RAW_TERMIOS_LAYOUT_GUARD_SPEEDS_OK')
