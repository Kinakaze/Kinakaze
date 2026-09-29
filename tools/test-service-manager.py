"""Real systemd/service lifecycle and timings in a disposable developer rootfs.

The supplied root must be owned by this test: fixture units and /dev/kmsg are
written there. The diagnostic kmsg file is only a headless log sink.
"""
import argparse
import json
from pathlib import Path
import re
import shlex
import socket
import time

from init_pool import InitPool, distribution_hashes
from rootfs_bootstrap import configure_service_dispatch


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('root', 'dist', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    parser.add_argument('--fresh-machine-id', action='store_true',
                        help='reset the disposable root identity before measuring first boot')
    parser.add_argument('--default-boot', action='store_true',
                        help='boot the manifest default target and check ssh on an isolated loopback port')
    parser.add_argument('--boot-only', action='store_true',
                        help='measure readiness and shutdown without running the lifecycle suite')
    parser.add_argument('--idle-seconds', type=float, default=0,
                        help='sample owned session CPU after readiness, before shutdown')
    parser.add_argument('--idle-processes', action='store_true',
                        help='include owned native process CPU attribution (requires psutil)')
    args = parser.parse_args()
    if not 0 <= args.idle_seconds <= 120:
        parser.error('idle-seconds must be between 0 and 120')
    if args.idle_processes:
        import psutil
    root, dist, output = (value.resolve() for value in (args.root, args.dist, args.output))
    output.mkdir(parents=True, exist_ok=True)
    repository = Path(__file__).resolve().parents[1]
    manifest = json.loads((repository / 'config/rootfs.manifest.json').read_text(encoding='utf-8'))
    modes = {}
    def write(name, content, mode='644'):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content.encode('utf-8'))
        modes['/' + name] = mode
    for entry in manifest['files']:
        if entry['path'].startswith(('etc/systemd/', 'lib/systemd/')) or entry['path'] == 'usr/local/sbin/kinakaze-systemd':
            write(entry['path'], entry['content'], '755' if entry['path'].startswith('usr/local/') else '644')
    service_script = {'usr/sbin/service': (root / 'usr/sbin/service').read_bytes()}
    configure_service_dispatch(service_script)
    write('usr/sbin/service', service_script['usr/sbin/service'].decode(), '755')
    directory = '/root/goal-service-manager'
    guest = directory + '/probe.py'
    write(guest.lstrip('/'), (repository / 'tests/guest/ServiceManagerProbe.py').read_text(encoding='utf-8'))
    unit = '[Unit]\nDefaultDependencies=no\n'
    service = '[Service]\nStandardOutput=null\nStandardError=null\nTimeoutStartSec=4\nTimeoutStopSec=2\n'
    def add(name, content):
        write('etc/systemd/system/goal-' + name + '.service', unit + service + content)
    write('etc/systemd/system/goal.target', unit + 'Description=Service manager regression target\n')
    add('simple', 'Type=simple\nExecStart=/bin/sleep infinity\n')
    add('missing-exec', 'Type=exec\nExecStart=/goal-no-such-executable\n'
        f'ExecStopPost=/bin/touch {directory}/missing-exec-cleanup\n')
    for status in (0, 1, 255):
        add(f'exec-condition-{status}', 'Type=oneshot\nRemainAfterExit=yes\n'
            f'ExecCondition=/bin/sh -c "exit {status}"\n'
            f'ExecStart=/bin/touch {directory}/condition-start-{status}\n'
            f'ExecStopPost=/bin/touch {directory}/condition-stop-{status}\n')
    for backend, trigger in (('native', directory + '/path-trigger'), ('tmpfs', '/run/goal-path-trigger')):
        add('path-' + backend, 'Type=oneshot\nRemainAfterExit=yes\n'
            f'ExecStart=/bin/touch {directory}/path-fired-{backend}\n')
        write(f'etc/systemd/system/goal-path-{backend}.path', unit + '[Path]\n'
              f'PathExists={trigger}\n')
    add('enabled', 'ExecStart=/bin/sleep infinity\n[Install]\nWantedBy=goal.target\n')
    add('context', f'Type=oneshot\nRemainAfterExit=yes\nUser=nobody\nGroup=nogroup\n'
        f'RuntimeDirectory=goal-context\nRuntimeDirectoryMode=0750\n'
        f'WorkingDirectory=/run/goal-context\nUMask=0077\n'
        f'Environment="CONTEXT_VALUE=with spaces"\n'
        f'ExecStart=/usr/bin/python3 {guest} serve context {directory}\n')
    add('credentials', 'Type=oneshot\nRemainAfterExit=yes\nUser=nobody\nGroup=nogroup\n'
        'RuntimeDirectory=goal-credentials\nRuntimeDirectoryMode=0700\n'
        'WorkingDirectory=/run/goal-credentials\nSetCredential=literal:built-in value\n'
        f'LoadCredential=file:{directory}/credential-source\n'
        f'ExecStart=/usr/bin/python3 {guest} serve credentials {directory}\n')
    write(directory.lstrip('/') + '/credential-source', 'initial value')
    add('fdstore', f'Type=notify\nFileDescriptorStoreMax=4\n'
        f'ExecStart=/usr/bin/python3 {guest} serve fdstore {directory}\n')
    # /etc takes precedence over /run: runtime masking applies to a vendor unit.
    write('lib/systemd/system/goal-mask.service', unit + service + 'ExecStart=/bin/sleep infinity\n')
    for kind in ('notify', 'forking', 'reload', 'tree'):
        add(kind, f'Type={kind if kind in ("notify", "forking") else "simple"}\n'
            f'ExecStart=/usr/bin/python3 {guest} serve {kind} {directory}\n'
            + (f'PIDFile={directory}/forking.pid\n' if kind == 'forking' else '')
            + ('ExecReload=/bin/kill -HUP $MAINPID\n' if kind == 'reload' else ''))
    for kind in ('start', 'stop'):
        add('timeout-' + kind, f'Type={"notify" if kind == "start" else "simple"}\n'
            'TimeoutStartSec=1\nTimeoutStopSec=500ms\n'
            f'ExecStart=/usr/bin/python3 {guest} serve timeout-{kind} {directory}\n')
    add('template@', f'Type=oneshot\nRemainAfterExit=yes\n'
        f'ExecStart=/bin/sh -c "printf %i > {directory}/instance-%i"\n')
    add('dropin', f'Type=oneshot\nEnvironment=VALUE=base\n'
        f'ExecStart=/bin/sh -c "printf %%s $$VALUE > {directory}/dropin"\n')
    add('envfile', f'Type=oneshot\nEnvironment=VALUE=base\n'
        f'EnvironmentFile=-{directory}/missing-environment\nEnvironmentFile={directory}/environment\n'
        f'ExecStart=/bin/sh -c "printf %%s \\"$$VALUE\\" > {directory}/environment-value"\n')
    write(directory.lstrip('/') + '/environment', "VALUE='file value'\n")
    write('etc/systemd/system/goal-dependent.service', unit +
          'Requires=goal-fail.service\nAfter=goal-fail.service\n' + service +
          f'Type=oneshot\nExecStart=/bin/touch {directory}/unexpected-dependent\n')
    write('etc/init.d/goal-simple', '#!/bin/sh\n'
          f'printf "%s\\n" "$@" > {directory}/sysv-arguments\n', '755')
    add('oneshot', 'Type=oneshot\nRemainAfterExit=yes\n'
        f'ExecStart=/bin/sh -c "printf started > {directory}/oneshot"\n'
        f'ExecStop=/bin/sh -c "printf stopped > {directory}/oneshot"\n')
    add('fail', 'Type=oneshot\nExecStart=/bin/sh -c "exit 23"\n')
    add('activation', f'ExecStart=/usr/bin/python3 {guest} serve socket {directory}\n')
    write('etc/systemd/system/goal-activation.socket', unit + '[Socket]\n'
          'ListenStream=/run/goal-activation.sock\nRemoveOnStop=yes\n')
    add('timer', f'Type=oneshot\nExecStart=/bin/touch {directory}/timer\n')
    write('etc/systemd/system/goal-timer.timer', unit + '[Timer]\nOnActiveSec=100ms\nAccuracySec=1ms\n')
    write('etc/systemd/system/goal-condition.service', unit +
          'ConditionPathExists=/goal-no-such-file\n' + service + 'ExecStart=/bin/false\n')
    write('etc/systemd/system/goal-restart.service', unit +
          'StartLimitIntervalSec=10\nStartLimitBurst=3\n' + service +
          f'ExecStart=/usr/bin/python3 {guest} serve restart {directory}\nRestart=on-failure\nRestartSec=50ms\n')
    (root / 'dev/kmsg').write_bytes(b'')
    for name in ('results.json', 'boot-ready', 'reload', 'tree.pids', 'forking.pid', 'oneshot', 'timer', 'restarts',
                 'timeout-start.pids', 'timeout-stop.pids', 'unexpected-dependent', 'dropin', 'sysv-arguments', 'fdstore-generations'):
        (root / directory.lstrip('/') / name).unlink(missing_ok=True)
    (root / 'etc/systemd/system/goal-dropin.service.d/override.conf').unlink(missing_ok=True)
    environment = ['PATH=/usr/sbin:/usr/bin:/sbin:/bin', 'HOME=/root', 'LANG=C.UTF-8',
                   'container=kinakaze', 'SYSTEMD_COLORS=0', 'SYSTEMD_LOG_LEVEL=info']
    report = dict(root=str(root), dist=str(dist), sha256=distribution_hashes(dist),
                  boot='manifest-default' if args.default_boot else 'fixture-target')
    if args.boot_only:
        environment.append('GOAL_BOOT_ONLY=1')
    ready_check = 'manifest-default-services-and-ssh-listener' if args.default_boot else 'boot-and-pid1'
    report['readiness_check'] = ready_check
    if args.default_boot:
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            report['ssh_port'] = listener.getsockname()[1]
        # Keep port isolation independent of EnvironmentFile compatibility in
        # the distribution being measured; that feature has its own test.
        configuration = next(entry['content'] for entry in manifest['files'] if entry['path'] == 'etc/ssh/sshd_config')
        configuration, count = re.subn(r'(?m)^Port\s+.*$', f'Port {report["ssh_port"]}', configuration)
        assert count == 1, 'expected one SSH listener port in the manifest'
        write('etc/ssh/sshd_config', configuration)
        write('etc/default/ssh', 'SSHD_OPTS=""\n')
        environment += ['GOAL_BOOT_TARGET=default.target', f'GOAL_SSH_PORT={report["ssh_port"]}']
    try:
        with InitPool(root, dist, output / 'configure', size=1, timeout=30) as pool:
            commands = ['chmod ' + mode + ' ' + shlex.quote(path) for path, mode in modes.items()]
            if args.default_boot:
                for path, target in manifest['links'].items():
                    if path.startswith('etc/systemd/'):
                        commands.append('mkdir -p ' + shlex.quote('/' + str(Path(path).parent).replace('\\', '/')))
                        commands.append('ln -sfn ' + shlex.quote(target) + ' ' + shlex.quote('/' + path))
            if args.fresh_machine_id:
                commands.append(": > /etc/machine-id\nchmod 444 /etc/machine-id")
            result = pool.run(['/bin/sh', '-ec', '\n'.join(commands)],
                environment=environment)
            assert result['status'] == 'passed', result
        identity_path = root / 'etc/machine-id'
        report['machine_id_before'] = identity_path.read_text().strip() if identity_path.exists() else ''
        # The full suite includes a separately bounded 95s all-unit enumeration.
        # Its budget must not consume the time reserved for the lifecycle cases.
        with InitPool(root, dist, output / 'session', size=1, timeout=(45 if args.boot_only else 275) + args.idle_seconds + 2) as pool:
            report['pool_preparation_ms'] = pool.preparation_ms
            started = time.monotonic()
            manager = pool.launch([*manifest['startup']['command'],
                                  *([] if args.default_boot else ['--unit=goal.target']),
                                  '--log-target=kmsg', '--show-status=no'],
                                 environment=environment)
            assert manager.pid == 1, manager.pid
            # The Linux PID remains 1 while the native exec owner is replaced.
            # Retry only this documented transient activation conflict.
            deadline = time.monotonic() + 5
            while True:
                try:
                    probe_started = time.monotonic()
                    report['probe'] = pool.run(['/usr/bin/python3', guest, 'probe', directory],
                                               environment=environment, parent_pid=1,
                                               ready_marker=f'"name": "{ready_check}", "status": "passed"')
                    break
                except RuntimeError as error:
                    if 'parent is not an active application' not in str(error) or time.monotonic() >= deadline:
                        raise
                    time.sleep(.02)
            if report['probe']['request_to_ready_ms'] is not None:
                report['manager_to_ready_ms'] = (probe_started-started)*1000 + report['probe']['request_to_ready_ms']
            report['lifecycle_ms'] = (time.monotonic()-started)*1000
            if args.idle_seconds:
                # Allow probe exit and background worker replenishment to settle.
                time.sleep(2)
                report['idle_samples'] = []
                report['idle_after_readiness_passed'] = report['probe']['status'] == 'passed'
                remaining = args.idle_seconds
                while remaining > 0:
                    duration = min(5, remaining)
                    processes = []
                    if args.idle_processes:
                        for process in psutil.process_iter(['pid', 'name', 'create_time']):
                            if pool.child.owns_process(process.pid):
                                try:
                                    cpu = process.cpu_times()
                                    processes.append((process, cpu.user + cpu.system))
                                except psutil.Error:
                                    pass
                    before = pool.child.cpu_metrics()
                    start = time.monotonic()
                    time.sleep(duration)
                    elapsed = time.monotonic() - start
                    after = pool.child.cpu_metrics()
                    delta = {key: after[key] - before[key] for key in after}
                    sample = dict(wall_seconds=elapsed, cpu_metrics=delta,
                        one_core_percent=delta['total_cpu_ms'] / (elapsed * 10))
                    if args.idle_processes:
                        sample['processes'] = []
                        for process, initial_cpu in processes:
                            try:
                                if not process.is_running():
                                    continue
                                cpu = process.cpu_times()
                                sample['processes'].append(dict(pid=process.pid, name=process.info['name'],
                                    birth=process.info['create_time'],
                                    one_core_percent=(cpu.user + cpu.system - initial_cpu) / elapsed * 100))
                            except psutil.Error:
                                pass
                    report['idle_samples'].append(sample)
                    remaining -= duration
                report['idle_scope'] = 'owned init, workers, PID 1 and services; 100 percent equals one CPU core'
            begin = time.monotonic()
            report['shutdown_request'] = pool.run(['/bin/systemctl', '--no-block', 'exit'],
                                                  environment=environment, parent_pid=1)
            report['pid1_exit'] = manager.wait()
            report['shutdown_ms'] = (time.monotonic()-begin)*1000
            report['machine_id_after'] = (root / 'etc/machine-id').read_text().strip()
            assert len(report['machine_id_after']) == 32 and int(report['machine_id_after'], 16)
            if report['machine_id_before']:
                assert report['machine_id_after'] == report['machine_id_before'], 'identity changed on restart'
    except Exception as error:
        report['error'] = str(error)
    finally:
        source = root / directory.lstrip('/') / 'results.json'
        if source.exists():
            report['guest'] = json.loads(source.read_text(encoding='utf-8'))
        (output / 'manager.log').write_bytes((root / 'dev/kmsg').read_bytes())
        report['status'] = 'passed' if (not report.get('error') and report.get('probe', {}).get('status') == 'passed'
                                           and report.get('pid1_exit') == 0) else 'failed'
        (output / 'results.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps({key: value for key, value in report.items() if key not in ('sha256', 'guest')}, indent=2))
    return int(report['status'] != 'passed')


if __name__ == '__main__':
    raise SystemExit(main())
