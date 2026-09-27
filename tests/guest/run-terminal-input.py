"""ConPTY regression: shell mouse reporting is off; guest TUIs can opt in."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import subprocess

spec = importlib.util.spec_from_file_location('terminal_helpers', Path(__file__).with_name('run-default-sshd.py'))
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)


def modes(text):
    result = {}
    for values, action in re.findall(r'\x1b\[\?([0-9;]+)([hl])', text):
        for value in values.split(';'):
            result[int(value)] = action == 'h'
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    dist, root = args.dist.resolve(), args.root.resolve()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    base = [str(dist / 'worker.exe')]
    options = ['--root', str(root), '--rootfs-manifest', str(args.manifest.resolve()), '--no-tray']
    report = {'status': 'failed', 'checks': []}
    terminals = []
    first_mouse = b'\x1b[<0;66;7M'
    second_mouse = b'\x1b[<0;66;7m'

    def command(arguments):
        return subprocess.check_output(base + arguments, creationflags=subprocess.CREATE_NO_WINDOW, timeout=120)

    def attach(label):
        term = helpers.Terminal(base + ['session', 'attach', *options, '--name', 'bash'], args.report.parent)
        terminals.append((label, term))
        term.wait_for('Ctrl+]')
        return term

    def assert_shell_mode(term):
        current = modes(term.text)
        assert not any(current.get(mode, False) for mode in (9, 1000, 1002, 1003)), current

    try:
        command(['session', 'start', *options])
        state = json.loads(command(['session', 'status', *options]))
        pid = next(t['pid'] for t in state['terminals'] if t['name'] == 'bash')
        term = attach('first')
        term.send("printf '\\nSHELL_%s\\n' READY\r")
        term.wait_for('SHELL_READY')
        assert_shell_mode(term)
        report['checks'].append('plain Bash does not enable mouse reporting')
        source = '''import os,select,termios,tty
previous=termios.tcgetattr(0)
def receive(expected):
    data=b''
    while len(data)<len(expected):
        if not select.select([0],[],[],60)[0]: raise TimeoutError(data)
        data+=os.read(0,len(expected)-len(data))
    assert data==expected,(data,expected)
try:
    tty.setraw(0)
    os.write(1,b'\\x1b[?1003h\\x1b[?1006hMOUSE_READY\\r\\n')
    receive(b'\\x1b[<0;66;7M')
    os.write(1,b'MOUSE_FIRST_OK\\r\\n')
    receive(b'\\x1b[<0;66;7m')
finally:
    os.write(1,b'\\x1b[?1003l\\x1b[?1006l')
    termios.tcsetattr(0,termios.TCSANOW,previous)
print('MOUSE_DONE',flush=True)
'''
        (root / 'tmp/session-mouse-probe.py').write_bytes(source.encode())
        term.send('/usr/bin/python3 /tmp/session-mouse-probe.py\r')
        term.wait_for('MOUSE_READY')
        assert modes(term.text).get(1003) and modes(term.text).get(1006), repr(term.text)
        term.send(first_mouse.decode())
        term.wait_for('MOUSE_FIRST_OK')
        term.send('\x1d')
        term.finish()
        assert_shell_mode(term)
        report['checks'].append('guest TUI explicitly enables mouse and receives SGR press')
        report['checks'].append('detach cleans host mouse modes while the guest TUI remains alive')
        again = attach('reconnected')
        again.wait_for('MOUSE_FIRST_OK')
        assert modes(again.text).get(1003), repr(again.text)
        again.send(second_mouse.decode())
        again.wait_for('MOUSE_DONE')
        again.send("printf '\\nRETURNED_%s\\n' TO_SHELL\r")
        again.wait_for('RETURNED_TO_SHELL')
        assert_shell_mode(again)
        report['checks'].append('reattached TUI receives SGR release; returning to Bash disables mouse')
        state = json.loads(command(['session', 'status', *options]))
        assert next(t['pid'] for t in state['terminals'] if t['name'] == 'bash') == pid
        again.send('\x1d')
        again.finish()
        assert_shell_mode(again)
        report['status'] = 'passed'
    finally:
        for label, terminal in terminals:
            (args.report.parent / (args.report.stem + '-' + label + '.log')).write_text(terminal.text, encoding='utf-8')
            terminal.close()
        subprocess.run(base + ['session', 'stop', *options], capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW)
        args.report.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
