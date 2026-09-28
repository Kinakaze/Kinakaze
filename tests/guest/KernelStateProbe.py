"""Cross-process kernel state; invoked in separate guest processes by the host test."""
import ctypes as c
import errno
import os
import struct
import sys
import time

libc = c.CDLL('libc.so.6', use_errno=True)
libc.shmat.restype = c.c_void_p
libc.shmat.argtypes = [c.c_int, c.c_void_p, c.c_int]
libc.shmdt.argtypes = [c.c_void_p]
libc.syscall.restype = c.c_long
mode, key, group = sys.argv[1], int(sys.argv[2]), sys.argv[3]


def checked(value):
    assert value != -1 and value != c.c_void_p(-1).value, (mode, c.get_errno())
    return value


def bpf(command, data):
    attr = c.create_string_buffer(data)
    result = libc.syscall(321, command, c.byref(attr), len(data))
    return result, attr.raw


def program_id():
    target = os.open(group, os.O_RDONLY | os.O_DIRECTORY)
    ids = (c.c_uint32 * 1)()
    result, data = bpf(16, struct.pack('<IIIIQII', target, 6, 0, 0, c.addressof(ids), 1, 0))
    os.close(target)
    checked(result)
    assert struct.unpack_from('I', data, 24)[0] == 1
    return ids[0]


if mode == 'pty-set':
    with open('/proc/sys/kernel/pty/max', 'w') as f:
        f.write('8192')
elif mode == 'pty-read':
    assert open('/proc/sys/kernel/pty/max').read().strip() == '8192'
elif mode in ('ipc-create', 'ipc-isolated'):
    if mode == 'ipc-isolated':
        checked(libc.unshare(0x08000000))
        assert libc.shmget(key, 0, 0) == -1 and c.get_errno() == errno.ENOENT
    segment = checked(libc.shmget(key, 4096, 0o1600))
    address = checked(libc.shmat(segment, None, 0))
    c.memmove(address, b'kernel-owned\0', 13)
    checked(libc.shmdt(address))
    semaphore = checked(libc.semget(key, 1, 0o1600))
    checked(libc.semctl(semaphore, 0, 16, 37))
elif mode in ('ipc-read', 'ipc-remove'):
    segment = checked(libc.shmget(key, 0, 0))
    address = checked(libc.shmat(segment, None, 0))
    assert c.string_at(address) == b'kernel-owned'
    semaphore = checked(libc.semget(key, 0, 0))
    assert libc.semctl(semaphore, 0, 12, 0) == 37
    if mode == 'ipc-remove':
        checked(libc.shmctl(segment, 0, None))
        checked(libc.semctl(semaphore, 0, 0, 0))
        assert c.string_at(address) == b'kernel-owned', 'IPC_RMID invalidated an attachment'
    checked(libc.shmdt(address))
elif mode == 'ipc-absent':
    assert libc.shmget(key, 0, 0) == -1 and c.get_errno() == errno.ENOENT
    assert libc.semget(key, 0, 0) == -1 and c.get_errno() == errno.ENOENT
elif mode == 'bpf-create':
    os.mkdir(group)
    instructions = c.create_string_buffer(struct.pack('<BBhiBBhi', 0xb7, 0, 0, 0, 0x95, 0, 0, 0))
    license = c.create_string_buffer(b'GPL')
    attr = bytearray(144)
    struct.pack_into('<IIQQ', attr, 0, 15, 2, c.addressof(instructions), c.addressof(license))
    program = checked(bpf(5, bytes(attr))[0])
    target = os.open(group, os.O_RDONLY | os.O_DIRECTORY)
    checked(bpf(8, struct.pack('<IIIII', target, program, 6, 0, 0))[0])
    assert program_id() == 1
    os.close(program)
    os.close(target)
elif mode == 'bpf-read':
    assert program_id() > 0
    with open(group + '/cgroup.procs', 'w') as f:
        f.write(str(os.getpid()))
    try:
        fd = os.open('/dev/null', os.O_RDONLY)
    except PermissionError:
        pass
    else:
        os.close(fd)
        raise AssertionError('creator exit lost the cgroup device filter')
elif mode == 'bpf-absent':
    assert bpf(13, struct.pack('<II', 1, 0))[0] == -1 and c.get_errno() == errno.ENOENT
elif mode == 'bpf-remove':
    program = checked(bpf(13, struct.pack('<III', program_id(), 0, 0))[0])
    target = os.open(group, os.O_RDONLY | os.O_DIRECTORY)
    checked(bpf(9, struct.pack('<III', target, program, 6))[0])
    os.close(program)
    os.close(target)
    os.rmdir(group)
elif mode == 'time-parent':
    checked(libc.unshare(0x80))
    with open('/proc/self/timens_offsets', 'w') as f:
        f.write('monotonic 123 0\nboottime 321 0\n')
    print('TIME_READY', flush=True)
    deadline = time.monotonic() + 30
    while not os.path.exists(group):
        assert time.monotonic() < deadline, 'namespace child did not finish'
        time.sleep(.01)
elif mode == 'time-child':
    offsets = open('/proc/self/timens_offsets').read()
    assert 'monotonic 123 0' in offsets and 'boottime 321 0' in offsets, offsets
    with open(group, 'w') as f:
        f.write('done')
else:
    raise AssertionError(mode)
print('PASS_' + mode, flush=True)
