"""Developer acceptance: real PID 1, service lifecycle and password SSH over ConPTY."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

spec = importlib.util.spec_from_file_location('ssh_acceptance', Path(__file__).with_name('run-default-sshd.py'))
ssh_helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ssh_helpers)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    dist, root, report = args.dist.resolve(), args.root.resolve(), args.report.resolve()
    fixture = report.parent / ('systemd-session-' + str(os.getpid()))
    fixture.mkdir(parents=True)
    session = fixture / 'session.json'
    result = {'status': 'failed', 'checks': [], 'dist': str(dist), 'root': str(root)}
    notify_unit = root / 'etc/systemd/system/kinakaze-notify-probe.service'
    notify_unit.write_text(
        '[Unit]\nDefaultDependencies=no\n[Service]\nType=notify\n'
        'ExecStart=/usr/bin/python3 -c "import os,socket,time; s=socket.socket(socket.AF_UNIX,socket.SOCK_DGRAM); '
        's.sendto(b\'READY=1\\\\nSTATUS=notify probe ready\',os.environ[\'NOTIFY_SOCKET\']); time.sleep(120)"\n'
        'StandardOutput=append:/var/log/kinakaze-notify-probe.log\n'
        'StandardError=append:/var/log/kinakaze-notify-probe.log\n', encoding='utf-8')
    subprocess.run([str(dist / 'worker.exe'), '--root', str(root), '--', '/bin/chmod', '0644',
                    '/etc/systemd/system/kinakaze-notify-probe.service'], check=True,
                   capture_output=True, timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    result['host_keys_before_boot'] = sorted(path.name for path in (root / 'etc/ssh').glob('ssh_host_*_key'))
    init = daemon = None
    terminals = []
    flags = subprocess.CREATE_NO_WINDOW
    started = time.monotonic()
    ssh = shutil.which('ssh')
    if not ssh or ssh_helpers.port_open():
        raise RuntimeError('Windows SSH required and 127.0.0.1:2222 must be unused')

    def check_port(expected):
        deadline = time.monotonic() + 45
        while ssh_helpers.port_open() != expected:
            if expected and daemon and daemon.poll() is not None:
                raise RuntimeError(f'systemd exited with {daemon.returncode}: {init.text[-3000:]}')
            if time.monotonic() > deadline:
                raise RuntimeError(f'SSH listener did not become {expected}: {init.text[-3000:]}')
            time.sleep(.05)

    def ctl(*arguments, expected=0):
        completed = subprocess.run(launch + ['--parent', '1', '--wait', '--', '/bin/systemctl', '--no-pager', *arguments],
                                   capture_output=True, timeout=25, creationflags=flags)
        if completed.returncode != expected:
            raise RuntimeError(f'systemctl {arguments}: {completed.returncode}: {completed.stderr.decode(errors="replace")}')
        result['checks'].append({'systemctl': list(arguments), 'status': completed.returncode})

    def login(label, password='kinakaze', tty=False):
        options = ['-F', 'NUL', '-p', '2222', '-o', 'PreferredAuthentications=password',
                   '-o', 'PubkeyAuthentication=no', '-o', 'NumberOfPasswordPrompts=1',
                   '-o', 'ConnectTimeout=10', '-o', 'StrictHostKeyChecking=accept-new',
                   '-o', f'UserKnownHostsFile={fixture / "known_hosts"}']
        command = [] if tty else ["set -e; printf 'SYSTEMD_SSH_OK\\n'; test \"$(cat /proc/1/comm)\" = systemd; id; exit 23"]
        term = ssh_helpers.Terminal([ssh, '-tt' if tty else '-T', *options, 'root@127.0.0.1', *command], fixture)
        terminals.append((label, term))
        term.wait_for('password:')
        term.send(password + '\r')
        return term

    with (fixture / 'init.log').open('wb') as log:
        try:
            init = ssh_helpers.Terminal([str(dist/'init.exe'), '--session-file', str(session), '--root', str(root), '--dist', str(dist)], fixture)
            deadline = time.monotonic() + 15
            while not session.exists():
                if not init.proc.isalive() or time.monotonic() > deadline:
                    raise RuntimeError('init did not become ready')
                time.sleep(.05)
            launch = [str(dist/'init.exe'), 'launch', '--session-file', str(session)]
            daemon = subprocess.Popen(launch + ['--wait', '--', '/usr/local/sbin/kinakaze-systemd', '--log-level=info'],
                                      stdout=log, stderr=subprocess.STDOUT, creationflags=flags)
            check_port(True)
            ctl('is-active', 'ssh.service')
            ctl('is-system-running')
            ctl('list-jobs')
            machine_id = (root / 'etc/machine-id').read_text().strip()
            assert len(machine_id) == 32 and int(machine_id, 16) != 0, machine_id
            result['checks'].append({'persistent_machine_id': True})
            ctl('start', 'kinakaze-notify-probe.service')
            ctl('is-active', 'kinakaze-notify-probe.service')
            ctl('stop', 'kinakaze-notify-probe.service')
            wrong = login('wrong-password', 'incorrect-test-password')
            wrong.finish(255)
            assert 'Permission denied' in wrong.text and 'SYSTEMD_SSH_OK' not in wrong.text
            remote = login('password-command')
            remote.finish(23)
            assert 'SYSTEMD_SSH_OK' in remote.text and 'uid=0(root)' in remote.text
            result['checks'].append({'ssh_password_and_exit_code': True, 'wrong_password_rejected': True})
            interactive = login('interactive', tty=True)
            interactive.wait_for('# ')
            interactive.send("printf 'SYSTEMD_TTY_OK\\n'; tty; exit 17\r")
            interactive.finish(17)
            assert 'SYSTEMD_TTY_OK' in interactive.text and '/dev/pts/' in interactive.text
            result['checks'].append({'interactive_ssh': True})
            ctl('restart', 'ssh.service')
            check_port(True)
            ctl('is-active', 'ssh.service')
            remote = login('after-restart')
            remote.finish(23)
            ctl('stop', 'ssh.service')
            check_port(False)
            ctl('is-active', 'ssh.service', expected=3)
            ctl('start', 'ssh.service')
            check_port(True)
            ctl('is-active', 'ssh.service')
            ctl('--failed')
            ctl('is-system-running')
            empty = subprocess.run(launch + ['--parent', '1', '--wait', '--', '/bin/sh', '-ec',
                'systemctl --no-pager --plain --no-legend --failed > /tmp/kinakaze-failed-units; '
                'test ! -s /tmp/kinakaze-failed-units; '
                'systemctl --no-pager --plain --no-legend list-jobs > /tmp/kinakaze-pending-jobs; '
                'test ! -s /tmp/kinakaze-pending-jobs'], capture_output=True, timeout=25, creationflags=flags)
            assert empty.returncode == 0, empty.stderr.decode(errors='replace')
            result['checks'].append({'failed_units': 0, 'pending_jobs': 0})
            ctl('stop', 'ssh.service')
            check_port(False)
            result['status'] = 'passed'
        except BaseException as error:
            result['error'] = repr(error)
            raise
        finally:
            for label, terminal in terminals:
                (fixture / (label + '.txt')).write_text(terminal.text, encoding='utf-8')
                terminal.close()
            if init:
                (fixture / 'console.txt').write_text(init.text, encoding='utf-8')
                init.close()
            if daemon:
                try:
                    daemon.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    daemon.terminate()
                    daemon.wait(timeout=5)
            result['elapsed_seconds'] = round(time.monotonic() - started, 2)
            result['logs'] = str(fixture)
            notify_unit.unlink(missing_ok=True)
            report.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
