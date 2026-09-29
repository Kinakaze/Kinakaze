"""Ctrl-C must interrupt a foreground job and return a usable Bash prompt."""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import sys
import termios
import time

pid, master = pty.fork()
if pid == 0:
    home = Path.cwd() / 'claude-home'
    home.mkdir(exist_ok=True)
    config = json.dumps({'hasCompletedOnboarding': True, 'theme': 'dark'})
    (home / '.claude.json').write_text(config)
    (home / '.claude').mkdir(exist_ok=True)
    (home / '.claude' / '.claude.json').write_text(config)
    environment = dict(os.environ, PS1='BASH_READY> ', TERM='xterm-256color',
                       PATH='/usr/bin:/bin', BASH_ENV='/dev/null', HOME=str(home),
                       CLAUDE_CONFIG_DIR=str(home / '.claude'))
    os.execve('/bin/bash', ['/bin/bash', '--noprofile', '--norc', '-i'], environment)
fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
transcript = bytearray()
pending = bytearray()
def expect(marker, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if marker in pending:
            end = pending.index(marker) + len(marker)
            data = bytes(pending[:end])
            del pending[:end]
            print('observed', repr(marker), flush=True)
            if marker == b'BASH_READY> ':
                time.sleep(.15)
            return data
        if select.select([master], [], [], .1)[0]:
            try:
                part = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            pending.extend(part)
            transcript.extend(part)
            if b'\x1b[6n' in part:
                os.write(master, b'\x1b[1;1R')
    raise AssertionError((marker, bytes(pending[-3000:])))

status = None
try:
    expect(b'BASH_READY> ')
    for _ in range(2):
        os.write(master, b"/bin/sh -c 'echo SLEEP_RUNNING; exec sleep 30'\r")
        expect(b'SLEEP_RUNNING\r\n')
        os.write(master, b'\x03')
        expect(b'BASH_READY> ')
        os.write(master, b"printf 'INTERRUPT_STATUS=%s\\n' $?\r")
        expect(b'INTERRUPT_STATUS=130\r\n')
        expect(b'BASH_READY> ')
    # Interactive agents receive literal ^C with ISIG disabled. Verify those
    # bytes reach the foreground application and Bash restores cooked mode.
    Path('raw_input.py').write_text('import os,tty,termios\n'
        'saved=termios.tcgetattr(0)\ntty.setraw(0)\nprint("RAW_READY",flush=True)\n'
        'try:\n assert os.read(0,1)==b"\\x03"\n print("RAW_CTRL_C_OK",flush=True)\n'
        'finally: termios.tcsetattr(0,termios.TCSANOW,saved)\n')
    os.write(master, b'python3 raw_input.py\r')
    expect(b'RAW_READY')
    os.write(master, b'\x03')
    expect(b'BASH_READY> ')
    assert b'RAW_CTRL_C_OK' in transcript
    if len(sys.argv) > 1:
        pending.clear()
        before = len(transcript)
        os.write(master, (sys.argv[1] + '\r').encode())
        # Claude initializes its raw-mode UI asynchronously. Let the terminal
        # queries resolve, then send two separate presses, as its UI requires.
        started = time.monotonic()
        while time.monotonic() - started < 12:
            if select.select([master], [], [], .1)[0]:
                part = os.read(master, 65536)
                transcript.extend(part)
                if b'\x1b[6n' in part:
                    os.write(master, b'\x1b[1;1R')
        screen = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b' ', bytes(transcript[before:])).lower()
        screen = b' '.join(screen.split())
        assert any(m in screen for m in (b'select a theme', b'choose a theme', b'welcome to claude', b'claude code')), screen[-3000:]
        assert b'exec format error' not in screen
        assert b'bash_ready>' not in screen, ('Claude exited before Ctrl-C', screen[-1500:])
        os.write(master, b'\x03')
        time.sleep(.4)
        os.write(master, b'\x03')
        expect(b'BASH_READY> ', timeout=15)
    os.write(master, b"printf '\\nBASH_INPUT_OK\\n'\r")
    expect(b'BASH_INPUT_OK\r\n')
    expect(b'BASH_READY> ')
    # The assertion is a usable shell after interruption. Terminate this owned
    # interactive shell in finally instead of conflating Bash's exit policy
    # (stopped jobs, EOF, startup configuration) with foreground interruption.
    print('BASH_INTERRUPT_OK')
finally:
    Path('bash-interrupt-transcript.txt').write_bytes(transcript)
    if status is None:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    os.close(master)
