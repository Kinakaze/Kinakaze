"""Non-consuming AF_UNIX peeks across stack/heap and record boundaries."""
import json
import select
import socket
import statistics
import time

report = {}
for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM, socket.SOCK_SEQPACKET):
    left, right = socket.socketpair(type=kind)
    try:
        right.setblocking(False)
        try:
            right.recv(64, socket.MSG_PEEK)
            raise AssertionError('empty peek did not return EAGAIN')
        except BlockingIOError:
            pass
        for size in (1, 63, 4096, 4097, 32768):
            payload = bytes(i % 251 for i in range(size))
            assert left.send(payload) == size
            for capacity in (1, 4096, 4097, size + 17):
                target = bytearray(b'!' * (capacity + 32))
                copied = right.recv_into(memoryview(target)[16:16+capacity], capacity, socket.MSG_PEEK)
                assert copied == min(size, capacity)
                assert target[:16] == b'!' * 16 and target[16+capacity:] == b'!' * 16
                assert target[16:16+copied] == payload[:copied]
                assert target[16+copied:] == b'!' * (capacity + 16 - copied)
            if kind != socket.SOCK_STREAM:
                assert len(right.recv(1, socket.MSG_PEEK | socket.MSG_TRUNC)) == size
            assert right.recv(size+1) == payload
            assert not select.select([right], [], [], 0)[0]
        if kind != socket.SOCK_STREAM:
            assert left.send(b'') == 0
            assert right.recv(8, socket.MSG_PEEK) == b''
            assert select.select([right], [], [], 0)[0]
            assert right.recv(8) == b''
            assert not select.select([right], [], [], 0)[0]
        # A large receive capacity is common for framing libraries even when
        # the current record is short. Preserve the unreturned buffer suffix.
        target = bytearray(b'!' * (1024 * 1024))
        assert left.send(b'short') == 5
        assert right.recv_into(target, len(target), socket.MSG_PEEK) == 5
        assert target[:5] == b'short' and target[5:] == b'!' * (len(target) - 5)
        assert right.recv(64) == b'short'
        for size in (64, 4096, 8192):
            payload = b'x' * size
            assert left.send(payload) == size
            target = bytearray(size)
            samples = []
            for repeat in range(8):
                start = time.perf_counter_ns()
                for _ in range(1000):
                    assert right.recv_into(target, size, socket.MSG_PEEK) == size
                if repeat:
                    samples.append((time.perf_counter_ns()-start)/1_000_000)
            assert target == payload and right.recv(size) == payload
            report[f'kind_{kind}_peek_{size}_1000'] = dict(median_ms=statistics.median(samples), samples_ms=samples)
        left.close()
        assert right.recv(64, socket.MSG_PEEK) == b''
    finally:
        left.close()
        right.close()
print(json.dumps(report, sort_keys=True), flush=True)
print('UnixPeekProbe: PASS', flush=True)
