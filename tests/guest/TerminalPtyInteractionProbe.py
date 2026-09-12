"""Run inside a dedicated GNOME terminal; host drives keyboard and scrolling."""
import json
import os
import select
import subprocess
import time
from pathlib import Path

report = Path('/tmp/terminal-pty-interaction.json')
assert os.isatty(0) and os.isatty(1), 'real terminal PTY required'
print('Kinakaze PTY keyboard test: enter terminal-input-123', flush=True)
assert select.select([0], [], [], 40)[0], 'keyboard input timed out'
line = input()
assert line == 'terminal-input-123', repr(line)
commands = subprocess.run(
    ['/usr/bin/python3.11', '/tmp/TerminalDesktopCommandsProbe.py'],
    capture_output=True, text=True, timeout=30,
)
print(commands.stdout, end='', flush=True)
assert commands.returncode == 0, commands.stderr
for index in range(240):
    print(f'Scroll row {index:03d}: selectable text and terminal rendering')
print('PTY_INPUT_LS_TOP_AND_SCROLL_READY', flush=True)
report.write_text(json.dumps({'status': 'ready', 'input': line, 'commands': commands.returncode}))
deadline = time.monotonic() + 40
while time.monotonic() < deadline:
    if Path('/tmp/terminal-pty-interaction-close').exists():
        break
    time.sleep(.1)
