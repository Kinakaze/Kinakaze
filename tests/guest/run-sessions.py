"""Developer acceptance for a single init, real guest parentage and persistent PTYs."""
import argparse
import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import time

spec = importlib.util.spec_from_file_location('terminal_helpers', Path(__file__).with_name('run-default-sshd.py'))
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--init-name', default='systemd')
    args = parser.parse_args()
    dist, root, manifest = args.dist.resolve(), args.root.resolve(), args.manifest.resolve()
    args.report.parent.mkdir(parents=True, exist_ok=True)
    result = {'status': 'failed', 'checks': [], 'init_name': args.init_name}
    terminals = []
    flags = subprocess.CREATE_NO_WINDOW
    base = [str(dist / 'worker.exe')]
    options = ['--root', str(root), '--rootfs-manifest', str(manifest)]

    def run(arguments, status=0, timeout=40):
        value = subprocess.run(base + arguments, capture_output=True, timeout=timeout, creationflags=flags)
        if value.returncode != status:
            raise AssertionError((arguments, value.returncode, value.stdout.decode(errors='replace'), value.stderr.decode(errors='replace')))
        return value.stdout.decode(errors='replace')

    def state():
        return json.loads(run(['session', 'status', *options]))

    def attach(label):
        terminal = helpers.Terminal(base + ['session', 'attach', *options, '--name', 'bash'], args.report.parent)
        terminals.append((label, terminal))
        terminal.wait_for('Ctrl+]')
        return terminal

    def probe(name, command, code=0):
        return run(['run', *options, '--name', name, '--', *command], status=code)

    try:
        # Independent launchers race startup. They must all reach the same PID 1.
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(lambda _: run(['session', 'start', *options], timeout=120), range(4)))
        before = state()
        init_pid = before['pid']
        bash_pid = next(t['pid'] for t in before['terminals'] if t['name'] == 'bash')
        result['checks'].append({'concurrent_reuse': init_pid, 'bash_pid': bash_pid})
        out = probe('identity', ['/bin/bash', '-c', 'printf "IDENTITY=%s,%s\\n" "$$" "$PPID"; cat /proc/1/comm; exit 23'], 23)
        assert args.init_name in out and 'IDENTITY=' in out, out
        result['checks'].append({'pid1_and_exit_status': out.strip()})
        mode = probe('config-mode', ['/usr/bin/stat', '-c', '%a %u:%g', '/etc/kinakaze/session.json'])
        assert mode.strip() == '600 0:0', mode
        result['checks'].append({'guest_control_config': mode.strip()})

        first = attach('original')
        first.send("export PERSIST_PROBE=retained; printf '\\nSTATE_%s_%s\\n' \"$$\" \"$PERSIST_PROBE\"\r")
        first.wait_for('STATE_' + str(bash_pid) + '_retained')
        first.send('\x1d')
        first.finish()
        second = attach('reconnected')
        # The replay contains the old output; use a different marker for new input.
        second.send("printf '\\nRECONNECTED_%s_%s\\n' \"$$\" \"$PERSIST_PROBE\"\r")
        second.wait_for('RECONNECTED_' + str(bash_pid) + '_retained')
        second.proc.setwinsize(40, 100)
        time.sleep(.4)
        second.send("printf '\\nSIZE_'; stty size\r")
        second.wait_for('SIZE_40 100')
        second.send('/bin/sleep 90\r')
        time.sleep(.3)
        second.send('\x1a')
        second.wait_for('Stopped')
        second.send('fg\r')
        time.sleep(.2)
        second.send('\x03')
        second.send("printf '\\nJOB_%s\\n' CONTROL_OK\r")
        second.wait_for('JOB_CONTROL_OK')
        second.send('\x1d')
        second.finish()
        assert state()['pid'] == init_pid
        result['checks'].append({'detach_preserves_pid_environment': True, 'ctrl_z_fg_ctrl_c': True, 'terminal_resize': [40, 100]})

        raw = helpers.Terminal(base + ['run', *options, '--name', 'elf', '--', '/usr/bin/python3', '-i', '-q'], args.report.parent)
        terminals.append(('elf', raw))
        raw.wait_for('>>>')
        raw.send("value=40; print('ELF_VALUE',value+2)\r")
        raw.wait_for('ELF_VALUE 42')
        raw.send('\x1d')
        raw.finish()
        again = helpers.Terminal(base + ['session', 'attach', *options, '--name', 'elf'], args.report.parent)
        terminals.append(('elf-reconnected', again))
        again.wait_for('>>>')
        again.send("print('ELF_REUSED',value+3)\r")
        again.wait_for('ELF_REUSED 43')
        again.send('raise SystemExit(17)\r')
        again.finish(17)
        result['checks'].append({'interactive_elf_reuse': True, 'elf_exit_status': 17})

        source = root / 'tmp/session-orphan-probe.py'
        source.write_bytes(b'import os,time\nparent=os.getppid()\npid=os.fork()\nif pid: os._exit(0)\nos.setsid()\ntime.sleep(.15)\nprint("ADOPTED",os.getpid(),os.getppid(),parent,flush=True)\nos._exit(0)\n')
        orphan = probe('orphan', ['/usr/bin/python3', '/tmp/session-orphan-probe.py'])
        match = re.search(r'ADOPTED (\d+) (\d+) (\d+)', orphan)
        assert match and match[2] == match[3], orphan
        time.sleep(.15)
        check = probe('reaped', ['/bin/sh', '-c', 'test ! -d /proc/' + match[1] + '; echo REAPED'])
        assert 'REAPED' in check
        result['checks'].append({'orphan_adopted_and_reaped': match.groups()})

        # A terminal exit is not an environment shutdown. Reuse does not restart it.
        run(['session', 'start', *options, '--name', 'elf', '--', '/usr/bin/python3', '-i', '-q'])
        after = state()
        assert after['pid'] == init_pid
        assert next(t for t in after['terminals'] if t['name'] == 'elf')['status'] == 17
        result['checks'].append({'exited_terminal_not_silently_restarted': True})
        started = time.monotonic()
        run(['session', 'stop', *options])
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            completed = subprocess.run(base + ['session', 'status', *options], capture_output=True, creationflags=flags)
            if completed.returncode != 0:
                break
            time.sleep(.1)
        else:
            raise AssertionError('environment survived shutdown')
        result['checks'].append({'shutdown_seconds': round(time.monotonic() - started, 3)})
        result['status'] = 'passed'
    finally:
        for name, terminal in terminals:
            (args.report.parent / (args.report.stem + '-' + name + '.log')).write_text(terminal.text, encoding='utf-8')
            terminal.close()
        subprocess.run(base + ['session', 'stop', *options], capture_output=True, creationflags=flags)
        args.report.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
