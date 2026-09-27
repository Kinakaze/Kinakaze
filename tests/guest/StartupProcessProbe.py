"""Exercise redirected spawn and controlling-terminal IO after fork/exec."""
import fcntl
import os
import select
import signal
import sys
import termios
import time


if sys.argv[1:] == ['--pty-child']:
    assert all(os.isatty(fd) for fd in range(3))
    before = termios.tcgetattr(0)
    quiet = before[:]
    quiet[3] &= ~termios.ECHO
    termios.tcsetattr(0, termios.TCSANOW, quiet)
    print('PTY_READY', flush=True)
    try:
        assert sys.stdin.readline() == 'private-input\n'
    finally:
        termios.tcsetattr(0, termios.TCSANOW, before)
    restored = termios.tcgetattr(0)
    # tcsetattr encodes the separately reported speeds into c_cflag. Compare
    # those speeds directly and mask their redundant encoding in c_cflag.
    baud_mask = termios.CBAUD | termios.CIBAUD
    assert restored[2] & ~baud_mask == before[2] & ~baud_mask, (before, restored)
    assert restored[:2] + restored[3:] == before[:2] + before[3:], (before, restored)
    print('PTY_STDOUT_OK', flush=True)
    print('PTY_STDERR_OK', file=sys.stderr, flush=True)
    raise SystemExit(7)


# posix_spawn must preserve explicit dup/close actions and a nonzero exit status.
reader, writer = os.pipe()
try:
    pid = os.posix_spawn('/bin/sh', ['/bin/sh', '-c',
        'test "$SPAWN_VALUE" = inherited || exit 99; printf spawn-out; printf spawn-err >&2; exit 9'],
        {'PATH': '/bin:/usr/bin', 'SPAWN_VALUE': 'inherited'},
        file_actions=[(os.POSIX_SPAWN_CLOSE, reader),
                      (os.POSIX_SPAWN_DUP2, writer, 1),
                      (os.POSIX_SPAWN_DUP2, writer, 2),
                      (os.POSIX_SPAWN_CLOSE, writer)])
    os.close(writer)
    writer = -1
    chunks = []
    while chunk := os.read(reader, 4096):
        chunks.append(chunk)
    assert b''.join(chunks) == b'spawn-outspawn-err'
    assert os.waitpid(pid, 0) == (pid, 9 << 8)
finally:
    os.close(reader)
    if writer >= 0:
        os.close(writer)


master, slave = os.openpty()
child = os.fork()
if child == 0:
    os.close(master)
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in range(3):
        os.dup2(slave, fd)
    if slave > 2:
        os.close(slave)
    os.execve(sys.executable, [sys.executable, '-B', __file__, '--pty-child'],
              {'PATH': '/bin:/usr/bin'})

output = b''
finished = False
try:
    deadline = time.monotonic() + 15
    sent = False
    while b'PTY_STDOUT_OK' not in output or b'PTY_STDERR_OK' not in output:
        assert time.monotonic() < deadline, output
        if not select.select([master], [], [], 0.1)[0]:
            continue
        chunk = os.read(master, 4096)
        assert chunk, output
        output += chunk
        assert b'Traceback' not in output, output
        if b'PTY_READY' in output and not sent:
            assert not termios.tcgetattr(slave)[3] & termios.ECHO
            os.write(master, b'private-input\n')
            sent = True
    assert b'private-input' not in output, output
    while True:
        pid, status = os.waitpid(child, os.WNOHANG)
        if pid:
            finished = True
            assert status == 7 << 8, (status, output)
            break
        assert time.monotonic() < deadline
        time.sleep(0.01)
finally:
    if not finished:
        os.kill(child, signal.SIGKILL)
        os.waitpid(child, 0)
    os.close(master)
    os.close(slave)
print('STARTUP_PROCESS_IO_OK')
