"""Native BPF references, allocation rollback, crash cleanup and descriptor handoff."""
import array
import ctypes as c
import errno
import json
import os
import resource
import socket
import struct
import sys
import time

libc = c.CDLL('libc.so.6', use_errno=True)
libc.syscall.restype = c.c_long
mode, path = sys.argv[1:3]


def bpf(command, data):
    attr = c.create_string_buffer(data)
    result = libc.syscall(321, command, c.byref(attr), len(data))
    if result == -1:
        raise OSError(c.get_errno(), f'bpf command {command}')
    return result, attr.raw


def load():
    instructions = c.create_string_buffer(struct.pack('<BBhiBBhi', 0xb7, 0, 0, 1, 0x95, 0, 0, 0))
    license = c.create_string_buffer(b'GPL')
    attr = bytearray(144)
    struct.pack_into('<IIQQ', attr, 0, 15, 2, c.addressof(instructions), c.addressof(license))
    return bpf(5, bytes(attr))[0]


def identity(fd):
    info = c.create_string_buffer(256)
    bpf(15, struct.pack('<IIQ', fd, len(info), c.addressof(info)))
    return struct.unpack_from('<I', info.raw, 4)[0]


def absent(program):
    try:
        fd = bpf(13, struct.pack('<III', program, 0, 0))[0]
    except OSError as error:
        assert error.errno == errno.ENOENT, error
    else:
        os.close(fd)
        raise AssertionError('program survived its final native reference')


if mode == 'crash-owner':
    fd = load()
    with open(path, 'w') as file:
        json.dump(identity(fd), file)
    print('HANDLE_CRASH_READY', flush=True)
    while True:
        time.sleep(.02)
elif mode == 'crash-check':
    with open(path) as file:
        absent(json.load(file))
    os.unlink(path)
elif mode == 'failed-load':
    old = resource.getrlimit(resource.RLIMIT_NOFILE)
    resource.setrlimit(resource.RLIMIT_NOFILE, (0, old[1]))
    try:
        for _ in range(140):
            try:
                load()
            except OSError as error:
                assert error.errno == errno.EMFILE, error
            else:
                raise AssertionError('loaded a descriptor above RLIMIT_NOFILE')
    finally:
        resource.setrlimit(resource.RLIMIT_NOFILE, old)
    fd = load()
    program = identity(fd)
    os.close(fd)
    absent(program)
elif mode == 'dup-exec':
    fd = load()
    program = identity(fd)
    duplicate = os.dup(fd)
    assert identity(duplicate) == program
    os.close(fd)
    os.set_inheritable(duplicate, True)
    child = os.fork()
    if child == 0:
        os.execv('/usr/bin/python3', ['/usr/bin/python3', '-c',
            sys.argv[3], 'after-exec', path, str(duplicate), str(program)])
    os.close(duplicate)
    assert os.waitpid(child, 0) == (child, 0)
    absent(program)
elif mode == 'after-exec':
    fd, program = map(int, sys.argv[3:5])
    assert identity(fd) == program
    os.close(fd)
elif mode == 'rights':
    parent, child_socket = socket.socketpair()
    child = os.fork()
    if child == 0:
        parent.close()
        fd = load()
        child_socket.sendmsg([str(identity(fd)).encode()],
                            [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
        os._exit(0)
    child_socket.close()
    assert os.waitpid(child, 0) == (child, 0)
    # The sender is dead before importing or peeking at its queued descriptor.
    for flags in [socket.MSG_PEEK, 0]:
        data, controls, _, _ = parent.recvmsg(64, socket.CMSG_SPACE(4), flags)
        received, = array.array('i', controls[0][2])
        program = int(data)
        assert identity(received) == program
        os.close(received)
    parent.close()
    # The escrow helper exits asynchronously after the final message consumption.
    deadline = time.monotonic() + 3
    while True:
        try:
            absent(program)
            break
        except AssertionError:
            assert time.monotonic() < deadline, 'escrow retained the consumed program'
            time.sleep(.01)
elif mode == 'fifo-owner':
    os.mkfifo(path)
    fd = os.open(path, os.O_RDWR | os.O_NONBLOCK)
    os.write(fd, b'first-domain')
    print('FIFO_OWNER_READY', flush=True)
    while not os.path.exists(path + '.stop'):
        time.sleep(.01)
    os.close(fd)
elif mode == 'fifo-isolated':
    fd = os.open(path, os.O_RDWR | os.O_NONBLOCK)
    try:
        os.read(fd, 64)
    except BlockingIOError:
        pass
    else:
        raise AssertionError('FIFO data crossed independent init domains')
    os.write(fd, b'second-domain')
    assert os.read(fd, 64) == b'second-domain'
    os.close(fd)
elif mode == 'fifo-original':
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    assert os.read(fd, 64) == b'first-domain'
    os.close(fd)
    open(path + '.stop', 'w').close()
elif mode == 'fifo-clean':
    os.unlink(path)
    os.unlink(path + '.stop')
elif mode in ('netlink-owner', 'netlink-occupied', 'netlink-released'):
    sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0)
    if mode == 'netlink-owner':
        sock.bind((0, 0))
        with open(path, 'w') as file:
            json.dump(sock.getsockname()[0], file)
        print('NETLINK_OWNER_READY', flush=True)
        while not os.path.exists(path + '.stop'):
            time.sleep(.01)
    else:
        with open(path) as file:
            port = json.load(file)
        if mode == 'netlink-occupied':
            try:
                sock.bind((port, 0))
            except OSError as error:
                assert error.errno == errno.EADDRINUSE, error
            else:
                raise AssertionError('two workers bound the same Netlink port')
            open(path + '.stop', 'w').close()
        else:
            sock.bind((port, 0))
            os.unlink(path)
            os.unlink(path + '.stop')
    sock.close()
elif mode == 'netlink-fork':
    sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW | socket.SOCK_NONBLOCK, 0)
    sock.bind((0, 0))
    sock.sendto(struct.pack('<IHHII', 32, 0x7777, 1, 31, 0) + bytes(16), (0, 0))
    child = os.fork()
    if child == 0:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 32768)
        assert sock.recv(4096)
        os._exit(0)
    assert os.waitpid(child, 0) == (child, 0)
    assert sock.getsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF) == 65536
    try:
        sock.recv(4096)
    except BlockingIOError:
        pass
    else:
        raise AssertionError('fork copied the Netlink receive queue')
    sock.close()
elif mode == 'netlink-rights':
    parent, child_socket = socket.socketpair()
    child = os.fork()
    if child == 0:
        parent.close()
        sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW | socket.SOCK_NONBLOCK, 0)
        sock.bind((0, 0))
        sock.sendto(struct.pack('<IHHII', 32, 0x7777, 1, 32, 0) + bytes(16), (0, 0))
        child_socket.sendmsg([str(sock.getsockname()[0]).encode()],
                            [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [sock.fileno()]))])
        os._exit(0)
    child_socket.close()
    assert os.waitpid(child, 0) == (child, 0)
    data, controls, _, _ = parent.recvmsg(64, socket.CMSG_SPACE(4))
    fd, = array.array('i', controls[0][2])
    sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0, fileno=fd)
    assert sock.getsockname()[0] == int(data)
    assert sock.recv(4096)
    sock.close()
    parent.close()
else:
    raise AssertionError(mode)
print('PASS_' + mode, flush=True)
