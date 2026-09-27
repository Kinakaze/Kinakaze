"""Exercise manifest SSH defaults using Windows OpenSSH and real ConPTY input.

Developer dependencies: Python and pywinpty. The release itself uses neither.
The supplied root must be a disposable installation with the default password.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import threading
import time

from winpty import PtyProcess


class Terminal:
    def __init__(self, command, cwd):
        self.output = []
        self.proc = PtyProcess.spawn(command, cwd=str(cwd), dimensions=(32, 110))
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            while self.proc.isalive():
                chunk = self.proc.read(8192)
                if chunk:
                    self.output.append(chunk)
        except (EOFError, OSError):
            pass

    @property
    def text(self):
        return ''.join(self.output)

    def wait_for(self, text, timeout=40):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if text in self.text:
                return
            if not self.proc.isalive():
                break
            time.sleep(0.05)
        raise RuntimeError(f'terminal did not produce {text!r}: {self.text[-3000:]}')

    def send(self, text):
        self.proc.write(text)

    def finish(self, expected=0, timeout=30):
        deadline = time.monotonic() + timeout
        while self.proc.isalive() and time.monotonic() < deadline:
            time.sleep(0.05)
        if self.proc.isalive():
            raise TimeoutError(f'terminal did not exit: {self.text[-3000:]}')
        status = self.proc.exitstatus
        self.reader.join(timeout=1)
        if status != expected:
            raise RuntimeError(f'expected exit {expected}, got {status}: {self.text[-3000:]}')
        return status

    def close(self):
        self.proc.close(force=True)


def port_open():
    with socket.socket() as sock:
        sock.settimeout(0.2)
        return sock.connect_ex(('127.0.0.1', 2222)) == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    dist, root, report = args.dist.resolve(), args.root.resolve(), args.report.resolve()
    report.parent.mkdir(parents=True, exist_ok=True)
    fixture = report.parent / f'ssh-default-{os.getpid()}'
    fixture.mkdir()
    ssh, sftp = shutil.which('ssh'), shutil.which('sftp')
    if not ssh or not sftp:
        parser.error('Windows OpenSSH ssh and sftp must be on PATH')
    if port_open():
        parser.error('127.0.0.1:2222 is already in use; close the previous test session')
    result = {'status': 'failed', 'dist': str(dist), 'root': str(root), 'checks': []}
    terminals = []
    started = time.monotonic()
    options = ['-F', 'NUL', '-o', 'PreferredAuthentications=password', '-o', 'PubkeyAuthentication=no',
               '-o', 'NumberOfPasswordPrompts=1', '-o', 'ConnectTimeout=10',
               '-o', 'StrictHostKeyChecking=accept-new', '-o', f'UserKnownHostsFile={fixture / "known_hosts"}']

    def terminal(command, label):
        term = Terminal(command, fixture)
        terminals.append((label, term))
        return term

    def launch(label):
        term = terminal([str(dist / 'worker.exe'), '--root', str(root)], label)
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            if not term.proc.isalive():
                raise RuntimeError(f'default login exited: {term.text}')
            if port_open():
                return term
            time.sleep(0.1)
        raise TimeoutError('default login did not start sshd on 127.0.0.1:2222')

    def login(command, label, password='kinakaze', tty=False):
        term = terminal([ssh, '-tt' if tty else '-T', '-p', '2222', *options,
                         'root@127.0.0.1', *command], label)
        term.wait_for('password:')
        term.send(password + '\r')
        return term

    def stop(term):
        term.send('exit\r')
        term.finish()
        deadline = time.monotonic() + 10
        while port_open() and time.monotonic() < deadline:
            time.sleep(0.1)
        if port_open():
            raise RuntimeError('sshd listener outlived its runtime session')

    def keys():
        return {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                for p in (root / 'etc/ssh').glob('ssh_host_*_key')}

    try:
        fresh = not keys()
        shell = launch('first-launch')
        first_keys = keys()
        if len(first_keys) != 3:
            raise RuntimeError(f'expected RSA, ECDSA and ED25519 host keys: {first_keys}')
        result['checks'].append({'first_login_autostart': True, 'generated_fresh_host_keys': fresh})
        denied = login(['echo MUST_NOT_RUN'], 'wrong-password', password='wrong-test-password')
        denied.finish(255)
        if 'Permission denied' not in denied.text or 'MUST_NOT_RUN' in denied.text:
            raise RuntimeError('incorrect password was not rejected')
        result['checks'].append({'wrong_password_rejected': True})
        command = (
            "set -e; printf 'SSH_DEFAULT_OK\\n'; id; "
            "test \"$(stat -c %a /etc/shadow)\" = 600; "
            "for key in /etc/ssh/ssh_host_*_key; do test \"$(stat -c %a \"$key\")\" = 600; done; "
            "/usr/sbin/sshd -T | grep -E '^(port|listenaddress|passwordauthentication|permitrootlogin) '; "
            "printf 'b\\na\\n' | sort; exit 23"
        )
        remote = login([command], 'password-command')
        remote.finish(23)
        for marker in ['SSH_DEFAULT_OK', 'uid=0(root)', 'port 2222', 'listenaddress 127.0.0.1:2222',
                       'passwordauthentication yes', 'permitrootlogin yes', 'a\r\nb\r\n']:
            if marker not in remote.text:
                raise RuntimeError(f'missing remote result {marker!r}: {remote.text}')
        result['checks'].append({'password_command_pipe_exit_status_and_modes': True})
        interactive = login([], 'interactive-shell', tty=True)
        interactive.wait_for('# ')
        interactive.send("printf 'TTY_OK\\n'; tty; exit 17\r")
        interactive.finish(17)
        if 'TTY_OK' not in interactive.text or '/dev/pts/' not in interactive.text:
            raise RuntimeError(f'SSH terminal allocation failed: {interactive.text}')
        result['checks'].append({'interactive_pty': True})
        payload = bytes(range(256)) * 37
        (fixture / 'upload.bin').write_bytes(payload)
        transfer = terminal([sftp, '-P', '2222', *options, 'root@127.0.0.1'], 'sftp')
        transfer.wait_for('password:')
        transfer.send('kinakaze\r')
        transfer.wait_for('sftp>')
        guest_file = f'/tmp/sshd-default-{os.getpid()}.bin'
        transfer.send(f'put upload.bin {guest_file}\rget {guest_file} download.bin\rrm {guest_file}\rbye\r')
        transfer.finish()
        if (fixture / 'download.bin').read_bytes() != payload or (root / guest_file.lstrip('/')).exists():
            raise RuntimeError('SFTP upload/download/delete failed')
        result['checks'].append({'sftp_binary_roundtrip': True})
        # A nested login runs profile.d again; the original listener must survive.
        repeated = login(['echo REPEATED_LOGIN_OK'], 'repeated-login')
        repeated.finish()
        if 'REPEATED_LOGIN_OK' not in repeated.text:
            raise RuntimeError('listener failed after nested login')
        stop(shell)
        result['checks'].append({'listener_stops_with_session': True})
        restarted = launch('restart')
        if keys() != first_keys:
            raise RuntimeError('restart replaced existing host keys')
        repeated = login(['echo RESTART_LOGIN_OK'], 'restart-login')
        repeated.finish()
        if 'RESTART_LOGIN_OK' not in repeated.text:
            raise RuntimeError('password login failed after restart')
        stop(restarted)
        result['checks'].append({'restart_preserves_host_keys_and_password_login': True})
        result['status'] = 'passed'
    except Exception as error:
        result['error'] = str(error)
    finally:
        for label, term in terminals:
            (fixture / f'{label}.log').write_text(term.text, encoding='utf-8')
            term.close()
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        result['evidence'] = str(fixture)
        report.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['status'] == 'passed' else 1)


if __name__ == '__main__':
    main()
