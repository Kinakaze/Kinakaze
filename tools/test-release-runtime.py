"""Validate a portable release in a new root and independent launch clients."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import hashlib
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dist', type=Path, required=True)
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    dist = args.dist.resolve()
    results = []
    with tempfile.TemporaryDirectory(prefix='kinakaze release ') as directory:
        temporary = Path(directory)
        root = temporary / 'root with spaces'
        env = os.environ.copy()
        env['PATH'] = ''
        for name in list(env):
            if name.startswith('KINAKAZE_'):
                del env[name]

        def run(command, expected=0):
            result = subprocess.run(command, cwd=temporary, env=env, capture_output=True, text=True,
                                    encoding='utf-8', errors='replace', timeout=45,
                                    creationflags=subprocess.CREATE_NO_WINDOW)
            if result.returncode != expected:
                raise AssertionError(f'exit {result.returncode}, expected {expected}: {result.stderr}')
            return result.stdout

        worker = [str(dist / 'worker.exe'), 'run', '--root', str(root), '--dist', str(dist), '--']
        bundled = temporary / 'bundled'
        shutil.copytree(dist, bundled)
        assert run([str(bundled / 'worker.exe'), 'run', '--', '/bin/echo', 'BUNDLED_ROOT_OK']).strip() == 'BUNDLED_ROOT_OK'
        results.append('bundled root works directly without runtime initialization')
        # Two supervisors concurrently initialize the same entirely absent root.
        with ThreadPoolExecutor(max_workers=2) as pool:
            outputs = list(pool.map(lambda _: run(worker + ['/bin/sh', '-c', 'printf FIRST_RUN_OK']), range(2)))
        assert outputs == ['FIRST_RUN_OK', 'FIRST_RUN_OK'], outputs
        assert (root / '.kinakaze-rootfs.sha256').is_file()
        assert (root / 'etc/passwd').is_file()
        manifest = json.loads((dist / 'rootfs.manifest.json').read_text(encoding='utf-8'))
        for entry in manifest['files']:
            expected = entry.get('sha256') or hashlib.sha256(entry['content'].encode()).hexdigest()
            assert hashlib.sha256((root / entry['path']).read_bytes()).hexdigest() == expected, entry['path']
        results.append('concurrent first run, paths with spaces, empty PATH')
        standalone = temporary / 'standalone'
        standalone.mkdir()
        for name in ('init.exe', 'worker.exe'):
            shutil.copyfile(dist / name, standalone / name)
        assert run([str(standalone / 'worker.exe'), 'run', '--dist', str(standalone),
                    '--rootfs-manifest', str(dist / 'rootfs.manifest.json'), '--',
                    '/bin/sh', '-c', 'printf COMPLETE_ROOT_OK']) == 'COMPLETE_ROOT_OK'
        results.append('external manifest installs complete native and guest dependencies into an absent distribution root')
        (root / 'etc/hostname').write_text('user-host\n', encoding='utf-8')
        assert run(worker + ['/bin/cat', '/etc/hostname']) == 'user-host\n'
        run(worker + ['/bin/sh', '-c', 'exit 23'], expected=23)
        results.append('preserve user configuration and propagate guest exit status')

        session = temporary / 'session.json'
        log_path = temporary / 'init.log'
        with log_path.open('wb') as log:
            init = subprocess.Popen([str(dist / 'init.exe'), '--session-file', str(session),
                                     '--root', str(root), '--dist', str(dist)],
                                    cwd=temporary, env=env, stdin=subprocess.DEVNULL, stdout=log,
                                    stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                deadline = time.monotonic() + 30
                while not session.exists():
                    if init.poll() is not None or time.monotonic() >= deadline:
                        raise AssertionError(log_path.read_text(errors='replace'))
                    time.sleep(0.01)
                launch = [str(dist / 'init.exe'), 'launch', '--session-file', str(session)]
                parent = int(run(launch + ['--', '/bin/sh', '-c', 'while :; do /bin/sleep 1; done']).strip())
                child = int(run(launch + ['--parent', str(parent), '--wait', '--env', 'INSERTED=yes',
                                          '--cwd', '/tmp', '--', '/bin/sh', '-c',
                                          'printf "%s %s %s" "$PPID" "$INSERTED" "$PWD" > /tmp/inserted; exit 23'], expected=23).strip())
                assert child != parent
                assert (root / 'tmp/inserted').read_text() == f'{parent} yes /tmp'
                results.append('new client inserts under live parent; guest PPID, cwd, env and exit 23')
                run(launch + ['--parent', '4294967295', '--', '/bin/true'], expected=1)
                run(launch + ['--wait', '--', '/bin/true'])
                results.append('missing parent rejected, reservation remains reusable')
                marker = root / 'tmp/detached'
                run(launch + ['--parent', str(parent), '--', '/bin/sh', '-c', '/bin/sleep 1; echo alive > /tmp/detached'])
                deadline = time.monotonic() + 10
                while not marker.exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                assert marker.read_text().strip() == 'alive'
                results.append('launched process survives launcher disconnect')
            finally:
                init.terminate()
                init.wait(timeout=10)
            run([str(dist / 'init.exe'), 'launch', '--session-file', str(session), '--', '/bin/true'], expected=1)
            results.append('stale session is rejected after init exits')
    images = [dist / 'init.exe', dist / 'worker.exe', dist / 'rootfs.manifest.json',
              *sorted((dist / 'rootfs/lib').glob('*'))]
    hashes = {path.relative_to(dist).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
              for path in images if path.is_file()}
    report = dict(passed=True, checks=results, images=hashes)
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
