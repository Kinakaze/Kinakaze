"""Measure short GUI poll deadlines, while verifying readiness is not consumed."""
import json
import select
import socket
import statistics
import time

results = {}
for kind in ("empty", "unix", "tcp", "epoll"):
    left = right = server = None
    if kind == "unix":
        left, right = socket.socketpair()
    elif kind == "tcp":
        server = socket.socket()
        server.bind(("127.0.0.1", 0))
        server.listen()
        right = socket.create_connection(server.getsockname())
        left, _ = server.accept()
    waiter = select.epoll() if kind == "epoll" else select.poll()
    if left is not None:
        waiter.register(left, select.POLLIN)
    try:
        for timeout in (1, 4, 7):
            samples = []
            for _ in range(40):
                start = time.perf_counter()
                ready = waiter.poll(timeout / 1000 if kind == "epoll" else timeout)
                samples.append((time.perf_counter() - start) * 1000)
                assert not ready, (kind, ready)
            results[f"{kind}_{timeout}ms"] = {
                "median_ms": round(statistics.median(samples), 3),
                "p95_ms": round(sorted(samples)[37], 3),
                "min_ms": round(min(samples), 3),
            }
        if left is not None:
            right.sendall(b"ready")
            assert waiter.poll(1000), kind
            assert waiter.poll(0), (kind, "readiness consumed")
            assert left.recv(5) == b"ready"
            assert not waiter.poll(0), (kind, "stale readiness")
    finally:
        if kind == "epoll":
            waiter.close()
        for sock in (left, right, server):
            if sock is not None:
                sock.close()
print(json.dumps(results, sort_keys=True), flush=True)
for name, sample in results.items():
    requested = int(name.rsplit("_", 1)[1][:-2])
    assert sample["min_ms"] >= requested * 0.85, (name, "returned before deadline", sample)
print("POLL_DEADLINE_READINESS_OK", flush=True)
