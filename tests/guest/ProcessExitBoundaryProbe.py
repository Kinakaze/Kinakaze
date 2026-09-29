"""Repeated exact exit status, exec, WNOWAIT, signal and pipe EOF checks."""
import errno
import os
import select
import signal
import sys
import threading


if len(sys.argv) > 1 and sys.argv[1] == 'exec-child':
    os._exit(int(sys.argv[2]))


def reaped(pid):
    try:
        os.waitpid(pid, os.WNOHANG)
    except ChildProcessError as error:
        assert error.errno == errno.ECHILD
    else:
        raise AssertionError('child was waitable after reap')


rounds = int(sys.argv[1]) if len(sys.argv) > 1 else 32
for index in range(rounds):
    code = (index * 37) % 256
    reader, writer = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(reader)
        os.write(writer, b'ready')
        if index % 2:
            os.execv(sys.executable, [sys.executable, __file__, 'exec-child', str(code)])
        os._exit(code)
    os.close(writer)
    # WNOWAIT must preserve the exact status for repeated observations.
    for _ in range(2):
        info = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOWAIT)
        assert (info.si_pid, info.si_code, info.si_status) == (pid, os.CLD_EXITED, code), info
    assert os.waitpid(pid, 0) == (pid, code << 8)
    payload = b''
    while True:
        assert select.select([reader], [], [], 3)[0], 'wait returned but pipe writer survived'
        chunk = os.read(reader, 64)
        if not chunk:
            break
        payload += chunk
    os.close(reader)
    assert payload == b'ready', payload
    reaped(pid)
    if (index + 1) % 8 == 0:
        print('EXIT_BOUNDARY_PROGRESS', index + 1, flush=True)

def concurrent_reapers():
    # Hold all children behind pipes so they are alive for the initial WNOHANG.
    children = []
    for index in range(8):
        reader, writer = os.pipe()
        pid = os.fork()
        if pid == 0:
            os.close(writer)
            os.read(reader, 1)
            os._exit(40 + index)
        os.close(reader)
        children.append((pid, writer, 40 + index))
    assert os.waitpid(-1, os.WNOHANG) == (0, 0)
    for pid, writer, code in children:
        os.write(writer, b'x')
        os.close(writer)
    expected = {pid: code << 8 for pid, _, code in children}
    observed = []
    errors = []
    barrier = threading.Barrier(4)


    def reap_concurrently():
        try:
            barrier.wait(timeout=10)
            while True:
                try:
                    result = os.waitpid(-1, 0)
                except ChildProcessError as error:
                    assert error.errno == errno.ECHILD
                    return
                observed.append(result)
        except BaseException as error:
            errors.append(repr(error))


    threads = [threading.Thread(target=reap_concurrently) for _ in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=15)
    assert not any(thread.is_alive() for thread in threads), 'concurrent reaper hung'
    assert not errors, errors
    assert len(observed) == len(expected) and dict(observed) == expected, (observed, expected)
    reaped(-1)


for index in range(rounds):
    concurrent_reapers()
    if (index + 1) % 8 == 0:
        print("CONCURRENT_REAP_PROGRESS", index + 1, flush=True)

# Native completion must preserve a Linux signal exit rather than guess a code.
for number in (signal.SIGTERM, signal.SIGKILL):
    reader, writer = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(reader)
        os.write(writer, b'x')
        while True:
            signal.pause()
    os.close(writer)
    assert os.read(reader, 1) == b'x'
    os.close(reader)
    os.kill(pid, number)
    waited, status = os.waitpid(pid, 0)
    assert waited == pid and os.WIFSIGNALED(status) and os.WTERMSIG(status) == number
    reaped(pid)
print('PROCESS_EXIT_BOUNDARY_OK', rounds, flush=True)
