"""Linux AF_UNIX timestamp flags follow the socket through fork/exec/SCM_RIGHTS.

Linux 6.1/6.12 retain these generic flags but unix recvmsg does not emit timestamp
cmsgs. Check that behavior without manufacturing a receive-time timestamp.
"""
import array
import errno
import os
import socket
import subprocess
import sys


OPTIONS = (29, 35, 63, 64)  # SO_TIMESTAMP/NS, old and time64 ABIs


def state(sock):
    return tuple(sock.getsockopt(socket.SOL_SOCKET, option) for option in OPTIONS)


def set_option(sock, option, value):
    sock.setsockopt(socket.SOL_SOCKET, option, value)


if len(sys.argv) > 1:
    with socket.socket(fileno=int(sys.argv[1])) as inherited:
        assert state(inherited) == (1, 0, 0, 0)
        set_option(inherited, 64, 1)
    raise SystemExit

for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM, socket.SOCK_SEQPACKET):
    # A separate unconnected endpoint exercises publication during bind.
    with socket.socket(socket.AF_UNIX, kind) as bound:
        set_option(bound, 35, 1)
        bound.bind('\0timestamp-' + str(os.getpid()) + '-' + str(kind))
        assert state(bound) == (0, 1, 0, 0)
    left, right = socket.socketpair(socket.AF_UNIX, kind)
    with left, right, left.dup() as alias:
        assert state(left) == (0, 0, 0, 0)
        for option, expected in zip(OPTIONS, [(1, 0, 0, 0), (0, 1, 0, 0),
                                              (0, 0, 1, 0), (0, 0, 1, 1)]):
            set_option(left, option, -1)
            assert state(alias) == expected
            assert state(right) == (0, 0, 0, 0)
        set_option(alias, 29, 0)
        assert state(left) == (0, 0, 0, 0)
        try:
            set_option(left, 29, b'\1')
        except OSError as error:
            assert error.errno == errno.EINVAL, error
        else:
            raise AssertionError('short integer option accepted')
        set_option(alias, 29, 1)
        subprocess.run([sys.executable, __file__, str(left.fileno())],
                       pass_fds=(left.fileno(),), check=True, timeout=10)
        assert state(alias) == (0, 0, 1, 1)
        right.send(b'x')
        data, ancillary, flags, _ = left.recvmsg(1, 256)
        assert data == b'x' and ancillary == [] and flags == 0

        # Transferred descriptors retain the same option state and can mutate it.
        first, second = socket.socketpair()
        with first, second:
            first.sendmsg([b'x'], [(socket.SOL_SOCKET, socket.SCM_RIGHTS,
                                   array.array('i', [left.fileno()]))])
            _, control, _, _ = second.recvmsg(1, socket.CMSG_SPACE(4))
            assert len(control) == 1 and control[0][:2] == (socket.SOL_SOCKET, socket.SCM_RIGHTS)
            fd, = array.array('i', control[0][2])
            with socket.socket(fileno=fd) as transferred:
                assert state(transferred) == (0, 0, 1, 1)
                set_option(transferred, 35, 0)
            assert state(alias) == (0, 0, 0, 0)

print('UNIX_SOCKET_OPTIONS_OK')
