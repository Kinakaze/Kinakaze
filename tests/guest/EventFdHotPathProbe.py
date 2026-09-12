"""Counter polling/read/write and cached-description lifetime regression."""
import os
import select
import time

fd = os.eventfd(0, os.EFD_NONBLOCK | os.EFD_CLOEXEC)
poller = select.poll(); poller.register(fd, select.POLLIN)
started = time.perf_counter()
for _ in range(2000): assert not poller.poll(0)
poll_ms = (time.perf_counter() - started) * 1000
started = time.perf_counter()
for _ in range(1000):
    os.eventfd_write(fd, 3); assert os.eventfd_read(fd) == 3
rw_ms = (time.perf_counter() - started) * 1000
alias = os.dup(fd); os.close(fd)
pid = os.fork()
if pid == 0:
    os.eventfd_write(alias, 7); os._exit(0)
assert os.waitpid(pid, 0)[1] == 0
assert os.eventfd_read(alias) == 7
os.close(alias)
# Reused fd numbers must not retain the previous counter mapping.
for value in range(1, 25):
    fd = os.eventfd(value, os.EFD_NONBLOCK)
    assert os.eventfd_read(fd) == value
    os.close(fd)
print(f'EVENTFD_POLL_DUP_FORK_REUSE_OK poll_2000_ms={poll_ms:.2f} rw_1000_ms={rw_ms:.2f}', flush=True)
