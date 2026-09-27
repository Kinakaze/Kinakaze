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
import zipfile
from release_download_checks import validate_download_settings, validate_startup_progress


def validate(dist, report_path=None, cache=None):
    results = []
    with tempfile.TemporaryDirectory(prefix='kinakaze release ') as directory:
        validate_in(dist, Path(directory), results, cache)
    native = dist / 'native' if (dist / 'native').is_dir() else dist / 'rootfs/lib'
    images = [dist / 'init.exe', dist / 'worker.exe', dist / 'rootfs.manifest.json',
              *sorted(dist.glob('*.dll')), *sorted(native.glob('*'))]
    hashes = {path.relative_to(dist).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
              for path in images if path.is_file()}
    report = dict(passed=True, checks=results, images=hashes)
    if report_path:
        report_path.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--dist', type=Path)
    source.add_argument('--archive', type=Path)
    parser.add_argument('--report', type=Path)
    parser.add_argument('--cache', type=Path, help='reuse downloaded Debian archives across acceptance runs')
    args = parser.parse_args()
    if args.archive:
        with tempfile.TemporaryDirectory(prefix='kinakaze zip ') as directory:
            with zipfile.ZipFile(args.archive) as archive:
                archive.extractall(directory)
            distributions = list(Path(directory).glob('*/worker.exe'))
            if len(distributions) != 1:
                raise ValueError('runtime ZIP must contain one distribution')
            validate(distributions[0].parent, args.report, args.cache)
    else:
        validate(args.dist.resolve(), args.report, args.cache)


def validate_in(dist, temporary, results, cache):
    root = temporary / 'root with spaces'
    env = os.environ.copy()
    env['PATH'] = ''
    for name in list(env):
        if name.startswith('KINAKAZE_'):
            del env[name]
    env['KINAKAZE_ROOTFS_CACHE'] = str((cache or temporary / 'downloads').resolve())
    validate_download_settings(dist, temporary, env, results)
    validate_startup_progress(dist, temporary, env, results)

    def run(command, expected=0, timeout=1800, input=None):
        result = subprocess.run(command, input=None if input is None else input.encode(),
                                cwd=temporary, env=env, capture_output=True, timeout=timeout,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        if result.returncode != expected:
            raise AssertionError(f'exit {result.returncode}, expected {expected}: {result.stderr.decode(errors="replace")}')
        return result.stdout.decode('utf-8', errors='replace').replace('\r\n', '\n')

    worker = [str(dist / 'worker.exe'), 'oneshot', '--root', str(root), '--dist', str(dist), '--']
    bundled = temporary / 'bundled'
    # Match the ZIP: copying an initialized root loses NTFS inode metadata
    # and cannot preserve Debian's case-sensitive names on an ordinary dir.
    bundled.mkdir()
    for name in ('init.exe', 'worker.exe', 'rootfs.manifest.json'):
        shutil.copyfile(dist / name, bundled / name)
    for path in dist.glob('*.dll'):
        shutil.copyfile(path, bundled / path.name)
    for name in ('rootfs-seed', 'native'):
        if (dist / name).is_dir():
            shutil.copytree(dist / name, bundled / name)
    assert run([str(bundled / 'worker.exe'), 'oneshot'], input='printf BUNDLED_ROOT_OK\nexit 17\n', expected=17).strip() == 'BUNDLED_ROOT_OK'
    results.append('disposable login installs rootfs from the bundled manifest')
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
    for path in dist.glob('*.dll'):
        shutil.copyfile(path, standalone / path.name)
    assert run([str(standalone / 'worker.exe'), 'oneshot', '--dist', str(standalone),
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
            # Shell redirection creates the file before echo writes the marker.
            # Under concurrent guest work, existence alone races that write.
            while time.monotonic() < deadline:
                if marker.exists() and marker.read_text().strip() == 'alive':
                    break
                time.sleep(0.02)
            assert marker.read_text().strip() == 'alive'
            results.append('launched process survives launcher disconnect')
        finally:
            init.terminate()
            init.wait(timeout=10)
        run([str(dist / 'init.exe'), 'launch', '--session-file', str(session), '--', '/bin/true'], expected=1)
        results.append('stale session is rejected after init exits')

    # The shipping entry reuses one environment. Keep this test independent of
    # another locally running SSH instance; the fixture belongs to this test.
    if manifest.get('startup'):
        run(worker + ['/bin/rm', '-f', '/etc/systemd/system/kinakaze.target.wants/ssh.service'])
        client = [str(dist / 'worker.exe'), 'session']
        options = ['--root', str(root), '--dist', str(dist), '--no-tray']
        try:
            with ThreadPoolExecutor(max_workers=3) as pool:
                list(pool.map(lambda _: run(client + ['start', *options]), range(3)))
            before = json.loads(run(client + ['status', *options]))
            output = run([str(dist / 'worker.exe'), 'run', *options, '--name', 'release-probe', '--',
                          '/bin/sh', '-c', 'printf PERSISTENT_OK; exit 23'], expected=23)
            assert output == 'PERSISTENT_OK', output
            after = json.loads(run(client + ['status', *options]))
            assert before['pid'] == after['pid'] and after['ready']
            results.append('native persistent boot with empty PATH, concurrent reuse and guest exit status')
        finally:
            run(client + ['stop', *options])
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                stopped = subprocess.run(client + ['status', *options], capture_output=True, env=env,
                                         creationflags=subprocess.CREATE_NO_WINDOW, timeout=10)
                if stopped.returncode:
                    break
                time.sleep(.05)
            else:
                raise AssertionError('persistent init outlived shutdown')
        results.append('persistent environment stops before temporary root cleanup')


if __name__ == '__main__':
    main()
