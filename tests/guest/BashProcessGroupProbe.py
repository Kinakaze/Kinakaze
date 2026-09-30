"""Keep unreaped pipeline leaders joinable and exercise real Bash job control."""
import errno
import json
import os
from pathlib import Path
import pty
import select
import signal
import termios
import time


def zombie_group():
    ready, writer = os.pipe()
    leader = os.fork()
    if leader == 0:
        os.close(ready)
        os.setpgid(0, 0)
        os.write(writer, b'R')
        os._exit(0)
    os.close(writer)
    assert os.read(ready, 1) == b'R'
    os.close(ready)
    # Observe exit without reaping: this process still owns its group ID.
    info = os.waitid(os.P_PID, leader, os.WEXITED | os.WNOWAIT)
    assert info.si_pid == leader
    gate, release = os.pipe()
    result, sender = os.pipe()
    child = os.fork()
    if child == 0:
        os.close(release)
        os.close(result)
        os.read(gate, 1)
        try:
            os.setpgid(0, leader)
            value = os.getpgrp()
        except OSError as error:
            value = -error.errno
        os.write(sender, str(value).encode())
        os._exit(0)
    os.close(gate)
    os.close(sender)
    parent_error = None
    try:
        os.setpgid(child, leader)
    except OSError as error:
        parent_error = error.errno
    os.write(release, b'R')
    os.close(release)
    value = int(os.read(result, 64))
    os.close(result)
    assert os.waitpid(child, 0)[1] == 0
    assert os.waitpid(leader, 0)[1] == 0
    assert parent_error is None and value == leader, (parent_error, value, leader)


def permission_checks():
    for pid, group, expected in [(-1, 0, errno.EINVAL), (0, -1, errno.EINVAL),
                                  (0x7fffffff, 0, errno.ESRCH)]:
        try:
            os.setpgid(pid, group)
        except OSError as error:
            assert error.errno == expected, error
        else:
            raise AssertionError(('unexpected success', pid, group))
    ready, sender = os.pipe()
    gate, release = os.pipe()
    child = os.fork()
    if child == 0:
        os.close(ready)
        os.close(release)
        os.setsid()
        os.write(sender, b'R')
        os.read(gate, 1)
        os._exit(0)
    os.close(sender)
    os.close(gate)
    try:
        assert os.read(ready, 1) == b'R'
        try:
            os.setpgid(child, os.getpgrp())
        except OSError as error:
            assert error.errno == errno.EPERM, error
        else:
            raise AssertionError('moved a child across sessions')
    finally:
        os.close(ready)
        os.write(release, b'R')
        os.close(release)
        os.waitpid(child, 0)


def bash_pipeline():
    child, master = pty.fork()
    if child == 0:
        attrs = termios.tcgetattr(0)
        attrs[3] &= ~termios.ECHO
        termios.tcsetattr(0, termios.TCSANOW, attrs)
        environment = dict(os.environ, PS1='PG_READY> ', PATH='/usr/bin:/bin',
                           TERM='dumb', LC_ALL='C', BASH_ENV='/dev/null')
        os.execve('/bin/bash', ['/bin/bash', '--noprofile', '--norc', '-i'], environment)
    transcript = bytearray()
    pending = bytearray()

    def expect(marker, timeout=60):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if marker in pending:
                end = pending.index(marker) + len(marker)
                observed = bytes(pending[:end])
                del pending[:end]
                return observed
            if select.select([master], [], [], .1)[0]:
                part = os.read(master, 65536)
                assert part, bytes(transcript[-2000:])
                pending.extend(part)
                transcript.extend(part)
        raise AssertionError(('terminal timeout', marker, bytes(transcript[-2000:])))

    try:
        expect(b'PG_READY> ')
        os.write(master, b"for i in {1..30}; do true | cat | cat | cat; done; printf 'PIPELINES_OK\\n'\n")
        expect(b'PIPELINES_OK\r\n')
        expect(b'PG_READY> ')
        os.write(master, b"for i in {1..10}; do true | cat | cat & wait; done; printf 'BACKGROUND_OK\\n'\n")
        expect(b'BACKGROUND_OK\r\n')
        expect(b'PG_READY> ')
        os.write(master, b'sleep 30 | cat | cat\n')
        time.sleep(1)
        os.write(master, b'\x03')
        expect(b'PG_READY> ', timeout=15)
        os.write(master, b"printf 'INTERRUPT_STATUS=%s\\n' $?\n")
        expect(b'INTERRUPT_STATUS=130\r\n')
        expect(b'PG_READY> ')
        assert b'child setpgid' not in transcript, bytes(transcript[-4000:])
        assert b'Operation not permitted' not in transcript, bytes(transcript[-4000:])
    finally:
        Path('bash-process-groups.txt').write_bytes(transcript)
        os.kill(child, signal.SIGKILL)
        os.waitpid(child, 0)
        os.close(master)


rows = []
for name, test in [('unreaped-group', zombie_group), ('permission-checks', permission_checks),
                    ('bash-pipelines-and-interrupt', bash_pipeline)]:
    try:
        test()
        rows.append(dict(case=name, passed=True))
    except Exception as error:
        rows.append(dict(case=name, passed=False, error=repr(error)))
    print(json.dumps(rows[-1]), flush=True)
assert all(row['passed'] for row in rows), rows
print('BASH_PROCESS_GROUPS_OK', flush=True)
