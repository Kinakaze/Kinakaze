"""Exercise the real system manager; record each operation and its elapsed time."""
import array
import errno
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time
import tempfile


def serve(mode, directory):
    directory = Path(directory)
    if mode == 'credentials':
        credentials = Path(os.environ['CREDENTIALS_DIRECTORY'])
        state = {name: (credentials / name).read_text() for name in ('literal', 'file')}
        state['uid'] = os.getuid()
        assert os.statvfs(credentials).f_flag & os.ST_RDONLY
        try:
            with (credentials / 'file').open('w'):
                raise AssertionError('credential mount accepted a write')
        except OSError as error:
            # Credential files also deny writes through their 0400 mode.
            assert error.errno in (errno.EROFS, errno.EACCES), error
        Path('state').write_text(json.dumps(state))
        return
    if mode == 'context':
        Path('state').write_text(json.dumps(dict(uid=os.getuid(), gid=os.getgid(),
            cwd=os.getcwd(), value=os.environ['CONTEXT_VALUE'])))
        return
    if mode == 'fdstore':
        inherited = int(os.environ.get('LISTEN_FDS', '0'))
        if inherited:
            assert inherited == 1 and int(os.environ['LISTEN_PID']) == os.getpid()
            assert os.environ['LISTEN_FDNAMES'] == 'state'
            fd = 3
            assert os.read(fd, 1) == b's'
        else:
            temporary = tempfile.TemporaryFile(dir=directory)
            temporary.write(b'stored-state'); temporary.flush(); temporary.seek(0)
            fd = temporary.fileno()
        os.lseek(fd, 0, os.SEEK_SET)
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as notify:
            address = os.environ['NOTIFY_SOCKET']
            address = '\0' + address[1:] if address.startswith('@') else address
            notify.sendmsg([b'FDSTORE=1\nFDNAME=state'],
                           [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))], 0, address)
            with (directory / 'fdstore-generations').open('a') as history:
                history.write(f'{os.getpid()} {inherited}\n')
            notify.sendto(b'READY=1', address)
        if inherited: os.close(fd)
        else: temporary.close()
    if mode == 'socket':
        assert int(os.environ['LISTEN_PID']) == os.getpid()
        assert os.environ['LISTEN_FDS'] == '1'
        with socket.socket(fileno=3) as listener:
            connection, _ = listener.accept()
            with connection:
                assert connection.recv(4) == b'ping'
                connection.sendall(b'pong')
        return
    if mode == 'restart':
        with (directory / 'restarts').open('a') as stream:
            stream.write('attempt\n')
        sys.exit(7)
    if mode == 'forking':
        read, write = os.pipe()
        if os.fork():
            os.close(write)
            assert os.read(read, 1) == b'1'
            return
        os.close(read)
        os.setsid()
        (directory / 'forking.pid').write_text(str(os.getpid()))
        os.write(write, b'1')
        os.close(write)
    if mode == 'notify':
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as notify:
            address = os.environ['NOTIFY_SOCKET']
            notify.sendto(b'READY=1\nSTATUS=probe ready', '\0' + address[1:] if address.startswith('@') else address)
    if mode == 'tree':
        child = subprocess.Popen(['/bin/sleep', 'infinity'], start_new_session=True)
        (directory / 'tree.pids').write_text(f'{os.getpid()} {child.pid}')
    if mode.startswith('timeout-'):
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        child = subprocess.Popen(['/bin/sleep', 'infinity'], start_new_session=True)
        (directory / (mode + '.pids')).write_text(f'{os.getpid()} {child.pid}')
    signal.signal(signal.SIGHUP, lambda *_: (directory / 'reload').write_text('reloaded'))
    while True:
        signal.pause()


def probe(directory):
    directory = Path(directory)
    results = []
    started = time.monotonic()
    boot_target = os.environ.get('GOAL_BOOT_TARGET', 'goal.target')

    def command(*arguments, expected=0, timeout=12):
        begin = time.monotonic()
        completed = subprocess.run(arguments, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                   text=True, timeout=timeout)
        result = dict(command=list(arguments), exit_code=completed.returncode,
                      expected_exit=expected, elapsed_ms=(time.monotonic()-begin)*1000,
                      output=completed.stdout[-8000:])
        results.append(result)
        assert completed.returncode == expected, result
        return completed.stdout.strip()

    def ctl(*arguments, **kwargs):
        return command('/bin/systemctl', '--no-pager', *arguments, **kwargs)

    def until(predicate, timeout=5):
        deadline = time.monotonic() + timeout
        while not predicate():
            assert time.monotonic() < deadline, 'condition timed out'
            time.sleep(.02)

    checks = []
    def case(name, action):
        begin = time.monotonic()
        try:
            action()
            item = dict(name=name, status='passed')
        except Exception as error:
            item = dict(name=name, status='failed', error=str(error))
        item['elapsed_ms'] = (time.monotonic()-begin)*1000
        checks.append(item)
        (directory / 'results.json').write_text(json.dumps(dict(checks=checks, operations=results), indent=2))
        print(json.dumps(item), flush=True)

    def boot():
        until(lambda: Path('/run/systemd/private').exists(), 30)
        until(lambda: ctl('show', '-p', 'ActiveState', '--value', boot_target) == 'active', 30)
        assert ctl('is-active', boot_target) == 'active'
        assert Path('/proc/1/comm').read_text().strip() == 'systemd'
        identity = Path('/etc/machine-id').read_text().strip()
        assert len(identity) == 32 and int(identity, 16), identity
    case('boot-and-pid1', boot)
    if checks[-1]['status'] != 'passed':
        return 1
    (directory / 'boot-ready').write_text(str(time.monotonic()-started))

    if 'GOAL_SSH_PORT' in os.environ:
        def default_services():
            assert ctl('is-system-running', '--wait', timeout=30) == 'running'
            assert ctl('is-active', 'ssh.service') == 'active'
            assert ctl('is-active', 'dbus.socket') == 'active'
            with socket.create_connection(('127.0.0.1', int(os.environ['GOAL_SSH_PORT'])), timeout=5) as client:
                assert client.recv(256).startswith(b'SSH-2.0-OpenSSH_')
            assert command('/bin/bash', '-lc', 'printf BOOT_BASH_OK') == 'BOOT_BASH_OK'
        case('manifest-default-services-and-ssh-listener', default_services)

    if os.environ.get('GOAL_BOOT_ONLY') == '1':
        return int(any(item['status'] != 'passed' for item in checks))

    if 'GOAL_SSH_PORT' in os.environ:
        def idle_ssh():
            port = int(os.environ['GOAL_SSH_PORT'])
            def listening():
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=1) as client:
                        return client.recv(256).startswith(b'SSH-2.0-OpenSSH_')
                except OSError:
                    return False
            pid = int(ctl('show', 'ssh', '-p', 'MainPID', '--value'))
            ctl('reload', 'ssh', timeout=6)
            until(listening)
            assert int(ctl('show', 'ssh', '-p', 'MainPID', '--value')) == pid
            # Leave the listener idle so socket activity cannot accidentally
            # release a ppoll that failed to unblock its signal mask.
            time.sleep(.1)
            ctl('stop', 'ssh', timeout=6)
            assert ctl('show', 'ssh', '-p', 'Result', '--value') == 'success'
            assert ctl('show', 'ssh', '-p', 'ExecMainStatus', '--value') == '0'
            until(lambda: not Path('/proc/' + str(pid)).exists())
            ctl('start', 'ssh', timeout=6)
            until(listening)
        case('idle-ssh-reload-stop-reap-restart', idle_ssh)

    def lifecycle():
        ctl('start', 'goal-simple')
        first = int(ctl('show', 'goal-simple', '-p', 'MainPID', '--value'))
        assert first > 1
        ctl('start', 'goal-simple')
        assert int(ctl('show', 'goal-simple', '-p', 'MainPID', '--value')) == first
        ctl('restart', 'goal-simple')
        assert int(ctl('show', 'goal-simple', '-p', 'MainPID', '--value')) != first
        ctl('stop', 'goal-simple')
        ctl('stop', 'goal-simple')
        assert ctl('is-active', 'goal-simple', expected=3) == 'inactive'
    case('simple-start-stop-restart-idempotence', lifecycle)

    def missing_exec():
        marker = directory / 'missing-exec-cleanup'
        marker.unlink(missing_ok=True)
        try:
            ctl('start', 'goal-missing-exec', expected=1)
            assert ctl('show', 'goal-missing-exec', '-p', 'ActiveState', '--value') == 'failed'
            assert ctl('show', 'goal-missing-exec', '-p', 'ExecMainStatus', '--value') == '203'
            until(marker.exists)
        finally:
            ctl('reset-failed', 'goal-missing-exec')
    case('type-exec-reports-missing-binary-and-runs-cleanup', missing_exec)

    def exec_conditions():
        for status, state in ((0, 'active'), (1, 'inactive'), (255, 'failed')):
            unit = f'goal-exec-condition-{status}'
            marker = directory / f'condition-start-{status}'
            cleanup = directory / f'condition-stop-{status}'
            marker.unlink(missing_ok=True)
            cleanup.unlink(missing_ok=True)
            try:
                ctl('start', unit, expected=1 if status == 255 else 0)
                assert ctl('show', unit, '-p', 'ActiveState', '--value') == state
                assert marker.exists() == (status == 0)
                if status:
                    until(cleanup.exists)
            finally:
                # Failed units remain loaded; successful/stopped ones may be
                # collected immediately and have no failed state to reset.
                if status == 255:
                    ctl('reset-failed', unit)
                ctl('stop', unit)
    case('exec-condition-success-skip-failure-and-stop-post', exec_conditions)

    def reload():
        ctl('start', 'goal-reload')
        before = ctl('show', 'goal-reload', '-p', 'MainPID', '--value')
        ctl('reload', 'goal-reload')
        until(lambda: (directory / 'reload').exists())
        assert ctl('show', 'goal-reload', '-p', 'MainPID', '--value') == before
        ctl('stop', 'goal-reload')
    case('reload-preserves-main-pid', reload)

    def oneshot():
        ctl('start', 'goal-oneshot')
        assert (directory / 'oneshot').read_text() == 'started'
        assert ctl('show', 'goal-oneshot', '-p', 'SubState', '--value') == 'exited'
        ctl('stop', 'goal-oneshot')
        assert (directory / 'oneshot').read_text() == 'stopped'
    case('oneshot-remain-after-exit', oneshot)

    def typed(kind):
        ctl('start', 'goal-' + kind)
        assert ctl('is-active', 'goal-' + kind) == 'active'
        pid = int(ctl('show', 'goal-' + kind, '-p', 'MainPID', '--value'))
        assert pid > 1
        if kind == 'notify':
            assert ctl('show', 'goal-notify', '-p', 'StatusText', '--value') == 'probe ready'
        if kind == 'forking':
            assert int((directory / 'forking.pid').read_text()) == pid
        ctl('stop', 'goal-' + kind)
    for kind in ('notify', 'forking'):
        case('type-' + kind, lambda kind=kind: typed(kind))

    def failure():
        ctl('start', 'goal-fail', expected=1)
        assert ctl('is-failed', 'goal-fail') == 'failed'
        assert ctl('show', 'goal-fail', '-p', 'ExecMainStatus', '--value') == '23'
        ctl('reset-failed', 'goal-fail')
        assert ctl('is-failed', 'goal-fail', expected=1) == 'inactive'
    case('failure-status-and-reset', failure)
    case('missing-unit', lambda: ctl('start', 'goal-does-not-exist', expected=5))

    def tree():
        ctl('start', 'goal-tree')
        until(lambda: (directory / 'tree.pids').exists())
        pids = [int(pid) for pid in (directory / 'tree.pids').read_text().split()]
        ctl('stop', 'goal-tree')
        def stopped():
            for pid in pids:
                path = Path(f'/proc/{pid}/stat')
                if path.exists() and path.read_text().split(') ')[1].split()[0] != 'Z':
                    return False
            return True
        until(stopped)
    case('kill-control-group-with-detached-child', tree)

    def timeout_cleanup(kind):
        unit = 'goal-timeout-' + kind
        ctl('start', unit, expected=1 if kind == 'start' else 0)
        record = directory / ('timeout-' + kind + '.pids')
        until(record.exists)
        pids = record.read_text().split()
        if kind == 'stop':
            ctl('stop', unit)
        assert ctl('show', unit, '-p', 'Result', '--value') == 'timeout'
        # PID 1 must reap the complete cgroup, including setsid descendants
        # which ignore SIGTERM. A zombie still retained by PID 1 is a failure.
        until(lambda: all(not Path('/proc/' + pid).exists() for pid in pids))
        ctl('reset-failed', unit)
    for kind in ('start', 'stop'):
        case('timeout-' + kind + '-kills-and-reaps-group', lambda kind=kind: timeout_cleanup(kind))

    def dependency_failure():
        ctl('start', 'goal-dependent', expected=1)
        assert not (directory / 'unexpected-dependent').exists()
        assert ctl('show', 'goal-fail', '-p', 'ExecMainStatus', '--value') == '23'
        # A unit whose job never ran is eligible for immediate unloading.
        ctl('reset-failed', 'goal-fail')
    case('failed-dependency-prevents-exec', dependency_failure)

    def templates():
        units = ['goal-template@alpha', 'goal-template@beta']
        ctl('start', *units)
        for instance in ('alpha', 'beta'):
            assert (directory / ('instance-' + instance)).read_text() == instance
        ctl('stop', *units)
    case('template-instances-remain-independent', templates)

    def dropin():
        ctl('start', 'goal-dropin')
        assert (directory / 'dropin').read_text() == 'base'
        override = Path('/etc/systemd/system/goal-dropin.service.d/override.conf')
        override.parent.mkdir(exist_ok=True)
        override.write_text('[Service]\nEnvironment=VALUE=changed\n')
        try:
            ctl('daemon-reload', timeout=25)
            ctl('start', 'goal-dropin')
            assert (directory / 'dropin').read_text() == 'changed'
        finally:
            override.unlink()
            ctl('daemon-reload', timeout=25)
        ctl('start', 'goal-dropin')
        assert (directory / 'dropin').read_text() == 'base'
    case('dropin-reload-and-removal', dropin)

    def environment_file():
        ctl('start', 'goal-envfile')
        assert (directory / 'environment-value').read_text() == 'file value'
        (directory / 'environment').write_text('VALUE="updated value"\n')
        # EnvironmentFile is reread for each execution, without daemon-reload.
        ctl('start', 'goal-envfile')
        assert (directory / 'environment-value').read_text() == 'updated value'
    case('environment-file-quotes-and-content-refresh', environment_file)

    def execution_context():
        ctl('start', 'goal-context')
        try:
            runtime = Path('/run/goal-context')
            state = runtime / 'state'
            assert json.loads(state.read_text()) == dict(uid=65534, gid=65534,
                cwd=str(runtime), value='with spaces')
            assert state.stat().st_uid == 65534 and state.stat().st_gid == 65534
            assert state.stat().st_mode & 0o777 == 0o600
            assert runtime.stat().st_mode & 0o777 == 0o750
        finally:
            ctl('stop', 'goal-context')
        assert not Path('/run/goal-context').exists()
    case('service-credentials-cwd-umask-and-runtime-directory', execution_context)

    def credential_mount():
        source = directory / 'credential-source'
        for value in ('initial value', 'updated value'):
            source.write_text(value)
            ctl('start', 'goal-credentials')
            try:
                state = json.loads(Path('/run/goal-credentials/state').read_text())
                assert state == dict(literal='built-in value', file=value, uid=65534), state
            finally:
                ctl('stop', 'goal-credentials')
            assert not Path('/run/goal-credentials').exists()
    case('credential-mount-is-readonly-and-refreshed-on-restart', credential_mount)

    def enable_disable():
        link = Path('/etc/systemd/system/goal.target.wants/goal-enabled.service')
        ctl('enable', '--now', 'goal-enabled')
        try:
            assert ctl('is-enabled', 'goal-enabled') == 'enabled'
            assert ctl('is-active', 'goal-enabled') == 'active'
            assert link.is_symlink()
            ctl('reenable', 'goal-enabled')
            assert ctl('is-active', 'goal-enabled') == 'active'
        finally:
            ctl('disable', '--now', 'goal-enabled')
        assert ctl('is-enabled', 'goal-enabled', expected=1) == 'disabled'
        assert ctl('is-active', 'goal-enabled', expected=3) == 'inactive'
        assert not link.is_symlink()
    case('enable-reenable-disable-with-now', enable_disable)

    def transient():
        # A fixture-only target does not enable the distribution's default bus.
        ctl('start', 'dbus.socket')
        output = command('/usr/bin/systemd-run', '--quiet', '--wait', '--pipe', '--collect',
            '--unit=goal-transient', '--property=Type=exec',
            '--property=WorkingDirectory=/tmp', '--setenv=GOAL_VALUE=transient value',
            '/bin/sh', '-ec', 'test "$PWD" = /tmp; printf "%s" "$GOAL_VALUE"')
        assert output == 'transient value', output
        command('/usr/bin/systemd-run', '--quiet', '--wait', '--pipe', '--collect',
            '--unit=goal-transient-fail', '/bin/sh', '-c', 'exit 23', expected=23)
    case('transient-service-pipes-context-and-exit-status', transient)

    def concurrent_clients():
        children = []
        try:
            for _ in range(8):
                children.append(subprocess.Popen(['/bin/systemctl', 'start', 'goal-simple'],
                                                 stdout=subprocess.PIPE, stderr=subprocess.STDOUT))
            for child in children:
                output = child.communicate(timeout=10)[0]
                assert child.returncode == 0, (child.returncode, output)
            first = ctl('show', 'goal-simple', '-p', 'MainPID', '--value')
            assert int(first) > 1
            ctl('start', 'goal-simple')
            assert ctl('show', 'goal-simple', '-p', 'MainPID', '--value') == first
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)
            ctl('stop', 'goal-simple')
    case('concurrent-clients-share-one-service', concurrent_clients)

    def sysv_fallback():
        command('/usr/sbin/service', 'goal-simple', 'custom-action', 'one', 'two')
        assert (directory / 'sysv-arguments').read_text().splitlines() == ['custom-action', 'one', 'two']
        command('/usr/sbin/service', 'goal-simple', 'reload')
        assert (directory / 'sysv-arguments').read_text().splitlines() == ['reload']
    case('service-custom-action-and-reload-fallback', sysv_fallback)

    def mask():
        ctl('mask', '--runtime', 'goal-mask')
        try:
            ctl('start', 'goal-mask', expected=1)
        finally:
            ctl('unmask', '--runtime', 'goal-mask')
        ctl('start', 'goal-mask')
        ctl('stop', 'goal-mask')
    case('runtime-mask-unmask', mask)
    case('daemon-reload', lambda: ctl('daemon-reload', timeout=25))

    def activation():
        ctl('start', 'goal-activation.socket')
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(5)
            client.connect('/run/goal-activation.sock')
            client.sendall(b'ping')
            assert client.recv(4) == b'pong'
        ctl('stop', 'goal-activation.socket', 'goal-activation.service')
        assert not Path('/run/goal-activation.sock').exists()
    case('socket-activation-and-fd-inheritance', activation)

    def fdstore():
        ctl('start', 'goal-fdstore')
        try:
            until(lambda: ctl('show', 'goal-fdstore', '-p', 'NFileDescriptorStore', '--value') == '1')
            ctl('restart', 'goal-fdstore')
            assert ctl('show', 'goal-fdstore', '-p', 'NFileDescriptorStore', '--value') == '1'
            rows = [line.split() for line in (directory / 'fdstore-generations').read_text().splitlines()]
            assert [row[1] for row in rows] == ['0', '1'], rows
            assert rows[0][0] != rows[1][0], rows
        finally:
            ctl('stop', 'goal-fdstore')
        assert ctl('show', 'goal-fdstore', '-p', 'NFileDescriptorStore', '--value') == '0'
    case('fdstore-preserves-descriptor-on-restart-and-clears-on-stop', fdstore)

    def timer():
        ctl('start', 'goal-timer.timer')
        try:
            until(lambda: (directory / 'timer').exists())
        finally:
            ctl('show', 'goal-timer.timer', '-p',
                'ActiveState,SubState,Result,NextElapseUSecMonotonic,LastTriggerUSecMonotonic,TimersMonotonic')
            ctl('stop', 'goal-timer.timer')
    case('timer-activation', timer)

    def path_activation(backend, trigger):
        unit = 'goal-path-' + backend
        marker = directory / ('path-fired-' + backend)
        trigger.unlink(missing_ok=True)
        for already_exists in (False, True):
            marker.unlink(missing_ok=True)
            if already_exists:
                trigger.touch()
            try:
                ctl('start', unit + '.path')
                if not already_exists:
                    assert not marker.exists()
                    # A distinct process mutates the watched filesystem.
                    command('/bin/touch', str(trigger))
                until(marker.exists)
                until(lambda: ctl('show', unit, '-p', 'ActiveState', '--value') == 'active')
            finally:
                ctl('stop', unit + '.path', unit + '.service')
                trigger.unlink(missing_ok=True)
    for backend, trigger in (('native', directory / 'path-trigger'), ('tmpfs', Path('/run/goal-path-trigger'))):
        case('path-activation-and-rearm-' + backend,
             lambda backend=backend, trigger=trigger: path_activation(backend, trigger))

    def condition():
        ctl('start', 'goal-condition')
        assert ctl('show', 'goal-condition', '-p', 'ConditionResult', '--value') == 'no'
        assert ctl('is-active', 'goal-condition', expected=3) == 'inactive'
    case('unmet-condition-skips-service', condition)

    def restart_limit():
        ctl('start', 'goal-restart')
        until(lambda: ctl('show', 'goal-restart', '-p', 'ActiveState', '--value') == 'failed', 8)
        assert (directory / 'restarts').read_text().splitlines() == ['attempt'] * 3
        assert ctl('show', 'goal-restart', '-p', 'NRestarts', '--value') == '3'
        ctl('start', 'goal-restart', expected=1)
        ctl('reset-failed', 'goal-restart')
    case('restart-policy-and-start-limit', restart_limit)

    def service():
        command('/usr/sbin/service', 'goal-simple', 'start', timeout=30)
        assert ctl('is-active', 'goal-simple') == 'active'
        command('/usr/sbin/service', 'goal-simple', 'restart', timeout=30)
        command('/usr/sbin/service', 'goal-simple', 'stop', timeout=30)
        assert ctl('is-active', 'goal-simple', expected=3) == 'inactive'
    case('enumerate-socket-unit-files', lambda: ctl('list-unit-files', '--full', '--type=socket', timeout=30))
    case('debian-service-dispatch', service)
    for index in range(5):
        case(f'systemctl-warm-show-{index}', lambda: ctl('show', '-p', 'Version', '--value'))

    def reexecute():
        ctl('start', 'goal-simple', 'goal-activation.socket', 'goal-fdstore')
        until(lambda: ctl('show', 'goal-fdstore', '-p', 'NFileDescriptorStore', '--value') == '1')
        before = ctl('show', 'goal-simple', '-p', 'MainPID', '--value')
        assert int(before) > 1
        ctl('daemon-reexec', timeout=25)
        # Reexec may briefly close the private bus connection. Bound the
        # reconnection window; once it answers, a changed service PID fails.
        deadline = time.monotonic() + 10
        while True:
            try:
                after = ctl('show', 'goal-simple', '-p', 'MainPID', '--value', timeout=3)
                break
            except (AssertionError, subprocess.TimeoutExpired):
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.02)
        assert after == before, (before, after)
        assert ctl('show', 'goal-fdstore', '-p', 'NFileDescriptorStore', '--value') == '1'
        ctl('restart', 'goal-fdstore')
        assert (directory / 'fdstore-generations').read_text().splitlines()[-1].split()[1] == '1'
        assert os.getppid() == 1
        assert Path('/proc/1/comm').read_text().strip() == 'systemd'
        assert ctl('is-active', boot_target) == 'active'
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(5)
            client.connect('/run/goal-activation.sock')
            client.sendall(b'ping')
            assert client.recv(4) == b'pong'
        ctl('stop', 'goal-simple', 'goal-activation.socket', 'goal-activation.service', 'goal-fdstore')
    case('daemon-reexec-preserves-processes-and-listeners', reexecute)
    # Normal units pull in sysinit.target even when boot used a custom target.
    # Check its core services explicitly: a transient command can finish while
    # these dependencies are failed or still in a restart loop.
    def standard_service(unit):
        ctl('start', unit)
        assert ctl('is-active', unit) == 'active'
        assert ctl('show', unit, '-p', 'Result', '--value') == 'success'
    for unit in ('systemd-journald.service', 'systemd-tmpfiles-setup.service'):
        case('standard-' + unit, lambda unit=unit: standard_service(unit))
    def journal_roundtrip():
        marker = 'kinakaze-journal-' + str(os.getpid()) + '-' + str(time.monotonic_ns())
        command('/usr/bin/systemd-cat', '-t', 'goal-journal', '/bin/echo', marker)
        command('/bin/journalctl', '--sync')
        assert marker in command('/bin/journalctl', '--no-pager', '-t', 'goal-journal', '-o', 'cat')
        command('/bin/journalctl', '--rotate')
        assert marker in command('/bin/journalctl', '--no-pager', '-t', 'goal-journal', '-o', 'cat')
    case('journal-write-sync-read-and-rotate', journal_roundtrip)
    def udev_control():
        marker = Path('/etc/kinakaze/enable-udev')
        assert not marker.exists(), 'test requires default hosted udev policy'
        ctl('start', 'systemd-udevd.service')
        assert ctl('is-active', 'systemd-udevd.service', expected=3) == 'inactive'
        assert ctl('show', 'systemd-udevd.service', '-p', 'ConditionResult', '--value') == 'no'
        marker.touch()
        try:
            standard_service('systemd-udevd.service')
            command('udevadm', 'control', '--ping', '--timeout=5')
            path = command('udevadm', 'info', '--query=path', '--path=/sys/class/net/lo')
            assert path.endswith('/net/lo'), path
            command('udevadm', 'control', '--reload', '--timeout=5')
            command('udevadm', 'settle', '--timeout=5')
        finally:
            ctl('stop', 'systemd-udevd.service', 'systemd-udevd-control.socket',
                'systemd-udevd-kernel.socket')
            marker.unlink()
    case('udev-default-off-and-explicit-opt-in', udev_control)
    return int(any(item['status'] != 'passed' for item in checks))


if __name__ == '__main__':
    if sys.argv[1] == 'serve':
        serve(sys.argv[2], sys.argv[3])
    else:
        sys.exit(probe(sys.argv[2]))
