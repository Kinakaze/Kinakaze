"""Keep a single Linux parent/PID alive beyond the transaction replay quota."""
import os
import sys
import time

mode, count, gate = sys.argv[1:4]
count = int(count)
initial_pid = int(sys.argv[4]) if len(sys.argv) > 4 else os.getpid()
started = float(sys.argv[5]) if len(sys.argv) > 5 else time.monotonic()
assert os.getpid() == initial_pid
if mode == 'fork':
    for index in range(count):
        child = os.fork()
        if child == 0:
            os._exit(0)
        assert os.waitpid(child, 0) == (child, 0)
        if (index + 1) % 100 == 0:
            print('CHURN_PROGRESS', mode, index + 1, flush=True)
elif mode == 'exec':
    if count:
        if count % 100 == 0:
            print('CHURN_PROGRESS', mode, count, flush=True)
        os.execv('/usr/bin/python3', ['python3', __file__, mode, str(count - 1),
                                    gate, str(initial_pid), str(started)])
else:
    raise AssertionError(mode)
print('CHURN_READY', mode, round(time.monotonic() - started, 3), flush=True)
deadline = time.monotonic() + 15
while not os.path.isfile(gate):
    assert time.monotonic() < deadline, 'host did not check live replay state'
    time.sleep(.01)
print('CHURN_OK', mode, flush=True)
