"""Drive real guest PTYs without stored credentials or model requests."""
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
import tempfile
import termios
import time


def run(command, home, recognize, keys, verify=None):
    pid, master = pty.fork()
    if pid == 0:
        environment = {key: value for key, value in os.environ.items() if not key.startswith(
            ('OPENAI_', 'ANTHROPIC_', 'CODEX_', 'CLAUDE_'))}
        environment.update(HOME=str(home), CODEX_HOME=str(home / 'codex'),
                           CLAUDE_CONFIG_DIR=str(home / 'claude'), TERM='xterm-256color',
                           PATH='/usr/sbin:/usr/bin:/sbin:/bin', LANG='C.UTF-8')
        os.chdir(home)
        os.execve(command[0], command, environment)
    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
    output = bytearray()
    status, sent = None, False
    deadline = time.monotonic() + 30
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], .1)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b''
                if data:
                    output.extend(data)
                    for query, response in [(b'\x1b[6n', b'\x1b[1;1R'),
                                             (b'\x1b[c', b'\x1b[?1;2c'),
                                             (b'\x1b]11;?', b'\x1b]11;rgb:0000/0000/0000\x1b\\')]:
                        if query in data:
                            os.write(master, response)
            if not sent and recognize(bytes(output)):
                os.write(master, keys)
                sent = True
                deadline = min(deadline, time.monotonic() + 8)
            waited, observed = os.waitpid(pid, os.WNOHANG)
            if waited:
                status = observed
                break
        plain = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b' ', bytes(output)).lower()
        if not sent and b'anthropic' in plain and b'403' in plain:
            return dict(command=command, status='external_service_rejected', interactive=False,
                        exit_code=os.waitstatus_to_exitcode(status) if status is not None else None,
                        error='Claude startup received HTTP 403 from api.anthropic.com')
        assert sent, ('interactive screen missing', bytes(output[-3000:]))
        assert status is not None, ('command did not exit after interactive input', command)
        exit_code = os.waitstatus_to_exitcode(status)
        if verify:
            assert exit_code == 0, (status, bytes(output[-3000:]))
            verify()
        else:
            assert exit_code in (0, 1, 130, -signal.SIGINT), (exit_code, bytes(output[-3000:]))
        return dict(command=command, status='passed', interactive=True,
                    exit_code=exit_code,
                    transcript=bytes(output[-10000:]).decode('utf-8', errors='replace'))
    finally:
        if status is None:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        os.close(master)


with tempfile.TemporaryDirectory(prefix='.command-pty-', dir='/root') as temporary:
    home = Path(temporary)
    (home / 'codex').mkdir()
    (home / 'claude').mkdir()
    results = []
    text = home / 'edited.txt'
    def edited():
        assert text.read_text() == 'interactive-ok\n'
    for editor in ('vi', 'vim'):
        text.write_text('')
        results.append(run(['/usr/bin/' + editor, '-u', 'NONE', '-i', 'NONE', str(text)], home,
            lambda output: b'[New' in output or b'edited.txt' in output,
            b'iinteractive-ok\x1b:wq\r', edited))
    results.append(run(['/usr/bin/top', '-d', '0.1'], home, lambda output: b'Tasks:' in output,
                       b'q', lambda: None))
    for name in sys.argv[1:]:
        command = ['/tmp/goal-cli/' + name]
        if name == 'codex':
            command += ['--no-daemon', '--no-alt-screen']
        def welcome(output):
            lower = re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]', b' ', output.lower())
            lower = b' '.join(lower.split())
            return any(marker in lower for marker in (b'sign in', b'select a theme', b'choose a theme'))
        results.append(run(command, home, welcome, b'\x03\x03'))
    Path('/tmp/command-pty-results.json').write_text(json.dumps(results, indent=2))
    print('COMMAND_PTY_RESULTS', json.dumps([{key: row[key] for key in ('command', 'status', 'exit_code')} for row in results]), flush=True)
    if all(row['status'] == 'passed' for row in results):
        print('COMMAND_PTY_OK', flush=True)
    else:
        raise SystemExit(1)
