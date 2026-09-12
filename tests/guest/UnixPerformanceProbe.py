"""AF_UNIX throughput and cross-process latency without GUI activity."""
import array
import fcntl
import json
import os
import socket
import statistics
import struct
import termios
import time


def samples(operation, repeats=7):
    operation()
    values = []
    for _ in range(repeats):
        start = time.perf_counter_ns()
        operation()
        values.append((time.perf_counter_ns() - start) / 1_000_000)
    return {"median_ms": statistics.median(values), "samples_ms": values}


def main():
    report = {"scope": "same-process ready transfers and forked ping-pong; warmup excluded"}
    identity = (os.getpid(), os.geteuid(), os.getegid())
    def connect_and_release():
        for _ in range(40):
            left, right = socket.socketpair()
            try:
                for channel in (left, right):
                    assert struct.unpack('iII', channel.getsockopt(
                        socket.SOL_SOCKET, socket.SO_PEERCRED, 12)) == identity
                left.sendall(b'new-connection')
                assert right.recv(32) == b'new-connection'
            finally:
                left.close()
                right.close()
    report['connect_peercred_release_40'] = samples(connect_and_release)
    for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM, socket.SOCK_SEQPACKET):
        left, right = socket.socketpair(type=kind)
        try:
            for size in (64, 4096):
                payload = b"x" * size
                def transfer():
                    for _ in range(200):
                        assert left.send(payload) == size
                        assert right.recv(size) == payload
                report[f"kind_{kind}_bytes_{size}_200"] = samples(transfer)
            if kind == socket.SOCK_STREAM:
                left.sendall(b"hello world")
                count = array.array("i", [0])
                fcntl.ioctl(right, termios.FIONREAD, count, True)
                assert count[0] == 11, count
                assert right.recv(11, socket.MSG_PEEK) == b"hello world"
                assert right.recv(11) == b"hello world"
            right.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
            payload = b"credentials" * 6
            expected = (os.getpid(), os.getuid(), os.getgid())
            def credentials_transfer():
                for _ in range(200):
                    assert left.send(payload) == len(payload)
                    data, ancillary, flags, _ = right.recvmsg(len(payload), socket.CMSG_SPACE(12))
                    assert data == payload and flags == 0
                    assert len(ancillary) == 1
                    level, control_kind, control = ancillary[0]
                    assert (level, control_kind) == (socket.SOL_SOCKET, socket.SCM_CREDENTIALS)
                    assert struct.unpack('3i', control) == expected
            key = 'stream_credentials_200' if kind == socket.SOCK_STREAM else f'kind_{kind}_credentials_200'
            report[key] = samples(credentials_transfer)
        finally:
            left.close()
            right.close()

    left, right = socket.socketpair()
    child = os.fork()
    if child == 0:
        left.close()
        try:
            while right.recv(1):
                right.sendall(b"y")
        finally:
            right.close()
        os._exit(0)
    right.close()
    try:
        def pingpong():
            for _ in range(100):
                left.sendall(b"x")
                assert left.recv(1) == b"y"
        report["fork_pingpong_100"] = samples(pingpong)
    finally:
        left.close()
        assert os.waitpid(child, 0) == (child, 0)
    print(json.dumps(report, sort_keys=True), flush=True)
    print("UnixPerformanceProbe: PASS", flush=True)


if __name__ == "__main__":
    main()
