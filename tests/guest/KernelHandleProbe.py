"""IPC identity, key reuse and exit bookkeeping across independent workers."""
import ctypes as c
import errno
import json
import os
import struct
import sys

mode, fixture, key = sys.argv[1], sys.argv[2], int(sys.argv[3])
lib = c.CDLL('libc.so.6', use_errno=True)
lib.shmat.restype = c.c_void_p
lib.shmat.argtypes = [c.c_int, c.c_void_p, c.c_int]
lib.shmdt.argtypes = [c.c_void_p]
lib.shmctl.argtypes = [c.c_int, c.c_int, c.c_void_p]
lib.shmget.argtypes = [c.c_int, c.c_size_t, c.c_int]

def checked(value):
    assert value not in (-1, c.c_void_p(-1).value), (mode, c.get_errno())
    return value

def attach(segment):
    return checked(lib.shmat(segment, None, 0))

def count(segment):
    stat = c.create_string_buffer(112)
    checked(lib.shmctl(segment, 2, stat))
    return struct.unpack_from('<Q', stat.raw, 88)[0]

if mode == 'create':
    queue_name = ('/' + fixture.rsplit('/', 1)[-1]).encode()
    queue = checked(lib.mq_open(queue_name, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600, None))
    checked(lib.mq_send(queue, b'kept-queue', 10, 7))
    checked(lib.mq_close(queue))
    segment = checked(lib.shmget(0, 4096, 0o600))
    semaphore = checked(lib.semget(0, 1, 0o600))
    named = checked(lib.shmget(key, 4096, 0o3600))
    named_sem = checked(lib.semget(key, 1, 0o3600))
    address = attach(segment)
    c.memmove(address, b'private-persistent', 18)
    checked(lib.shmdt(address))
    checked(lib.semctl(semaphore, 0, 16, 37))
    checked(lib.semctl(named_sem, 0, 16, 9))
    with open(fixture, 'w') as f:
        json.dump([segment, semaphore, named, named_sem], f)
else:
    segment, semaphore, named, named_sem = json.load(open(fixture))
    if mode == 'read-ids':
        address = attach(segment)
        assert c.string_at(address, 18) == b'private-persistent'
        assert checked(lib.semctl(semaphore, 0, 12, 0)) == 37
        assert checked(lib.shmget(key, 0, 0)) == named
        assert checked(lib.semget(key, 0, 0)) == named_sem
        checked(lib.shmdt(address))
        queue_name = ('/' + fixture.rsplit('/', 1)[-1]).encode()
        queue = checked(lib.mq_open(queue_name, os.O_RDWR))
        message, priority = c.create_string_buffer(8192), c.c_uint()
        checked(lib.mq_receive(queue, message, len(message), c.byref(priority)))
        assert message.value == b'kept-queue' and priority.value == 7
        checked(lib.mq_close(queue))
        checked(lib.mq_unlink(queue_name))
    elif mode == 'reuse-key':
        old = attach(named)
        c.memmove(old, b'old', 3)
        checked(lib.shmctl(named, 0, None))
        new = checked(lib.shmget(key, 4096, 0o3600))
        assert new != named
        address = attach(new)
        assert c.string_at(address, 3) == bytes(3)
        c.memmove(address, b'new', 3)
        assert c.string_at(old, 3) == b'old'
        checked(lib.shmdt(old))
        checked(lib.shmdt(address))
        checked(lib.shmctl(new, 0, None))
        checked(lib.semctl(named_sem, 0, 0, 0))
        new_sem = checked(lib.semget(key, 1, 0o3600))
        assert new_sem != named_sem
        assert checked(lib.semctl(new_sem, 0, 12, 0)) == 0
        assert lib.semctl(named_sem, 0, 12, 0) == -1
        checked(lib.semctl(new_sem, 0, 0, 0))
    elif mode == 'exit-attached':
        attach(segment)
        assert count(segment) == 1
        operation = struct.pack('<Hhh', 0, -1, 0x1000)
        checked(lib.semop(semaphore, c.c_char_p(operation), 1))
        assert checked(lib.semctl(semaphore, 0, 12, 0)) == 36
        print('PASS_' + mode, flush=True)
        os._exit(0)
    elif mode == 'after-exit':
        assert count(segment) == 0, 'dead worker left a phantom attachment'
        assert checked(lib.semctl(semaphore, 0, 12, 0)) == 37, 'SEM_UNDO lost on exit'
    elif mode == 'namespace-isolation':
        checked(lib.unshare(0x08000000))
        assert lib.shmat(segment, None, 0) == c.c_void_p(-1).value
        assert lib.semctl(semaphore, 0, 12, 0) == -1
    elif mode == 'fork':
        address = attach(segment)
        assert checked(lib.semctl(semaphore, 0, 12, 0)) == 37
        child = os.fork()
        if child == 0:
            assert c.string_at(address, 18) == b'private-persistent'
            assert checked(lib.semctl(semaphore, 0, 12, 0)) == 37
            checked(lib.semctl(semaphore, 0, 16, 38))
            checked(lib.shmdt(address))
            os._exit(0)
        assert os.waitpid(child, 0) == (child, 0)
        assert checked(lib.semctl(semaphore, 0, 12, 0)) == 38
        assert count(segment) == 1
        checked(lib.semctl(semaphore, 0, 16, 37))
        checked(lib.shmdt(address))
    elif mode == 'exec':
        attach(segment)
        checked(lib.semop(semaphore, c.c_char_p(struct.pack('<Hhh', 0, -1, 0x1000)), 1))
        os.execv('/usr/bin/python3', ['/usr/bin/python3', '-c', '''
import ctypes as c, json, struct, sys
segment, semaphore, _, _ = json.load(open(sys.argv[1]))
lib = c.CDLL('libc.so.6', use_errno=True)
stat = c.create_string_buffer(112)
assert lib.shmctl(segment, 2, stat) == 0
assert struct.unpack_from('<Q', stat.raw, 88)[0] == 0, 'exec retained old attachments'
assert lib.semctl(semaphore, 0, 12, 0) == 36, 'exec applied SEM_UNDO prematurely'
assert lib.semop(semaphore, c.c_char_p(struct.pack('<Hhh', 0, 1, 0x1000)), 1) == 0
assert lib.semctl(semaphore, 0, 12, 0) == 37
print('PASS_exec', flush=True)
''', fixture])
    elif mode == 'cgroup-create':
        group = '/sys/fs/cgroup/' + fixture.rsplit('/', 1)[-1]
        os.mkdir(group)
        with open(group + '/cgroup.procs', 'w') as f:
            f.write(str(os.getpid()))
        sum(i * i for i in range(1000000))
        usage = dict(line.split() for line in open(group + '/cpu.stat'))
        assert int(usage['usage_usec']) > 0
        with open(fixture + '.cpu', 'w') as f:
            f.write(usage['usage_usec'])
    elif mode == 'cgroup-read':
        group = '/sys/fs/cgroup/' + fixture.rsplit('/', 1)[-1]
        usage = dict(line.split() for line in open(group + '/cpu.stat'))
        assert int(usage['usage_usec']) >= int(open(fixture + '.cpu').read())
        os.unlink(fixture + '.cpu')
        os.rmdir(group)
    elif mode == 'remove':
        checked(lib.shmctl(segment, 0, None))
        checked(lib.semctl(semaphore, 0, 0, 0))
    elif mode == 'absent':
        assert lib.shmat(segment, None, 0) == c.c_void_p(-1).value
        assert lib.semctl(semaphore, 0, 12, 0) == -1
        os.unlink(fixture)
    else:
        raise AssertionError(mode)
print('PASS_' + mode, flush=True)
