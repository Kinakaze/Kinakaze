"""Real OpenSSH login and remote command acceptance with isolated test keys."""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time

NO_WINDOW = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
WORKSPACE = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, default=WORKSPACE / 'dist')
    parser.add_argument('--report', type=Path, default=WORKSPACE / 'artifacts/sshd-login.json')
    args = parser.parse_args()
    dist = args.dist.resolve()
    root = dist / 'rootfs'
    worker = dist / 'worker.exe'
    ssh = shutil.which('ssh')
    keygen = shutil.which('ssh-keygen')
    if not ssh or not keygen:
        parser.error('a native OpenSSH client and ssh-keygen are required')
    fixture = root / 'root/.ssh' / f'acceptance-{os.getpid()}'
    fixture.mkdir(parents=True)
    guest = '/' + fixture.relative_to(root).as_posix()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    stdout = args.report.with_suffix('.stdout.log')
    stderr = args.report.with_suffix('.stderr.log')
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    result = {'status': 'failed', 'port': port, 'dist': str(dist)}
    server = None
    started = time.monotonic()
    try:
        for name in ('host', 'client'):
            subprocess.run([keygen, '-q', '-t', 'ed25519', '-N', '', '-f', str(fixture / name)],
                           check=True, timeout=20, creationflags=NO_WINDOW, capture_output=True)
        (fixture / 'authorized_keys').write_bytes((fixture / 'client.pub').read_bytes())
        (fixture / 'sshd_config').write_text(f'''Port {port}
ListenAddress 127.0.0.1
HostKey {guest}/host
PidFile {guest}/sshd.pid
AuthorizedKeysFile {guest}/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
UsePAM no
StrictModes yes
LogLevel DEBUG3
''', encoding='utf-8')
        setup = f'/bin/busybox mkdir -p /run/sshd; /bin/busybox chmod 755 / /run /run/sshd; /bin/busybox chmod 700 /root /root/.ssh {guest}; /bin/busybox chmod 600 {guest}/host {guest}/authorized_keys; exec /usr/sbin/sshd -D -e -f {guest}/sshd_config'
        with stdout.open('wb') as out, stderr.open('wb') as err:
            server = subprocess.Popen([str(worker), 'run', '--', '/bin/sh', '-ec', setup],
                                      stdin=subprocess.DEVNULL, stdout=out, stderr=err,
                                      creationflags=NO_WINDOW)
            deadline = time.monotonic() + 25
            while time.monotonic() < deadline:
                if server.poll() is not None:
                    raise RuntimeError(f'sshd exited before listen: {server.returncode}')
                if 'Server listening on 127.0.0.1' in stderr.read_text(encoding='utf-8', errors='replace'):
                    break
                time.sleep(0.1)
            else:
                raise TimeoutError('sshd did not listen within 25 seconds')
            command = [ssh, '-T', '-F', 'NUL', '-p', str(port), '-i', str(fixture / 'client'),
                       '-o', 'IdentitiesOnly=yes', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10',
                       '-o', 'StrictHostKeyChecking=accept-new', '-o', f'UserKnownHostsFile={fixture / "known_hosts"}',
                       'root@127.0.0.1', "printf 'SSH_LOGIN_OK\\n'; /bin/busybox id; printf 'b\\na\\n' | /bin/busybox sort; exit 23"]
            remote = subprocess.run(command, capture_output=True, timeout=30, creationflags=NO_WINDOW)
            result.update(exit_code=remote.returncode, stdout=remote.stdout.decode(errors='replace'),
                          stderr=remote.stderr.decode(errors='replace'))
            if remote.returncode != 23 or b'SSH_LOGIN_OK' not in remote.stdout or b'uid=0' not in remote.stdout or b'a\nb\n' not in remote.stdout:
                raise RuntimeError('authenticated remote command/pipe/exit-status check failed')
            result['status'] = 'passed'
    except (OSError, RuntimeError, subprocess.SubprocessError, TimeoutError) as error:
        result['error'] = str(error)
    finally:
        if server is not None and server.poll() is None:
            # The supervisor's job owns init and every worker in this fixture.
            server.terminate()
            server.wait(timeout=10)
        result['elapsed_seconds'] = round(time.monotonic() - started, 3)
        args.report.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
        # Keys belong exclusively to this invocation, never a user's SSH home.
        for name in ('client', 'client.pub', 'host', 'host.pub', 'authorized_keys', 'known_hosts', 'sshd_config', 'sshd.pid'):
            (fixture / name).unlink(missing_ok=True)
        try:
            fixture.rmdir()
        except OSError:
            pass
    print(json.dumps(result, indent=2))
    raise SystemExit(0 if result['status'] == 'passed' else 1)


if __name__ == '__main__':
    main()
